//! Skate 3's music screens on the shared APT runtime (apt_screen, layer MUSIC), prepared by tools/prepare_music_ui.py:
//! the now-playing chyron, Audio Settings (the Game Settings `options` movie) and the Music Player (`musicplayertest`).
//! Their ActionScript runs as shipped; what retail's natives do (volume steps, song toggles, Play) is done here.
use crate::{
    apt_screen::{Layer, MUSIC, Screen, Screens},
    core_menu_runtime::{Key, SOUND_A_BUTTON, SOUND_B_BUTTON, fe_sound_id},
    customiser::Navigation,
    graphics_menu::Menu,
    music::{AudioSettings, Music, Track},
    skate_audio::FrontEndSoundRequest,
};
use bevy::prelude::*;
use serde_json::{Value as Json, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Panel {
    /// Game Settings > Audio Settings: SFX, Dialog and Music Volume (and Soundtrack while the music folder has songs).
    AudioSettings,
    /// Options > Music Player: Volume and the song list (Playlist Mode tab).
    Player,
}

#[derive(Resource, Default)]
pub(crate) struct MusicUi {
    /// core_menu's pick: open this panel.
    pub request: Option<Panel>,
    /// A panel is open: it owns input and the crossbar hides.
    pub modal: bool,
    /// The panel closed this frame; core_menu ignores that press.
    pub closed_this_frame: bool,
    open: Option<Panel>,
    /// Seconds since B played the panel's outro.
    closing: Option<f32>,
    /// Seconds since the chyron's Display.
    chyron: Option<f32>,
    /// The Music Player's speaker shows this track.
    speaker: Option<String>,
    /// The chyron manifest is missing (warned once).
    no_chyron: bool,
    /// The chyron was visible last frame ([`flagged`]; its gate acts on changes).
    at: bool,
    /// The songs the open Music Player lists (a rescan that changes them redraws the list).
    listed: Vec<Track>,
    /// Seconds since the open Music Player last read the music folder.
    rescanned: f32,
}

/// Bug 10-17 #36 (not retail): the Music Player's help bar also shows Y, Open Music Folder. The movie's panel
/// (musicplayer::Initialise: mMusicPlayerTest.panel, its buttonHelp a button_menu) rebuilds the bar (ClearButtonItems)
/// when edit mode changes, so this is added again after each MusicPlayer.SetEditMode.
fn folder_help(screen: &mut Screen) {
    let added = screen
        .call_at("mMusicPlayerTest.panel", "AddButtonItem", &[json!("Y"), json!(crate::apt_text::literal("Open Music Folder"))])
        .and_then(|_| screen.call_at("mMusicPlayerTest.panel", "RefreshButtonHelp", &[]));
    if let Err(error) = added {
        warn!("Music Player: no Open Music Folder help ({error})");
    }
}

/// The chyron's screen: drawn over a panel (screens draw in name order), as retail intros it over the Music Player.
const CHYRON: &str = "z-chyron";
/// The chyron's 18-frame intro (frame 'shown' calls IntroDone), retail's 5000 ms hold after it (825D8B50), then its
/// 15-frame outro.
const CHYRON_OUTRO: f32 = 18. / 60. + 5.;
const CHYRON_GONE: f32 = CHYRON_OUTRO + 16. / 60.;
/// Outro fallback if a panel never reports OutroComplete.
const CLOSE_TIMEOUT: f32 = 0.6;
/// Audio Settings rows: (title id, AudioSettings::volumes slot, retail's [decrease, increase] GlobalFEPlaySound ids
/// from the setter at 0x8260CD44).
const SLIDERS: [(&str, usize, [u64; 2]); 4] = [
    ("ID_GAMESETTINGS_SFXVOLUME", crate::music::SFX, [0xFE3E_1FA3_6930_1FB5, 0xE100_8976_1F0C_EFF2]),
    ("ID_GAMESETTINGS_DIALOGVOLUME", crate::music::DIALOG, [0x64C2_0226_4FE3_9DFF, 0x55C3_6E54_4584_9A99]),
    ("ID_GAMESETTINGS_MUSICVOLUME", crate::music::MUSIC, [0xDC1C_3539_8D55_97BC, 0x4648_BEBA_06CE_C261]),
    // Not retail (user 10-14, feature #10); SFX Volume's sounds.
    ("Rain Volume", crate::music::RAIN, [0xFE3E_1FA3_6930_1FB5, 0xE100_8976_1F0C_EFF2]),
];
/// The selector rows' Left/Right sounds (every retail selector, the SFX pack row included).
const SELECTOR_SOUNDS: [u64; 2] = [0xAD8B_F9A0_B499_7B49, 0xD8EE_38A6_1C45_EA28];
/// musicplayertest's kState_Player / kState_Edit, and its refresh states.
const PLAYER: i64 = 1;
const EDIT: i64 = 2;
const REFRESH_PLAYER: i64 = 60;
const REFRESH_PLAYLIST: i64 = 63;
/// The options movie's "refresh every row" state.
const REFRESH_OPTIONS: i64 = 50;
/// `_global.AptMisc`, the X press musicplayertest's UpdateInput reads (native_vm defines only the six it maps).
const APT_MISC: i64 = 7;

/// What Audio Settings' `Game.GameSettings_*` calls answer. The Soundtrack row (INFERRED, styled as retail's SFX pack
/// selector, which shows only when there is a choice) appears while the music folder has songs.
fn options_answers(audio: &AudioSettings, soundtrack: bool) -> Vec<(String, Json)> {
    let mut answers = vec![
        ("Game.GameSettings_GetScreenTitle".to_owned(), json!("ID_GAMESETTINGS_AUDIO_SETTINGS")),
        ("Game.GameSettings_GetNumOptions".to_owned(), json!(SLIDERS.len() + usize::from(soundtrack))),
    ];
    for (row, (title, slot, _)) in SLIDERS.iter().enumerate() {
        answers.push((format!("Game.GameSettings_GetOptionType({row})"), json!("slider")));
        answers.push((format!("Game.GameSettings_GetOptionTitle({row})"), json!(title)));
        answers.push((format!("Game.GameSettings_GetIntegerValue({row})"), json!(audio.level(*slot))));
    }
    let (value, description) = match (audio.my_music, audio.both) {
        (true, false) => ("My Music", "Your songs from the music folder play instead of the game's."),
        (true, true) => ("Both", "Your songs from the music folder shuffle in with the game's soundtrack."),
        (false, _) => ("Game Music", "The game's soundtrack plays."),
    };
    let row = SLIDERS.len();
    answers.push((format!("Game.GameSettings_GetOptionType({row})"), json!("selector")));
    answers.push((format!("Game.GameSettings_GetOptionTitle({row})"), json!("Soundtrack")));
    answers.push((format!("Game.GameSettings_GetStringValue({row})"), json!(value)));
    answers.push((format!("Game.GameSettings_GetValueDescription({row})"), json!(description)));
    answers
}

/// What the Music Player's `MusicPlayer.*` calls answer: song ids are 1-based positions in the playing library
/// (retail's are its song enum, 1..=46, in the same order).
fn player_answers(audio: &AudioSettings, library: &[Track], playing: Option<&str>) -> Vec<(String, Json)> {
    let playing = playing.and_then(|key| library.iter().position(|t| t.key == key)).map_or(0, |i| i + 1);
    let mut answers = vec![
        ("MusicPlayer.GetDefaultSongList".to_owned(), json!((1..=library.len()).collect::<Vec<_>>())),
        ("MusicPlayer.GetMusicVolume".to_owned(), json!(f32::from(audio.volumes[crate::music::MUSIC]) / 10.)),
        ("MusicPlayer.GetMusicPlayerSong".to_owned(), json!(playing)),
        ("MusicPlayer.GetPlayList".to_owned(), json!([])),
    ];
    for (i, track) in library.iter().enumerate() {
        answers.push((format!("MusicPlayer.GetSongInfo({})", i + 1), json!([track.artist, track.title, track.album])));
        answers.push((format!("MusicPlayer.IsSongInPlayList({i})"), json!(!audio.songs_off.contains(&track.key))));
    }
    answers
}

impl MusicUi {
    /// The open panel (music::sequence's tape stop follows it).
    pub(crate) fn panel(&self) -> Option<Panel> {
        self.open
    }
}

fn answer_all(screen: &mut Screen, answers: Vec<(String, Json)>) {
    for (key, value) in answers {
        screen.answer(&key, value);
    }
}

fn open(layer: &mut Layer, panel: Panel, audio: &AudioSettings, music: &Music) -> Result<(), String> {
    let (movie, answers) = match panel {
        Panel::AudioSettings => ("options", options_answers(audio, music.has_custom())),
        Panel::Player => {
            let playing = music.playing.as_ref().map(|(_, key)| key.as_str());
            ("musicplayertest", player_answers(audio, music.library(audio), playing))
        }
    };
    let borrowed: Vec<(&str, Json)> = answers.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    let screen = layer.open("panel", movie, &borrowed)?;
    screen.set_global("AptMisc", json!(APT_MISC))?;
    screen.play("intro")?;
    if screen.has("OnLoaded") {
        screen.call("OnLoaded", &[])?;
    }
    screen.call("SetState", &[json!(1)])?;
    if panel == Panel::Player {
        screen.call("SetState", &[json!(REFRESH_PLAYLIST)])?;
    }
    Ok(())
}

/// One Left/Right step of a retail volume (tenths; 0..=10).
fn step(volume: u8, right: bool) -> u8 {
    if right { (volume + 1).min(10) } else { volume.saturating_sub(1) }
}

/// The Music Player's X: switch song `i` of `library` on or off. Retail keeps one on ("There must be at least one
/// song in a playlist."); false when refused.
fn toggle(audio: &mut AudioSettings, library: &[Track], i: usize) -> bool {
    let Some(key) = library.get(i).map(|t| &t.key) else { return false };
    if let Some(at) = audio.songs_off.iter().position(|k| k == key) {
        audio.songs_off.remove(at);
        return true;
    }
    if library.iter().filter(|t| !audio.songs_off.contains(&t.key)).count() <= 1 {
        return false;
    }
    audio.songs_off.push(key.clone());
    true
}

pub(crate) struct MusicUiPlugin;
impl Plugin for MusicUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MusicUi>()
            .add_systems(PreUpdate, interact.after(crate::customiser::navigation).before(crate::core_menu::interact))
            .add_systems(PreUpdate, open_requested.after(crate::core_menu::interact).before(crate::graphics_menu::MenuInput))
            .add_systems(Update, chyron.after(crate::music::sequence).before(crate::apt_screen::render));
    }
}

/// perf 10-18: the crossbar's pick opens on its own frame (retail_ui::open_requested). SKATE_SAME_FRAME_OPEN=0: a frame
/// later, in `interact`.
fn open_requested(mut ui: ResMut<MusicUi>, menu: Res<Menu>, music: Option<ResMut<Music>>, screens: Option<ResMut<Screens>>) {
    if let (Some(mut music), Some(mut screens)) = (music, screens)
        && ui.request.is_some()
        && crate::config::perf_on("SKATE_SAME_FRAME_OPEN")
    {
        open_request(&mut ui, screens.layer(MUSIC), &menu, &mut music);
    }
}
fn open_request(ui: &mut MusicUi, layer: &mut Layer, menu: &Menu, music: &mut Music) {
    if let Some(panel) = ui.request.take() {
        match open(layer, panel, menu.audio(), music) {
            Ok(()) => {
                // The chyron shows over it (flag 1, see `chyron`).
                (ui.open, ui.closing) = (Some(panel), None);
                ui.speaker = music.playing.as_ref().map(|(_, key)| key.clone());
                if panel == Panel::Player {
                    // Bug 10-17 #36: songs added while the game runs show up the next time the player opens.
                    music.rescan(crate::music::folder());
                    ui.rescanned = 0.;
                    ui.listed = music.library(menu.audio()).to_vec();
                    if let Some(screen) = layer.get("panel") {
                        folder_help(screen);
                    }
                }
            }
            Err(error) => {
                error!("Music screen: {error} (prepare it with tools/prepare_runtime_huds.py)");
                layer.close("panel");
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn interact(
    mut ui: ResMut<MusicUi>,
    (keys, nav): (Res<ButtonInput<KeyCode>>, Res<Navigation>),
    mut menu: ResMut<Menu>,
    music: Option<ResMut<Music>>,
    screens: Option<ResMut<Screens>>,
    mut sounds: MessageWriter<FrontEndSoundRequest>,
    real: Res<Time<Real>>,
) {
    let ui = &mut *ui;
    ui.closed_this_frame = false;
    let (Some(mut music), Some(mut screens)) = (music, screens) else {
        ui.request = None;
        return;
    };
    let layer = screens.layer(MUSIC);
    open_request(ui, layer, &menu, &mut music);
    let Some(panel) = ui.open else {
        (ui.modal, layer.modal) = (false, false);
        return;
    };
    let Some(screen) = layer.get("panel") else {
        // The screen stopped (apt_screen logged it).
        (ui.open, ui.modal, layer.modal, ui.closed_this_frame) = (None, false, false, true);
        return;
    };
    if let Some(closing) = ui.closing.as_mut() {
        *closing += real.delta_secs();
        if screen.bindings.outro_complete || *closing >= CLOSE_TIMEOUT {
            layer.close("panel");
            (ui.open, ui.closing, ui.modal, layer.modal, ui.closed_this_frame) = (None, None, false, false, true);
        }
        return;
    }
    // Not retail (owner 10-18): while the player is open the folder is read again each second, so a song dropped in
    // is listed at once. ponytail: polling; a file watcher if a huge folder makes the 1 s walk show.
    ui.rescanned += real.delta_secs();
    if panel == Panel::Player && ui.rescanned >= 1. {
        ui.rescanned = 0.;
        music.rescan(crate::music::folder());
    }
    let mut audio = menu.audio().clone();
    let library = music.library(&audio).to_vec();
    let pressed = [
        (nav.pressed & 1 != 0, Key::Up),
        (nav.pressed & 2 != 0, Key::Down),
        (nav.pressed & 4 != 0, Key::Left),
        (nav.pressed & 8 != 0, Key::Right),
        (keys.just_pressed(KeyCode::Enter) || nav.pressed & 0x1000 != 0, Key::Accept),
        (keys.just_pressed(KeyCode::Backspace) || nav.pressed & 0x2000 != 0, Key::Back),
    ];
    let x = keys.just_pressed(KeyCode::KeyX) || nav.pressed & 0x4000 != 0;
    let y = keys.just_pressed(KeyCode::KeyY) || nav.pressed & 0x8000 != 0;
    let field = |screen: &Screen, name: &str| screen.field(name).as_i64().unwrap_or(-1);
    let result = (|| -> Result<(), String> {
        for key in pressed.into_iter().filter(|(down, _)| *down).map(|(_, key)| key) {
            match (panel, key) {
                (_, Key::Back) => {
                    sounds.write(FrontEndSoundRequest(SOUND_B_BUTTON));
                    ui.closing = Some(0.);
                    return screen.play("outro");
                }
                // Retail's native Left/Right (0x8260CD44): the row's sound, the value 0.1 a press, refresh.
                (Panel::AudioSettings, Key::Left | Key::Right) => {
                    let (row, right) = (field(screen, "mCurrentIndex"), key == Key::Right);
                    if let Some((_, slot, ids)) = usize::try_from(row).ok().and_then(|r| SLIDERS.get(r)) {
                        sounds.write(FrontEndSoundRequest(ids[usize::from(right)]));
                        let level = step(audio.level(*slot), right);
                        *audio.level_mut(*slot) = level;
                    } else if row == SLIDERS.len() as i64 && music.has_custom() {
                        sounds.write(FrontEndSoundRequest(SELECTOR_SOUNDS[usize::from(right)]));
                        crate::music::step_soundtrack(&mut audio, right);
                    } else {
                        continue;
                    }
                    answer_all(screen, options_answers(&audio, music.has_custom()));
                    screen.call("SetState", &[json!(REFRESH_OPTIONS)])?;
                }
                // The Volume slider, while it is the Music Player's selected control (INFERRED sounds: Music Volume's).
                (Panel::Player, Key::Left | Key::Right) if field(screen, "mState") == PLAYER => {
                    let right = key == Key::Right;
                    sounds.write(FrontEndSoundRequest(SLIDERS[2].2[usize::from(right)]));
                    audio.volumes[crate::music::MUSIC] = step(audio.volumes[crate::music::MUSIC], right);
                    answer_all(screen, player_answers(&audio, &library, ui.speaker.as_deref()));
                    screen.call("SetState", &[json!(REFRESH_PLAYER)])?;
                }
                // A on a song: play it now (the speaker moves when it starts).
                (Panel::Player, Key::Accept) if field(screen, "mState") == EDIT => {
                    let selected = screen.field("mSelectedIndex")[EDIT as usize].as_i64().unwrap_or(0);
                    let i = field(screen, "mPlayListFirstIndex").max(0) + selected;
                    if let Some(track) = library.get(i as usize) {
                        sounds.write(FrontEndSoundRequest(SOUND_A_BUTTON));
                        music.play_now = Some(track.key.clone());
                    }
                }
                (_, key) => screen.input(key)?,
            }
        }
        // X on a song: retail toggles it natively, then the movie (AptMisc) redraws its checkbox.
        if x && panel == Panel::Player && field(screen, "mState") == EDIT {
            let selected = screen.field("mSelectedIndex")[EDIT as usize].as_i64().unwrap_or(0);
            let i = field(screen, "mPlayListFirstIndex").max(0) + selected;
            // ponytail: refusing the last song plays B; retail opens ID_MUSIC_PLAYER_ONE_SONG_DESC's popup.
            let toggled = toggle(&mut audio, &library, i as usize);
            sounds.write(FrontEndSoundRequest(if toggled { SOUND_A_BUTTON } else { SOUND_B_BUTTON }));
            answer_all(screen, player_answers(&audio, &library, ui.speaker.as_deref()));
            screen.call("UpdateInput", &[json!(APT_MISC), json!(0)])?;
        }
        if panel == Panel::Player && screen.take_calls().iter().any(|(call, _)| call == "MusicPlayer.SetEditMode") {
            folder_help(screen);
        }
        // Bug 10-17 #36: Y opens the music folder (a song dropped in is listed within a second).
        if y && panel == Panel::Player {
            let folder = crate::music::folder();
            match crate::config::open_folder(&folder) {
                Ok(()) => sounds.write(FrontEndSoundRequest(SOUND_A_BUTTON)),
                Err(error) => {
                    warn!("Music Player: could not open {}: {error}", folder.display());
                    sounds.write(FrontEndSoundRequest(SOUND_B_BUTTON))
                }
            };
        }
        // The speaker follows the song that plays; a rescan that changed the songs redraws the list.
        let playing = music.playing.as_ref().map(|(_, key)| key.clone());
        if panel == Panel::Player && (playing != ui.speaker || ui.listed != library) {
            ui.speaker = playing;
            ui.listed.clone_from(&library);
            answer_all(screen, player_answers(&audio, &library, ui.speaker.as_deref()));
            screen.call("SetState", &[json!(REFRESH_PLAYLIST)])?;
        }
        Ok(())
    })();
    sounds.write_batch(screen.bindings.sounds.drain(..).map(|key| FrontEndSoundRequest(fe_sound_id(&key))));
    if let Err(error) = result {
        error!("Music screen stopped: {error}");
        layer.close("panel");
        (ui.open, ui.closing, ui.closed_this_frame) = (None, None, true);
    }
    menu.set_audio(audio);
    ui.modal = ui.open.is_some();
    layer.modal = ui.modal;
}

/// The FE state's chyron flag (table 0x83026088, gate 825DA4D8): 1 for the Music Player (musicplayer), Audio Settings
/// (options), CAS (cac) and the Challenge Map (challengemap); 0 for idle (free skate), the crossbar, replay2 and every
/// other screen here. The chyron starts hidden (825d7538 and its AS constructor) and only the gate's flag-1 path shows it
/// (SetVisible 825E4408's other caller, 825EBF40, is UNTRACED).
fn flagged(panel: Option<Panel>, customiser: bool, challenge_map: bool) -> bool {
    panel.is_some() || customiser || challenge_map
}
/// Whether a song starting now shows: over the flag-1 screens, and in free skate (`idle`: no crossbar, no replay). The
/// user saw retail show new songs while skating (bug list 10-14 #10), so 825EBF40 is INFERRED to be idle's show path;
/// entering idle doesn't show the playing song again (the gate's flag-1 `again`). ponytail: loading screens count as idle.
fn shows_new_song(flagged: bool, idle: bool) -> bool {
    flagged || idle
}
#[derive(Debug, Default, PartialEq, Eq)]
struct Gate {
    /// Flag 0: SetVisible(false) plays the outro of an intro or a shown chyron (825DA55C).
    outro: bool,
    /// Flag 1 while not visible: SetVisible(true), then Display of the playing song (825DA5D4 / 5E4).
    again: bool,
}
fn gate(was: bool, now: bool, shown: Option<f32>) -> Gate {
    Gate { outro: was && !now && shown.is_some_and(|s| s < CHYRON_OUTRO), again: now && !was }
}

/// Retail 8249A7E8: a song that starts while Music Volume is up shows its artist, title and album while the chyron is
/// visible ([`flagged`]); 825D8B50 plays the outro 5 s after the intro.
fn chyron(
    mut ui: ResMut<MusicUi>,
    music: Option<ResMut<Music>>,
    menu: Res<Menu>,
    (customiser, challenge_map): (Option<Res<crate::customiser::Customiser>>, Option<Res<crate::challenge_map::ChallengeMap>>),
    (crossbar, replay): (Option<Res<crate::core_menu::Crossbar>>, Option<Res<crate::replay::Replay>>),
    screens: Option<ResMut<Screens>>,
    real: Res<Time<Real>>,
) {
    let (Some(mut music), Some(mut screens)) = (music, screens) else { return };
    let ui = &mut *ui;
    let layer = screens.layer(MUSIC);
    let idle = crossbar.is_none_or(|c| c.runtime.is_none()) && replay.is_none_or(|r| !r.active);
    let now = flagged(ui.open, customiser.is_some_and(|c| c.open), challenge_map.is_some_and(|c| c.active()));
    let gate = gate(std::mem::replace(&mut ui.at, now), now, ui.chyron);
    // Over the flag-1 screens the layer draws while the graphics menu is up.
    layer.modal |= now && ui.chyron.is_some();
    if gate.outro {
        if let Some(Err(error)) = layer.get(CHYRON).map(|s| s.call("PlayOutro", &[])) {
            warn!("Now-playing popup: {error}");
        }
        ui.chyron = Some(CHYRON_OUTRO);
    }
    let again = gate.again.then(|| {
        let key = music.playing.as_ref().map(|(_, key)| key.clone())?;
        music.library(menu.audio()).iter().find(|t| t.key == key).cloned()
    });
    if let Some(track) = music.started.take().or(again.flatten()) {
        // ponytail: retail skips the chyron for the console's own music (825E4380); this shows My Music too, by request.
        if shows_new_song(now, idle) && !ui.no_chyron {
            // Loaded once and kept, hidden between songs, as retail keeps it: opening it took ~15 ms of a song's first
            // frame (report 10-30: an FPS drop at each song change).
            let opened = if layer.get(CHYRON).is_none() { layer.open(CHYRON, "chyron", &[]).map(drop) } else { Ok(()) };
            let shown = opened.and_then(|()| {
                let screen = layer.get(CHYRON).ok_or_else(|| "not open".to_string())?;
                screen.call("SetVisible", &[json!(true)])?;
                screen.call("Display", &[json!(track.artist), json!(track.title), json!(track.album)])
            });
            match shown {
                Ok(_) => ui.chyron = Some(0.),
                Err(error) => {
                    warn!("Now-playing popup unavailable: {error}");
                    ui.no_chyron = true;
                    layer.close(CHYRON);
                }
            }
        }
    }
    if let Some(shown) = ui.chyron.as_mut() {
        let before = *shown;
        *shown += real.delta_secs();
        if before < CHYRON_OUTRO && *shown >= CHYRON_OUTRO {
            if let Some(Err(error)) = layer.get(CHYRON).map(|s| s.call("PlayOutro", &[])) {
                warn!("Now-playing popup: {error}");
            }
        }
        if *shown >= CHYRON_GONE {
            ui.chyron = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::music::Source;
    use std::path::PathBuf;

    fn track(n: usize) -> Track {
        Track {
            key: format!("file:{n}.mp3"),
            artist: format!("Artist {n}"),
            title: format!("Title {n}"),
            album: String::new(),
            source: Source::File(PathBuf::from(format!("{n}.mp3"))),
        }
    }

    #[test]
    fn the_last_song_stays_on() {
        let library: Vec<Track> = (0..2).map(track).collect();
        let mut audio = AudioSettings::default();
        assert!(toggle(&mut audio, &library, 0));
        assert!(!toggle(&mut audio, &library, 1), "one song must stay on");
        assert!(toggle(&mut audio, &library, 0), "back on");
        assert!(audio.songs_off.is_empty());
        assert_eq!((step(10, true), step(0, false), step(5, false)), (10, 0, 4));
    }

    /// The FE state gate (825DA4D8): flag-1 screens (the Music Player, Audio Settings, CAS, the Challenge Map) show the
    /// chyron and show the playing song again on entry; everything else hides it, an intro or a shown chyron through its
    /// outro. Free skate shows a new song (user 10-14 #10).
    #[test]
    fn the_chyron_follows_retails_screen_flags() {
        assert!(flagged(Some(Panel::Player), false, false) && flagged(Some(Panel::AudioSettings), false, false));
        assert!(flagged(None, true, false) && flagged(None, false, true), "cac, challengemap");
        assert!(!flagged(None, false, false), "idle, the crossbar, replay2");
        assert!(shows_new_song(false, true), "user 10-14 #10: free skate shows the new song");
        assert!(!shows_new_song(false, false), "the crossbar, replay2");
        assert_eq!(gate(false, true, None), Gate { outro: false, again: true }, "entering the Music Player");
        assert_eq!(gate(true, false, Some(1.)), Gate { outro: true, again: false }, "back to the crossbar mid-intro");
        assert_eq!(gate(true, false, Some(CHYRON_OUTRO + 0.1)), Gate::default(), "already in its outro");
        assert_eq!(gate(false, false, None), Gate::default(), "entering free skate doesn't repeat the song");
        assert_eq!(gate(true, true, Some(1.)), Gate::default());
    }

    /// The prepared music screens (SKATE3_ASSET_ROOT/private/music-ui), driven as the game drives them.
    fn root() -> Option<PathBuf> {
        let root = PathBuf::from(std::env::var_os("SKATE3_ASSET_ROOT")?).join("private/music-ui");
        root.join("runtime/chyron.json").is_file().then_some(root)
    }
    fn run(screen: &mut Screen, frames: usize) {
        (0..frames).for_each(|_| screen.advance().unwrap());
    }
    /// What the screen's text fields show (localised).
    fn texts(screen: &Screen) -> Vec<String> {
        let mut out: Vec<String> = (screen.bindings.movie.instances.keys())
            .filter_map(|&id| match screen.vm.get(id, "_displayText") {
                crate::apt_vm::Value::Text(t) if !t.is_empty() => Some(t),
                _ => None,
            })
            .collect();
        out.sort();
        out
    }
    fn shoot(screen: &Screen, root: &std::path::Path, name: &str) {
        if let Some(out) = std::env::var_os("SKATE_MUSIC_SHOTS").map(PathBuf::from) {
            crate::core_menu_runtime::tests::raster_draws(&screen.draws().unwrap(), root, &out.join(format!("{name}.png")));
        }
    }

    /// Report 10-30: `chyron` keeps the popup loaded between songs. After its outro it draws nothing (so kept open it
    /// shows nothing), and the next song shows on the same screen (SKATE3_ASSET_ROOT).
    #[test]
    fn chyron_shows_each_song_on_the_screen_it_keeps() {
        let Some(root) = root() else { return };
        let mut screen = Screen::load(&root.join("runtime/chyron.json"), &[]).unwrap();
        for (artist, title) in [("Pixies", "Debaser"), ("Motorhead", "Ace of Spades")] {
            screen.call("SetVisible", &[json!(true)]).unwrap();
            screen.call("Display", &[json!(artist), json!(title), json!("")]).unwrap();
            run(&mut screen, 30);
            let texts = texts(&screen);
            assert!(texts.iter().any(|t| t == artist) && texts.iter().any(|t| t == title), "{artist}: {texts:?}");
            assert!(!screen.draws().unwrap().is_empty(), "{artist} shown");
            screen.call("PlayOutro", &[]).unwrap();
            run(&mut screen, 20);
            assert!(screen.draws().unwrap().is_empty(), "after {artist}'s outro it draws nothing");
        }
    }

    /// Report 10-30 ("an FPS drop when the song changes"): the now-playing popup's cost on the frame a song starts (its
    /// manifest read and built, a first frame), on the frame it closes (dropped), and each frame it would stay open
    /// after its outro. Measurement only: SKATE3_ASSET_ROOT, --nocapture.
    #[test]
    #[ignore = "measurement"]
    fn chyron_open_close_and_idle_cost() {
        let Some(root) = root() else { return };
        let file = root.join("runtime/chyron.json");
        let ms = |t: std::time::Instant| t.elapsed().as_secs_f64() * 1000.;
        let (mut open, mut close, mut idle) = (vec![], vec![], vec![]);
        for _ in 0..8 {
            let t = std::time::Instant::now();
            let mut screen = Screen::load(&file, &[]).unwrap();
            screen.call("SetVisible", &[json!(true)]).unwrap();
            screen.call("Display", &[json!("Artist"), json!("Title"), json!("Album")]).unwrap();
            screen.advance().unwrap();
            drop(screen.draws().unwrap());
            open.push(ms(t));
            run(&mut screen, 18);
            screen.call("PlayOutro", &[]).unwrap();
            run(&mut screen, 20);
            let t = std::time::Instant::now();
            for _ in 0..60 {
                screen.advance().unwrap();
                drop(screen.draws().unwrap());
            }
            idle.push(ms(t) / 60.);
            let t = std::time::Instant::now();
            drop(screen);
            close.push(ms(t));
        }
        let median = |v: &mut Vec<f64>| {
            v.sort_by(f64::total_cmp);
            v[v.len() / 2]
        };
        eprintln!(
            "chyron: open {:.1} ms, close {:.1} ms, open but idle {:.3} ms a frame (medians of 8; first open {:.1} ms)",
            median(&mut open.clone()),
            median(&mut close),
            median(&mut idle),
            open[0]
        );
    }

    #[test]
    fn chyron_shows_the_song() {
        let Some(root) = root() else { return };
        let mut screen = Screen::load(&root.join("runtime/chyron.json"), &[]).unwrap();
        screen.call("SetVisible", &[json!(true)]).unwrap();
        let long = "A Title Far Too Long To Fit The Now Playing Popup Without Being Cut Short";
        screen.call("Display", &[json!("Pixies"), json!(long), json!("Doolittle")]).unwrap();
        run(&mut screen, 30);
        let texts = texts(&screen);
        assert!(texts.iter().any(|t| t == "Pixies"), "{texts:?}");
        assert!(texts.iter().any(|t| t == "Doolittle"), "{texts:?}");
        assert!(texts.iter().any(|t| t.starts_with("A Title")), "{texts:?}");
        assert!(!screen.draws().unwrap().is_empty());
        // Bug 10-17 #37: retail's widescreen spot. TU3 0x825D95A8 skips SetScreenEdgeOffset(136) on a widescreen
        // display, so Chyron_SetVisible's mScreen._x = Screen_EdgeOffset is 0 and mcChyron stays at stage (82, 523).
        let chyron = screen.clip("mcChyron").expect("mcChyron");
        let m = screen.stage_matrix(chyron);
        assert_eq!((m[4].round(), m[5].round()), (82., 523.), "{m:?}");
        let edge = screen.vm.get(screen.bindings.movie.root, "Screen_EdgeOffset").number();
        assert_eq!(edge, 0.);
        shoot(&screen, &root, "chyron");
        screen.call("PlayOutro", &[]).unwrap();
        run(&mut screen, 20);
    }

    #[test]
    fn audio_settings_slide_like_retail() {
        let Some(root) = root() else { return };
        let mut audio = AudioSettings::default();
        audio.volumes = [10, 7, 3];
        let answers = options_answers(&audio, true);
        let borrowed: Vec<(&str, Json)> = answers.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        let mut screen = Screen::load(&root.join("runtime/options.json"), &borrowed).unwrap();
        screen.play("intro").unwrap();
        screen.call("OnLoaded", &[]).unwrap();
        screen.call("SetState", &[json!(1)]).unwrap();
        run(&mut screen, 40);
        assert_eq!(screen.field("mNumOptions"), json!(5));
        assert_eq!(screen.field("mCurrentIndex"), json!(0), "the first row is selected");
        let shown = texts(&screen);
        for want in ["Audio Settings", "SFX Volume", "Dialog Volume", "Music Volume", "Rain Volume", "100%", "70%", "30%", "Soundtrack", "My Music"] {
            assert!(shown.iter().any(|t| t == want), "{want} missing from {shown:?}");
        }
        shoot(&screen, &root, "audio-settings");
        screen.input(Key::Down).unwrap();
        screen.input(Key::Down).unwrap();
        assert_eq!(screen.field("mCurrentIndex"), json!(2), "Music Volume");
        assert_eq!(screen.bindings.sounds, ["core_nav_down", "core_nav_down"]);
        audio.volumes[2] = step(audio.volumes[2], false);
        answer_all(&mut screen, options_answers(&audio, true));
        screen.call("SetState", &[json!(REFRESH_OPTIONS)]).unwrap();
        run(&mut screen, 10);
        assert!(texts(&screen).iter().any(|t| t == "20%"));
        shoot(&screen, &root, "audio-settings-music-20");
        // Not retail (user 10-14, feature #10): the rain's own slider, under Music.
        screen.input(Key::Down).unwrap();
        assert_eq!(screen.field("mCurrentIndex"), json!(3), "Rain Volume");
        *audio.level_mut(crate::music::RAIN) = step(audio.level(crate::music::RAIN), false);
        answer_all(&mut screen, options_answers(&audio, true));
        screen.call("SetState", &[json!(REFRESH_OPTIONS)]).unwrap();
        run(&mut screen, 10);
        assert!(texts(&screen).iter().any(|t| t == "90%"));
        assert_eq!(audio.volumes, [10, 7, 2], "rain is its own setting");
    }

    #[test]
    fn music_player_lists_toggles_and_plays() {
        let Some(root) = root() else { return };
        let library: Vec<Track> = (0..14).map(track).collect();
        let mut audio = AudioSettings::default();
        audio.songs_off.push(library[1].key.clone());
        let answers = player_answers(&audio, &library, Some(&library[2].key));
        let borrowed: Vec<(&str, Json)> = answers.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        let mut screen = Screen::load(&root.join("runtime/musicplayertest.json"), &borrowed).unwrap();
        screen.set_global("AptMisc", json!(APT_MISC)).unwrap();
        screen.play("intro").unwrap();
        screen.call("OnLoaded", &[]).unwrap();
        screen.call("SetState", &[json!(1)]).unwrap();
        screen.call("SetState", &[json!(REFRESH_PLAYLIST)]).unwrap();
        run(&mut screen, 40);
        let shown = texts(&screen);
        for want in ["Title 0", "Artist 0", "Title 10", "100%"] {
            assert!(shown.iter().any(|t| t == want), "{want} missing from {shown:?}");
        }
        assert!(!shown.iter().any(|t| t == "Title 11"), "11 rows show (kPlayListSize)");
        shoot(&screen, &root, "music-player");
        assert_eq!(screen.field("mState"), json!(PLAYER));
        // Down from the Volume slider enters the list.
        screen.input(Key::Down).unwrap();
        assert_eq!(screen.field("mState"), json!(EDIT));
        assert!(screen.take_calls().iter().any(|(call, args)| call == "MusicPlayer.SetEditMode" && args == &[json!(true)]));
        // Bug 10-17 #36: the help bar takes Y, Open Music Folder, through the movie's own panel.
        folder_help(&mut screen);
        assert!(screen.call_at("mMusicPlayerTest.panel", "RefreshButtonHelp", &[]).is_ok(), "the panel path is the movie's");
        run(&mut screen, 10);
        assert!(texts(&screen).iter().any(|t| t.contains("Open Music Folder")), "{:?}", texts(&screen));
        screen.input(Key::Down).unwrap();
        assert_eq!(screen.field("mSelectedIndex")[EDIT as usize], json!(1));
        // X turns song 1 back on and its checkbox redraws.
        assert!(toggle(&mut audio, &library, 1));
        answer_all(&mut screen, player_answers(&audio, &library, Some(&library[2].key)));
        screen.call("UpdateInput", &[json!(APT_MISC), json!(0)]).unwrap();
        assert!(screen.take_calls().iter().any(|(call, args)| call == "MusicPlayer.IsSongInPlayList" && args == &[json!(1)]));
        run(&mut screen, 20);
        shoot(&screen, &root, "music-player-edit");
        // The list scrolls past its 11 rows.
        for _ in 0..12 {
            screen.input(Key::Down).unwrap();
        }
        run(&mut screen, 20);
        assert!(texts(&screen).iter().any(|t| t == "Title 13"));
        screen.play("outro").unwrap();
        run(&mut screen, 60);
    }
}
