//! Free Play (spec 6.1, freeplay 2-7, freeplay2 Q2-Q4): the skater list, the Settings values, and what Go Skate! and
//! Resume Career hand the glue. Nothing here is saved (freeplay2 Q3).

use bevy::prelude::Resource;

use super::cheats::Cheats;
use super::data::CareerData;
use super::mode::GameMode;
use super::unlocks::Store;
use crate::team_data::Team;

/// W+0x14C traffic, W+0x150 pedestrians, W+0x154 A.I. skaters (freeplay 5.3).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub traffic: f32,
    pub pedestrians: f32,
    pub ai_skaters: bool,
}

impl Default for Settings {
    /// 10 bars, 10 bars, On (0x824FAC00 / 0x824FAC08 / 0x824FAC48).
    fn default() -> Self {
        Self { traffic: 1.0, pedestrians: 1.0, ai_skaters: true }
    }
}

/// The bar count a slider shows (natives 0x825AD680..: `v * 10` rounded, below 1 is 0), at most 10.
#[must_use]
pub fn bars(v: f32) -> u8 {
    let b = (v * 10.0).round();
    if b < 1.0 { 0 } else { b.min(10.0) as u8 }
}

/// The value kept: below 0.09 is 0.0 ([0x820C205C] = 0.09, 0x8260A684 / 0x82706A38), clamped to 1.0 (0x8260A704).
#[must_use]
pub fn stored(v: f32) -> f32 {
    if v < 0.09 { 0.0 } else { v.min(1.0) }
}

/// RefreshTraffic (freeplay 5.3).
#[must_use]
pub fn traffic_label(bars: u8) -> &'static str {
    match bars {
        0 => "ID_FREESKATEOPTION_NO_TRAFFIC",
        1..=2 => "ID_FREESKATEOPTION_LOW_TRAFFIC",
        3..=5 => "ID_FREESKATEOPTION_MEDIUM_TRAFFIC",
        _ => "ID_FREESKATEOPTION_HIGH_TRAFFIC",
    }
}

/// RefreshPedestrians (freeplay 5.3). INFERRED (the `ID_FREESKATEOPTION_` prefix, as Traffic's; 5.3 lists `NO_PEDS` ..).
#[must_use]
pub fn peds_label(bars: u8) -> &'static str {
    match bars {
        0 => "ID_FREESKATEOPTION_NO_PEDS",
        1..=2 => "ID_FREESKATEOPTION_LOW_PEDS",
        3..=6 => "ID_FREESKATEOPTION_MEDIUM_PEDS",
        _ => "ID_FREESKATEOPTION_HIGH_PEDS",
    }
}

/// GetSkatersData's provider headers (0x825B8FE8, freeplay2 Q4). Community Skaters (provider 2) is online only and
/// Preset Skaters (5) never in mode 200 (0x8264D958), so neither is listed here.
pub const YOU_HEADER: &str = "ID_FREESKATE_YOU_HEADER";
pub const TEAMMATES_HEADER: &str = "ID_CIC_TEAMMATES";
pub const PRO_HEADER: &str = "ID_CIC_PRO_SKATERS";
pub const OTHER_HEADER: &str = "ID_CIC_IP_SKATERS";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Header(&'static str),
    /// `you` for the player's own skater, a teammate's name, or a roster character's `key`.
    Skater(String),
}

/// The SpawnSkater list in mode 200 (freeplay 3, freeplay2 Q4): You, Teammates, Pro Skaters, Other Skaters, an empty
/// category left out with its header, a locked character left out (filter 0x8264D6C8). The index is the row a fresh
/// open highlights: the player's own skater under "You" (0x8264DA10).
#[must_use]
pub fn list(you: &str, team: &Team, d: &CareerData, store: &Store) -> (Vec<Entry>, usize) {
    let mut out = vec![Entry::Header(YOU_HEADER), Entry::Skater(you.to_owned())];
    let mut category = |header: &'static str, skaters: Vec<String>| {
        if !skaters.is_empty() {
            out.push(Entry::Header(header));
            out.extend(skaters.into_iter().map(Entry::Skater));
        }
    };
    category(TEAMMATES_HEADER, (1..team.count()).map(|k| team.member_name(k).to_owned()).collect());
    let roster = |kind: &str| -> Vec<String> {
        d.characters
            .iter()
            .filter(|c| c.category == kind && store.character_available(d, &c.unlock))
            .map(|c| c.key.clone())
            .collect()
    };
    category(PRO_HEADER, roster("pro"));
    category(OTHER_HEADER, roster("other"));
    (out, 1)
}

/// Free Play's state for this session only: never saved (freeplay2 Q3), so a launch starts with the defaults.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct FreePlay {
    pub settings: Settings,
    /// The skater Go Skate! swapped to; None in the career.
    pub skater: Option<String>,
}

/// The traffic census multiplier (0x826B7010): the Traffic setting in Free Play, 1.0 in every other mode.
#[must_use]
pub fn density(mode: GameMode, s: &Settings) -> f32 {
    if mode == GameMode::ProfileFreeSkate { s.traffic.clamp(0.0, 1.0) } else { 1.0 }
}

/// What the glue does for Go Skate! or Resume Career, in this order: drops the career's challenge preloads
/// (0x82DD4D18), swaps to `skater` (None: the career skater), shows the HUD elements by `hud` without writing the
/// Game Settings option (0x825DA2A0), sets `mode`, and sends `ModeExit` at once when `mode_exit`. Neither loads a
/// world nor moves the skater (freeplay 2.6, 7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Switch {
    pub mode: GameMode,
    pub skater: Option<String>,
    pub hud: bool,
    pub mode_exit: bool,
}

/// Go Skate! (0x82706938, 0x82706B40): the screen's settings kept as `stored`, the chosen skater, the saved HUD option
/// (freeplay2 Q2), mode 3, and the challenge-system flags re-evaluated at once (0x82551E58: the mode exit runs now).
pub fn go_skate(fp: &mut FreePlay, screen: &Settings, chosen: &str, hud_option: bool) -> Switch {
    fp.settings =
        Settings { traffic: stored(screen.traffic), pedestrians: stored(screen.pedestrians), ai_skaters: screen.ai_skaters };
    fp.skater = Some(chosen.to_owned());
    Switch { mode: GameMode::ProfileFreeSkate, skater: fp.skater.clone(), hud: hud_option, mode_exit: true }
}

/// Resume Career (0x82708390): mode 1, the HUD on whatever the option says (0x825DA2A0(fe, 1), freeplay2 Q2), the
/// career skater back (0x827D8810), and the three Free Play cheats off (freeplay2 Q5). The settings stay for this
/// session. The next frame the sign-up manager rebuilds the markers in place (freeplay 7).
pub fn resume_career(fp: &mut FreePlay, cheats: &mut Cheats) -> Switch {
    fp.skater = None;
    *cheats = Cheats::default();
    Switch { mode: GameMode::SinglePlayer, skater: None, hud: true, mode_exit: false }
}

#[cfg(test)]
mod tests {
    use super::super::data::Character;
    use super::*;

    fn character(key: &str, category: &str) -> Character {
        Character { key: key.into(), recipe: key.into(), name: format!("ID_COMMON_{key}"), category: category.into(), unlock: key.into() }
    }

    fn team(teammates: &[&str]) -> Team {
        Team {
            version: 1,
            name: "Team".into(),
            logo: String::new(),
            boss: "Boss".into(),
            teammates: teammates.iter().map(|t| (*t).to_owned()).collect(),
            active: 0,
        }
    }

    fn data(characters: Vec<Character>) -> CareerData {
        CareerData { characters, all_characters_key: "all_characters_unlock".into(), ..CareerData::default() }
    }

    fn skaters(entries: &[Entry]) -> Vec<&str> {
        entries.iter().filter_map(|e| if let Entry::Skater(s) = e { Some(s.as_str()) } else { None }).collect()
    }

    #[test]
    fn the_bars_and_labels() {
        let cases = [
            (0.0, 0, "NO_TRAFFIC", "NO_PEDS"),
            (0.1, 1, "LOW_TRAFFIC", "LOW_PEDS"),
            (0.2, 2, "LOW_TRAFFIC", "LOW_PEDS"),
            (0.3, 3, "MEDIUM_TRAFFIC", "MEDIUM_PEDS"),
            (0.6, 6, "HIGH_TRAFFIC", "MEDIUM_PEDS"),
            (0.7, 7, "HIGH_TRAFFIC", "HIGH_PEDS"),
            (1.0, 10, "HIGH_TRAFFIC", "HIGH_PEDS"),
        ];
        for (v, n, traffic, peds) in cases {
            assert_eq!(bars(v), n, "{v}");
            assert_eq!(traffic_label(n), format!("ID_FREESKATEOPTION_{traffic}"), "{v}");
            assert_eq!(peds_label(n), format!("ID_FREESKATEOPTION_{peds}"), "{v}");
        }
        assert_eq!(stored(0.05), 0.0);
        assert_eq!(stored(0.1), 0.1);
        assert_eq!(Settings::default(), Settings { traffic: 1.0, pedestrians: 1.0, ai_skaters: true });
    }

    #[test]
    fn the_list_order_leaves_empty_categories_out_and_highlights_you() {
        let d = data(vec![character("rob_dyrdek", "pro"), character("danny_way", "pro"), character("reda", "other")]);
        let mut store = Store::default();
        store.unlock(1, "rob_dyrdek");
        store.unlock(1, "reda");
        let (entries, at) = list("Me", &team(&["Ann", "Bo"]), &d, &store);
        assert_eq!(
            entries,
            vec![
                Entry::Header(YOU_HEADER),
                Entry::Skater("Me".into()),
                Entry::Header(TEAMMATES_HEADER),
                Entry::Skater("Ann".into()),
                Entry::Skater("Bo".into()),
                Entry::Header(PRO_HEADER),
                Entry::Skater("rob_dyrdek".into()),
                Entry::Header(OTHER_HEADER),
                Entry::Skater("reda".into()),
            ]
        );
        assert_eq!(entries[at], Entry::Skater("Me".into()));

        // No teammates and no pro unlocked: both headers go.
        let (entries, at) = list("Me", &team(&[]), &d, &Store::default());
        assert_eq!(entries, vec![Entry::Header(YOU_HEADER), Entry::Skater("Me".into())]);
        assert_eq!(at, 1);
    }

    #[test]
    fn a_locked_pro_is_not_listed_and_everything_lists_all_41() {
        // The real roster's split: 32 pros, 9 others (freeplay 3.3).
        let roster: Vec<Character> = (0..41).map(|i| character(&format!("c{i}"), if i < 32 { "pro" } else { "other" })).collect();
        let d = data(roster);
        let mut store = Store::default();
        store.unlock(1, "c3");
        let (entries, _) = list("Me", &team(&[]), &d, &store);
        assert_eq!(skaters(&entries), vec!["Me", "c3"], "locked characters are left out, not greyed");

        store.set_everything(true);
        let (entries, _) = list("Me", &team(&[]), &d, &store);
        assert_eq!(skaters(&entries).len(), 1 + 41);

        let mut store = Store::default();
        store.unlock(1, "all_characters_unlock");
        let (entries, _) = list("Me", &team(&[]), &d, &store);
        assert_eq!(skaters(&entries).len(), 1 + 41, "the all-characters key lists them too");
    }

    #[test]
    fn go_skate_keeps_the_stored_settings_and_exits_the_career_with_no_world_load() {
        let mut fp = FreePlay::default();
        let screen = Settings { traffic: 0.05, pedestrians: 0.5, ai_skaters: false };
        let switch = go_skate(&mut fp, &screen, "rob_dyrdek", false);
        assert_eq!(
            switch,
            Switch { mode: GameMode::ProfileFreeSkate, skater: Some("rob_dyrdek".into()), hud: false, mode_exit: true }
        );
        assert_eq!(fp.settings, Settings { traffic: 0.0, pedestrians: 0.5, ai_skaters: false });
        assert_eq!(fp.skater.as_deref(), Some("rob_dyrdek"));
        assert!(go_skate(&mut fp, &screen, "rob_dyrdek", true).hud, "the HUD follows the option in Free Play");
    }

    #[test]
    fn resume_career_turns_the_hud_on_brings_the_career_skater_back_and_clears_the_cheats() {
        let mut fp = FreePlay::default();
        go_skate(&mut fp, &Settings { traffic: 0.4, ..Settings::default() }, "reda", false);
        let mut cheats = Cheats { zombie: true, mini: true, hover: true };
        let switch = resume_career(&mut fp, &mut cheats);
        assert_eq!(switch, Switch { mode: GameMode::SinglePlayer, skater: None, hud: true, mode_exit: false });
        assert_eq!(cheats, Cheats::default());
        assert_eq!(fp.skater, None);
        assert_eq!(fp.settings.traffic, 0.4, "the settings stay for the session");
    }

    #[test]
    fn round_trips_leave_no_cheat_or_free_play_skater_behind() {
        let mut fp = FreePlay::default();
        let mut cheats = Cheats::default();
        let mut store = Store::default();
        for round in 0..5 {
            go_skate(&mut fp, &Settings::default(), "Me", round % 2 == 0);
            super::super::cheats::enter("zombie", GameMode::ProfileFreeSkate, &mut cheats, &mut store, |_| panic!("saved"));
            assert!(cheats.zombie);
            resume_career(&mut fp, &mut cheats);
            assert_eq!(cheats, Cheats::default(), "round {round}");
            assert_eq!(fp.skater, None);
        }
        assert_eq!(store, Store::default());
    }

    #[test]
    fn density_is_the_traffic_setting_only_in_free_play() {
        let s = Settings { traffic: 0.5, ..Settings::default() };
        assert_eq!(density(GameMode::ProfileFreeSkate, &s), 0.5);
        assert_eq!(density(GameMode::SinglePlayer, &s), 1.0);
        assert_eq!(density(GameMode::OnlineFreeSkate, &s), 1.0);
    }
}
