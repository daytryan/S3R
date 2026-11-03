//! Extras > Enter Cheat Code (spec 6.1, freeplay 8, freeplay2 Q5): retail's code table, the Free Play gate on three of
//! the codes, and the popups.

use bevy::prelude::Resource;

use super::mode::GameMode;
use super::unlocks::{CHARACTERS, Store};

/// The codes of table 0x83025F00 that carry a code string (ids 4-9; ids -1 and 0..3 have none, freeplay 8.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Code {
    StreetSweeper,
    Zombie,
    MiniSkaters,
    McFly,
    DeadSpaceToo,
    DontBeSoMayo,
}

/// The records' code strings, in table order (0x83025F28..0x83025F8C).
pub const CODES: [(&str, Code); 6] = [
    ("streetsweeper", Code::StreetSweeper),
    ("zombie", Code::Zombie),
    ("miniskaters", Code::MiniSkaters),
    ("mcfly", Code::McFly),
    ("deadspacetoo", Code::DeadSpaceToo),
    ("dontbesomayo", Code::DontBeSoMayo),
];

/// The matcher 0x82656080: strcmp (0x82AE89B0), so case and spaces count.
#[must_use]
pub fn matches(text: &str) -> Option<Code> {
    CODES.iter().find(|(code, _)| *code == text).map(|(_, c)| *c)
}

/// Mini 0x830B98A0, hoverboard 0x830B98A1 and the LivingWorld's zombie state LW+0x21640 (freeplay2 Q5): off at every
/// launch, cleared by `resume_career`, kept across world loads (spec 18.32).
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cheats {
    pub zombie: bool,
    pub mini: bool,
    pub hover: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Popup {
    pub title: &'static str,
    pub text: &'static str,
}

pub const FAILED: Popup = Popup { title: "ID_VALIDATE_CODE_TITLE", text: "ID_UNLOCK_CODE_FAILED" };
/// Ids 5-7 outside modes {0, 3, 4} (0x8265647C..0x826564D0); both texts read "This cheat can only be entered when in
/// Freeplay Mode.".
pub const ZOMBIE_REFUSED: Popup = Popup { title: "ID_INVALID_CODE_TITLE", text: "ID_INVALID_CODE_CHEAT_ZOMBIEMODE" };
pub const MUST_BE_IN_FREESKATE: Popup =
    Popup { title: "ID_INVALID_CODE_TITLE", text: "ID_INVALID_CODE_CHEAT_MUST_BE_IN_FREESKATE" };
/// INFERRED (the valid records' title id was not listed; id 0's popup reads "Valid Code", freeplay 8.2).
const VALID: &str = "#Valid Code";

impl Popup {
    /// Continue on the failed verification (0x82D0A8D0); OK otherwise. INFERRED (`ID_COMMON_OK`: freeplay 8.2 says
    /// "button OK" for the refusal; the valid popups' button was not listed).
    #[must_use]
    pub fn button(&self) -> &'static str {
        if *self == FAILED { "ID_COMMON_CONTINUE" } else { "ID_COMMON_OK" }
    }
}

fn toggle(on: &mut bool, enabled: &'static str, disabled: &'static str) -> Popup {
    *on = !*on;
    Popup { title: VALID, text: if *on { enabled } else { disabled } }
}

/// A typed code (0x82656080 -> switch 0x82656198). `save` writes the store, called once when a grant changed it
/// (retail saves right after a valid cheat, freeplay2 Q3 autosave row 7).
pub fn enter(text: &str, mode: GameMode, cheats: &mut Cheats, store: &mut Store, save: impl FnOnce(&Store)) -> Popup {
    let Some(code) = matches(text) else { return FAILED };
    // The free-skate check 0x826563D8.
    let free = matches!(mode, GameMode::NoProfileFreeSkate | GameMode::ProfileFreeSkate | GameMode::OnlineFreeSkate);
    match code {
        // Retail resets the moved objects ([0x8306709C]->vt[5]); here the popup only (ledger ruling, as spec 18.31
        // does for mini/mcfly). The engine's nearest reset is physics.rs `reset_props`, which the glue may call.
        Code::StreetSweeper => Popup { title: VALID, text: "ID_VALID_CODE_CHEAT_STREETSWEEPER" },
        Code::Zombie if !free => ZOMBIE_REFUSED,
        Code::MiniSkaters | Code::McFly if !free => MUST_BE_IN_FREESKATE,
        // Toggles retail's state; the engine has no pedestrians, so nothing visible changes (spec 18.32).
        Code::Zombie => toggle(&mut cheats.zombie, "#Zombie Mode Enabled", "#Zombie Mode Disabled"),
        // The mini scale's input r4 is not traced, so these two change nothing but their flag (spec 18.31).
        Code::MiniSkaters => toggle(&mut cheats.mini, "#Mini Skater Mode Enabled", "#Mini Skater Mode Disabled"),
        Code::McFly => toggle(&mut cheats.hover, "#Hoverboard Mode Enabled", "#Hoverboard Mode Disabled"),
        // 0x826563A0: Unlock(1, FNV "z_legend"), Isaac in the Free Play list.
        Code::DeadSpaceToo => {
            if store.unlock(CHARACTERS, "z_legend") {
                save(store);
            }
            Popup { title: VALID, text: "#Isaac Clarke Unlocked" }
        }
        // 0x82656500: Unlock(7, ..) and Unlock(5, ..) (unlocks.md A.7). The record's text is "#Shouldn't see this!";
        // the localized text at 0x821F9938 is the one shown here.
        Code::DontBeSoMayo => {
            let park = store.unlock(7, "skatepark_unlock_miraclewhip_dlc");
            let clothing = store.unlock(5, "cac_dlc_unlock_clothing_miraclewhip");
            if park || clothing {
                save(store);
            }
            Popup {
                title: VALID,
                text: "#Miracle Whip items unlocked.  Download the Skate.Create Upgrade Pack to view them.",
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::data::{CareerData, Character};
    use super::super::freeplay::{self, Entry, FreePlay};
    use super::*;

    const FREE: GameMode = GameMode::ProfileFreeSkate;

    fn enter_free(text: &str, cheats: &mut Cheats) -> Popup {
        enter(text, FREE, cheats, &mut Store::default(), |_| {})
    }

    #[test]
    fn each_code_shows_its_popup() {
        let mut c = Cheats::default();
        let valid = |text| Popup { title: VALID, text };
        assert_eq!(enter_free("streetsweeper", &mut c), valid("ID_VALID_CODE_CHEAT_STREETSWEEPER"));
        assert_eq!(enter_free("zombie", &mut c), valid("#Zombie Mode Enabled"));
        assert_eq!(enter_free("zombie", &mut c), valid("#Zombie Mode Disabled"));
        assert_eq!(enter_free("miniskaters", &mut c), valid("#Mini Skater Mode Enabled"));
        assert_eq!(enter_free("miniskaters", &mut c), valid("#Mini Skater Mode Disabled"));
        assert_eq!(enter_free("mcfly", &mut c), valid("#Hoverboard Mode Enabled"));
        assert_eq!(enter_free("mcfly", &mut c), valid("#Hoverboard Mode Disabled"));
        assert_eq!(enter_free("deadspacetoo", &mut c), valid("#Isaac Clarke Unlocked"));
        assert!(enter_free("dontbesomayo", &mut c).text.starts_with("#Miracle Whip items unlocked."));
        assert_eq!(c, Cheats::default(), "each toggled twice");
        assert!(CODES.iter().all(|(code, c)| matches(code) == Some(*c)));
    }

    #[test]
    fn a_wrong_code_fails_verification() {
        let mut c = Cheats::default();
        let mut store = Store::default();
        for text in ["", "Zombie", "zombie ", "letsdowork", "strangeloops"] {
            assert_eq!(enter(text, FREE, &mut c, &mut store, |_| panic!("saved")), FAILED, "{text:?}");
        }
        assert_eq!(FAILED.button(), "ID_COMMON_CONTINUE");
        assert_eq!(c, Cheats::default());
        assert_eq!(store, Store::default());
    }

    #[test]
    fn the_three_free_play_cheats_are_refused_in_the_career() {
        for mode in [GameMode::SinglePlayer, GameMode::PartyPlay, GameMode::OnlineChallenge, GameMode::Tutorial] {
            let mut c = Cheats::default();
            let mut store = Store::default();
            let mut go = |text| enter(text, mode, &mut c, &mut store, |_| {});
            assert_eq!(go("zombie"), ZOMBIE_REFUSED);
            assert_eq!(go("miniskaters"), MUST_BE_IN_FREESKATE);
            assert_eq!(go("mcfly"), MUST_BE_IN_FREESKATE);
            assert_eq!(go("streetsweeper").title, VALID, "the others need no Free Play");
            assert_eq!(go("deadspacetoo").title, VALID);
            assert_eq!(c, Cheats::default(), "{mode:?}");
        }
        for mode in [GameMode::NoProfileFreeSkate, GameMode::OnlineFreeSkate] {
            let mut c = Cheats::default();
            enter("mcfly", mode, &mut c, &mut Store::default(), |_| {});
            assert!(c.hover, "{mode:?} takes them too");
        }
    }

    #[test]
    fn they_are_off_after_resume_career_and_at_launch() {
        assert_eq!(Cheats::default(), Cheats { zombie: false, mini: false, hover: false });
        let mut c = Cheats::default();
        for code in ["zombie", "miniskaters", "mcfly"] {
            enter_free(code, &mut c);
        }
        assert_eq!(c, Cheats { zombie: true, mini: true, hover: true });
        freeplay::resume_career(&mut FreePlay::default(), &mut c);
        assert_eq!(c, Cheats::default());
    }

    #[test]
    fn deadspacetoo_lists_isaac_and_is_saved_at_once() {
        let isaac = Character {
            key: "isaak".into(),
            recipe: "z_legend".into(),
            name: "ID_COMMON_ISAAC".into(),
            category: "other".into(),
            unlock: "z_legend".into(),
        };
        let d = CareerData { characters: vec![isaac], ..CareerData::default() };
        let team = crate::team_data::Team {
            version: 1,
            name: String::new(),
            logo: String::new(),
            boss: "Me".into(),
            teammates: Vec::new(),
            active: 0,
        };
        let mut store = Store::default();
        assert!(!freeplay::list("Me", &team, &d, &store).0.contains(&Entry::Skater("isaak".into())));

        let mut saves = 0;
        enter("deadspacetoo", GameMode::SinglePlayer, &mut Cheats::default(), &mut store, |s| {
            assert!(s.earned_retail(CHARACTERS, "z_legend"), "the save sees the grant");
            saves += 1;
        });
        assert_eq!(saves, 1);
        assert!(freeplay::list("Me", &team, &d, &store).0.contains(&Entry::Skater("isaak".into())));
        enter("deadspacetoo", GameMode::SinglePlayer, &mut Cheats::default(), &mut store, |_| panic!("nothing new"));
    }

    #[test]
    fn dontbesomayo_unlocks_the_miracle_whip_items() {
        let mut store = Store::default();
        let mut saves = 0;
        enter("dontbesomayo", GameMode::SinglePlayer, &mut Cheats::default(), &mut store, |_| saves += 1);
        assert_eq!(saves, 1);
        assert!(store.earned_retail(7, "skatepark_unlock_miraclewhip_dlc"));
        assert!(store.earned_retail(5, "cac_dlc_unlock_clothing_miraclewhip"));
        assert_eq!(store.store.len(), 2);
    }
}
