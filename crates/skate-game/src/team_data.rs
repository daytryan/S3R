//! The career team, saved beside career.json: retail's team block in the career save (the boss's name at career+40,
//! member slots and names at +16, team name +864, logo hash +880; research.md section 4) as settings/team.json.
//! Looks: the boss's stays settings/character.json (the profile online publishes, decision L2); teammate k's is
//! settings/teammate<k>.json, a customiser profile in the same format, so Edit Skater edits it in place.
//! The player skates as the team's active member (engine feature, not retail: [`Team::active`]); its look file is the
//! customiser's profile.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Retail's five player cards: the boss and four teammates (kNumPlayerCards).
pub(crate) const MEMBERS: usize = 5;
/// Keyboard maxLen for team, boss and teammate names (snprintf 17 at 0x826521e0 / 0x826522a0).
pub(crate) const NAME_MAX: usize = 16;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct Team {
    pub version: u32,
    /// Team name (+864).
    pub name: String,
    /// The team logo's vault key (16 hex digits; +880).
    pub logo: String,
    /// The boss's name (career+40): you.
    pub boss: String,
    /// Unlocked teammates' names, in slot order (member 1 first).
    pub teammates: Vec<String>,
    /// The member the player skates as (0 the boss), set by Edit Skaters' Select Skater. Not retail (its player always
    /// skates as the boss, research.md): the engine's own. A team.json from before has none: the boss.
    #[serde(default)]
    pub active: usize,
}

/// What a new or migrated team starts with (from the game data and its strings).
#[derive(Clone)]
pub(crate) struct Defaults<'a> {
    pub team_name: &'a str,
    pub boss: &'a str,
    pub logo: &'a str,
    /// (name, look) per teammate slot.
    pub teammates: Vec<(String, Value)>,
}

pub(crate) fn team_path(settings: &Path) -> PathBuf {
    settings.join("team.json")
}
/// Member 0 is the boss (character.json); member k the teammate profile beside it.
pub(crate) fn look_path(settings: &Path, member: usize) -> PathBuf {
    if member == 0 { settings.join("character.json") } else { settings.join(format!("teammate{member}.json")) }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    crate::saves::write_atomic(path, bytes)?;
    crate::autosave_tag::saving();
    Ok(())
}

pub(crate) fn write_look(settings: &Path, member: usize, look: &Value) -> std::io::Result<()> {
    write_atomic(&look_path(settings, member), &serde_json::to_vec_pretty(look).unwrap_or_default())
}

/// A damaged look comes back from its last good copy, else it is kept as .bad and None (saves::load).
pub(crate) fn read_look(settings: &Path, member: usize) -> Option<Value> {
    crate::saves::load_json(&look_path(settings, member)).ok()
}

/// The player's look file: the active member's ([`Team::active`]); the boss's when team.json does not read or names
/// no member it has.
pub(crate) fn active_look_path(settings: &Path) -> PathBuf {
    let team = crate::saves::read(&team_path(settings)).ok().and_then(|b| serde_json::from_slice::<Team>(&b).ok());
    look_path(settings, team.filter(|t| t.version == VERSION && t.active < t.count()).map_or(0, |t| t.active))
}

/// Skate as the boss again: team.json's active member back to 0, nothing else in it changed (a new career's boss is
/// the one Create a Skater makes, boot.rs). No team.json, or one that does not read, is left alone.
pub(crate) fn reset_active(settings: &Path) -> std::io::Result<()> {
    let path = team_path(settings);
    let Some(mut team) = crate::saves::read(&path).ok().and_then(|b| serde_json::from_slice::<Team>(&b).ok()) else { return Ok(()) };
    if team.active == 0 {
        return Ok(());
    }
    team.active = 0;
    write_atomic(&path, &serde_json::to_vec_pretty(&team).unwrap_or_default())
}

fn clip(name: &str) -> String {
    name.chars().take(NAME_MAX).collect()
}

impl Team {
    /// Members on the team (the boss included): retail's occupied-slot count 0x827caee8, 1 to 5.
    pub fn count(&self) -> usize {
        1 + self.teammates.len().min(MEMBERS - 1)
    }
    pub fn member_name(&self, member: usize) -> &str {
        match member {
            0 => &self.boss,
            k => self.teammates.get(k - 1).map_or("", String::as_str),
        }
    }
    pub fn set_member_name(&mut self, member: usize, name: &str) {
        match member {
            0 => self.boss = clip(name),
            k => {
                if let Some(slot) = self.teammates.get_mut(k - 1) {
                    *slot = clip(name);
                }
            }
        }
    }
    pub fn save(&self, settings: &Path) -> std::io::Result<()> {
        write_atomic(&team_path(settings), &serde_json::to_vec_pretty(self).unwrap_or_default())
    }
    /// A team as the defaults say (in memory only).
    fn new(defaults: &Defaults) -> Self {
        Self {
            version: VERSION,
            name: clip(defaults.team_name),
            logo: defaults.logo.to_owned(),
            boss: clip(defaults.boss),
            teammates: defaults.teammates.iter().take(MEMBERS - 1).map(|(name, _)| clip(name)).collect(),
            active: 0,
        }
    }
    /// settings/team.json, or a team made from what an earlier version saved: the team name career.json holds
    /// (else the default), the boss's look left where it is (character.json), teammates from the defaults, saved.
    /// A teammate whose look file is missing gets the default look; an existing look file is never overwritten.
    /// A team.json that does not read (or has an unknown version) comes back from its last good copy, else it is kept
    /// aside as team.json.bad-<unix> and the defaults are used in memory only, so nothing on disk is lost. One that
    /// can't be read at all (locked) is not saved over this session (saves::load).
    pub fn load(settings: &Path, defaults: &Defaults) -> Self {
        let path = team_path(settings);
        let team = match crate::saves::load(&path, |b| serde_json::from_slice::<Team>(b).ok().filter(|t| t.version == VERSION)) {
            crate::saves::Loaded::Loaded(mut team) | crate::saves::Loaded::Restored(mut team, _) => {
                team.name = clip(&team.name);
                team.boss = clip(&team.boss);
                team.teammates = team.teammates.iter().take(MEMBERS - 1).map(|n| clip(n)).collect();
                if team.active >= team.count() {
                    team.active = 0;
                }
                team
            }
            crate::saves::Loaded::Damaged(_) | crate::saves::Loaded::Unreadable => return Self::new(defaults),
            crate::saves::Loaded::Missing => {
                let career: Option<Value> = crate::saves::read(&settings.join("career.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
                let name = career.as_ref().and_then(|c| c["team_name"].as_str()).filter(|n| !n.is_empty()).unwrap_or(defaults.team_name);
                let team = Self::new(&Defaults { team_name: name, ..defaults.clone() });
                if let Err(error) = team.save(settings) {
                    bevy::log::warn!("Team save {}: {error}", path.display());
                }
                team
            }
        };
        for (k, (_, look)) in defaults.teammates.iter().take(team.teammates.len()).enumerate() {
            if !crate::saves::exists(&look_path(settings, k + 1)) {
                if let Err(error) = write_look(settings, k + 1, look) {
                    bevy::log::warn!("Teammate {} look: {error}", k + 1);
                }
            }
        }
        team
    }
}

/// team.json's format version.
const VERSION: u32 = 1;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    pub(crate) fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("skate3-team-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
    pub(crate) fn defaults() -> Defaults<'static> {
        Defaults {
            team_name: "New Team",
            boss: "The Legend",
            logo: "07267298ecf35aa6",
            teammates: (1..=4).map(|i| (format!("Skater 0{i}"), json!({"gender": "male", "selections": {}, "n": i}))).collect(),
        }
    }

    /// An install from before teams: career.json's team name and character.json carry over, character.json is
    /// never touched (it stays the boss's look and what online sends), teammates get the default looks.
    #[test]
    fn an_existing_career_migrates_to_a_team() {
        let settings = dir("migrate");
        let boss = r#"{"gender":"female","selections":{"Hair":{"asset_id":"a","material_id":"b"}}}"#;
        std::fs::write(settings.join("character.json"), boss).unwrap();
        std::fs::write(settings.join("career.json"), r#"{"intro_done":true,"team_name":"Bones Brigade","tutorial":null}"#).unwrap();
        let team = Team::load(&settings, &defaults());
        assert_eq!(team.name, "Bones Brigade");
        assert_eq!((team.boss.as_str(), team.count()), ("The Legend", 5));
        assert_eq!(team.member_name(3), "Skater 03");
        assert_eq!(std::fs::read_to_string(settings.join("character.json")).unwrap(), boss, "the boss's look is left alone");
        assert_eq!(read_look(&settings, 2).unwrap()["n"], 2, "teammate 2 wears the second default look");
        assert_eq!(look_path(&settings, 0), settings.join("character.json"));
        // Saved: a second load reads it back as it is, even with other defaults.
        let mut other = defaults();
        other.team_name = "Other";
        assert_eq!(Team::load(&settings, &other), team);
        let _ = std::fs::remove_dir_all(settings);
    }

    #[test]
    fn a_fresh_settings_folder_gets_the_default_team() {
        let settings = dir("fresh");
        let team = Team::load(&settings, &defaults());
        assert_eq!((team.name.as_str(), team.logo.as_str()), ("New Team", "07267298ecf35aa6"));
        assert!(!settings.join("character.json").exists(), "no boss look is invented");
        assert!(team_path(&settings).exists() && look_path(&settings, 4).exists());
        let _ = std::fs::remove_dir_all(settings);
    }

    #[test]
    fn names_keep_sixteen_characters_and_a_lost_look_comes_back() {
        let settings = dir("names");
        let mut team = Team::load(&settings, &defaults());
        team.set_member_name(0, "ABCDEFGHIJKLMNOPQRST");
        team.set_member_name(4, "Kid");
        team.set_member_name(9, "nobody");
        assert_eq!((team.member_name(0), team.member_name(4), team.member_name(5)), ("ABCDEFGHIJKLMNOP", "Kid", ""));
        team.save(&settings).unwrap();
        std::fs::remove_file(look_path(&settings, 1)).unwrap();
        let again = Team::load(&settings, &defaults());
        assert_eq!(again.member_name(4), "Kid");
        assert_eq!(read_look(&settings, 1).unwrap()["n"], 1);
        let _ = std::fs::remove_dir_all(settings);
    }

    /// Select Skater: the player's look file is the active member's, saved with the team; a team.json from before
    /// (no active member) or one naming a member it does not have is the boss's.
    #[test]
    fn the_player_skates_as_the_active_member() {
        let settings = dir("active");
        assert_eq!(active_look_path(&settings), settings.join("character.json"), "no team yet: the boss");
        let mut team = Team::load(&settings, &defaults());
        assert_eq!((team.active, active_look_path(&settings)), (0, settings.join("character.json")));
        team.active = 2;
        team.save(&settings).unwrap();
        assert_eq!(active_look_path(&settings), settings.join("teammate2.json"));
        assert_eq!(Team::load(&settings, &defaults()).active, 2);
        let old = r#"{"version":1,"name":"x","logo":"","boss":"b","teammates":["a","b"]}"#;
        std::fs::write(team_path(&settings), old).unwrap();
        assert_eq!((Team::load(&settings, &defaults()).active, active_look_path(&settings)), (0, settings.join("character.json")));
        let gone = r#"{"version":1,"name":"x","logo":"","boss":"b","teammates":["a"],"active":4}"#;
        std::fs::write(team_path(&settings), gone).unwrap();
        assert_eq!(active_look_path(&settings), settings.join("character.json"), "member 4 is not on the team");
        assert_eq!(Team::load(&settings, &defaults()).active, 0);
        team.save(&settings).unwrap();
        reset_active(&settings).unwrap();
        let back = Team::load(&settings, &defaults());
        assert_eq!((back.active, active_look_path(&settings)), (0, settings.join("character.json")), "a new career's boss");
        assert_eq!(back.teammates, team.teammates, "nothing else changes");
        let _ = std::fs::remove_dir_all(settings);
    }

    /// A team.json that does not read never costs an edited teammate: it is kept aside, the looks stay, and the
    /// defaults are used in memory only. Over-long names in a hand-edited file are clipped.
    #[test]
    fn a_bad_team_file_is_kept_aside_and_the_looks_stay() {
        let settings = dir("bad");
        Team::load(&settings, &defaults());
        write_look(&settings, 2, &json!({"gender": "female", "edited": true})).unwrap();
        for bad in [r#"{"version":1,"name":"#, r#"{"version":9,"name":"x","logo":"","boss":"b","teammates":[]}"#] {
            std::fs::write(team_path(&settings), bad).unwrap();
            let team = Team::load(&settings, &defaults());
            assert_eq!(team.name, "New Team");
            assert_eq!(read_look(&settings, 2).unwrap()["edited"], true, "the edited look is untouched");
            let kept: Vec<String> = settings.read_dir().unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("team.json.bad-"))
                .map(|e| std::fs::read_to_string(e.path()).unwrap()).collect();
            assert!(kept.iter().any(|k| k == bad), "kept aside: {kept:?}");
            assert!(!team_path(&settings).exists(), "nothing is written until the team changes");
        }
        let long = r#"{"version":1,"name":"ABCDEFGHIJKLMNOPQRSTUVWXYZ","logo":"","boss":"b","teammates":["a","b","c","d","e","f"]}"#;
        std::fs::write(team_path(&settings), long).unwrap();
        let team = Team::load(&settings, &defaults());
        assert_eq!((team.name.len(), team.count()), (16, 5));
        let _ = std::fs::remove_dir_all(settings);
    }
}
