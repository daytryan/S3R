//! Retail Skate 3's unlock store and the Skate 1 / Skate 2 rules on top of it (unlocks.md B.2-B.6, rulings.md U1-U4).
//!
//! The retail store mirrors `UNLOCKABLES` (A.1): `{category, fnv1_32(key), new}`. Skate 1/2 tags (`s1:`, `s2:`,
//! `s2mq:`) never enter it (A.12's 13 collisions); their state is derived every time from [`super::unlock_rules::RULES`]
//! and the reward names granted, so a table fix applies to old saves with no migration.

use std::collections::BTreeSet;
use std::io;
use std::path::Path;

use bevy::prelude::Resource;
use serde::{Deserialize, Serialize};

use super::data::CareerData;
use super::unlock_rules::RULES;
use crate::saves::{self, Loaded};

pub const FILE: &str = "unlocks.json";
pub const VERSION: u32 = 1;
/// The categories Unlock Everything opens (B.3): not 2 REPLAY, 8 ENTITLEMENTS, 10 CHALLENGES, 12 DLC_PRODUCT.
pub const EVERYTHING_CATS: [u8; 9] = [0, 1, 3, 4, 5, 6, 7, 9, 11];
pub const CHARACTERS: u8 = 1;
pub const GESTURES: u8 = 4;
/// What Reset (0x825046b0) grants a new store: 7 CHARACTERS (strings 0x82241f64..) and 37 gestures (..0x82242210).
pub const RESET_CHARACTERS: [&str; 7] = ["sammy", "mike", "slappy", "shingo", "cuz", "seb", "freeway"];
pub const RESET_GESTURES: [&str; 37] = [
    "gesture_airguitar", "gesture_airplane", "gesture_boxing", "gesture_bruce", "gesture_checktime", "gesture_devil",
    "gesture_doublegun", "gesture_dunno", "gesture_fingerwag", "gesture_fists", "gesture_flex", "gesture_fliptable",
    "gesture_fonz", "gesture_freedom", "gesture_fu", "gesture_getaway", "gesture_getouttahere", "gesture_handcuffs",
    "gesture_pumphi", "gesture_pumplo", "gesture_prewind", "gesture_peace", "gesture_fingerpoint",
    "gesture_raisetheroof", "gesture_shaka", "gesture_shrug", "gesture_sky", "gesture_snap", "gesture_soularch",
    "gesture_spock", "gesture_surfsup", "gesture_swinghigh", "gesture_swinglow", "gesture_throwarms",
    "gesture_thumbdown", "gesture_wings", "gesture_yardsale",
];

// The B.6 popup and Options row (engine literal strings).
pub const POPUP_TITLE: &str = "#Unlock Everything?";
pub const POPUP_BODY: &str = "#Do you want every item, skater, location and skate.Park piece from skate 3, skate 2 and skate 1 unlocked right away? Your career, Board Sales and achievements are still earned by playing. You can change this later in Options.";
pub const CHOICE_YES: &str = "#Unlock Everything";
pub const CHOICE_NO: &str = "#Earn It";
pub const OPTION_LABEL: &str = "#Unlock Everything";
pub const OPTION_HELP: &str =
    "#On: every item, skater, location and skate.Park piece is available. Off: only what you have earned. Career progress is not affected.";

/// How a Skate 1 / Skate 2 tag (or a Skate 3 key rulings.md U2 gives a path) becomes available.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule {
    /// Owned from a new career.
    Start,
    /// Any of these progression_rewards names was granted.
    Rewards(&'static [&'static str]),
    /// That DLC id's parts are installed.
    Dlc(&'static str),
    /// That retail CHARACTERS key is available (roster outfits, A.13).
    Character(&'static str),
    /// Never shown, Unlock Everything included (U1).
    Hidden,
}

/// The rule for `tag`, if the table has one.
#[must_use]
pub fn rule(tag: &str) -> Option<Rule> {
    RULES.binary_search_by(|(t, _)| (*t).cmp(tag)).ok().map(|i| RULES[i].1)
}

/// FNV-1 32 (A.1): start 0x811C9DC5, `h = h * 0x01000193 ^ byte`.
#[must_use]
pub fn fnv1_32(s: &str) -> u32 {
    s.bytes().fold(0x811C_9DC5_u32, |h, b| h.wrapping_mul(0x0100_0193) ^ u32::from(b))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub cat: u8,
    pub key: String,
    pub new: bool,
}

/// `data\settings\unlocks.json` (B.2).
#[derive(Resource, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Store {
    pub version: u32,
    pub everything: bool,
    pub store: Vec<Entry>,
    pub rewards_granted: BTreeSet<String>,
    pub new_tags: BTreeSet<String>,
}

impl Default for Store {
    fn default() -> Self {
        Self { version: VERSION, everything: false, store: Vec::new(), rewards_granted: BTreeSet::new(), new_tags: BTreeSet::new() }
    }
}

impl Store {
    /// A new career's store: Reset's defaults and the popup's answer.
    #[must_use]
    pub fn reset(everything: bool) -> Self {
        let mut s = Self { everything, ..Self::default() };
        for key in RESET_CHARACTERS {
            s.unlock(CHARACTERS, key);
        }
        for key in RESET_GESTURES {
            s.unlock(GESTURES, key);
        }
        s
    }

    /// Retail `Unlock(cat, hash)` (0x82503bd8): rejects cat >= 13, adds the entry as new if missing.
    pub fn unlock(&mut self, cat: u8, key: &str) -> bool {
        if cat >= 13 || self.earned_retail(cat, key) {
            return false;
        }
        self.store.push(Entry { cat, key: key.to_owned(), new: true });
        true
    }

    /// Retail `UnlockProgressionReward` / an Own/Kill reward: the retail unlock, the name, and the port tags it opens.
    /// Idempotent; false when nothing changed.
    pub fn grant_reward(&mut self, d: &CareerData, name: &str) -> bool {
        if self.rewards_granted.contains(name) {
            return false;
        }
        if let Some(r) = d.rewards.get(name) {
            if let Ok(cat) = u8::try_from(r.category) {
                self.unlock(cat, &r.item);
            }
        }
        let before: Vec<&str> = RULES.iter().filter(|(_, r)| self.rewards_hold(r)).map(|(t, _)| *t).collect();
        self.rewards_granted.insert(name.to_owned());
        for (tag, r) in RULES {
            if matches!(r, Rule::Rewards(names) if names.contains(&name)) && !before.contains(tag) {
                self.new_tags.insert((*tag).to_owned());
            }
        }
        true
    }

    fn rewards_hold(&self, r: &Rule) -> bool {
        matches!(r, Rule::Rewards(names) if names.iter().any(|n| self.rewards_granted.contains(*n)))
    }

    /// Retail `IsUnlocked` without `store+0x24`.
    #[must_use]
    pub fn earned_retail(&self, cat: u8, key: &str) -> bool {
        let hash = fnv1_32(key);
        self.store.iter().any(|e| e.cat == cat && fnv1_32(&e.key) == hash)
    }

    #[must_use]
    pub fn available_retail(&self, cat: u8, key: &str) -> bool {
        (self.everything && EVERYTHING_CATS.contains(&cat)) || self.earned_retail(cat, key)
    }

    /// 0x8264d5xx: its own key or `all_characters_unlock`.
    #[must_use]
    pub fn character_available(&self, d: &CareerData, key: &str) -> bool {
        self.available_retail(CHARACTERS, key) || self.earned_retail(CHARACTERS, &d.all_characters_key)
    }

    /// A tag's rule (B.3); a tag the table does not know is open (the completeness test keeps that list empty).
    #[must_use]
    pub fn port_available(&self, d: &CareerData, tag: &str, dlc_installed: &dyn Fn(&str) -> bool) -> bool {
        match rule(tag) {
            Some(Rule::Hidden) => false,
            _ if self.everything => true,
            None | Some(Rule::Start) => true,
            Some(r @ Rule::Rewards(_)) => self.rewards_hold(&r),
            Some(Rule::Dlc(id)) => dlc_installed(id),
            Some(Rule::Character(key)) => self.character_available(d, key),
        }
    }

    /// A Skate 3 item (0x825f46c8): no lock, or either key available; rulings.md U1/U2's `s3:` rules on top.
    #[must_use]
    pub fn s3_item_available(&self, d: &CareerData, unlock: &str, sponsor: &str, dlc_installed: &dyn Fn(&str) -> bool) -> bool {
        let keys = [(5, unlock), (6, sponsor)];
        let keys = keys.iter().filter(|(_, k)| !k.is_empty());
        if keys.clone().any(|(_, k)| rule(&format!("s3:{k}")) == Some(Rule::Hidden)) {
            return false;
        }
        let mut keys = keys.peekable();
        keys.peek().is_none()
            || keys.any(|(cat, k)| {
                self.available_retail(*cat, k)
                    || rule(&format!("s3:{k}")).is_some() && self.port_available(d, &format!("s3:{k}"), dlc_installed)
            })
    }

    pub fn set_everything(&mut self, on: bool) {
        self.everything = on;
    }

    /// Missing: Earn It with no entries (boot decides an existing career's default, B.6). Damaged: kept aside, defaults.
    #[must_use]
    pub fn load(settings: &Path) -> Self {
        match saves::load_json::<Self>(&settings.join(FILE)) {
            Loaded::Loaded(s) | Loaded::Restored(s, _) => s,
            other => {
                if let Some(line) = other.problem() {
                    bevy::log::warn!("{FILE}: {line}");
                }
                Self::default()
            }
        }
    }

    /// Writes whole, keeping the last good copy.
    ///
    /// # Errors
    ///
    /// The write failed, or the file could not be read this session.
    pub fn write(&self, settings: &Path) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        saves::write_atomic(&settings.join(FILE), &bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::super::data::Reward;
    use super::super::unlock_rules::EVERYTHING_ONLY;
    use super::*;

    fn data() -> CareerData {
        let mut d = CareerData { all_characters_key: "all_characters_unlock".into(), ..CareerData::default() };
        let reward = |category: u32, item: &str| Reward {
            category,
            item: item.into(),
            title: String::new(),
            description: String::new(),
            icon: String::new(),
            raw: Vec::new(),
        };
        d.rewards.insert("rob_dyrdek".into(), reward(1, "rob_dyrdek"));
        d.rewards.insert("lakai_sponsorship".into(), reward(6, "cac_shoe_lakai_inventory"));
        d.rewards.insert("all_characters".into(), reward(1, "all_characters_unlock"));
        d.rewards.insert("bs_group_22".into(), reward(5, "cac_unlock_bs_group22"));
        d
    }
    fn no_dlc(_: &str) -> bool {
        false
    }
    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("s3unlocks-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn grant_reward_is_idempotent() {
        let d = data();
        let mut s = Store::default();
        assert!(s.grant_reward(&d, "rob_dyrdek"));
        let once = s.clone();
        assert!(!s.grant_reward(&d, "rob_dyrdek"));
        assert_eq!(s, once);
        assert_eq!(s.store, vec![Entry { cat: 1, key: "rob_dyrdek".into(), new: true }]);
        assert!(s.new_tags.contains("s2:cac_clothing_dyrdek"), "{:?}", s.new_tags);
        assert!(s.new_tags.contains("s1:pro_rob_dyrdek"));
    }

    #[test]
    fn port_tags_never_enter_the_store() {
        let d = data();
        let mut s = Store::default();
        for name in ["rob_dyrdek", "lakai_sponsorship", "sbm_cover_photo", "milestone_02"] {
            s.grant_reward(&d, name);
        }
        assert!(s.store.iter().all(|e| !e.key.contains(':')), "{:?}", s.store);
        // `s2:cac_shoe_lakai_inventory` spells the retail key; the port tag opens by its rule, not the store.
        assert!(s.port_available(&d, "s2:cac_shoe_lakai_inventory", &no_dlc));
        assert!(!s.port_available(&d, "s2:cac_shoe_vans_inventory", &no_dlc));
        assert!(!s.earned_retail(6, "cac_shoe_vans_inventory"));
    }

    #[test]
    fn everything_opens_only_its_cats() {
        let mut s = Store::default();
        s.set_everything(true);
        for cat in 0..13 {
            assert_eq!(s.available_retail(cat, "anything"), EVERYTHING_CATS.contains(&cat), "cat {cat}");
        }
        assert!(!s.available_retail(10, "film_univ_01"), "challenges stay closed");
        assert!(!s.available_retail(2, "replay"));
        s.set_everything(false);
        assert!(!s.available_retail(1, "anything"));
    }

    #[test]
    fn a_character_is_available_by_its_own_or_the_all_characters_unlock() {
        let d = data();
        let mut s = Store::default();
        assert!(!s.character_available(&d, "rob_dyrdek"));
        s.grant_reward(&d, "rob_dyrdek");
        assert!(s.character_available(&d, "rob_dyrdek"));
        assert!(!s.character_available(&d, "danny_way"));
        s.grant_reward(&d, "all_characters");
        assert!(s.character_available(&d, "danny_way"));
        assert!(s.port_available(&d, "s2mq:danny_way", &no_dlc), "the outfit follows its character");
        assert!(Store::reset(false).character_available(&d, "shingo"));
    }

    #[test]
    fn u1_test_items_stay_hidden_with_everything_on() {
        let d = data();
        let mut s = Store::reset(true);
        assert!(!s.s3_item_available(&d, "cac_dlc_unlock_test", "", &|_| true));
        assert!(!s.s3_item_available(&d, "", "cac_pack_dwgh_dlc_test", &|_| true));
        s.set_everything(false);
        assert!(!s.s3_item_available(&d, "cac_dlc_unlock_test", "", &|_| true));
        assert!(EVERYTHING_ONLY.is_empty());
    }

    #[test]
    fn u2_paths_ride_on_retail_rewards() {
        let d = data();
        assert_eq!(rule("s2mq:dr_pepper"), Some(Rule::Rewards(&["bs_group_22"])));
        assert_eq!(rule("s3:cac_shoe_etnies_inventory"), Some(Rule::Rewards(&["path_gvr"])));
        assert_eq!(rule("s3:cac_dlc_first_gear_pack_inventory"), Some(Rule::Start));
        assert_eq!(rule("s3:cac_hom_points"), Some(Rule::Rewards(&["steak_man"])));
        let mut s = Store::default();
        assert!(!s.s3_item_available(&d, "cac_promo_drpepper1", "", &no_dlc));
        assert!(s.s3_item_available(&d, "", "cac_dlc_first_gear_pack_inventory", &no_dlc));
        s.grant_reward(&d, "bs_group_22");
        assert!(s.s3_item_available(&d, "cac_promo_drpepper1", "", &no_dlc));
        assert!(s.port_available(&d, "s2mq:dr_pepper", &no_dlc));
        assert!(s.earned_retail(5, "cac_unlock_bs_group22"), "the retail unlock still lands");
        assert!(s.s3_item_available(&d, "", "", &no_dlc), "no lock is free");
    }

    #[test]
    fn the_store_round_trips() {
        let dir = scratch("round");
        let mut s = Store::reset(false);
        s.grant_reward(&data(), "rob_dyrdek");
        s.write(&dir).unwrap();
        crate::saves::flush();
        assert_eq!(Store::load(&dir), s);
        assert_eq!(s.store.len(), 7 + 37 + 1);
    }

    #[test]
    fn a_missing_store_is_earn_it() {
        let s = Store::load(&scratch("missing"));
        assert!(!s.everything);
        assert!(s.store.is_empty());
    }

    #[test]
    fn fnv1_matches_retail() {
        // FNV-1 32 of the empty string is the offset basis; "a" is the published test vector 0x050c5d7e.
        assert_eq!(fnv1_32(""), 0x811C_9DC5);
        assert_eq!(fnv1_32("a"), 0x050C_5D7E);
    }
}
