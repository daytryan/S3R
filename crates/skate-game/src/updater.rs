//! Updates belong to the Skate 3 Remaster launcher. What stays here clears the helper copies
//! older builds left in %TEMP%.
use bevy::prelude::*;
use std::time::SystemTime;

pub(crate) struct UpdaterPlugin;
impl Plugin for UpdaterPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, remove_stale_copies);
    }
}
/// A session's helper copy (12 MB) is only removed when the helper exits first,
/// so closed or killed games left them in %TEMP% (73 copies, 842 MB, observed
/// 2026-09-29). Remove copies untouched for an hour; a helper that is still
/// running keeps its executable locked, so its folder survives the attempt.
fn remove_stale_copies() {
    if let Some(hour_ago) = SystemTime::now().checked_sub(std::time::Duration::from_secs(3600)) {
        remove_copies_before(&std::env::temp_dir(), hour_ago);
    }
}
fn remove_copies_before(temp: &std::path::Path, cutoff: SystemTime) {
    let Ok(entries) = std::fs::read_dir(temp) else {
        return;
    };
    for entry in entries.flatten() {
        let stale = entry
            .file_name()
            .to_string_lossy()
            .starts_with("skate-update-")
            && entry
                .metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|modified| modified < cutoff);
        if stale {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_old_helper_copies_are_removed() {
        let temp = std::env::temp_dir().join(format!("skate-updater-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        for dir in ["skate-update-1-2", "unrelated"] {
            std::fs::create_dir_all(temp.join(dir)).unwrap();
            std::fs::write(temp.join(dir).join("skate3update.exe"), b"x").unwrap();
        }
        let hour = std::time::Duration::from_secs(3600);
        super::remove_copies_before(&temp, std::time::SystemTime::now() - hour);
        assert!(
            temp.join("skate-update-1-2").exists(),
            "a recent copy may belong to a running game"
        );
        super::remove_copies_before(&temp, std::time::SystemTime::now() + hour);
        assert!(!temp.join("skate-update-1-2").exists());
        assert!(temp.join("unrelated").exists());
        std::fs::remove_dir_all(&temp).unwrap();
    }
}
