//! PC port addition: the game's update over the main menu. The app looks for a newer release at
//! startup (`skate2-update`); once the main menu is idle a newer one is offered, Yes downloads it
//! under a progress popup and the game then closes and starts again on it, No asks again only at
//! the next start or after the server refuses this version (`OnlineNotice::UpdateRequired`).

/// What the app's updater reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateStatus {
    /// No newer release is known.
    None,
    Available { version: String },
    Downloading { version: String, percent: u8 },
    /// The release is in place; the game closes and starts again.
    Installed { version: String },
    /// The update failed and the game is as it was: why, for the player.
    Failed { message: String },
}

/// What the main menu asks of the app's updater.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateRequest {
    /// Look for a newer release again.
    Check,
    /// Download and install the release offered.
    Install,
}

/// The update's popup over the main menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdatePrompt {
    /// A newer release: Yes (0) or No (1).
    Offer { version: String, current: String, selected: usize },
    Downloading { version: String, percent: u8 },
    Installed { version: String },
    Failed { message: String },
}

impl UpdatePrompt {
    /// The popup as sk8popup shows it: title, text, options, the highlighted option and whether
    /// B closes it.
    #[must_use]
    pub fn dialog(&self) -> (String, String, Vec<String>, usize, bool) {
        match self {
            Self::Offer { version, current, selected } => (
                "#Update Available".into(),
                format!("#Version {version} is out (you have {current}). Online play needs the newest version. Update now?"),
                vec!["ID_COMMON_YES".into(), "ID_COMMON_NO".into()],
                *selected,
                true,
            ),
            Self::Downloading { version, percent } => ("#Updating".into(), format!("#Downloading version {version}: {percent}%"), Vec::new(), 0, false),
            Self::Installed { version } => ("#Updating".into(), format!("#Version {version} is installed. The game starts again now."), Vec::new(), 0, false),
            Self::Failed { message } => (
                "ID_COMMON_WARNING_TITLE".into(),
                format!("#The update could not be installed, and the game is unchanged. {message}"),
                vec!["ID_COMMON_OK".into()],
                0,
                false,
            ),
        }
    }
}
