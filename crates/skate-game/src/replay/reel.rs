//! Create > skate.Reel > My Replays (review I3): retail's footage screen, the `browser` movie (CommonBrowser; the
//! skate.Reel footage table 0x830279a0, Stage 0 §7) over every save in the Replays folder, newest first. A selects
//! (load), Y deletes (ID_MEMCARD_PROMPT_DELETE_REPLAY), B leaves. The movie shows one row of 8; left / right move the
//! selection and page the window natively, as retail's screen code does. The headers are read off the main thread
//! (review I10); the movie waits in WaitForData meanwhile.
use super::file::{self, Header, MAX_SLOTS};
use crate::apt_screen::Screen;
use serde_json::{Value as Json, json};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

/// browser.as: one row of 8 thumbnails.
const MAX_THUMBNAILS: usize = 8;
/// `_global.FECommonBrowserMode_*`, natives in retail: any distinct numbers serve, the movie only compares them.
const MODES: [&str; 11] = [
    "WaitForData",
    "OnlineDataReady",
    "LocalPhotoDataReady",
    "LocalVideoDataReady",
    "LocalSkateParkDataReady",
    "SetupFilterTabs",
    "UpdateSelectedTopTab",
    "UpdateSelectedBottomTab",
    "UpdateAvailableSpace",
    "UpdateSelectedMediaThumb",
    "CareerPhotoDataReady",
];
fn mode(name: &str) -> u32 {
    MODES.iter().position(|m| *m == name).expect("a browser mode") as u32 + 1
}
/// `_global.AptOption` / `AptMisc` (Y / X), which native_vm leaves out.
const APT_OPTION: i64 = 8;
const APT_MISC: i64 = 7;

/// A press on the footage screen, from [`Reel::key`].
#[derive(Debug, PartialEq)]
pub(crate) enum Pick {
    /// A on a save: load it (None header: damaged, offer the delete).
    Select(PathBuf, Option<Header>),
    /// Y on a save: ask before deleting it.
    Delete(PathBuf),
}

pub(crate) type Listing = Arc<Mutex<Option<Vec<(PathBuf, Option<Header>)>>>>;

/// Updates a fresh list ignores A and Y for (user 10-27: the crossbar's A reached the list a few updates late and
/// loaded the newest save at once), as a new popup holds its answer (online_menu POPUP_HOLD).
pub(crate) const REEL_HOLD: u32 = 15;

#[derive(Default)]
pub(crate) struct Reel {
    /// Every save, newest first; None until the background listing lands.
    pub saves: Option<Vec<(PathBuf, Option<Header>)>>,
    pub index: usize,
    listing: Option<Listing>,
    /// Updates left in [`REEL_HOLD`].
    hold: u32,
}

impl Reel {
    /// Once per update: whether A and Y count yet.
    pub fn takes_presses(&mut self) -> bool {
        let ready = self.hold == 0;
        self.hold = self.hold.saturating_sub(1);
        ready
    }
    /// Starts listing `dir` on a worker thread.
    pub fn start(dir: Option<PathBuf>) -> Self {
        let listing: Listing = Arc::default();
        let slot = listing.clone();
        std::thread::spawn(move || {
            let saves = dir.map(|d| file::list(&d)).unwrap_or_default();
            if let Ok(mut slot) = slot.lock() {
                *slot = Some(saves);
            }
        });
        Self { saves: None, index: 0, listing: Some(listing), hold: REEL_HOLD }
    }

    /// The listing, once it has landed (true the frame it does).
    pub fn poll(&mut self) -> bool {
        let landed = self.listing.as_ref().and_then(|l| l.lock().ok().and_then(|mut s| s.take()));
        let Some(saves) = landed else { return false };
        self.listing = None;
        self.saves = Some(saves);
        self.index = 0;
        true
    }

    fn len(&self) -> usize {
        self.saves.as_ref().map_or(0, Vec::len)
    }

    /// Every CommonBrowser native the movie reads, for the current list and selection.
    pub fn answers(&self, localize: &dyn Fn(&str) -> String) -> Vec<(String, Json)> {
        let n = self.len();
        let mut out: Vec<(String, Json)> = vec![
            ("CommonBrowser.GetCurrentContentType".into(), json!(1)),
            // Local footage: RefreshButtons reads the Delete button only when not from WebKit.
            ("CommonBrowser.GetIsFromWebKit".into(), json!(false)),
            // TU3 0x826263e0 (video mode, vtable 0x823040c0 +0x18): not from WebKit, the title is 0x82208548.
            ("CommonBrowser.GetTitle".into(), json!("ID_COMMON_BROWSER_SKATE_REEL_VIDEO")),
            // RULING (build-progress, review I3): signed in on PC (as CanSaveMedia), so A reads Select, not Sign In.
            ("CommonBrowser.IsOnline".into(), json!(true)),
            ("CommonBrowser.GetHasOnlineContentError".into(), json!(false)),
            ("CommonBrowser.ShowSelectButton".into(), json!(n > 0)),
            ("CommonBrowser.ShowDeleteButton".into(), json!(n > 0)),
            ("CommonBrowser.ShowDeleteBookmarkButton".into(), json!(false)),
            ("CommonBrowser.ShowBookmarkButton".into(), json!(false)),
            ("CommonBrowser.ShowUploadButton".into(), json!(false)),
            ("CommonBrowser.ShowFeatureButton".into(), json!(false)),
            ("CommonBrowser.ShowPlayerFeatureButton".into(), json!(false)),
            ("CommonBrowser.ShouldShowSpaceAvailableText".into(), json!(true)),
            (
                "CommonBrowser.GetSpaceAvailableText".into(),
                json!(localize("ID_REPLAY_EDITOR_REPLAY_SPACE_LEFT").replace("{0:s}", &(MAX_SLOTS.saturating_sub(n)).to_string())),
            ),
            ("CommonBrowser.GetNumItems".into(), json!(n)),
            ("CommonBrowser.GetNumItemsForScrollBar".into(), json!(n)),
            ("CommonBrowser.GetCurrentMediaThumbIndex".into(), json!(self.index)),
            ("CommonBrowser.GetCurrentWindowIndex".into(), json!(self.index / MAX_THUMBNAILS)),
            // The video mode's tabs (TU3 0x82624860: top [0, 1]; 0x82624968: bottom [3, 4] under top tab 0), named
            // from table 0x83027ab0 (name, icon). Icon "" is tabs.as mNoIconString: plain text tabs, as retail's.
            ("CommonBrowser.GetTopLevelTabNames".into(), json!(["ID_COMMON_BROWSER_MY_FOOTAGE", "ID_COMMON_BROWSER_COMMUNITY"])),
            (
                "CommonBrowser.GetBottomLevelTabNames".into(),
                json!(["ID_COMMON_BROWSER_SAVED_VIDEOS", "ID_COMMON_BROWSER_SHARED_VIDEOS"]),
            ),
            ("CommonBrowser.GetBottomLevelTabIconNames".into(), json!(["", ""])),
            ("CommonBrowser.GetSelectedTopLevelTabIndex".into(), json!(0)),
            ("CommonBrowser.GetSelectedBottomLevelTabIndex".into(), json!(0)),
        ];
        for (i, (path, header)) in self.saves.iter().flatten().enumerate() {
            out.push((format!("CommonBrowser.GetMediaThumbData({i})"), thumb(i, path, header.as_ref())));
        }
        out
    }

    /// Opens the movie: answers, the key globals, the mode globals, OnLoaded, then WaitForData (or the list).
    pub fn boot(&self, screen: &mut Screen) -> Result<(), String> {
        let localize = |id: &str| screen.bindings.movie.text_assets.localize(id);
        let answers = self.answers(&localize);
        for (key, value) in answers {
            screen.answer(&key, value);
        }
        screen.set_global("AptOption", json!(APT_OPTION))?;
        screen.set_global("AptMisc", json!(APT_MISC))?;
        for name in MODES {
            screen.set_global(&format!("FECommonBrowserMode_{name}"), json!(mode(name)))?;
        }
        screen.play("intro").or(Ok::<(), String>(()))?;
        if screen.has("OnLoaded") {
            screen.call("OnLoaded", &[])?;
        }
        // Initialize only sizes the tab strips (InitTabs); SetupFilterTabs fills them. Unfilled, they draw their
        // authored stubs (bare LT/RT/LB/RB glyphs and backer pieces) over the panel: bug #33. Before the data state:
        // the movie's SetupFilterTabs only builds the tabs, so the last state set (WaitForData) still gates input.
        // Tab 0 of each strip: TU3 zero-fills the browser state (0x8263F020) and nothing writes its indices (+0x58/+0x5C).
        screen.call("SetState", &[json!(mode("SetupFilterTabs"))])?;
        self.refresh(screen)
    }

    /// The list (or the wait) on screen.
    pub fn refresh(&self, screen: &mut Screen) -> Result<(), String> {
        let localize = |id: &str| screen.bindings.movie.text_assets.localize(id);
        let answers = self.answers(&localize);
        for (key, value) in answers {
            screen.answer(&key, value);
        }
        let state = if self.saves.is_some() { "LocalVideoDataReady" } else { "WaitForData" };
        screen.call("SetState", &[json!(mode(state))]).map(drop)
    }

    /// Left / right: the selection, paging the window when it leaves the row.
    pub fn step(&mut self, screen: &mut Screen, right: bool) -> Result<(), String> {
        let n = self.len();
        if n == 0 {
            return Ok(());
        }
        let next = if right { (self.index + 1).min(n - 1) } else { self.index.saturating_sub(1) };
        if next == self.index {
            return Ok(());
        }
        let paged = next / MAX_THUMBNAILS != self.index / MAX_THUMBNAILS;
        self.index = next;
        if paged {
            return self.refresh(screen);
        }
        let localize = |id: &str| screen.bindings.movie.text_assets.localize(id);
        let answers = self.answers(&localize);
        for (key, value) in answers {
            screen.answer(&key, value);
        }
        screen.call("SetState", &[json!(mode("UpdateSelectedMediaThumb"))]).map(drop)
    }

    /// A (true) or Y (false) through the movie's own UpdateInput; what it asked for.
    pub fn press(&mut self, screen: &mut Screen, accept: bool) -> Result<Option<Pick>, String> {
        let _ = screen.take_calls();
        let code = if accept { crate::core_menu_runtime::Key::Accept.apt().1 as i64 } else { APT_OPTION };
        screen.call("UpdateInput", &[json!(code), json!(0)])?;
        let calls = screen.take_calls();
        let saves = self.saves.as_deref().unwrap_or_default();
        let picked = |args: &[Json]| args.first().and_then(Json::as_u64).and_then(|i| saves.get(i as usize)).cloned();
        for (call, args) in calls {
            match call.as_str() {
                "CommonBrowser.OnMediaThumbSelected" => return Ok(picked(&args).map(|(p, h)| Pick::Select(p, h))),
                "CommonBrowser.OnMediaThumbDelete" => return Ok(picked(&args).map(|(p, _)| Pick::Delete(p))),
                _ => {}
            }
        }
        Ok(None)
    }

    /// A save is gone (deleted): drop it from the list and keep the selection in range.
    pub fn remove(&mut self, path: &std::path::Path) {
        if let Some(saves) = self.saves.as_mut() {
            saves.retain(|(p, _)| p != path);
            self.index = self.index.min(saves.len().saturating_sub(1));
        }
    }
}

/// GetMediaThumbData's array (browser.as UpdateItemInfo / FillThumbs): [2] the memcard file, [5] creator, [6] rating,
/// [8] views, [9] location, [10] tags, [12] team, [13] uploaded, [16] hides the fields, [17] stars, [18] park, [19]
/// author, [20] DLC. A save fills the date and the map; a damaged one says so.
fn thumb(i: usize, path: &std::path::Path, header: Option<&Header>) -> Json {
    let dash = || json!("-");
    let mut data = vec![Json::Null; 21];
    data[2] = json!(i);
    for f in [5, 6, 8, 12, 19] {
        data[f] = dash();
    }
    data[10] = json!("ID_TAG_NONE");
    data[16] = json!(false);
    data[17] = json!(0);
    data[20] = json!(false);
    match header {
        Some(h) => {
            let [y, mo, d, hh, mi, _] = file::local(h.created_unix);
            data[13] = json!(format!("#{y:04}-{mo:02}-{d:02} {hh:02}:{mi:02}"));
            data[9] = json!(format!("#{} ({}:{:02})", h.map, h.seconds as u32 / 60, h.seconds as u32 % 60));
            data[18] = json!(h.map);
        }
        None => {
            data[13] = json!("#-");
            data[9] = json!(format!("#{}", path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned())));
            data[18] = dash();
        }
    }
    Json::Array(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// User 10-27: skate.Reel loaded the newest save by itself, the crossbar's A reaching the list a few updates
    /// after it opened. A fresh list takes no A or Y for REEL_HOLD updates, then does.
    #[test]
    fn a_fresh_list_ignores_the_press_that_opened_it() {
        let mut reel = Reel::start(None);
        assert!((0..REEL_HOLD).all(|_| !reel.takes_presses()), "held while it opens");
        assert!(reel.takes_presses() && reel.takes_presses(), "then every update");
    }
}
