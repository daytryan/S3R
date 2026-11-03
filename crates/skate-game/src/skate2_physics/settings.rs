//! The Skate 2 physics setting (user 10-20): which physics drives the skater, and the four off-board gestures Skate 2
//! gives the D-pad. Saved in `settings/gameplay.json` beside the difficulty, as one record. Not tested in play.
use serde::{Deserialize, Serialize};
use skate2_physics_protocol::Profile;
use std::path::Path;

use crate::{difficulty::Difficulty, graphics_menu::Menu};

/// Which physics drives the skater.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Physics {
    /// The engine's own (the default).
    #[default]
    Skate3,
    /// The Skate 2 port's.
    Skate2,
}
impl Physics {
    pub const ALL: [Self; 2] = [Self::Skate3, Self::Skate2];
    pub fn label(self) -> &'static str {
        match self {
            Self::Skate3 => "Skate 3",
            Self::Skate2 => "Skate 2",
        }
    }
}

/// Skate 2's 35 off-board gestures in the port's `eAnimGestureNames` order (`GESTURE_UNLOCK_NAMES`,
/// `edit_skater_frontend.rs` 1549-1584, without the `gesture_` prefix): a slot holds an index into this list.
pub const GESTURES: [&str; 35] = [
    "sky", "what", "snap", "thumbdown", "pumphi", "fonz", "bruce", "shrug", "pumplo", "devil", "fu", "throwarms",
    "doublegun", "peace", "shaka", "spock", "airguitar", "airplane", "flex", "boxing", "clothesline", "paper", "rock",
    "scissors", "dunno", "fingerpoint", "fingerwag", "fists", "fliptable", "freedom", "getouttahere", "push",
    "raisetheroof", "wings", "axe",
];
/// The slots' native gestures: sky, what, snap and thumbdown.
pub const DEFAULT_GESTURES: [u32; 4] = [0, 1, 2, 3];
/// The slots in the port's order: D-pad Up, Down, Left, Right (`edit_skater_frontend.rs` 153-160, `gesture_slot`).
pub const SLOT_NAMES: [&str; 4] = ["D-pad Up", "D-pad Down", "D-pad Left", "D-pad Right"];

fn default_gestures() -> [u32; 4] {
    DEFAULT_GESTURES
}

/// The one record behind `settings/gameplay.json`. An older build reads only `difficulty` and ignores the rest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gameplay {
    pub difficulty: Difficulty,
    #[serde(default)]
    pub physics: Physics,
    #[serde(default = "default_gestures")]
    pub skate2_gestures: [u32; 4],
}
impl Default for Gameplay {
    fn default() -> Self {
        Self { difficulty: Difficulty::default(), physics: Physics::default(), skate2_gestures: DEFAULT_GESTURES }
    }
}
impl Gameplay {
    /// Every slot in 0..=34: the sim refuses (and ends on) anything else, so a bad slot of a hand-edited or newer
    /// file gives the default of that slot.
    fn sanitized(mut self) -> Self {
        for (gesture, default) in self.skate2_gestures.iter_mut().zip(DEFAULT_GESTURES) {
            if *gesture as usize >= GESTURES.len() {
                *gesture = default;
            }
        }
        self
    }
    /// A damaged or unreadable file comes back from its last good copy, else the defaults (`saves::load`).
    pub fn load(root: &Path) -> Self {
        crate::saves::load_json::<Self>(&Difficulty::path(root)).ok().map_or_else(Self::default, Self::sanitized)
    }
    pub fn save(&self, root: &Path) -> Result<(), String> {
        crate::saves::write_atomic(
            &Difficulty::path(root),
            &serde_json::to_vec_pretty(&self.clone().sanitized()).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        crate::autosave_tag::saving();
        Ok(())
    }
    /// Changes one setting and keeps the others as saved.
    pub fn update(root: &Path, change: impl FnOnce(&mut Self)) -> Result<(), String> {
        let mut saved = Self::load(root);
        change(&mut saved);
        saved.save(root)
    }
}

/// Amendment D: the engine's camera options as Skate 2's profile (its `ProfileSettings`). The Camera Angle row's Low or
/// High gives Skate 2's Low (0) or High (1) whichever game's style is chosen; Skate 1 has neither, so it gives the
/// port's default angle. Invert Look X/Y give `invert_x`/`invert_y`. The engine has no grind transparency setting, so
/// that keeps the port's default. The switch sends it. Not tested in play.
pub fn profile(menu: &Menu) -> Profile {
    let default = Profile::default();
    let [invert_x, invert_y] = menu.invert_look();
    Profile {
        // camera::STYLES index 2 is Skate 1.
        camera_angle: if menu.camera_style() == 2 { default.camera_angle } else { i32::from(!menu.camera_low()) },
        invert_x,
        invert_y,
        grind_transparency: default.grind_transparency,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::difficulty::Difficulty;

    /// A scratch asset root; the settings folder is `<dir>/settings`.
    fn scratch(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("sk8-s2settings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let root = dir.join("assets");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(Difficulty::path(&root).parent().unwrap()).unwrap();
        (dir, root)
    }

    #[test]
    fn unsaved_physics_is_skate3_with_the_default_gestures() {
        let (dir, root) = scratch("unsaved");
        assert_eq!(Gameplay::load(&root), Gameplay::default(), "no file at all");
        std::fs::write(Difficulty::path(&root), br#"{"difficulty":"normal"}"#).unwrap();
        let loaded = Gameplay::load(&root);
        assert_eq!(loaded.difficulty, Difficulty::Normal);
        assert_eq!(loaded.physics, Physics::Skate3);
        assert_eq!(loaded.skate2_gestures, [0, 1, 2, 3]);
        assert_eq!(DEFAULT_GESTURES, [0, 1, 2, 3]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn saved_settings_load_unchanged() {
        let (dir, root) = scratch("saved");
        std::fs::write(
            Difficulty::path(&root),
            br#"{"difficulty":"hardcore","physics":"skate2","skate2_gestures":[4,5,6,34]}"#,
        )
        .unwrap();
        let loaded = Gameplay::load(&root);
        assert_eq!(loaded.difficulty, Difficulty::Hardcore);
        assert_eq!(loaded.physics, Physics::Skate2);
        assert_eq!(loaded.skate2_gestures, [4, 5, 6, 34]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn saving_one_setting_keeps_the_others() {
        let (dir, root) = scratch("keeps");
        Gameplay { physics: Physics::Skate2, skate2_gestures: [7, 8, 9, 10], ..Gameplay::default() }.save(&root).unwrap();
        Difficulty::Hardcore.save(&root).unwrap();
        let loaded = Gameplay::load(&root);
        assert_eq!(loaded.physics, Physics::Skate2);
        assert_eq!(loaded.difficulty, Difficulty::Hardcore);
        assert_eq!(loaded.skate2_gestures, [7, 8, 9, 10]);
        assert_eq!(Difficulty::load(&root), Difficulty::Hardcore);
        // And the other way round: a physics pick keeps the difficulty.
        Gameplay::update(&root, |g| g.physics = Physics::Skate3).unwrap();
        let loaded = Gameplay::load(&root);
        assert_eq!((loaded.physics, loaded.difficulty), (Physics::Skate3, Difficulty::Hardcore));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_older_build_still_reads_the_file() {
        #[derive(serde::Deserialize)]
        struct Old {
            difficulty: Difficulty,
        }
        let (dir, root) = scratch("older");
        Gameplay { difficulty: Difficulty::Easy, physics: Physics::Skate2, skate2_gestures: [1, 2, 3, 4] }
            .save(&root)
            .unwrap();
        let old: Old = serde_json::from_slice(&std::fs::read(Difficulty::path(&root)).unwrap()).unwrap();
        assert_eq!(old.difficulty, Difficulty::Easy);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_35_gestures() {
        assert_eq!(GESTURES.len(), 35);
        assert_eq!((GESTURES[0], GESTURES[1], GESTURES[2], GESTURES[3]), ("sky", "what", "snap", "thumbdown"));
        assert_eq!(GESTURES[34], "axe");
        // A slot of 35 or more (a hand-edited or newer file) gives that slot's default, never a value the sim refuses.
        let (dir, root) = scratch("range");
        std::fs::write(
            Difficulty::path(&root),
            br#"{"difficulty":"normal","physics":"skate2","skate2_gestures":[35,34,4294967295,36]}"#,
        )
        .unwrap();
        assert_eq!(Gameplay::load(&root).skate2_gestures, [0, 34, 2, 3]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Amendment D: the engine's camera options reach Skate 2's profile.
    #[test]
    fn the_camera_options_reach_skate_2() {
        use crate::graphics_menu::Setting;
        let mut menu = crate::graphics_menu::test_menu();
        // The engine's own Low camera, nothing inverted: Skate 2's Low.
        assert_eq!(profile(&menu), Profile::default());
        // Skate 2 High (CAMERA_ANGLES index 3) with Invert Y.
        menu.choose(Setting::CameraAngle, 3);
        menu.choose(Setting::InvertY, 1);
        let high = profile(&menu);
        assert_eq!((high.camera_angle, high.invert_x, high.invert_y), (1, false, true));
        assert_eq!(high.grind_transparency, Profile::default().grind_transparency);
        // Skate 3's High gives the same angle: whichever game's style is chosen.
        menu.choose(Setting::CameraAngle, 1);
        menu.choose(Setting::InvertX, 1);
        let high = profile(&menu);
        assert_eq!((high.camera_angle, high.invert_x, high.invert_y), (1, true, true));
        // Skate 2 Low.
        menu.choose(Setting::CameraAngle, 2);
        assert_eq!(profile(&menu).camera_angle, 0);
        // Skate 1 has no Low or High: the port's default angle.
        menu.choose(Setting::CameraAngle, 1);
        menu.choose(Setting::CameraAngle, 4);
        assert_eq!(profile(&menu).camera_angle, Profile::default().camera_angle);
    }
}
