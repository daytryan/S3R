//! PC port addition: `config.json` beside `online-config.json` (the working directory the game
//! runs from, or `SKATE2_SAVE_ROOT`), naming the folder of the original game a launch plays from.
//! The player picks it once, by the game's `.xex`; `SKATE2_GAME_ROOT` overrides it.
use super::online_config::{quote, string_fields};
use std::{fs, io, path::PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GameConfig {
    /// The folder of the game's `.xex`, which holds its `data` folder; empty for none.
    pub game_root: String,
}

impl GameConfig {
    /// `SKATE2_SAVE_ROOT` relocates it with the saves; otherwise it sits in the working
    /// directory the game runs from.
    pub fn path() -> io::Result<PathBuf> {
        // ENGINE HOOK (user 10-20): the host's save folder first, so the engine's working folder is never read or written.
        if let Some(root) = crate::edit_skater_profile::host_save_root() {
            return Ok(root.join("config.json"));
        }
        Ok(match std::env::var_os("SKATE2_SAVE_ROOT") {
            Some(root) => PathBuf::from(root),
            None => std::env::current_dir()?,
        }
        .join("config.json"))
    }

    /// A missing or unreadable file names no folder.
    #[must_use]
    pub fn load() -> Self {
        Self::path().and_then(fs::read_to_string).ok().map_or_else(Self::default, |text| parse(&text))
    }

    pub fn save(&self) -> io::Result<()> {
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, self.text())
    }

    fn text(&self) -> String {
        format!("{{\n  \"game_root\": {}\n}}\n", quote(&self.game_root))
    }
}

fn parse(text: &str) -> GameConfig {
    let game_root = string_fields(text).and_then(|fields| fields.into_iter().find(|(key, _)| key == "game_root")).map(|(_, value)| value).unwrap_or_default();
    GameConfig { game_root }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_game_folder_round_trips_through_its_own_writer() {
        let config = GameConfig { game_root: r#"D:\Games\Skate 2 "Copy" \ é"#.into() };
        assert_eq!(parse(&config.text()), config);
    }

    #[test]
    fn other_keys_and_broken_text_name_no_folder_or_are_skipped() {
        assert_eq!(parse(r#"{"other": [1, {"a": "b"}], "game_root": "C:\\Games\\Skate 2"}"#).game_root, r"C:\Games\Skate 2");
        assert_eq!(parse("not json"), GameConfig::default());
        assert_eq!(parse("{}"), GameConfig::default());
    }
}
