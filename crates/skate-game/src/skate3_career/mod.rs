//! Retail Skate 3's career, stage 1 (spec `docs/superpowers/specs/2026-10-20-skate3-story-mode-stage1-design.md`):
//! the export it reads, the gate that turns it on, and its plugin. Not tested in play.

pub mod core;
pub mod mode;
pub mod unlocks;
pub mod unlock_rules;
pub mod signup;
pub mod stage;
pub mod glue;
pub mod popups;
pub mod screens;
pub mod lesson_run;
pub mod speech;
pub mod freeplay;
pub mod cheats;
pub mod data;
pub mod progression;
pub mod rules;
pub mod save;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bevy::prelude::*;

pub use data::{CareerData, EXPORT_VERSION, LoadError, load};

/// The export's folder: `SKATE3_CAREER_ROOT` when set (a staged export), else `<assets>/private/skate3-career`.
#[must_use]
pub fn root(asset_root: &Path) -> PathBuf {
    root_with(std::env::var_os("SKATE3_CAREER_ROOT"), asset_root)
}

fn root_with(variable: Option<std::ffi::OsString>, asset_root: &Path) -> PathBuf {
    variable.map_or_else(|| asset_root.join("private").join("skate3-career"), PathBuf::from)
}

/// Whether the career runs this launch, with its data, or why it is off (logged once).
#[derive(Resource, Debug, Default)]
pub struct CareerGate {
    pub data: Option<Arc<CareerData>>,
    pub off: Option<String>,
}

/// The `story-mode` feature: in dev builds, and in a release only with Build-Release.ps1 -StoryMode.
pub const STORY_MODE: bool = cfg!(feature = "story-mode");

/// The gate for a launch. A build without story mode, a developer launch or `SKATE_MODE` never calls `load`, so
/// neither the export nor the career's save is read (spec 6.0, Global Constraints).
pub fn decide(
    in_build: bool,
    developer_launch: bool,
    skate_mode: bool,
    load: impl FnOnce() -> Result<CareerData, LoadError>,
) -> CareerGate {
    if !in_build {
        return CareerGate { data: None, off: Some("story mode is not in this build".into()) };
    }
    if developer_launch {
        return CareerGate { data: None, off: Some("developer launch".into()) };
    }
    if skate_mode {
        return CareerGate { data: None, off: Some("SKATE_MODE".into()) };
    }
    match load() {
        Ok(data) => CareerGate { data: Some(Arc::new(data)), off: None },
        Err(why) => CareerGate { data: None, off: Some(why.to_string()) },
    }
}

pub struct Skate3CareerPlugin;

impl Plugin for Skate3CareerPlugin {
    fn build(&self, app: &mut App) {
        let developer = !crate::boot::wanted(std::env::args());
        let skate_mode = std::env::var_os("SKATE_MODE").is_some();
        let asset_root = crate::setup::asset_root().unwrap_or_default();
        let gate = decide(STORY_MODE, developer, skate_mode, || load(&root(&asset_root)));
        if let Some(off) = &gate.off {
            info!("Skate 3 career: {off}");
        }
        app.insert_resource(gate);
        // With the career off, `Career` holds nothing and nothing reads or writes its save.
        glue::build(app, &crate::saves::settings_dir(&asset_root));
        popups::build(app);
        lesson_run::build(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An export folder of its own under the temp dir, emptied first.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("s3career-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        dir
    }

    fn minimal_career(version: u32) -> String {
        format!(
            r#"{{"version": {version}, "actions": [], "groups": {{}}, "stategraphs": {{}}, "rewards": {{}}, "rows": {{}},
            "fe_locations": {{"schoolstart": {{"world": "a", "location": "b"}}, "gamestart": {{"world": "c", "location": "d"}}}},
            "worlds": {{}}, "locators": {{}}, "volumes": {{}}, "indicators": {{}}, "characters": [],
            "all_characters_key": "all", "reward_sites": {{}}}}"#
        )
    }

    fn gate(root: &Path) -> CareerGate {
        decide(true, false, false, || load(root))
    }

    #[test]
    fn a_partial_or_old_export_turns_the_career_off() {
        let none = std::env::temp_dir().join(format!("s3career-{}-absent", std::process::id()));
        let off = gate(&none);
        assert!(off.data.is_none());
        assert!(off.off.as_deref().unwrap().contains(&none.display().to_string()), "{:?}", off.off);

        let old = scratch("old");
        std::fs::write(old.join("career.json"), minimal_career(0)).unwrap();
        let off = gate(&old);
        assert!(off.data.is_none());
        assert!(off.off.as_deref().unwrap().contains("version 0"), "{:?}", off.off);

        let partial = scratch("partial");
        std::fs::write(partial.join("career.json"), minimal_career(EXPORT_VERSION)).unwrap();
        std::fs::write(partial.join("strings.json"), "{}").unwrap();
        let off = gate(&partial);
        assert!(off.data.is_none());
        assert!(off.off.as_deref().unwrap().contains("progression.lua"), "{:?}", off.off);

        // The same folder made whole loads.
        std::fs::write(partial.join("scripts").join("progression.lua"), "-- lua").unwrap();
        let on = gate(&partial);
        assert!(on.off.is_none(), "{:?}", on.off);
        assert_eq!(on.data.unwrap().lua, "-- lua");

        let damaged = scratch("damaged");
        std::fs::write(damaged.join("career.json"), "{\"version\": 1, \"actions\": 7}").unwrap();
        let off = gate(&damaged);
        assert!(off.data.is_none());
        assert!(off.off.as_deref().unwrap().contains("damaged"), "{:?}", off.off);
        for dir in [old, partial, damaged] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn a_developer_launch_never_reads_the_export() {
        assert_eq!(decide(true, true, false, || panic!("read the export")).off.as_deref(), Some("developer launch"));
        assert_eq!(decide(true, false, true, || panic!("read the export")).off.as_deref(), Some("SKATE_MODE"));
    }

    /// A release built without story mode (user 10-22): the career is off before anything is read.
    #[test]
    fn a_build_without_story_mode_never_reads_the_export() {
        let gate = decide(false, false, false, || panic!("read the export"));
        assert!(gate.data.is_none());
        assert_eq!(gate.off.as_deref(), Some("story mode is not in this build"));
    }

    #[test]
    fn the_root_follows_skate3_career_root() {
        let assets = Path::new("assets-root");
        assert_eq!(root_with(None, assets), assets.join("private").join("skate3-career"));
        assert_eq!(root_with(Some("S:/staged".into()), assets), PathBuf::from("S:/staged"));
    }

    #[test]
    #[ignore = "requires the skate3-career export (SKATE3_CAREER_DIR)"]
    fn the_export_loads() {
        let dir = PathBuf::from(std::env::var_os("SKATE3_CAREER_DIR").expect("set SKATE3_CAREER_DIR"));
        let d = load(&dir).unwrap_or_else(|why| panic!("{why}"));
        assert_eq!(d.actions.len(), 204);
        assert_eq!(d.groups.len(), 53);
        assert_eq!(d.stategraphs.len(), 210);
        assert_eq!(d.rewards.len(), 222);
        assert_eq!(d.fe_locations.gamestart.location, "ftl_univ_02_challengelocator_01");
        assert_eq!(d.worlds["dist_university"].as_deref(), Some("maps/University.skate"));
        assert!(d.row("tut_sksc_01").is_some());
        assert!(!d.lua.is_empty());
    }
}
