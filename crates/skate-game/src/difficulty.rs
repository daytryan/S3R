//! Host preference selects the native physics_mode collection, never a multiplier.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub(crate) const NATIVE_MODES: [&str; 5] = ["easy", "normal", "hardcore", "motorized", "test"];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[repr(u32)]
pub(crate) enum Difficulty {
    Easy = 0,
    /// Retail's default when nothing was saved (game settings +180 = 1; defaults 0x824FFAB0).
    #[default]
    Normal = 1,
    Hardcore = 2,
}
impl Difficulty {
    pub const ALL: [Self; 3] = [Self::Easy, Self::Normal, Self::Hardcore];
    pub fn key(self) -> &'static str {
        NATIVE_MODES[self as usize]
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Easy => "Easy",
            Self::Normal => "Normal",
            Self::Hardcore => "Hardcore",
        }
    }
    pub fn parse(value: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|d| d.key().eq_ignore_ascii_case(value))
            .ok_or_else(|| {
                format!("Unknown difficulty {value:?}; expected easy, normal or hardcore")
            })
    }
    pub fn path(root: &Path) -> PathBuf {
        crate::saves::settings_dir(root).join("gameplay.json")  // Not retail (user 10-13)
    }
    /// S1 (release review 10-18): a damaged or unreadable gameplay.json no longer stops the game starting; it comes
    /// back from its last good copy, else the default (saves::load keeps the damaged one and logs it).
    pub fn load(root: &Path) -> Self {
        crate::skate2_physics::settings::Gameplay::load(root).difficulty
    }
    /// Saved in the one gameplay record, so the Skate 2 physics setting and gestures beside it keep their values.
    pub fn save(self, root: &Path) -> Result<(), String> {
        crate::skate2_physics::settings::Gameplay::update(root, |gameplay| gameplay.difficulty = self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skate2_physics::settings::Gameplay;
    #[test]
    fn retail_indices_and_saved_names_agree() {
        for (index, d) in Difficulty::ALL.into_iter().enumerate() {
            assert_eq!(d as usize, index);
            assert_eq!(Difficulty::parse(d.label()).unwrap(), d);
            let bytes = serde_json::to_vec(&Gameplay { difficulty: d, ..Gameplay::default() }).unwrap();
            assert_eq!(
                serde_json::from_slice::<Gameplay>(&bytes).unwrap().difficulty,
                d
            );
        }
        assert!(Difficulty::parse("motorized").is_err());
        assert!(serde_json::from_str::<Gameplay>(r#"{"difficulty":"made-up"}"#).is_err());
    }

    /// F9: never saved -> retail's Normal; a saved Easy (or any value) loads unchanged.
    #[test]
    fn unsaved_difficulty_is_normal_and_saved_ones_load_unchanged() {
        let dir = std::env::temp_dir().join(format!("sk8-difficulty-{}", std::process::id()));
        let root = dir.join("assets");
        std::fs::create_dir_all(&root).unwrap();
        let _ = std::fs::remove_file(Difficulty::path(&root));
        assert_eq!(Difficulty::load(&root), Difficulty::Normal);
        for saved in Difficulty::ALL {
            saved.save(&root).unwrap();
            assert_eq!(Difficulty::load(&root), saved);
        }
        // S1: a damaged file (made-up value) no longer stops the game starting: it comes back from the session's
        // first good copy (.last-good, the Easy loaded first), else the default.
        std::fs::write(Difficulty::path(&root), br#"{"difficulty":"made-up"}"#).unwrap();
        assert_eq!(Difficulty::load(&root), Difficulty::Easy);
        std::fs::remove_file(crate::saves::backup(&Difficulty::path(&root))).unwrap();
        std::fs::write(Difficulty::path(&root), br#"{"difficulty":"made-up"}"#).unwrap();
        assert_eq!(Difficulty::load(&root), Difficulty::Normal);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
