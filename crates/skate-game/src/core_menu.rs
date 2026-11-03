//! Start-button crossbar: Skate 3's original main/core_menu movie, filled with this
//! engine's features. Missing or invalid prepared assets leave the classic pause
//! panel in charge; items that need many controls hand off to that panel.
use crate::skate3_career::mode::GameMode;
use crate::{
    apt_canvas::{Canvas, movie_textures},
    challenge_map::{ChallengeMap, Spot},
    config::Config,
    core_menu_runtime::{Booted, Category, Item, Key, Outcome, Runtime},
    difficulty::Difficulty,
    graphics_menu::{Menu, Setting},
    online_menu::OnlineAction,
    scoring_hud::{HudComposite, HudMaterial},
};
use bevy::{prelude::*, window::PrimaryWindow};
use std::path::PathBuf;

const LAYER: usize = 30;
/// The movie is authored at 16 ms per frame.
const FRAME: f32 = 1. / 60.;
/// Outro fallback if the movie never reports OutroComplete.
const CLOSE_TIMEOUT: f32 = 0.6;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Action {
    Resume,
    Quit,
    Teleport,
    /// Open the original Challenge Map screen (Locations).
    ChallengeMap,
    LoadMap,
    Difficulty,
    /// Game Settings > Gameplay > Physics (user 10-20, not tested in play): Skate 3's physics or Skate 2's.
    Physics,
    /// The Skate 2 gestures page. Extras > "#Skate 2 Gestures" opens it (`RetailUi::gestures`); no crossbar row holds it.
    #[allow(dead_code)] // matched in `interact`; nothing builds it outside the tests
    Skate2Gestures,
    /// One of that page's four D-pad slot pickers: 0 Up, 1 Down, 2 Left, 3 Right (the port's order).
    Skate2Gesture(u8),
    /// Open the Game Settings page of this crossbar.
    GameSettings,
    /// Options > Unlock Everything (#Off / #On) while the Skate 3 career runs (spec 11.1). Not retail (user 10-20).
    UnlockEverything,
    /// Single Player > Free Play (retail item 27): Go Skate! into Free Play (story mode Task 35).
    FreePlay,
    /// Free Play's Single Player > Resume Career (retail item 32).
    ResumeCareer,
    Setting(Setting),
    /// Continue in the classic panel at `row`, optionally performing it.
    Panel { row: usize, multiplayer: bool, perform: bool },
    /// Open the retail online challenge map (Online > Online Challenge Map).
    OnlineChallengeMap,
    /// A tutorial lesson is running (crate::challenge::LessonMenu): restart or abandon it.
    Lesson(crate::challenge::LessonRequest),
    /// Learn > Skate 2 Tutorial: start/resume it, or start over.
    Tutorial(crate::skate2_challenge::TutorialCommand),
    /// Learn > Skate 1 Tutorial: play or resume it (crate::skate1_challenge::Skate1Challenge::resume).
    Skate1Tutorial,
    /// Learn > Skate 1 Tutorial: Start Over (crate::skate1_challenge::Skate1Challenge::start_over).
    Skate1StartOver,
    /// Online session rows, handed to crate::online_menu.
    Online(OnlineAction),
    /// Online freeskate: watch the chosen player (crate::match_core::Spectate).
    Spectate,
    /// Options > Sign in to / Sign out of Steam (retail item 12, EA Nation).
    SignInOut,
    /// Options > Music Player, Game Settings > Audio: the retail screen, handed to crate::music_ui.
    Music(crate::music_ui::Panel),
    /// Learn > Trick Guide and the other retail screens, handed to crate::retail_ui.
    Retail(crate::retail_ui::Panel),
    /// Edit Skaters (retail item 25): FE slot 63 TeamManagement, handed to crate::team_select.
    TeamManagement,
    /// Not retail (user 10-16): the online Create tab's Edit Skater, the CAS with the session running on underneath.
    EditSkater,
    /// Learn > On Screen Help: toggle the Trick Analyzer (choice 0) or the Manual Meter (1), crate::hud_meters.
    OnScreenHelp,
    /// The host's End Own the Spot Session (crate::skate3_ots).
    EndOts,
    /// Main > Party Play (retail item 47): the setup screen, handed to crate::party_play.
    PartyPlay,
    /// The Party Play crossbar's Restart Challenge (retail item 6).
    RestartChallenge,
    /// The Party Play crossbar's Exit Party Play (retail item 48): popup 213 first.
    ExitPartyPlay,
    /// Create > Replay Editor (0x8261bc30 posts FlowMessage 144), online Instant Replay (state 20): crate::replay::hud.
    Replay,
    /// Create > skate.Reel: My Replays, the saved replays (Stage 0 §7), crate::replay::hud.
    SkateReel,
    /// Not retail (user 10-26): Settings > Audio > Voice Chat opens the Voice Chat page.
    VoiceChat,
}
/// The offline crossbar is retail's career table (mode 2, vtable 0x82303d54; chooser 0x8261b1f0), titled "Career".
const MAIN_TITLE: &str = "ID_CROSSBAR_SINGLE_PLAYER_MODE_TITLE";
/// Free Play's crossbar title (GetCurrentModeTitle in mode 3).
const FREE_PLAY_TITLE: &str = "ID_CROSSBAR_OFFLINE_FREESKATE_MODE_TITLE";
/// In a session ("Online Freeskate").
const ONLINE_TITLE: &str = "ID_CROSSBAR_ONLINE_FREESKATE_MODE_TITLE";

/// What the online tables depend on.
pub(crate) struct OnlineMenu {
    pub host: bool,
    /// Steam lobby (false: Direct UDP, which has no privacy).
    pub steam: bool,
    pub private: bool,
    /// A challenge lobby (`Some(team)`), else Online Freeskate.
    pub challenge: Option<bool>,
    /// Host Actions > Disable Challenge Proposals is on.
    pub no_proposals: bool,
    /// Not retail (user 10-25): Host Actions > Player Collisions is on.
    pub collisions: bool,
    /// Not retail (user 10-25): the session is in the server browser (Host Actions > Hide from Server Browser off).
    pub listed: bool,
    /// The lobby's challenge can be started (match_core::startable).
    pub can_start: bool,
    /// The host in an Own the Spot SimulPlay session (skate3_ots::host_can_end).
    pub end_ots: bool,
    /// An Own the Lot lobby: its playable activities' titles (Select Activity replaces Start Challenge).
    pub activities: Vec<String>,
    /// A match is under way (match_core::Match not Idle): retail's "active challenge" Multiplayer row.
    pub active: bool,
    /// Online freeskate: the other players' names, for Spectate (match_core::spectatable).
    pub spectate: Vec<String>,
    /// Why Edit Skater is greyed ([`edit_blocked`]); None: it opens.
    pub edit: Option<&'static str>,
}

/// Not retail (user 10-16: retail made you leave the session to edit your skater). Edit Skater online is greyed while
/// a match is under way (the skater is held still while editing, so it would sit out the run) and while a custom
/// model is worn (the CAS edits the stock skater, and a swapped model can't change online).
pub(crate) fn edit_blocked(match_active: bool, custom_model: bool) -> Option<&'static str> {
    if match_active {
        Some("#Edit your skater between challenges")
    } else if custom_model {
        Some("#Switch back to your own skater to edit it (custom models can't be changed online)")
    } else {
        None
    }
}
const SETTINGS_TITLE: &str = "ID_GAMESETTINGS_TITLE";
const GESTURES_TITLE: &str = "#Skate 2 Gestures";
/// The Voice Chat page's header reads "Game Settings> Voice Chat", as its tabs do (its one category is "#Voice Chat");
/// a literal, not ID_GAMESETTINGS_TITLE, so Y does not search there.
const VOICE_TITLE: &str = "#Game Settings";
/// A mode's Pause.DisablePauseMenuAndBackpack keeps the Start menu shut offline only. Not retail (online safety Q13):
/// online the world never pauses, and a stalled or hostile match must not keep a player from Quit Session.
fn start_blocked_by_mode(pause_blocked: bool, online: bool) -> bool {
    pause_blocked && !online
}

#[derive(Component)]
struct CrossbarOverlay;
#[derive(Component)]
struct Dimmer;
/// The blur drawn live from the scene each frame, and its held copy ([`BlurCache`]).
#[derive(Component)]
struct LiveBlur;
#[derive(Component)]
struct CachedBlur;
/// C2: pause_blur.wgsl reads the scene 121 times a pixel. While the world camera holds its image (camera::present)
/// the blur is drawn once into `image`, by a camera that renders after the presentation's, and shown from there:
/// the same 8-bit pixels the live blur writes. `state`: 0 live, 1 caching this frame, 2 shown from the cache.
struct BlurCache {
    camera: Entity,
    image: Handle<Image>,
    state: u8,
}
/// Its camera draws nothing but its UI node.
const BLUR_LAYER: usize = 60;
/// main/dimmer fades its vignette to 229/255 over 29 authored frames, and back on close.
const DIMMER_ALPHA: f32 = 229. / 255.;
const DIMMER_FADE: f32 = 29. * FRAME;

/// Blurred copy of the paused world, drawn beneath the dimmer vignette.
#[derive(Asset, TypePath, bevy::render::render_resource::AsBindGroup, Debug, Clone)]
struct PauseBlur {
    #[texture(0)]
    #[sampler(1)]
    scene: Handle<Image>,
}
impl UiMaterial for PauseBlur {
    fn fragment_shader() -> bevy::shader::ShaderRef {
        "embedded://skate3rust/pause_blur.wgsl".into()
    }
}

#[derive(Resource)]
pub(crate) struct Crossbar {
    /// The movie loaded at setup, copied for each page (None: not loaded).
    booted: Option<Booted<Runtime<Action>>>,
    canvas: Canvas,
    pub(crate) runtime: Option<Runtime<Action>>,
    shown: bool,
    closing: Option<f32>,
    accumulated: f32,
    /// Seconds since Start opened the menu (dimmer fade-in).
    opened: f32,
    teleports: Vec<[[f32; 4]; 4]>,
    /// The Spectate list's players as the page shows them (match_core::spectate_pick).
    watchable: Vec<u64>,
    /// Not retail (user 10-25): opened by the marker panel's Player List (LB+B) straight into the Spectate list; Back
    /// closes the menu with it, and the main page's remembered row is left alone.
    marker_list: bool,
    /// Showing the Game Settings page; Back returns to the main crossbar.
    settings: bool,
    /// Not retail (user 10-26): Settings search's page while it shows: its words and the rows they found, kept as the
    /// page rebuilds after a pick. Back returns to Game Settings.
    search: Option<(String, Vec<Action>)>,
    /// Not retail (user 10-26): the Voice Chat page shows (its rows are `voice_categories`). Back returns to Game
    /// Settings on its Audio row.
    voice: bool,
    /// Push to Talk Key waits for the next key (Accept on its row); Esc, B or Start cancels.
    capture: Option<Setting>,
    blur: Option<Handle<PauseBlur>>,
    cache: Option<BlurCache>,
    /// Front-end sounds to play (`GlobalFEPlaySound` ids), sent at the next update.
    sounds: Vec<u64>,
    /// The main page's mode (its title) and highlighted (menu, row): SetMode reopens there in the same mode.
    last: Option<(String, (usize, usize))>,
    /// What each page's GetProgressionData answers (the career's, else the welcome panel).
    progression: [f64; 6],
}

impl Crossbar {
    /// Game Settings or its search page shows (not Extras' gestures page): Y types a search there.
    fn on_game_settings(&self) -> bool {
        self.settings && self.runtime.as_ref().is_some_and(|r| r.bindings.title == SETTINGS_TITLE)
    }
    /// The open settings page's rows as they stand now: Game Settings', its search page's or the Voice Chat page's.
    fn settings_rows(&self, menu: &Menu) -> Vec<Category<Action>> {
        match &self.search {
            Some((words, found)) => search_categories(menu, words, found),
            None if self.voice => voice_categories(menu),
            None => settings_categories(menu),
        }
    }
}

/// SetMode 0x8261bac8..bb08: the same mode reopens on its last menu and row; a new mode on (0, 0), except online
/// freeskate (mode 4), which opens on menu 1 (Multiplayer).
fn reopen_at(last: Option<&(String, (usize, usize))>, title: &str) -> (usize, usize) {
    match last {
        Some((mode, at)) if mode == title => *at,
        _ => (usize::from(title == ONLINE_TITLE), 0),
    }
}

pub(crate) struct CoreMenuPlugin;
impl Plugin for CoreMenuPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "pause_blur.wgsl");
        app.add_plugins(bevy::ui_render::UiMaterialPlugin::<PauseBlur>::default())
            .add_systems(PostStartup, setup.after(crate::graphics_menu::PresentationSetup))
            .add_systems(
                PreUpdate,
                // Same input frame as the classic panel, and strictly before it, so
                // one Start press is never seen by both.
                interact
                    .after(bevy::input::InputSystems)
                    .after(crate::customiser::navigation)
                    // Never sees the press that closed the Challenge Map.
                    .after(crate::challenge_map::interact)
                    .before(crate::graphics_menu::MenuInput),
            )
            .add_systems(PreUpdate, early_resume.after(interact).after(crate::graphics_menu::MenuInput))
            .add_systems(Update, (resize_target, rebind_blur, animate).chain())
            .add_systems(Update, cache_blur.after(animate).after(crate::camera::present));
        if std::env::var_os("SKATE_PERF_PAUSE").is_some() {
            app.add_systems(PreUpdate, perf_pause.after(bevy::input::InputSystems).before(interact));
        }
    }
}

/// SKATE_PERF_PAUSE (benchmarks, with SKATE_PERF_REPORT): Start is pressed 5 s in, so the report's samples (from 10 s)
/// measure the open Start menu.
fn perf_pause(time: Res<Time<Real>>, mut keys: ResMut<ButtonInput<KeyCode>>, mut pressed: Local<bool>) {
    if *pressed {
        keys.release(KeyCode::Escape);
    } else if time.elapsed_secs() > 5. {
        keys.press(KeyCode::Escape);
        *pressed = true;
    }
}

fn item(name: &str, icon: &'static str, help: &str, action: Action) -> Item<Action> {
    Item { name: name.into(), icon, help: help.into(), enabled: true, action, choices: Vec::new(), chosen: 0, disabled_choices: Vec::new(), slider: false }
}
fn picker(mut item: Item<Action>, choices: Vec<String>, chosen: usize) -> Item<Action> {
    item.enabled = !choices.is_empty();
    item.chosen = chosen.min(choices.len().saturating_sub(1));
    item.choices = choices;
    item
}
/// An original item this engine does not implement: shown greyed, like the shipped
/// game's unavailable options.
fn off(name: &str, icon: &'static str, help: &str) -> Item<Action> {
    Item { enabled: false, ..item(name, icon, help, Action::Resume) }
}
fn panel(row: usize, multiplayer: bool, perform: bool) -> Action {
    Action::Panel { row, multiplayer, perform }
}
/// Icons are the movie's authored frame labels (category: singleplayer/multiplayer/
/// create/options; option: freeskate, map, skatepark, quit, onlineteam, ...).
/// `challenge_map`: the original Challenge Map screen is installed; otherwise
/// Challenge Map picks a spot on the loaded map.
fn categories(
    menu: &Menu,
    destinations: &[crate::teleport_menu::Destination],
    challenge_map: bool,
    tutorial: bool,
    skate1_tutorial: bool,
    online: bool,
    signed_in: bool,
) -> Vec<Category<Action>> {
    let difficulty = Difficulty::ALL.iter().position(|d| *d == menu.difficulty()).unwrap_or(0);
    // Names/help are the game's own string IDs (localised by the movie, including
    // the skate. wordmark glyph U+00AB); "#" marks literal text for engine extras.
    // Retail rows follow the PS3 retail Start menu (RPCS3, BLUS30464) row for row;
    // engine extras (marked) sit next to their nearest retail row without reordering it.
    // The movie has 8 option slots per category (mSubcat0-7); a 9th row is never drawn.
    vec![
        Category {
            name: "ID_CROSSBAR_SINGLE_PLAYER".into(),
            icon: "singleplayer",
            items: [
                if challenge_map {
                    Some(item("ID_START_HUB_CHALLENGE_MAP", "map", "ID_CROSSBAR_CHALLENGE_MAP_HELPER_TEXT", Action::ChallengeMap))
                } else {
                    Some(picker(
                        item("ID_START_HUB_CHALLENGE_MAP", "map", "ID_CROSSBAR_CHALLENGE_MAP_HELPER_TEXT", Action::Teleport),
                        destinations.iter().map(|d| d.name.clone()).collect(),
                        0,
                    ))
                },
                // Extra, only without the Challenge Map: its Locations tab travels between maps.
                (!challenge_map).then(|| picker(
                    item("ID_CROSSBAR_CHANGE_LOCATION", "skatepark", "#Skate a different city district or park", Action::LoadMap),
                    menu.map_labels(),
                    menu.selected_map(),
                )),
            ]
            .into_iter()
            .flatten()
            .chain([
                item("ID_CROSSBAR_MY_CAREER_TEAM", "mycareerteam", "ID_CROSSBAR_MY_CAREER_TEAM_HELPER_TEXT", Action::TeamManagement),
                off("ID_CROSSBAR_PROFILE_CAREER", "profile", "ID_CROSSBAR_SKATE_PROFILE_OFFLINE_HELPER_TEXT"),
                off("ID_CROSSBAR_SKATE_WITH", "skatewith", "ID_CROSSBAR_SKATE_WITH_HELPER_TEXT"),
                // ponytail: retail opens the «Feed screen, whose messages came from EA's servers; greyed like the other
                // unported online rows until a local feed exists.
                off("ID_CROSSBAR_SKATE_FEED", "skatefeed", "ID_CROSSBAR_SKATE_FEED_HELPER_TEXT"),
                off("ID_CROSSBAR_SOLO_FREESKATE", "solofreesakte", "ID_CROSSBAR_SOLO_FREESKATE_OPTIONS_HELPER_TEXT"),
                // Greyed without the San Van Party Pack (with_party).
                off(PARTY_PLAY_ROW, "freeskate", "#"),
            ])
            .collect(),
        },
        Category {
            // The string table is the 360 build's ("Xbox LIVE"); PS3 retail says Multiplayer.
            name: "#Multiplayer".into(),
            icon: "multiplayer",
            items: vec![
                online_challenge_map(online),
                // Extra.
                server_browser(),
                off("ID_CROSSBAR_ONLINE_TEAM", "onlineteam", "ID_CROSSBAR_ONLINE_TEAM_HELPER_TEXT"),
                off("ID_ONLINE_SKATEPROFILE_ONLINE", "profile", "ID_CROSSBAR_SKATE_PROFILE_ONLINE_HELPER_TEXT"),
                off("ID_CROSSBAR_CONTACTS", "contacts", "ID_CROSSBAR_CONTACTS_HELPER_TEXT"),
                // Extra.
                item("ID_CROSSBAR_FRIENDS", "contacts", "#See which Steam friends are online and join them", Action::Online(OnlineAction::Friends)),
                // Retail's XBL Party row (item 38) comes before Leaderboards (table 0x82205a68); PS3 reads View Invites.
                off("ID_ONLINE_HUB_VIEW_INVITES", "view", "ID_CROSSBAR_VIEW_INVITES_HELPER_TEXT"),
                off("ID_CROSSBAR_LEADERBOARDS", "leaderboards", "ID_CROSSBAR_LEADERBOARDS_HELPER_TEXT"),
            ],
        },
        Category {
            name: "ID_CROSSBAR_CREATE".into(),
            icon: "create",
            items: vec![
                // Stage 0 §7: the editor's crossbar row is ID_START_HUB_SKATE_REPLAY_EDITOR (0x83027558; review M1).
                item("ID_START_HUB_SKATE_REPLAY_EDITOR", "replay", "ID_CROSSBAR_REPLAY_EDITOR_HELPER_TEXT", Action::Replay),
                item("ID_SKATE_REEL_GENERAL_TITLE", "skatereel", "ID_CROSSBAR_SKATE_REEL_HELPER_TEXT", Action::SkateReel),
                off("ID_CROSSBAR_SKATE_PARK", "skatepark", "ID_CROSSBAR_SKATE_PARK_HELPER_TEXT"),
                // Extra.
                item("#Custom Models", "profile", "#Swap in imported character models", panel(12, false, true)),
            ],
        },
        Category {
            name: "ID_CROSSBAR_LEARN".into(),
            icon: "learn",
            items: [
                Some(trick_guide()),
                Some(off("ID_CROSSBAR_ON_SCREEN_HELP", "onscreenhelp", "ID_CROSSBAR_ON_SCREEN_HELP_HELPER_TEXT")),
                // Extra: Skate 2's tutorial, on New_San_Vanelona.
                tutorial.then(|| item("#Skate 2 Tutorial", "parktutorial", "#Play or resume Skate 2's tutorial", Action::Tutorial(crate::skate2_challenge::TutorialCommand::Start))),
                tutorial.then(|| item("#Skate 2 Tutorial: Start Over", "parktutorial", "#Erase tutorial progress and play it from the beginning", Action::Tutorial(crate::skate2_challenge::TutorialCommand::StartOver))),
                // Not retail (user 10-15, bug 10-17 #23): the Skate 2 map's two items, for the Skate 1 map's career.
                skate1_tutorial.then(|| item("#Skate 1 Tutorial", "parktutorial", "#Play or resume Skate 1's tutorial", Action::Skate1Tutorial)),
                skate1_tutorial.then(|| item("#Skate 1 Tutorial: Start Over", "parktutorial", "#Erase tutorial progress and play it from the beginning", Action::Skate1StartOver)),
            ]
            .into_iter()
            .flatten()
            .collect(),
        },
        Category {
            name: "ID_CROSSBAR_OPTIONS".into(),
            icon: "options",
            items: vec![
                item("ID_START_HUB_GAME_SETTINGS", "settings", "ID_CROSSBAR_GAME_SETTINGS_HELPER_TEXT", Action::GameSettings), // retail item 4 (0x830275AC -> 0x82206430)
                // Extra row (placement is the user's): retail's offline Game Settings "Play Mode" option.
                picker(
                    item(PLAY_MODE[0], "hostoptions", play_mode_help(menu.difficulty()), Action::Difficulty),
                    PLAY_MODE[1..].iter().map(|id| id.to_string()).collect(),
                    difficulty,
                ),
                item("ID_CROSSBAR_MUSIC_PLAYER", "musicplayer", "ID_CROSSBAR_MUSIC_PLAYER_HELPER_TEXT", Action::Music(crate::music_ui::Panel::Player)),
                // Retail item 34: the Extras screen (retail_ui.rs).
                item("ID_MAIN_MENU_EXTRAS", "extras", "ID_CROSSBAR_EXTRAS_HELPER_TEXT", Action::Retail(crate::retail_ui::Panel::Extras)),
                sign_in_out(signed_in),
                // Extras, after every retail row: the mod manager, then Quit.
                item("#Mods", "skatepark", "#Enable or disable installed mods", panel(15, false, true)),
                item("#Quit", "quit", "#Exit to the desktop", Action::Quit),
            ],
        },
    ]
}

/// Offline retail's difficulty option: "Play Mode" (ID_GAMESETTINGS_DIFFICULTY_OPTIONS), then its Easy / Normal /
/// Hardcore values; ID_CROSSBAR_SESSION_DIFFICULTY is the online host's row.
const PLAY_MODE: [&str; 4] = [
    "ID_GAMESETTINGS_DIFFICULTY_OPTIONS",
    "ID_GAMESETTINGS_DIFFICULTY_EASY",
    "ID_GAMESETTINGS_DIFFICULTY_NORMAL",
    "ID_GAMESETTINGS_DIFFICULTY_HARDCORE",
];
/// Play Mode's helper line: retail's description of the value in effect.
fn play_mode_help(difficulty: Difficulty) -> &'static str {
    match difficulty {
        Difficulty::Easy => "ID_GAMESETTINGS_DIFFICULTY_DESCRIPTION_EASY",
        Difficulty::Normal => "ID_GAMESETTINGS_DIFFICULTY_DESCRIPTION_NORMAL",
        Difficulty::Hardcore => "ID_GAMESETTINGS_DIFFICULTY_DESCRIPTION_HARDCORE",
    }
}

const ON_SCREEN_HELP: [&str; 2] = ["ID_CROSSBAR_ON_SCREEN_HELP", "ID_CROSSBAR_ON_SCREEN_HELP_HELPER_TEXT"];
/// Learn > On Screen Help (retail picker 8261DE88 / 8261EC50), on both crossbars; greyed without the prepared
/// movies (crate::hud_meters).
fn with_help(mut categories: Vec<Category<Action>>, help: Option<&crate::hud_meters::OnScreenHelp>) -> Vec<Category<Action>> {
    let rows = categories.iter_mut().flat_map(|c| &mut c.items).filter(|i| i.name == ON_SCREEN_HELP[0]);
    for row in rows {
        *row = match help.filter(|h| h.available) {
            Some(help) => picker(item(ON_SCREEN_HELP[0], "onscreenhelp", ON_SCREEN_HELP[1], Action::OnScreenHelp), help.choices(), 0),
            None => off(ON_SCREEN_HELP[0], "onscreenhelp", ON_SCREEN_HELP[1]),
        };
    }
    categories
}

/// Options > Unlock Everything, right after Play Mode, while the Skate 3 career runs (`everything`: its answer).
fn with_unlocks(mut categories: Vec<Category<Action>>, everything: Option<bool>) -> Vec<Category<Action>> {
    use crate::skate3_career::unlocks::{OPTION_HELP, OPTION_LABEL};
    let Some(on) = everything else { return categories };
    if let Some(options) = categories.iter_mut().find(|c| c.name == "ID_CROSSBAR_OPTIONS") {
        let at = options.items.iter().position(|i| i.action == Action::Difficulty).map_or(1, |i| i + 1);
        let row = picker(item(OPTION_LABEL, "settings", OPTION_HELP, Action::UnlockEverything), vec!["#Off".into(), "#On".into()], usize::from(on));
        options.items.insert(at, row);
    }
    categories
}

/// Story mode Task 35 (not tested in play): with the Skate 3 career on (`career`: its mode, and whether a challenge or
/// lesson runs), Single Player's Free Play row (item 27) is live, and absent in a challenge or lesson. In Free Play the
/// tab trades it for Resume Career (item 32), drops My Career Team, Skate Feed and Party Play, and Multiplayer goes,
/// under Free Play's title. shortcut: Go Skate! keeps the skater and the default settings (no spawnskater screen yet,
/// Task 36); add the screen when the skater swap lands.
fn with_free_play(mut categories: Vec<Category<Action>>, career: Option<(GameMode, bool)>) -> (Vec<Category<Action>>, &'static str) {
    let Some((mode, busy)) = career else { return (categories, MAIN_TITLE) };
    let free = mode == GameMode::ProfileFreeSkate;
    if let Some(main) = categories.iter_mut().find(|c| c.name == "ID_CROSSBAR_SINGLE_PLAYER") {
        if let Some(at) = main.items.iter().position(|i| i.name == "ID_CROSSBAR_SOLO_FREESKATE") {
            if free {
                main.items[at] = item("ID_CROSSBAR_RESUME_CAREER", "resumecareer", "#", Action::ResumeCareer);
            } else if busy {
                main.items.remove(at);
            } else {
                main.items[at] = item("ID_CROSSBAR_SOLO_FREESKATE", "solofreesakte", "ID_CROSSBAR_SOLO_FREESKATE_HELPER_TEXT", Action::FreePlay);
            }
        }
        if free {
            main.items.retain(|i| !matches!(i.name.as_str(), "ID_CROSSBAR_MY_CAREER_TEAM" | "ID_CROSSBAR_SKATE_FEED" | PARTY_PLAY_ROW));
        }
    }
    if free {
        categories.retain(|c| c.name != "#Multiplayer");
    }
    (categories, if free { FREE_PLAY_TITLE } else { MAIN_TITLE })
}

/// Learn > Trick Guide (retail item 3): the trickguide screen (retail_ui.rs).
fn trick_guide() -> Item<Action> {
    item("ID_START_HUB_TRICK_GUIDE", "trickguide", "ID_CROSSBAR_TRICK_GUIDE_HELPER_TEXT", Action::Retail(crate::retail_ui::Panel::TrickGuide))
}

/// Retail item 47's English label (name getter 0x8261dd88); its helper is empty.
const PARTY_PLAY_ROW: &str = "#Party Play";
/// Main > Party Play (the last Main row): enabled while the Party Pack's screens are installed (D4).
fn with_party(mut categories: Vec<Category<Action>>, installed: bool) -> Vec<Category<Action>> {
    let rows = categories.iter_mut().flat_map(|c| &mut c.items).filter(|i| i.name == PARTY_PLAY_ROW);
    for row in rows.filter(|_| installed) {
        *row = item(PARTY_PLAY_ROW, "freeskate", "#", Action::PartyPlay);
    }
    categories
}

/// Audit 10-27 #8: Create > Replay Editor is greyed until there's a replay to edit (0x8261E208 calls 0x8250b8f8:
/// MIN_BACK_FRAMES recorded, Back's own gate). The pause crossbar only: Party Play's enables every row (7c).
fn with_replay(mut categories: Vec<Category<Action>>, ready: bool) -> Vec<Category<Action>> {
    for row in categories.iter_mut().flat_map(|c| &mut c.items).filter(|i| i.action == Action::Replay) {
        row.enabled &= ready;
    }
    categories
}

/// The crossbar while a Party Play match runs (vtable 0x82303ebc, menus 0x82207ad4, rows 0x82207ae4): Main, Create,
/// Learn, Options. Retail's IsItemEnabled (0x82620870) reads the wrong row, so it enables every item (Restart
/// Challenge with no challenge running too, D8), the Replay Editor too (audit 10-27 7c): replay/hud.rs asks to end the
/// match first (ID_REPLAY_QUIT_CONF), as over every running mode.
pub(crate) fn party_categories(signed_in: bool) -> (Vec<Category<Action>>, &'static str) {
    let categories = vec![
        Category {
            name: "ID_CROSSBAR_SINGLE_PLAYER".into(),
            icon: "singleplayer",
            items: vec![
                item("ID_START_HUB_RESTART_CHALLENGE", "restart", "ID_CROSSBAR_RESTART_CHALLENGE_HELPER_TEXT", Action::RestartChallenge),
                // ponytail: retail's label is ID_PARTY_PLAY_EXIT_PARTY_PLAY, the Party Pack's string; this movie carries the
                // disc's table only, so the English text stands in (other languages would need the pack's table merged).
                item("#Exit Party Play", "quit", "#", Action::ExitPartyPlay),
            ],
        },
        Category {
            name: "ID_CROSSBAR_CREATE".into(),
            icon: "create",
            items: vec![item("ID_START_HUB_SKATE_REPLAY_EDITOR", "replay", "ID_CROSSBAR_REPLAY_EDITOR_HELPER_TEXT", Action::Replay)],
        },
        Category {
            name: "ID_CROSSBAR_LEARN".into(),
            icon: "learn",
            items: vec![trick_guide()],
        },
        Category {
            name: "ID_CROSSBAR_OPTIONS".into(),
            icon: "options",
            items: vec![
                item("ID_START_HUB_GAME_SETTINGS", "settings", "ID_CROSSBAR_GAME_SETTINGS_HELPER_TEXT", Action::GameSettings),
                sign_in_out(signed_in),
            ],
        },
    ];
    (categories, "#Party Play")
}

/// Retail item 12 (label fn 0x8261dcf0) with EA Nation read as Steam.
fn sign_in_out(signed_in: bool) -> Item<Action> {
    let name = if signed_in { "#Sign out of Steam" } else { "#Sign in to Steam" };
    item(name, "eanation", "#Sign In or Out of Steam", Action::SignInOut)
}

/// Online Challenge Map: the retail screen when installed, else the classic panel's multiplayer rows.
fn online_challenge_map(online: bool) -> Item<Action> {
    if online {
        item("ID_CROSSBAR_ONLINE_CHALLENGE_MAP", "map", "ID_CROSSBAR_ONLINE_CHALLENGE_MAP_HELPER_TEXT", Action::OnlineChallengeMap)
    } else {
        item("ID_CROSSBAR_ONLINE_CHALLENGE_MAP", "map", "#Host or join a session: local test or Steam", panel(0, true, false))
    }
}

/// Not retail (user 10-25): every public session on this version, to join one (server_browser).
fn server_browser() -> Item<Action> {
    item(crate::server_browser::TITLE, "skatewith", "#See every public session on your version and join one", Action::Online(OnlineAction::ServerBrowser))
}

/// The retail online crossbar (docs/superpowers/specs/online-session-ui-reference.md section 2): Online Freeskate
/// (vtable 0x82303d7c), or in a challenge lobby Online (Team) Challenge (0x82303da4) with no challenge running yet.
/// Text follows PS3 retail (BLUS30464): the tab reads Multiplayer, Party is View Invites, and a player in a session is
/// signed in (Sign Out). Engine extras as noted; greyed rows are retail features this engine lacks.
pub(crate) fn online_categories(menu: &OnlineMenu, online_map: bool) -> (Vec<Category<Action>>, &'static str) {
    use OnlineAction::*;
    let mut host_actions = picker(
        item("ID_ONLINE_HUB_HOST_OPTIONS", "hostoptions", "ID_CROSSBAR_HOST_OPTIONS_HELPER_TEXT", Action::Online(HostActions)),
        vec![
            if menu.no_proposals { PROPOSALS_ON } else { PROPOSALS_OFF }.into(),
            if menu.private { PUBLIC } else { PRIVATE }.into(),
            // Open Game Settings on Difficulty, Weather (Time of Day is the next row) and Physics; guests take the
            // host's (online_moves). User 10-29: Physics added; Weather and Time of Day share a choice (8 at most).
            "ID_CROSSBAR_SESSION_DIFFICULTY".into(),
            "#Session Weather and Time".into(),
            "#Session Physics".into(),
            // Not retail (user 10-25): players pass through each other unless the host turns collisions on.
            if menu.collisions { COLLISIONS_ON } else { COLLISIONS_OFF }.into(),
            // Not retail (user 10-25): the server browser's listing (session_details). One row types both the name and
            // the description: the picker lists 8 choices at most.
            if menu.listed { UNLISTED } else { LISTED }.into(),
            SERVER_DETAILS.into(),
        ],
        1,
    );
    // Privacy and the server browser are Steam-only (ruling 4).
    host_actions.disabled_choices = if menu.steam { vec![] } else { vec![1, 6, 7] };
    let in_challenge = menu.challenge.is_some();
    let team = menu.challenge == Some(true);
    // Host Actions sits after Online Challenge Map in freeskate, after Player List / Switch Team in a challenge lobby.
    let (freeskate_host, challenge_host) = (menu.host && !in_challenge, menu.host && in_challenge);
    let player_list = item("ID_ONLINE_HUB_PLAYER_LIST", "players", "ID_CROSSBAR_PLAYER_LIST_HELPER_TEXT", Action::Online(PlayerList));
    let contacts = picker(
        item("ID_CROSSBAR_CONTACTS", "contacts", "ID_CROSSBAR_CONTACTS_HELPER_TEXT", Action::Online(Contacts)),
        ["ID_CROSSBAR_TEAMMATES", "ID_CROSSBAR_FRIENDS", "ID_CROSSBAR_RECENT_PLAYERS", "ID_CROSSBAR_INVITE_FRIEND"].map(String::from).into(),
        1,
    );
    let quit = item("ID_CROSSBAR_QUIT_SESSION", "quitsession", "ID_CROSSBAR_QUIT_SESSION_HELPER_TEXT", Action::Online(QuitSession));
    // The active-challenge row (selector 0x8261ff58, case 3): End Own the Spot Session (item 41, icon quit; the OTS
    // SimulPlay host) first, then Player List, Contacts, Quit Session. ponytail: the challenge-byte variant with
    // Challenge Objectives (item 7) is not ported; that row reads as the plain one.
    let multiplayer = if menu.active {
        let end = menu.end_ots.then(|| item("ID_CROSSBAR_OTS_RETURN_TO_LOBBY", "quit", "ID_ONLINE_HELPER_MESSAGE_END_OTS", Action::EndOts));
        Category { name: "#Multiplayer".into(), icon: "multiplayer", items: end.into_iter().chain([player_list, contacts, quit]).collect() }
    } else {
        let mut items: Vec<Item<Action>> = [
            // Greyed until a mode plays the lobby's row (match_core::Playable).
            challenge_host.then(|| {
                let (id, icon, help) = ("ID_ONLINE_HUB_START_CHALLENGE", "skatechallenge", "ID_CROSSBAR_ONLINE_START_CHALLENGE_HELPER_TEXT");
                if !menu.activities.is_empty() {
                    // An Own the Lot lobby: Select Activity (item 42) lists the lot's activities. ponytail: retail offers
                    // it to guests too (their pick is proposed, ID_CHALLENGE_FREESKATEACTIVITY_ACTIVITYPROPOSED); the
                    // host only here.
                    let select = item(crate::skate3_otl::SELECT_ACTIVITY, icon, help, Action::Online(SelectActivity));
                    if menu.can_start { picker(select, menu.activities.clone(), 0) } else { off(crate::skate3_otl::SELECT_ACTIVITY, icon, help) }
                } else if menu.can_start {
                    item(id, icon, help, Action::Online(StartChallenge))
                } else {
                    off(id, icon, help)
                }
            }),
            Some(online_challenge_map(online_map)),
            freeskate_host.then(|| host_actions.clone()),
            Some(player_list),
            // Extra in this place: retail's Swap Camera (the LB marker menu's B option) cycles players; here every player
            // is listed by name, ourselves to stop watching (user 10-10). Greyed alone in the session.
            (!in_challenge).then(|| picker(
                item("#Spectate", "players", "#Watch another skater in this session", Action::Spectate),
                menu.spectate.iter().map(|n| format!("#{n}")).collect(),
                0,
            )),
            // Decision T1: host-assigned teams (online_moves::teams); a guest proposes the switch to the host.
            team.then(|| item("ID_ONLINE_HUB_SWITCH_TEAM", "switchteam", "ID_CROSSBAR_SWITCH_TEAM_HELPER_TEXT", Action::Online(SwitchTeam))),
            challenge_host.then_some(host_actions),
            Some(contacts),
            // Steam shows its own invites.
            Some(off("ID_ONLINE_HUB_VIEW_INVITES", "view", "ID_CROSSBAR_VIEW_INVITES_HELPER_TEXT")),
            Some(quit),
        ]
        .into_iter()
        .flatten()
        .collect();
        // Not retail (user 10-25): the server browser right under Online Challenge Map, while the movie's 8 rows leave
        // room (a team challenge's host has all 8).
        if items.len() < 8 {
            items.insert(usize::from(challenge_host) + 1, server_browser());
        }
        Category { name: "#Multiplayer".into(), icon: "multiplayer", items }
    };
    let learn = Category {
        name: "ID_CROSSBAR_LEARN".into(),
        icon: "learn",
        items: vec![
            trick_guide(),
            off("ID_CROSSBAR_ON_SCREEN_HELP", "onscreenhelp", "ID_CROSSBAR_ON_SCREEN_HELP_HELPER_TEXT"),
        ],
    };
    let options = Category {
        name: "ID_CROSSBAR_OPTIONS".into(),
        icon: "options",
        items: [
            Some(item("ID_START_HUB_GAME_SETTINGS", "settings", "ID_CROSSBAR_GAME_SETTINGS_HELPER_TEXT", Action::GameSettings)),
            (!in_challenge).then(|| item("ID_CROSSBAR_MUSIC_PLAYER", "musicplayer", "ID_CROSSBAR_MUSIC_PLAYER_HELPER_TEXT", Action::Music(crate::music_ui::Panel::Player))),
            // In a session the player is signed in.
            Some(sign_in_out(true)),
            // Extra (ruling 5).
            Some(item("#Quit", "quit", "#Exit to the desktop", Action::Quit)),
        ]
        .into_iter()
        .flatten()
        .collect(),
    };
    let edit = match menu.edit {
        None => item("#Edit Skater", "mycareerteam", "#Edit your skater; the others see it once you save", Action::EditSkater),
        Some(why) => off("#Edit Skater", "mycareerteam", why),
    };
    match menu.challenge {
        // Not retail (user 10-25: Edit Skater went missing in challenge lobbies): retail's three tabs and a Create tab
        // with Edit Skater alone, as in freeskate.
        Some(team) => (
            vec![multiplayer, Category { name: "ID_CROSSBAR_CREATE".into(), icon: "create", items: vec![edit] }, learn, options],
            if team { "ID_CROSSBAR_ONLINE_TEAM_CHALLENGE_MODE_TITLE" } else { "ID_CROSSBAR_ONLINE_CHALLENGE_MODE_TITLE" },
        ),
        None => (
            vec![
                Category {
                    name: "ID_CROSSBAR_SINGLE_PLAYER".into(),
                    icon: "singleplayer",
                    items: vec![off("ID_CROSSBAR_PROFILE_CAREER", "profile", "ID_CROSSBAR_SKATE_PROFILE_OFFLINE_HELPER_TEXT")],
                },
                multiplayer,
                Category {
                    name: "ID_CROSSBAR_CREATE".into(),
                    icon: "create",
                    // Retail's row is Replay Editor (0x83027558; audit 10-27 7b) over its online state 20, view and photo
                    // only; ours opens the whole editor (user 10-27), the session running on underneath (research Q7).
                    // Extra, greyed: a swapped model would desync what other players see.
                    items: vec![
                        item("ID_START_HUB_SKATE_REPLAY_EDITOR", "replay", "ID_CROSSBAR_REPLAY_EDITOR_HELPER_TEXT", Action::Replay),
                        edit,
                        off("#Custom Models", "profile", "#Custom models can't be changed in an online session"),
                    ],
                },
                learn,
                options,
            ],
            ONLINE_TITLE,
        ),
    }
}

/// While a tutorial lesson runs, Single Player opens with its Restart / Abandon entries; the greyed rows make room
/// (the movie draws 8 rows per category).
fn with_lesson(mut categories: Vec<Category<Action>>, lesson: bool) -> Vec<Category<Action>> {
    if lesson {
        use crate::challenge::LessonRequest;
        let items = &mut categories[0].items;
        items.retain(|i| i.enabled);
        items.insert(0, item("#Restart Lesson", "freeskate", "#Start this lesson again", Action::Lesson(LessonRequest::Restart)));
        items.insert(1, item("#Abandon Lesson", "quit", "#Leave the tutorial and skate free", Action::Lesson(LessonRequest::Abandon)));
        items.truncate(8);
    }
    categories
}

/// Game Settings as a second crossbar page, laid out like a PC settings screen: each
/// tab and row has its own authored icon, every row shows its value ("Name: Value"),
/// and the helper line says what the row does. Only working settings are listed.
fn settings_categories(menu: &Menu) -> Vec<Category<Action>> {
    let set = |name: &str, icon: &'static str, help: &str, setting: Setting| {
        setting_row(name, icon, help, Action::Setting(setting), menu.choices(setting))
    };
    let difficulty = (
        Difficulty::ALL.iter().map(|d| d.label().to_string()).collect(),
        Difficulty::ALL.iter().position(|d| *d == menu.difficulty()).unwrap_or(0),
    );
    // Tab icons are the movie's category frames ("learn" and "debug" fit no settings tab).
    let page = |name: &str, icon: &'static str, items| Category { name: format!("#{name}"), icon, items };
    // Online, the host sets these for the whole session (a co-host the world too); other guests see them greyed.
    let session = |mut item: Item<Action>, what: &str| {
        let cohost = menu.cohost && !matches!(what, "difficulty" | "physics");
        if menu.guest && !cohost {
            item.enabled = false;
            item.help = format!("#The host sets the {what} in an online session");
        } else if cohost {
            item.help = format!("#Co-host: this session's {what}; the host sets it for everyone");
        } else if menu.session_world().is_some() {
            item.help = format!("#This session's {what}: everyone in it gets yours");
        }
        item
    };
    // Offline the Options page has its own Difficulty picker; online this is where Host Actions opens it.
    let difficulty = menu
        .session_world()
        .map(|_| session(setting_row("Difficulty", "hostoptions", "How forgiving the physics are", Action::Difficulty, difficulty), "difficulty"));
    let mut fov = set("Field of View", "view", "Slider: Left and Right change it a degree at a time", Setting::FieldOfView);
    fov.slider = true;
    fov.choices = vec![slider_line(menu, Setting::FieldOfView)];
    // While DLSS is on it sets the internal resolution and anti-aliasing, and Bevy's SSAO ignores its lower internal
    // resolution: those rows grey out, reading what DLSS put in their place.
    let dlss_owned = |mut item: Item<Action>, value: &str| {
        if menu.dlss() != 0 {
            item.enabled = false;
            if let Some((name, _)) = item.name.split_once(": ") {
                item.name = format!("{name}: {value}");
            }
            item.help = "#Set by DLSS while it is on (Graphics > DLSS)".into();
        }
        item
    };
    // Only where this build and GPU run it (dlss.rs). Graphics' eighth row: Field of View moved to Controls for it.
    let dlss = menu.dlss_supported.then(|| {
        set("DLSS", "view", "NVIDIA DLSS: renders fewer pixels and upscales them with AI; DLAA keeps full resolution", Setting::Dlss)
    });
    // Gameplay's last row. Online (user 10-28, spec 2026-10-20 P4) the host's goes to everyone in the session and other
    // guests see it greyed; with the host's Difficulty row the page has nine and scrolls (user 10-26). Skate 2 physics
    // runs only on New San Vanelona in this build, so the row greys elsewhere; a saved Skate 2 can still be switched back.
    let physics = {
        use crate::skate2_physics::settings::Physics;
        let chosen = Physics::ALL.iter().position(|p| *p == menu.physics()).unwrap_or(0);
        let mut item = session(
            setting_row(
                "Physics",
                "freeskate",
                "Skate 3's physics, or Skate 2's skating and tricks (New San Vanelona)",
                Action::Physics,
                (Physics::ALL.iter().map(|p| p.label().to_string()).collect(), chosen),
            ),
            "physics",
        );
        if !menu.on_nsv && item.enabled {
            if menu.physics() == Physics::Skate3 {
                item.enabled = false;
                item.help = "#Skate 2 physics runs only on New San Vanelona in this build".into();
            } else {
                item.help = "#Skate 2 physics runs only on New San Vanelona in this build; pick Skate 3 here".into();
            }
        }
        item
    };
    vec![
        page("Gameplay", "singleplayer", difficulty.into_iter().chain([
            set("HUD", "players", "Show or hide the on-screen display", Setting::Hud),
            set("HUD Style", "freeskate", "One game's HUD at a time: the map's own, or Skate 3's, 2's or 1's everywhere", Setting::HudStyle),
            // Retail Units (ID_GAMESETTINGS_UNITS_TOGGLE): Skate 1 shows it on Settings > Display; Skate 2 and 3 have
            // the option but no page lists it. No retail help line.
            set("Units", "map", "Distances and speeds on the HUD: metres and Km/h, or feet and MPH", Setting::Units),
            set("Edit Skater Room", "skateschool", "Backdrop of the character customiser", Setting::EditSkaterRoom),
            set("Player Graphics", "create", "Other players' Graphic Creator designs; Hide shows their clothing and boards plain", Setting::PlayerGraphics),
            // Moved from World for the Fog row (World has the movie's 8).
            set("Brightness", "extras", "Brightness of the world; menus are unchanged", Setting::Brightness),
            // Moved from Graphics for its Preset row.
            item("ID_GAMESETTINGS_VIDEO_CALIBRATION", "view", "#Contrast and brightness test screens for your display", Action::Retail(crate::retail_ui::Panel::VideoCalibration)),
        ]).chain([physics]).chain([
            // Not retail (user 11-02): the page's ninth row, so it scrolls (as Video's).
            set("Skip Intro", "start", "Start straight into the game: no legal screen, EA logo or Press Start. Applies when the game restarts", Setting::SkipIntro),
        ]).collect()),
        page("Controls", "options", vec![
            set("Camera Angle", "replay", "Skate 3's Low or High camera, or Skate 2's or Skate 1's", Setting::CameraAngle),
            set("Camera Shake", "skatereel", "The camera's shake on landings, bumps and speed", Setting::CameraShake),
            fov,
            set("Invert Look X", "quitsession", "Off-board camera, left and right", Setting::InvertX),
            set("Invert Look Y", "soloranked", "Off-board camera, up and down", Setting::InvertY),
            // Retail Control Settings > Vibration (option 19, Off/On), between Offboard Y Axis and Hall Of Meat Mode.
            set("Vibration", "options", "Controller rumble on bumps, landings, grinds and slides", Setting::Vibration),
            // Retail Control Settings > Hall Of Meat Mode (Off/On); retail has no help line for it.
            set("Hall Of Meat Mode", "skatechallenge", "Bails score Hall of Meat points in free skate, with the bail slow-motion", Setting::HallOfMeat),
            // Not retail (online safety Q6): the Controls page has room; Camera Shake is the other comfort row here.
            set("Reduce Flashing", "view", "Dimmer lightning, slower weather and time changes from the host, steady colours on other players' mod boxes", Setting::ReduceFlashing),
        ]),
        // Nine rows: the page scrolls past the movie's eight (user 10-26).
        page("Video", "create", vec![
            set("Display Mode", "profile", "Windowed, borderless or exclusive fullscreen (the fastest)", Setting::DisplayMode),
            set("Monitor", "view", "The display to play on; Automatic uses the one the window is on", Setting::Monitor),
            set("Resolution", "map", "Window size, or the fullscreen video mode", Setting::Resolution),
            dlss_owned(set("Render Scale", "uploadpark", "3D resolution as a share of the window", Setting::RenderScale), "DLSS"),
            dlss_owned(set("Anti-Aliasing", "skatereel", "Smooths jagged edges: FXAA and SMAA are cheap filters, MSAA is sharper but costs more", Setting::Msaa), "DLSS"),
            set("V-Sync", "skatefeed", "Match the monitor refresh to stop tearing", Setting::VSync),
            set("Frame Rate Limit", "start", "Cap frames per second", Setting::FpsLimit),
            // Not retail (user 10-26): the FPS box can be hidden.
            set("FPS Counter", "start", "The frames-per-second box in the top right corner", Setting::FpsCounter),
            set("Occlusion Culling", "uploadpark", "Skip drawing hidden objects", Setting::Occlusion),
        ]),
        page("World", "multiplayer", vec![
            session(set("Weather", "skatefeed", "Rain or snow, clouds and fog; visual only", Setting::Weather), "weather"),
            session(set("Time of Day", "resumecareer", "Day, Night, or Cycle: a whole day every 18 minutes", Setting::TimeOfDay), "time of day"),
            // Not retail (user 10-14, feature #5).
            set("Fog", "skatefeed", "Follow the weather, off, or Light, Medium or Heavy fog starting far, normal or near", Setting::Fog),
        ].into_iter().chain(crate::graphics_menu::EFFECTS.iter().enumerate().map(|(i, (name, help))| {
            // User 10-26: Colour Grading picks the game whose look shows (or Off); the others are On/Off.
            set(name, "view", help, if i == 0 { Setting::Look } else { Setting::Effect(i) })
        })).collect()),
        // ponytail: shares Video's tab icon; the movie has no spare fitting category frame.
        page("Graphics", "create", [
            // Not retail (user 10-14, feature #4): sets the rows below at once.
            set("Preset", "uploadpark", "Performance: a rough look (half resolution, blurry textures, no shadows) for much higher FPS on any PC; map textures change on the next map load. Default: the standard look", Setting::Preset),
            set("Texture Quality", "create", "Sharpness of the world's textures; lower uses less video memory. Applies on the next map load", Setting::TextureQuality),
            set("Draw Distance", "map", "How far the world, props and other skaters draw; lower is faster", Setting::DrawDistance),
            set("Shadow Quality", "resumecareer", "Sharpness of the skater's shadow close to the camera", Setting::ShadowQuality),
            dlss_owned(set("Ambient Occlusion", "parktutorial", "Soft shading in corners and under objects; turns MSAA off. Costs FPS (about 15% on big cities)", Setting::AmbientOcclusion), "Off"),
            set("Anisotropic Filtering", "map", "Sharper ground and walls at an angle; applies on the next map load", Setting::Anisotropy),
        ].into_iter().chain(dlss).chain([
            set("Sharpening", "view", "Contrast-adaptive sharpening; helps at a low Render Scale or with DLSS", Setting::Sharpening),
        ]).collect()),
        // The movie's sixth and last category slot. Both rows open retail screens (music_ui.rs).
        // ponytail: shares Controls' tab icon.
        page("Audio", "options", vec![
            item("ID_GAMESETTINGS_AUDIO_SETTINGS", "settings", "#SFX, dialog and music volume, and the soundtrack", Action::Music(crate::music_ui::Panel::AudioSettings)),
            item("ID_CROSSBAR_MUSIC_PLAYER", "musicplayer", "ID_CROSSBAR_MUSIC_PLAYER_HELPER_TEXT", Action::Music(crate::music_ui::Panel::Player)),
            // Not retail (voice chat, user 10-26): one output for game sound and voices; bevy_audio opens it at startup.
            set("Output Device", "musicplayer", "Applies when the game restarts", Setting::OutputDevice),
            // Retail Game Settings > Subtitles (ID_GAMESETTINGS_SUBTITLE_TOGGLE, +157): the movies' subtitle lines.
            set("Subtitles", "skatereel", "Subtitle lines over the movies", Setting::Subtitles),
            // Not retail (3.3): Discord Rich Presence; Off clears it. ponytail: on the Audio page, the only one with room.
            set("Discord Status", "players", "Show your Discord friends what you play: the mode, map and session size", Setting::DiscordStatus),
            // Not retail (online safety 3.12, 3.2); per-player Mute / Hide / Block are in the Player List.
            set("Text Chat", "players", "Online text chat: Off hides the feed and the typing bar; Filtered also hides swearing. Slurs never show", Setting::Chat),
            set("Chat From", "players", "Whose chat lines show online: everyone, or only your Steam friends", Setting::ChatFrom),
            // Not retail (user 10-26): opens the Voice Chat page; the movie's six tabs are all used. Ten rows: it scrolls.
            item("#Voice Chat", "players", "#Push to Talk or Open Mic, your microphone, noise suppression and voice volume", Action::VoiceChat),
            set("Custom Models", "create", "Whose own skater models show online; the others show a stock skater", Setting::CustomModels),
            // Not retail (perf B3). ponytail: on Audio, the only page with a free row, as Discord Status.
            set("Low Latency", "start", "Your input shows a few milliseconds sooner, for about 15% fewer frames per second. Applies when the game restarts", Setting::LowLatency),
        ]),
    ]
}
/// Not retail (user 10-26): Settings search. The Game Settings and Voice Chat rows whose name (its value left out) or
/// help line holds every typed word, any case: rows named by them first, each group in page order. `say` reads a retail
/// text id.
fn search(menu: &Menu, words: &str, say: impl Fn(&str) -> String) -> Vec<Action> {
    let words: Vec<String> = words.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return Vec::new();
    }
    let holds = |text: &str| words.iter().all(|w| text.contains(w.as_str()));
    let mut found: Vec<(bool, Action)> = settings_categories(menu)
        .into_iter()
        .chain(voice_categories(menu))
        .flat_map(|c| c.items)
        .filter_map(|item| {
            let name = say(&item.name).to_lowercase();
            let name = name.split_once(": ").map_or(name.as_str(), |(name, _)| name);
            holds(&format!("{name} {}", say(&item.help).to_lowercase())).then(|| (holds(name), item.action))
        })
        .collect();
    found.sort_by_key(|(named, _)| !named);
    found.into_iter().map(|(_, action)| action).collect()
}
/// The search page: `found`'s rows as Game Settings shows them now, in that order (a pick that changes a row's words
/// never moves it), or one greyed line when nothing matched.
fn search_categories(menu: &Menu, words: &str, found: &[Action]) -> Vec<Category<Action>> {
    let mut rows: Vec<Item<Action>> = settings_categories(menu).into_iter().chain(voice_categories(menu)).flat_map(|c| c.items).collect();
    let mut items: Vec<Item<Action>> =
        found.iter().filter_map(|action| Some(rows.swap_remove(rows.iter().position(|i| i.action == *action)?))).collect();
    if items.is_empty() {
        items.push(off(&format!("#No setting matches \"{words}\""), "options", "#Press Y to search again"));
    }
    vec![Category { name: format!("#Search: {words}"), icon: "options", items }]
}
/// "Y Search" bottom right on Game Settings and its search page: the spectate legend, in the movie's Futura.
fn search_hint(runtime: &Runtime<Action>) -> Vec<crate::apt_scene::Draw> {
    let text = &runtime.bindings.movie.text_assets;
    let font = text.font_named("Futura Std Medium").and_then(|id| text.fonts.get(&id));
    font.map_or_else(Vec::new, |font| crate::online_menu::legend(font, "Y", "Search"))
}
/// Settings search's page for `words`, its rows found once in the open page's language.
fn open_search(crossbar: &mut Crossbar, menu: &mut Menu, words: &str) {
    let Some(runtime) = crossbar.runtime.as_ref() else { return };
    let found = search(menu, words, |text| runtime.bindings.movie.text_assets.localize(text));
    if let Err(error) = switch_page(crossbar, search_categories(menu, words, &found), SETTINGS_TITLE, true, None) {
        error!("Settings search page failed to open: {error}");
        crossbar.runtime = None;
        menu.hand_off(0, false, false, None);
        return;
    }
    crossbar.search = Some((words.to_owned(), found));
}
/// Extras > Skate 2 Gestures (user 10-20, not tested in play): one tab of four pickers, one per D-pad direction in the
/// port's slot order, each over Skate 2's 35 gestures.
fn gesture_categories(menu: &Menu) -> Vec<Category<Action>> {
    use crate::skate2_physics::settings::{GESTURES, SLOT_NAMES};
    let items = menu
        .skate2_gestures()
        .into_iter()
        .zip(SLOT_NAMES)
        .enumerate()
        .map(|(slot, (gesture, direction))| {
            let chosen = (gesture as usize).min(GESTURES.len() - 1);
            // Bug 10-25 #21: the row's D-pad glyph lights its direction (button_icons::EXTRA_GLYPHS); every icon looked alike.
            let arm = direction.trim_start_matches("D-pad ");
            let name = format!("#<B:DPad{arm}> {arm}: {}", GESTURES[chosen]);
            let help = "#Skate 2's gesture on this D-pad direction while you are off the board";
            picker(item(&name, "freeskate", help, Action::Skate2Gesture(slot as u8)), GESTURES.map(String::from).to_vec(), chosen)
        })
        .collect();
    vec![Category { name: "#Gestures".into(), icon: "freeskate", items }]
}
/// A slider's picker line: a bar and the value (Field of View, the voice levels).
fn slider_line(menu: &Menu, setting: Setting) -> String {
    const STEPS: usize = 12;
    let (fraction, value) = menu.slider(setting);
    let at = ((fraction * STEPS as f32).round() as usize).min(STEPS);
    format!("< {}|{} >  {}", "=".repeat(at), "-".repeat(STEPS - at), value)
}
/// A settings row: "Name: Value" over its picker, the helper line saying what it does.
fn setting_row(name: &str, icon: &'static str, help: &str, action: Action, (choices, chosen): (Vec<String>, usize)) -> Item<Action> {
    let name = format!("#{name}: {}", choices.get(chosen).map_or("", String::as_str));
    picker(item(&name, icon, &format!("#{help}"), action), choices, chosen)
}
/// Not retail (user 10-26): the Voice Chat page (Settings > Audio > Voice Chat): one tab, the spec's twelve rows.
fn voice_categories(menu: &Menu) -> Vec<Category<Action>> {
    let set = |name: &str, icon: &'static str, help: &str, setting: Setting| {
        setting_row(name, icon, help, Action::Setting(setting), menu.choices(setting))
    };
    let slider = |name: &str, help: &str, setting: Setting| {
        let mut row = set(name, "settings", help, setting);
        (row.slider, row.choices) = (true, vec![slider_line(menu, setting)]);
        row
    };
    // Accept waits for the next key pressed (interact's capture): no picker.
    let mut key = set("Push to Talk Key", "options", "Accept, then press the key (or a mouse side button) to hold to talk; Esc keeps this one", Setting::PushToTalkKey);
    key.choices.clear();
    let pad = crate::graphics_menu::PTT_BUTTONS[menu.choices(Setting::PushToTalkButton).1].2;
    let mut sensitivity = slider("Open Mic Sensitivity", "Slider: how loud you must speak for Open Mic to send, the mark on the Mic Test bar; higher needs louder", Setting::OpenMicSensitivity);
    if !menu.open_mic() {
        sensitivity.enabled = false;
        sensitivity.help = "#For Open Mic (Mic Mode)".into();
    }
    // Audit 10-30: Default is Windows' default device, not its default communication device, so a headset set only
    // as the latter is passed over.
    let mut microphone = set("Microphone", "musicplayer", "The microphone you talk into; Default is Windows' default device, so pick your headset by name if it isn't", Setting::Microphone);
    if menu.voice_devices.as_ref().is_some_and(|(inputs, _)| inputs.is_empty()) {
        microphone.enabled = false;
        microphone.name = "#Microphone: No microphone".into();
        microphone.help = "#No microphone found: you can still hear the others".into();
    } else if menu.mic_error.is_some() {
        // Audit 10-30: a microphone that won't open sent nothing, and nothing said why.
        microphone.help = "#Your microphone didn't open: allow microphone access in Windows Settings > Privacy > Microphone, or close the app using it".into();
    }
    let mut noise = set("Noise Suppression", "settings", "Cleans background noise out of your voice before it is sent; High is cleaner and uses more CPU", Setting::NoiseSuppression);
    if skate_voice::HIGH_NEEDS_FMA && !std::is_x86_feature_detected!("fma") {
        noise.disabled_choices = vec![2];
    }
    let items = vec![
        set("Voice Chat", "players", "Online voice chat: Off sends and hears no voices", Setting::VoiceChat),
        set("Voice From", "players", "Whose voices you hear online: everyone, or only your Steam friends", Setting::VoiceFrom),
        set("Mic Mode", "options", "Push to Talk sends while you hold the key or button; Open Mic sends whenever you speak", Setting::MicMode),
        key,
        set("Push to Talk Button", "options", pad, Setting::PushToTalkButton),
        sensitivity,
        microphone,
        slider("Mic Volume", "Slider: how loud your voice is sent", Setting::MicVolume),
        set("Mic Test", "musicplayer", "Hear yourself as the others will; the bar bottom right is your level, its mark the Open Mic line", Setting::MicTest),
        noise,
        slider("Voice Volume", "Slider: everyone's voices; each player's own volume is in the Player List", Setting::VoiceVolume),
        set("Lower Game Sound While Talking", "musicplayer", "Music and game sound dip while someone talks", Setting::LowerGameSound),
    ];
    vec![Category { name: "#Voice Chat".into(), icon: "players", items }]
}
/// The Game Settings row Back lands on from the Voice Chat page (`voice`) or a search page (`row`: its highlighted
/// row): the Audio tab's Voice Chat for a voice row, else that row.
fn settings_back(menu: &Menu, voice: bool, row: Option<&Action>) -> Action {
    let on_voice_page = |row: &Action| voice_categories(menu).iter().flat_map(|c| &c.items).any(|i| i.action == *row);
    match row {
        Some(row) if !voice && !on_voice_page(row) => row.clone(),
        // No row: the page's first (no settings row is Resume).
        None if !voice => Action::Resume,
        _ => Action::VoiceChat,
    }
}
/// Push to Talk Key's capture: None while nothing is pressed; Some(None) cancels (Esc, the pad's B or Start); else the
/// new key's name ("KeyV", "Mouse4").
fn captured_key(keys: &[KeyCode], mouse: &[MouseButton], pad_back: bool) -> Option<Option<String>> {
    if pad_back || keys.contains(&KeyCode::Escape) {
        return Some(None);
    }
    let key = keys.iter().find(|k| !matches!(k, KeyCode::Unidentified(_))).map(|k| format!("{k:?}"));
    let side = mouse.iter().find_map(|b| match b {
        MouseButton::Back => Some("Mouse4"),
        MouseButton::Forward => Some("Mouse5"),
        _ => None,
    });
    key.or_else(|| side.map(String::from)).map(Some)
}
/// Mic Test stays on only while the menu is open on a page showing its row: the Voice Chat page, or a search page
/// that found it.
fn keeps_mic_test(open: bool, voice: bool, found: Option<&[Action]>) -> bool {
    open && (voice || found.is_some_and(|found| found.contains(&Action::Setting(Setting::MicTest))))
}
/// Mic Test's level bar, bottom right under where Game Settings' "Y Search" sits (the help line moves as the page
/// scrolls): its frame, the level filled in from -60 to 0 dBFS, and a mark at the Open Mic line.
fn mic_meter(level_dbfs: f32, line_dbfs: f32) -> Vec<crate::apt_scene::Draw> {
    use crate::graphic_board::quad;
    const LEFT: f32 = 884.;
    const RIGHT: f32 = 1164.;
    const TOP: f32 = 662.;
    const BOTTOM: f32 = 674.;
    let x = |dbfs: f32| LEFT + (RIGHT - LEFT) * (dbfs.max(-60.).min(0.) + 60.) / 60.;
    let white = "runtime/white.rgba";
    vec![
        quad(white, [1, 1], [LEFT - 10., TOP - 6., RIGHT + 10., BOTTOM + 6.], [0.024, 0.04, 0.055, 0.85]),
        quad(white, [1, 1], [LEFT, TOP, x(level_dbfs), BOTTOM], [0.36, 0.78, 1., 1.]),
        quad(white, [1, 1], [x(line_dbfs) - 1., TOP - 4., x(line_dbfs) + 1., BOTTOM + 4.], [0.92, 0.95, 0.97, 1.]),
    ]
}
/// Device lists read on their thread came in while a settings page shows: its rows as they stand now, redrawn.
fn show_devices(crossbar: &mut Crossbar, menu: &Menu) -> Result<(), String> {
    let rows = crossbar.settings_rows(menu);
    let Some(runtime) = crossbar.runtime.as_mut() else { return Ok(()) };
    runtime.bindings.categories = rows;
    runtime.refresh()
}
/// Settings > Audio > Voice Chat (user 10-26): its page, the device lists read afresh. Back returns to its Audio row.
fn open_voice(crossbar: &mut Crossbar, menu: &mut Menu) {
    menu.refresh_voice_devices();
    if let Err(error) = switch_page(crossbar, voice_categories(menu), VOICE_TITLE, true, None) {
        error!("Voice Chat page failed to open: {error}");
        crossbar.runtime = None;
        menu.hand_off(0, false, false, None);
        return;
    }
    crossbar.voice = true;
}
/// The gestures page (a one-tab settings page: Back returns to the main crossbar on Extras).
fn open_gestures(crossbar: &mut Crossbar, menu: &mut Menu) {
    if let Err(error) = switch_page(crossbar, gesture_categories(menu), GESTURES_TITLE, true, None) {
        error!("Skate 2 gestures page failed to open: {error}");
        crossbar.runtime = None;
        menu.hand_off(0, false, false, None);
    }
}
/// Swap the crossbar's page; the movie replays its intro as a screen change.
fn switch_page(crossbar: &mut Crossbar, categories: Vec<Category<Action>>, title: &str, settings: bool, focus: Option<&Action>) -> Result<(), String> {
    let (menu, option) = focus
        .and_then(|focus| {
            categories.iter().enumerate().find_map(|(m, c)| c.items.iter().position(|i| i.enabled && i.action == *focus).map(|o| (m, o)))
        })
        .unwrap_or((0, 0));
    let booted = crossbar.booted.as_mut().ok_or("Start menu movie not loaded")?;
    let mut runtime = booted.take().with(categories, title.to_string())?;
    runtime.bindings.progression = crossbar.progression;
    runtime.focus(menu, option);
    crossbar.runtime = Some(runtime);
    crossbar.canvas.forget_drawn();
    crossbar.settings = settings;
    crossbar.search = None;
    crossbar.voice = false;
    crossbar.capture = None;
    crossbar.shown = false;
    crossbar.closing = None;
    crossbar.accumulated = opened_accumulator();
    Ok(())
}
/// perf 10-18: an opened screen's 60 Hz accumulator starts a frame full, so its first frame plays on the opening update
/// rather than one or more updates later. SKATE_FIRST_FRAME=0: empty.
pub(crate) fn opened_accumulator() -> f32 {
    if crate::config::perf_on("SKATE_FIRST_FRAME") { FRAME } else { 0. }
}
/// perf 10-18: a direction pressed while a screen's intro plays (the Start menu's 3 frames, the Challenge Map's 433 ms)
/// was dropped; the last one is kept (`held`) and played once the screen takes input. SKATE_INTRO_BUFFER=0: dropped.
pub(crate) fn buffer_intro_nav(held: &mut Option<Key>, pressed: u16) {
    let key = [(1, Key::Up), (2, Key::Down), (4, Key::Left), (8, Key::Right)].into_iter().find(|(bit, _)| pressed & bit != 0);
    if let Some((_, key)) = key.filter(|_| crate::config::perf_on("SKATE_INTRO_BUFFER")) {
        *held = Some(key);
    }
}
/// perf 10-18: the manifest a highlighted row's screen opens, parsed ahead (apt_screen::warm) so a press opens it
/// without the read and parse. Rows whose screens need no file (or keep theirs) warm nothing.
fn warm_files(action: &Action, asset_root: &std::path::Path) -> Vec<PathBuf> {
    use crate::apt_screen::{MUSIC, PARTY_PLAY, REPLAY, RETAIL, movie_file};
    use crate::{music_ui::Panel as M, retail_ui::Panel as R};
    let (game, movie) = match action {
        Action::Retail(R::TrickGuide) => (RETAIL, "trickguide"),
        Action::Retail(R::VideoCalibration) => (RETAIL, "videocal"),
        Action::Retail(R::Extras) => (RETAIL, "extras"),
        Action::Music(M::AudioSettings) => (MUSIC, "options"),
        Action::Music(M::Player) => (MUSIC, "musicplayertest"),
        Action::PartyPlay => (PARTY_PLAY, "partyplay"),
        Action::Replay => (REPLAY, "replay_editor3"),
        Action::SkateReel => (REPLAY, "browser"),
        _ => return Vec::new(),
    };
    vec![movie_file(asset_root, game, movie)]
}
const PRIVATE: &str = "ID_ONLINE_HUB_MAKE_PRIVATE_SLOT";
const PUBLIC: &str = "ID_ONLINE_HUB_MAKE_PUBLIC_SLOT";
const PROPOSALS_ON: &str = "ID_CROSSBAR_ALLOW_PROPOSALS";
const PROPOSALS_OFF: &str = "ID_CROSSBAR_DENY_PROPOSALS";
/// The choice names what picking it does, as the proposals toggle's.
const COLLISIONS_ON: &str = "#Turn Off Player Collisions";
const COLLISIONS_OFF: &str = "#Turn On Player Collisions";
/// As the collisions toggle's: what picking it does.
const UNLISTED: &str = "#Hide from Server Browser";
const LISTED: &str = "#Show in Server Browser";
const SERVER_DETAILS: &str = "#Server Name and Description";
/// A Host Actions toggle's choice names the other state after a pick. Flipping before set_private is safe: privacy
/// is greyed outside Steam, the only case where set_private fails (and reopening rebuilds it from state).
fn flip_choice(runtime: &mut Runtime<Action>, index: usize, [a, b]: [&str; 2]) -> Result<(), String> {
    if let Some(choice) = runtime.current_mut().and_then(|i| i.choices.get_mut(index)) {
        *choice = if choice == a { b } else { a }.into();
    }
    runtime.refresh()
}
/// A settings row's "Name: Value" follows its new value.
fn show_choice(runtime: &mut Runtime<Action>, index: usize) -> Result<(), String> {
    let value = runtime.current_mut().and_then(|i| i.choices.get(index).cloned()).unwrap_or_default();
    show_value(runtime, index, &value)
}
fn show_value(runtime: &mut Runtime<Action>, index: usize, value: &str) -> Result<(), String> {
    if let Some(item) = runtime.current_mut() {
        if !item.slider {
            item.chosen = index;
        }
        if let Some((name, _)) = item.name.split_once(": ") {
            item.name = format!("{name}: {value}");
        }
    }
    runtime.refresh()
}
/// A Game Settings choice can change other rows: a Preset sets several, any row it covers turns it Custom, DLSS greys
/// three (bug 10-25 #31: they only showed after leaving the page). Same rows, new values: the page is rebuilt from the
/// menu (`rows`: [`Crossbar::settings_rows`]), then the chosen row shows the value in effect (a guest's refused pick, or
/// a slider's value).
fn show_setting(runtime: &mut Runtime<Action>, menu: &Menu, setting: Setting, rows: Option<Vec<Category<Action>>>) -> Result<(), String> {
    if let Some(rows) = rows {
        runtime.bindings.categories = rows;
    }
    let (choices, chosen) = menu.choices(setting);
    show_value(runtime, chosen, choices.get(chosen).map_or("", String::as_str))
}
/// A Game Settings pick goes in at once, but (not retail, user 10-27) one that turns Performance mode on or off asks to
/// restart first: online_menu's question, whose Yes puts it in and restarts the game.
fn pick_setting(menu: &mut Menu, requests: &mut Vec<crate::online_menu::OnlineRequest>, setting: Setting, index: usize) {
    if menu.restarts(setting, index) {
        requests.push(crate::online_menu::OnlineRequest::Restart(setting, index));
    } else {
        menu.choose(setting, index);
    }
}

const MANIFEST: &str = "runtime/core_menu.json";
fn root(asset_root: &std::path::Path) -> PathBuf {
    std::env::var_os("SKATE_CORE_MENU_ROOT").map(PathBuf::from).unwrap_or_else(|| asset_root.join("private/core-menu"))
}
/// What setup reads, for the startup prefetch (apt_canvas::prefetch).
pub(crate) fn manifest(asset_root: &std::path::Path) -> PathBuf {
    root(asset_root).join(MANIFEST)
}

fn setup(
    mut commands: Commands,
    config: Res<Config>,
    cameras: Query<Entity, With<IsDefaultUiCamera>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut images: ResMut<Assets<Image>>,
    mut composites: ResMut<Assets<HudComposite>>,
    mut blurs: ResMut<Assets<PauseBlur>>,
    scene: Option<Res<crate::graphics_menu::SceneTarget>>,
) {
    let Ok(output) = cameras.single() else {
        error!("Original Start menu requires the presentation camera");
        return;
    };
    let root = root(&config.asset_root);
    let mut canvas = Canvas::new(&mut commands, &mut images, &mut composites, &window, root.clone(), LAYER, -2);
    let result = (|| -> Result<(Runtime<Action>, Handle<Image>), String> {
        let source: serde_json::Value = match crate::apt_canvas::prefetched_json(&root.join(MANIFEST)) {
            Some(json) => json,
            None => serde_json::from_slice(
                &std::fs::read(root.join(MANIFEST)).map_err(|e| format!("{}: {e}", root.display()))?,
            )
            .map_err(|e| e.to_string())?,
        };
        if source["format"] != "skate3-core-menu" || source["version"] != 1 {
            return Err("Unsupported core menu manifest".into());
        }
        // Execute the movie once now, so a defective install falls back before anyone presses Start; each page is a
        // copy of this load.
        let booted = Runtime::load(&source, crate::core_menu_runtime::boot_probe(Action::Resume), String::new())?;
        for (path, size) in movie_textures(&source)? {
            canvas.preload(&mut images, &path, size)?;
        }
        let backdrop = &source["backdrop"];
        let vignette = canvas.preload(
            &mut images,
            backdrop["rgba"].as_str().ok_or("Core menu backdrop missing")?,
            [backdrop["width"].as_u64().unwrap_or(0) as u32, backdrop["height"].as_u64().unwrap_or(0) as u32],
        )?;
        Ok((booted, vignette))
    })();
    let (booted, vignette) = match result {
        Ok(value) => value,
        Err(error) => {
            warn!("Original Start menu unavailable ({error}); using the classic pause panel. Prepare it with tools/prepare_core_menu.py");
            return;
        }
    };
    let composite = canvas.composite.clone();
    let full = || Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() };
    let blur = scene.map(|scene| blurs.add(PauseBlur { scene: scene.0.clone() }));
    let cache = blur.as_ref().map(|blur| {
        let size = window.physical_size().max(UVec2::ONE);
        let mut image = Image::new_target_texture(size.x, size.y, bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb, None);
        image.data = None; // GPU-drawn: no CPU copy (perf 10-18, see apt_canvas::Canvas::new)
        image.copy_on_resize = false;
        let image = images.add(image);
        let camera = commands
            .spawn((
                Camera2d,
                Camera { order: 3, is_active: false, ..default() },
                bevy::camera::RenderTarget::Image(image.clone().into()),
                bevy::camera::visibility::RenderLayers::layer(BLUR_LAYER),
                Msaa::Off,
            ))
            .id();
        commands.spawn((MaterialNode(blur.clone()), UiTargetCamera(camera), Pickable::IGNORE, full()));
        BlurCache { camera, image, state: 0 }
    });
    // Stack as in the shipped game: blurred paused world, main/dimmer vignette, crossbar.
    let mut overlay = commands.spawn((
        CrossbarOverlay,
        UiTargetCamera(output),
        GlobalZIndex(10),
        Pickable::IGNORE,
        Node { display: Display::None, ..full() },
    ));
    match (&blur, &cache) {
        (Some(blur), Some(cache)) => {
            overlay.with_child((MaterialNode(blur.clone()), LiveBlur, Pickable::IGNORE, full()));
            overlay.with_child((ImageNode::new(cache.image.clone()), CachedBlur, Pickable::IGNORE, Node { display: Display::None, ..full() }));
        }
        _ => warn!("Pause blur unavailable: no scene target"),
    }
    overlay
        .with_child((
            Dimmer,
            ImageNode { color: Color::WHITE.with_alpha(0.), ..ImageNode::new(vignette) },
            Pickable::IGNORE,
            full(),
        ))
        .with_child((
            MaterialNode(composite.clone()),
            Pickable::IGNORE,
            Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() },
        ));
    commands.insert_resource(Crossbar {
        booted: Some(Booted::new(booted)),
        canvas,
        runtime: None,
        shown: false,
        closing: None,
        accumulated: 0.,
        opened: 0.,
        teleports: Vec::new(),
        watchable: Vec::new(),
        marker_list: false,
        settings: false,
        search: None,
        voice: false,
        capture: None,
        blur,
        cache,
        sounds: Vec::new(),
        last: None,
        progression: crate::core_menu_runtime::NO_PROGRESSION,
    });
    info!("Original Start menu loaded from {}", root.display());
}

fn begin_close(crossbar: &mut Crossbar) {
    // Nothing to close: a session move's pick runs with the crossbar shut.
    if crossbar.runtime.is_none() {
        return;
    }
    crossbar.sounds.push(crate::core_menu_runtime::SOUND_CROSSBAR_OUT);
    let result = crossbar.runtime.as_mut().map(|runtime| runtime.close());
    if let Some(Err(error)) = result {
        warn!("Original Start menu outro: {error}");
    }
    crossbar.closing = Some(0.);
    crossbar.capture = None;
}

/// A Challenge Map pick: teleport on this map, or load the spot's map and travel
/// once it is up. Closing leaves the Start menu as a teleport does.
fn travel_to_spot(
    spot: Spot,
    crossbar: &mut Crossbar,
    menu: &mut Menu,
    travel: &mut crate::teleport_menu::Travel,
    skater: &mut crate::physics::SkaterRuntime,
    current: &crate::map_transition::CurrentMap,
    arrival: Option<&mut Option<[[f32; 4]; 4]>>,
) -> Option<bool> {
    info!("Challenge Map: travel to {:?} on {} (current map {:?})", spot.id, spot.map, current.path);
    if current.path.as_ref().is_some_and(|p| crate::teleport_menu::same_map(p, &spot.map)) {
        // Same map: the exact spot, else the teleport destination (as the post-load path does).
        let matrix = spot.matrix.or_else(|| {
            let id = spot.id.as_ref()?;
            travel.destinations().iter().find(|d| &d.id == id)?.matrix
        });
        match arrival {
            // User 10-26: an online pick's spot is reached behind its session's loading screen (OnlineSession::arrival).
            Some(arrival) => *arrival = matrix,
            None => {
                if let Some(Err(error)) = matrix.map(|m| skater.travel_and_mark(m)) {
                    warn!("Teleport: {error}");
                }
            }
        }
        begin_close(crossbar);
        return Some(false);
    }
    match menu.map_labels().iter().position(|label| crate::map_library::same_name(label, &spot.map)) {
        Some(index) => {
            travel.pending = spot.id;
            travel.pending_matrix = spot.matrix;
            // The classic panel owns map loading (multiplayer guard, progress, resume).
            crossbar.runtime = None;
            menu.hand_off(7, false, true, Some(index));
            Some(true)
        }
        None => {
            warn!("Challenge Map: {} is not installed", spot.map);
            None
        }
    }
}

/// An Online Challenge Map pick: maybe travel, then queue the lobby action.
#[allow(clippy::too_many_arguments)]
fn perform_online(
    pick: crate::online_map::Pick,
    crossbar: &mut Crossbar,
    menu: &mut Menu,
    travel: &mut crate::teleport_menu::Travel,
    skater: &mut crate::physics::SkaterRuntime,
    current: &crate::map_transition::CurrentMap,
    net: &mut crate::multiplayer::Multiplayer,
    session: &mut crate::online_session::OnlineSession,
) {
    use crate::online_map::Pick;
    use crate::online_session::Intent;
    info!("Online pick: {pick:?}");
    let follow = session.follow.take();
    // A new pick supersedes any earlier one whose map load never finished, and any browse in flight.
    session.pending = None;
    session.awaiting_world = false;
    session.cancel_browse();
    travel.pending = None;
    travel.pending_matrix = None;
    let mut intent = crate::online_session::intent(&pick);
    session.lobby = crate::online_session::lobby(&intent);
    session.location = match &pick {
        Pick::Location { name, .. } => Some(name.clone()),
        _ => None,
    };
    // A session move: the host's new lobby carries the move's token, which the guests join by.
    if let Some(follow) = follow {
        intent = crate::online_session::follow_intent(intent, &follow);
        session.carry = follow.host.then_some(follow.carry);
    } else {
        session.carry = crate::online_session::fresh_carry(&intent);
    }
    // The host spawns on slot 0 (online_boundary moves a joining client to its own slot).
    let start = |spawn: &crate::online_map::Spawn| spawn.start.as_ref().map(|s| s.slot(0));
    let mut area = None;
    let spot = match pick {
        // Commands act on the map you're on (every browse is limited to it).
        Pick::Command { .. } => None,
        Pick::Location { location, .. } => {
            let spawn = location.spawn_at(travel.destinations());
            let matrix = start(&spawn);
            area = Some(crate::online_boundary::Area::new(spawn));
            Some(Spot { map: location.map, id: location.teleport, matrix })
        }
        Pick::Challenge { challenge, .. } => {
            let matrix = start(&challenge.spawn).or_else(|| {
                challenge.position.map(|[x, y, z]| [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [x, y, z, 1.]])
            });
            area = Some(crate::online_boundary::Area::new(challenge.spawn));
            Some(Spot { map: challenge.map, id: challenge.teleport, matrix })
        }
    };
    session.area = None;
    let Some(spot) = spot else {
        if intent == Intent::Solo {
            net.leave();
        } else {
            session.pending = Some(intent);
        }
        // Close as a same-map teleport does: the outro's close step gives back the world and the camera.
        begin_close(crossbar);
        return;
    };
    // The classic panel refuses map loads while connected; an uninstalled map keeps the session.
    if menu.map_labels().iter().any(|label| crate::map_library::same_name(label, &spot.map)) {
        net.leave();
    }
    if let Some(loading) = travel_to_spot(spot, crossbar, menu, travel, skater, current, Some(&mut session.arrival)) {
        session.awaiting_world = loading;
        session.pending = Some(intent);
        // Same map: bind the area to it now, so a later load (even before a gameplay frame) ends it.
        session.area = area.map(|mut a| {
            a.generation = (!loading).then_some(current.generation);
            a
        });
    } else {
        // Review 10-24: a pick that goes nowhere (an uninstalled map) leaves no private state for the next session.
        session.carry = None;
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn interact(
    crossbar: Option<ResMut<Crossbar>>,
    (mut config, mut menu, mut physics, mut skater): (
        ResMut<Config>,
        ResMut<Menu>,
        ResMut<crate::physics::GamePhysics>,
        ResMut<crate::physics::SkaterRuntime>,
    ),
    (keys, mouse, nav, matched, mut end_ots): (
        Res<ButtonInput<KeyCode>>,
        Res<ButtonInput<MouseButton>>,
        Res<crate::customiser::Navigation>,
        Res<crate::match_core::Match>,
        Option<ResMut<crate::skate3_ots::EndSession>>,
    ),
    (mut travel, mut customiser, custom_models, mods, transition, mut net): (
        ResMut<crate::teleport_menu::Travel>,
        ResMut<crate::customiser::Customiser>,
        Res<crate::custom_models::CustomModels>,
        Res<crate::modding::ModMenu>,
        Res<crate::map_transition::MapTransition>,
        ResMut<crate::multiplayer::Multiplayer>,
    ),
    mut online: ResMut<crate::online_session::OnlineSession>,
    mut time: ResMut<Time<Virtual>>,
    (mut exit, mut sounds, mut tutorial_start, lots): (
        MessageWriter<AppExit>,
        MessageWriter<crate::skate_audio::FrontEndSoundRequest>,
        MessageWriter<crate::skate2_challenge::TutorialCommand>,
        Option<Res<crate::skate3_otl::OtlLots>>,
    ),
    (mut challenge, current, mut lesson, tutorial, mut skate1): (
        Option<ResMut<ChallengeMap>>,
        Res<crate::map_transition::CurrentMap>,
        Option<ResMut<crate::challenge::LessonMenu>>,
        Option<Res<crate::skate2_challenge::TutorialMenu>>,
        Option<ResMut<crate::skate1_challenge::Skate1Challenge>>,
    ),
    (mut online_ui, roster, playable, mut music_ui, mut help, mut party, mut retail_ui, mut team_ui): (
        ResMut<crate::online_menu::OnlineUi>,
        Res<crate::online_roster::OnlineRoster>,
        Res<crate::match_core::Playable>,
        ResMut<crate::music_ui::MusicUi>,
        Option<ResMut<crate::hud_meters::OnScreenHelp>>,
        ResMut<crate::party_play::PartyPlay>,
        ResMut<crate::retail_ui::RetailUi>,
        ResMut<crate::team_select::TeamUi>,
    ),
    (mut replay_hud, replay): (Option<ResMut<crate::replay::hud::ReplayHud>>, Option<Res<crate::replay::Replay>>),
    (mut spectate, mut marker): (ResMut<crate::match_core::Spectate>, Option<ResMut<crate::session_marker::SessionMarker>>),
    mut intro_nav: Local<Option<Key>>,
    mut career: Option<ResMut<crate::skate3_career::glue::Career>>,
    (mut game_mode, mut free_play, mut cheats): (
        Option<ResMut<GameMode>>,
        Option<ResMut<crate::skate3_career::freeplay::FreePlay>>,
        Option<ResMut<crate::skate3_career::cheats::Cheats>>,
    ),
) {
    let in_lesson = lesson.as_ref().is_some_and(|l| l.active);
    let tutorial_available = tutorial.as_ref().is_some_and(|t| t.available);
    let skate1_available = skate1.as_ref().is_some_and(|s| s.available());
    let Some(mut crossbar) = crossbar else { return };
    // Voice chat (user 10-26): Mic Test runs only while a page shows its row.
    let open = crossbar.runtime.is_some() && crossbar.closing.is_none();
    if menu.mic_test && !keeps_mic_test(open, crossbar.voice, crossbar.search.as_ref().map(|(_, found)| found.as_slice())) {
        menu.mic_test = false;
    }
    // Device lists read on their thread (Menu::refresh_voice_devices): Game Settings, its search page or the Voice Chat
    // page redraws its rows with them.
    if menu.voice_devices_reading.is_some() && menu.take_voice_devices() && open && (crossbar.voice || crossbar.on_game_settings()) {
        if let Err(error) = show_devices(&mut crossbar, &menu) {
            warn!("Game Settings: {error}");
        }
    }
    // Taken every frame, so a request this frame can't serve (another screen up) is dropped, never kept for later.
    let players = marker.as_mut().filter(|m| m.open_player_list).is_some_and(|m| std::mem::take(&mut m.open_player_list));
    crossbar.progression = career
        .as_ref()
        .and_then(|c| c.state.as_ref())
        .map_or(crate::core_menu_runtime::NO_PROGRESSION, crate::skate3_career::screens::progression_data);
    for id in crossbar.sounds.drain(..) {
        sounds.write(crate::skate_audio::FrontEndSoundRequest(id));
    }
    if let Some(challenge) = challenge.as_mut() {
        if let Some(pick) = challenge.picked_online.take() {
            // A challenge row no mode plays here (Team, Career, an unported mode) makes no lobby: its retail notice.
            if let Some(id) = crate::match_core::refused_pick(&pick, &playable).filter(|_| online.follow.is_none()) {
                online_ui.requests.push(crate::online_menu::OnlineRequest::Notice(id));
                return;
            }
            // In a session a challenge or location moves everyone (host) or is proposed (guest): online_menu decides.
            match pick.target().filter(|_| net.active() && online.follow.is_none()) {
                Some(target) => {
                    let capacity = crate::online_session::capacity(&pick);
                    online_ui.requests.push(crate::online_menu::OnlineRequest::Move(target, capacity));
                    begin_close(&mut crossbar);
                }
                None => perform_online(pick, &mut crossbar, &mut menu, &mut travel, &mut skater, &current, &mut net, &mut online),
            }
            return;
        }
        if let Some(spot) = challenge.picked.take() {
            travel_to_spot(spot, &mut crossbar, &mut menu, &mut travel, &mut skater, &current, None);
            return;
        }
        // The Challenge Map owns input while open, and the press that closed it.
        if challenge.active() || challenge.closed_this_frame {
            return;
        }
    }
    // Quit Session was confirmed, or the session ended or changed.
    if std::mem::take(&mut online_ui.close_crossbar) && crossbar.runtime.is_some() && crossbar.closing.is_none() {
        begin_close(&mut crossbar);
        return;
    }
    // Not retail (user 10-27): Performance Mode's restart was confirmed: the pick goes in, then the game restarts
    // (crash_report's supervisor starts it again).
    if let Some((setting, index)) = online_ui.restart.take() {
        menu.choose(setting, index);
        eprintln!("{}", crate::crash_report::RESTART);
        exit.write(AppExit::Success);
        return;
    }
    // Party Play launched, or its Exit was confirmed.
    if std::mem::take(&mut party.close_crossbar) && crossbar.runtime.is_some() && crossbar.closing.is_none() {
        begin_close(&mut crossbar);
        return;
    }
    // Not retail (user 10-26): Settings search's Done (online_menu's keyboard): the page of what its words found.
    if let Some(words) = online_ui.searched.take() {
        if crossbar.runtime.is_some() && crossbar.closing.is_none() && !words.is_empty() {
            open_search(&mut crossbar, &mut menu, &words);
        }
        return;
    }
    // The confirm popup owns input while open, and the press that closed it.
    if online_ui.modal || online_ui.closed_this_frame || music_ui.modal || music_ui.closed_this_frame {
        return;
    }
    if retail_ui.modal || retail_ui.closed_this_frame {
        return;
    }
    // Extras > Skate 2 Gestures closed its screen: the crossbar shows the gestures page.
    if std::mem::take(&mut retail_ui.gestures) && crossbar.runtime.is_some() && crossbar.closing.is_none() {
        open_gestures(&mut crossbar, &mut menu);
        return;
    }
    if team_ui.modal || team_ui.closed_this_frame {
        return;
    }
    // So do Party Play's setup, Press-A card and popups.
    if party.modal || party.closed_this_frame {
        return;
    }
    // In a session: Some(hosting or co-host) (the info panel moves or proposes).
    let in_session = net.active().then(|| net.mod_identity().2 || net.cohost(&roster.state));
    // A sign-in for the Online Challenge Map succeeded (online_menu): open it now, as retail replays the pick.
    if std::mem::take(&mut online_ui.open_challenge_map) {
        if let Some(Err(error)) = challenge.as_mut().map(|c| c.open_online(&menu.map_labels(), in_session)) {
            warn!("Online Challenge Map: {error}");
        }
        return;
    }
    let has_map = challenge.is_some();
    let online = challenge.as_ref().is_some_and(|c| c.online_available());
    let signed_in = online_ui.signed_in;
    let (party_active, party_installed) = (party.active, party.installed);
    let replay_ready = replay.as_deref().is_some_and(|r| r.frames().len() >= crate::replay::MIN_BACK_FRAMES);
    let unlock_everything = career.as_ref().filter(|c| c.state.is_some()).map(|c| c.store.everything);
    let free_play_state =
        unlock_everything.map(|_| (game_mode.as_deref().copied().unwrap_or_default(), matched.phase != crate::match_core::Phase::Idle));
    // In a session the crossbar shows the online tables; in a Party Play match, the Party Play page.
    let main_page = |menu: &Menu, destinations: &[crate::teleport_menu::Destination], help: Option<&crate::hud_meters::OnScreenHelp>| {
        if party_active {
            return party_categories(signed_in);
        }
        let (page, title) = if net.active() {
            let (_, _, host) = net.mod_identity();
            let challenge = roster.state.challenge.as_ref().map(|c| c.team);
            let s = &roster.state;
            let can_start = crate::match_core::startable(s, &playable).is_some();
            let end_ots = crate::skate3_ots::host_can_end(&matched, host);
            let lot = s.challenge.as_ref().and_then(|c| c.row.as_ref()).map(|r| r.id.as_str());
            let activities = (lots.as_deref().zip(lot).and_then(|(l, lot)| l.0.get(lot)))
                .map_or_else(Vec::new, |a| a.iter().map(|(_, title)| format!("#{title}")).collect());
            let state = OnlineMenu { host, steam: net.is_steam(), private: s.private, challenge, no_proposals: s.no_proposals, collisions: s.collisions, listed: net.details.listed, can_start, end_ots, activities, active: matched.phase != crate::match_core::Phase::Idle, spectate: crate::match_core::spectatable(&roster).into_iter().map(|(_, n)| n).collect(), edit: edit_blocked(matched.phase != crate::match_core::Phase::Idle, custom_models.active.is_some()) };
            online_categories(&state, online)
        } else {
            let page = categories(menu, destinations, has_map, tutorial_available, skate1_available, online, signed_in);
            let (page, title) = with_free_play(page, free_play_state.map(|(mode, busy)| (mode, busy || in_lesson)));
            (with_replay(with_unlocks(with_party(with_lesson(page, in_lesson), party_installed), unlock_everything), replay_ready), title)
        };
        (with_help(page, help), title)
    };
    let start = keys.just_pressed(KeyCode::Escape) || nav.pressed & 0x10 != 0;
    if crossbar.runtime.is_none() {
        *intro_nav = None;
        let blocked = menu.open || transition.busy() || travel.open || travel.closed_this_frame
            || customiser.open || custom_models.open || mods.open || tutorial.as_ref().is_some_and(|t| t.busy)
            || start_blocked_by_mode(matched.pause_blocked(), net.active());
        if !(start || players) || blocked {
            return;
        }
        let destinations = travel.destinations();
        let (page, title) = main_page(&menu, destinations, help.as_deref());
        // Not retail (user 10-25): the marker panel's Player List opens on the Spectate row with its list open.
        let list = players && !start;
        if list && !page.iter().flat_map(|c| &c.items).any(|i| i.enabled && i.action == Action::Spectate) {
            warn!("Player List: this Start menu page has no Spectate list");
            return;
        }
        match switch_page(&mut crossbar, page, title, false, list.then_some(&Action::Spectate)) {
            Ok(()) => {
                let (m, o) = reopen_at(crossbar.last.as_ref(), title);
                if let Some(runtime) = crossbar.runtime.as_mut().filter(|_| !list) {
                    runtime.focus(m, o);
                }
                // The row's Accept, played once the intro is done, opens the list.
                if list {
                    *intro_nav = Some(Key::Accept);
                }
                crossbar.marker_list = list;
                crossbar.teleports = destinations.iter().filter_map(|d| d.matrix).collect();
                crossbar.watchable = crate::match_core::spectatable(&roster).into_iter().map(|(a, _)| a).collect();
                crossbar.opened = 0.;
                crossbar.sounds.extend([crate::core_menu_runtime::SOUND_PAUSE_MENU, crate::core_menu_runtime::SOUND_CROSSBAR_IN]);
                menu.open = true;
                menu.crossbar = true;
                if !net.active() {
                    time.pause();
                }
            }
            // The classic panel sees this same press and opens instead.
            Err(error) => error!("Original Start menu failed to open: {error}"),
        }
        return;
    }
    if crossbar.closing.is_some() || !crossbar.shown {
        // The Player List's queued Accept is not replaced by a direction.
        if crossbar.closing.is_none() && !crossbar.marker_list {
            buffer_intro_nav(&mut intro_nav, nav.pressed);
        }
        return;
    }
    // Push to Talk Key (user 10-26): the next key or mouse side button pressed is the key; Esc, B or Start keeps the old.
    if let Some(setting) = crossbar.capture {
        let pressed: Vec<KeyCode> = keys.get_just_pressed().copied().collect();
        let buttons: Vec<MouseButton> = mouse.get_just_pressed().copied().collect();
        let Some(key) = captured_key(&pressed, &buttons, nav.pressed & (0x2000 | 0x10) != 0) else { return };
        crossbar.capture = None;
        if let Some(key) = key {
            menu.set_ptt_key(key);
        }
        let rows = Some(crossbar.settings_rows(&menu));
        if let Some(Err(error)) = crossbar.runtime.as_mut().map(|r| show_setting(r, &menu, setting, rows)) {
            warn!("Game Settings: {error}");
        }
        return;
    }
    if start {
        begin_close(&mut crossbar);
        return;
    }
    // Not retail (user 10-26): Y (pad or keyboard) on Game Settings or its search page types a search, starting from
    // the page's words.
    if crossbar.on_game_settings() && (nav.pressed & 0x8000 != 0 || keys.just_pressed(KeyCode::KeyY)) {
        let words = crossbar.search.as_ref().map_or_else(String::new, |(words, _)| words.clone());
        online_ui.requests.push(crate::online_menu::OnlineRequest::SearchSettings(words));
        return;
    }
    let pressed = [
        (nav.pressed & 1 != 0, Key::Up),
        (nav.pressed & 2 != 0, Key::Down),
        (nav.pressed & 4 != 0, Key::Left),
        (nav.pressed & 8 != 0, Key::Right),
        (keys.just_pressed(KeyCode::Enter) || nav.pressed & 0x1000 != 0, Key::Accept),
        (keys.just_pressed(KeyCode::Backspace) || nav.pressed & 0x2000 != 0, Key::Back),
    ];
    let buffered = intro_nav.take().map(|key| (true, key));
    for (_, key) in buffered.into_iter().chain(pressed).filter(|(down, _)| *down) {
        let (settings_page, marker_list) = (crossbar.settings, crossbar.marker_list);
        let Some(runtime) = crossbar.runtime.as_mut() else { return };
        let result = runtime.input(key);
        if let (Ok(None), Some(item)) = (&result, runtime.current_mut()) {
            crate::apt_screen::warm(warm_files(&item.action, &config.asset_root));
        }
        let played = std::mem::take(&mut runtime.sounds);
        let at = (!settings_page && !marker_list).then(|| (runtime.bindings.title.clone(), runtime.position()));
        crossbar.sounds.extend(played);
        if at.is_some() {
            crossbar.last = at;
        }
        // The Player List: Back leaves the list and the menu together.
        if marker_list && key == Key::Back {
            begin_close(&mut crossbar);
            return;
        }
        match result {
            Ok(None) => {}
            // Not retail (user 10-26): Back from Settings search's page lands on Game Settings, on the row it showed; from
            // the Voice Chat page, or a voice row a search found, on the Audio tab's Voice Chat row.
            Ok(Some(Outcome::Close)) if settings_page && (crossbar.search.is_some() || crossbar.voice) => {
                let row = crossbar.runtime.as_mut().and_then(|r| r.current_mut()).map(|i| i.action.clone());
                let focus = settings_back(&menu, crossbar.voice, row.as_ref());
                if let Err(error) = switch_page(&mut crossbar, settings_categories(&menu), SETTINGS_TITLE, true, Some(&focus)) {
                    error!("Game Settings page failed to open: {error}");
                    crossbar.runtime = None;
                    menu.hand_off(0, false, false, None);
                }
                return;
            }
            Ok(Some(Outcome::Close)) if settings_page => {
                // Back leaves Game Settings for the Options row that opened it.
                let (page, title) = main_page(&menu, travel.destinations(), help.as_deref());
                crossbar.watchable = crate::match_core::spectatable(&roster).into_iter().map(|(a, _)| a).collect();
                // Back from the gestures page lands on Extras, which opened it; Game Settings' pages on Game Settings.
                let from_gestures = crossbar.runtime.as_ref().is_some_and(|r| {
                    r.bindings.categories.first().and_then(|c| c.items.first()).is_some_and(|i| matches!(i.action, Action::Skate2Gesture(_)))
                });
                let focus = if from_gestures { Action::Retail(crate::retail_ui::Panel::Extras) } else { Action::GameSettings };
                if let Err(error) = switch_page(&mut crossbar, page, title, false, Some(&focus)) {
                    error!("Original Start menu stopped: {error}");
                    crossbar.runtime = None;
                    menu.hand_off(8, false, false, None);
                }
                return;
            }
            Ok(Some(Outcome::Activate(Action::Skate2Gestures))) => {
                open_gestures(&mut crossbar, &mut menu);
                return;
            }
            Ok(Some(Outcome::Activate(Action::GameSettings))) => {
                // The Audio tab's Output Device lists the outputs as they are now.
                menu.refresh_voice_devices();
                if let Err(error) = switch_page(&mut crossbar, settings_categories(&menu), SETTINGS_TITLE, true, None) {
                    error!("Game Settings page failed to open: {error}");
                    crossbar.runtime = None;
                    menu.hand_off(0, false, false, None);
                }
                return;
            }
            Ok(Some(Outcome::Activate(Action::VoiceChat))) => {
                open_voice(&mut crossbar, &mut menu);
                return;
            }
            // The next key pressed sets it (the capture above); the row says so meanwhile.
            Ok(Some(Outcome::Activate(Action::Setting(Setting::PushToTalkKey)))) => {
                crossbar.capture = Some(Setting::PushToTalkKey);
                if let Some(Err(error)) = crossbar.runtime.as_mut().map(|r| show_value(r, 0, "Press a key")) {
                    warn!("Game Settings: {error}");
                }
                return;
            }
            Ok(Some(Outcome::Choose(Action::Setting(setting), index))) => {
                pick_setting(&mut menu, &mut online_ui.requests, setting, index);
                let rows = settings_page.then(|| crossbar.settings_rows(&menu));
                if let Some(Err(error)) = crossbar.runtime.as_mut().map(|r| show_setting(r, &menu, setting, rows)) {
                    warn!("Game Settings: {error}");
                }
            }
            Ok(Some(Outcome::Slide(Action::Setting(setting), step))) => {
                if menu.slide(setting, step) {
                    if let Some(Err(error)) = crossbar.runtime.as_mut().map(|r| r.show_slider(slider_line(&menu, setting))) {
                        warn!("Game Settings: {error}");
                    }
                }
            }
            Ok(Some(Outcome::Close | Outcome::Activate(Action::Resume))) => {
                begin_close(&mut crossbar);
                return;
            }
            Ok(Some(Outcome::Activate(Action::Lesson(request)))) => {
                if let Some(lesson) = lesson.as_mut() {
                    lesson.request = Some(request);
                }
                begin_close(&mut crossbar);
                return;
            }
            Ok(Some(Outcome::Activate(Action::Tutorial(command)))) => {
                tutorial_start.write(command);
                begin_close(&mut crossbar);
                return;
            }
            Ok(Some(Outcome::Activate(Action::Skate1Tutorial))) => {
                if let Some(skate1) = skate1.as_mut() {
                    skate1.resume();
                }
                begin_close(&mut crossbar);
                return;
            }
            Ok(Some(Outcome::Activate(Action::Skate1StartOver))) => {
                if let Some(skate1) = skate1.as_mut() {
                    skate1.start_over();
                }
                begin_close(&mut crossbar);
                return;
            }
            Ok(Some(Outcome::Activate(Action::FreePlay))) => {
                if let (Some(mode), Some(fp)) = (game_mode.as_mut(), free_play.as_mut()) {
                    let screen = fp.settings;
                    **mode = crate::skate3_career::freeplay::go_skate(fp, &screen, "", true).mode;
                }
                begin_close(&mut crossbar);
                return;
            }
            Ok(Some(Outcome::Activate(Action::ResumeCareer))) => {
                if let (Some(mode), Some(fp)) = (game_mode.as_mut(), free_play.as_mut()) {
                    let mut none = crate::skate3_career::cheats::Cheats::default();
                    let cheats = cheats.as_deref_mut().unwrap_or(&mut none);
                    **mode = crate::skate3_career::freeplay::resume_career(fp, cheats).mode;
                }
                begin_close(&mut crossbar);
                return;
            }
            Ok(Some(Outcome::Activate(Action::Quit))) => {
                exit.write(AppExit::Success);
                return;
            }
            Ok(Some(Outcome::Activate(Action::EndOts))) => {
                if let Some(end) = end_ots.as_mut() {
                    end.0 = true;
                }
                begin_close(&mut crossbar);
                return;
            }
            Ok(Some(Outcome::Activate(Action::Online(action)))) => {
                online_ui.requests.extend(crate::online_menu::request(action, None));
                return;
            }
            // Host Actions > Session Difficulty / Weather and Time / Physics: Game Settings on that row.
            Ok(Some(Outcome::Choose(Action::Online(OnlineAction::HostActions), index @ 2..=4))) => {
                let row = [Action::Difficulty, Action::Setting(Setting::Weather), Action::Physics][index - 2].clone();
                menu.refresh_voice_devices();
                if let Err(error) = switch_page(&mut crossbar, settings_categories(&menu), SETTINGS_TITLE, true, Some(&row)) {
                    error!("Game Settings page failed to open: {error}");
                    crossbar.runtime = None;
                    menu.hand_off(0, false, false, None);
                }
                return;
            }
            Ok(Some(Outcome::Choose(Action::Online(action), index))) => {
                use crate::online_menu::OnlineRequest;
                let request = crate::online_menu::request(action, Some(index));
                // A toggle's label follows at once (online_menu applies it next frame).
                let flip = match request {
                    Some(OnlineRequest::TogglePrivacy) => Some((1, [PRIVATE, PUBLIC])),
                    Some(OnlineRequest::ToggleProposals) => Some((0, [PROPOSALS_ON, PROPOSALS_OFF])),
                    Some(OnlineRequest::ToggleCollisions) => Some((5, [COLLISIONS_ON, COLLISIONS_OFF])),
                    Some(OnlineRequest::ToggleListed) => Some((6, [UNLISTED, LISTED])),
                    _ => None,
                };
                if let Some(Err(error)) = flip.and_then(|(i, names)| crossbar.runtime.as_mut().map(|r| flip_choice(r, i, names))) {
                    warn!("Host Actions: {error}");
                }
                online_ui.requests.extend(request);
                return;
            }
            Ok(Some(Outcome::Activate(Action::Panel { row, multiplayer, perform }))) => {
                crossbar.runtime = None;
                menu.hand_off(row, multiplayer, perform, None);
                return;
            }
            Ok(Some(Outcome::Choose(Action::Spectate, index))) => {
                // The name shown at `index` (online audit 10-12: the session as of now shifted on a join or leave).
                spectate.0 = crate::match_core::spectate_pick(&crossbar.watchable, index, &roster);
                begin_close(&mut crossbar);
                return;
            }
            Ok(Some(Outcome::Choose(Action::Teleport, index))) => {
                match crossbar.teleports.get(index).map(|m| skater.travel_and_mark(*m)) {
                    Some(Ok(())) => {
                        begin_close(&mut crossbar);
                        return;
                    }
                    Some(Err(error)) => warn!("Teleport: {error}"),
                    None => {}
                }
            }
            // Retail RequireSignIn: signed out, the pick signs in first and is replayed after.
            Ok(Some(Outcome::Activate(Action::OnlineChallengeMap))) if !online_ui.signed_in => {
                online_ui.sign_in_for = Some(crate::online_menu::Then::ChallengeMap);
                return;
            }
            Ok(Some(Outcome::Activate(Action::Music(panel)))) => {
                music_ui.request = Some(panel);
                return;
            }
            Ok(Some(Outcome::Activate(Action::Retail(panel)))) => {
                retail_ui.request = Some(panel);
                return;
            }
            // 0x8261c474: push FE slot 63 over the crossbar.
            Ok(Some(Outcome::Activate(Action::TeamManagement))) => {
                team_ui.request = true;
                return;
            }
            // The crossbar closes under the CAS (it runs with the world: customiser::world_pause). The skater is put
            // on its feet where it is, so it stands still while the pad edits (ControllerInput::cas).
            Ok(Some(Outcome::Activate(Action::EditSkater))) => {
                let active = matched.phase != crate::match_core::Phase::Idle;
                if net.active() && edit_blocked(active, custom_models.active.is_some()).is_none() {
                    let here = skater.animated_skeleton.roots.animation_to_world;
                    if let Err(error) = skater.travel_to_as(here, false) {
                        warn!("Edit Skater: the skater could not stop ({error})");
                    }
                    begin_close(&mut crossbar);
                    customiser.begin();
                }
                return;
            }
            // 0x8261c57c: push the setup (FE slot 31) over the crossbar.
            Ok(Some(Outcome::Activate(Action::PartyPlay))) => {
                party.open_setup = true;
                return;
            }
            Ok(Some(Outcome::Activate(Action::ExitPartyPlay))) => {
                party.ask = Some(crate::party_play::Ask::Exit);
                return;
            }
            Ok(Some(Outcome::Activate(Action::RestartChallenge))) => {
                party.restart = true;
                begin_close(&mut crossbar);
                return;
            }
            // 0x8261bc30 posts FlowMessage 144: the crossbar closes and StateReplay pushes the editor (Stage 0 §5).
            Ok(Some(Outcome::Activate(Action::Replay))) => {
                info!("Create: Replay Editor picked");
                if let Some(hud) = replay_hud.as_deref_mut() {
                    hud.request = Some(net.active());
                }
                begin_close(&mut crossbar);
                return;
            }
            Ok(Some(Outcome::Activate(Action::SkateReel))) => {
                // User 10-27: skate.Reel opened the Replay Editor; these lines say which path ran.
                info!("Create: skate.Reel picked");
                if let Some(hud) = replay_hud.as_deref_mut() {
                    hud.open_list = true;
                }
                begin_close(&mut crossbar);
                return;
            }
            // 8261EC50: the pick toggles one HUD (the other goes off); both choices then name what they'd do.
            Ok(Some(Outcome::Choose(Action::OnScreenHelp, index))) => {
                if let Some(help) = help.as_deref_mut() {
                    help.choose(index);
                    menu.set_on_screen_help([help.trick_analyzer, help.manual_meter]);
                    let choices = help.choices();
                    if let Some(runtime) = crossbar.runtime.as_mut() {
                        if let Some(item) = runtime.current_mut() {
                            item.choices = choices;
                        }
                        if let Err(error) = runtime.refresh() {
                            warn!("On Screen Help: {error}");
                        }
                    }
                }
            }
            Ok(Some(Outcome::Activate(Action::SignInOut))) => {
                online_ui.requests.push(crate::online_menu::OnlineRequest::SignInOut);
                return;
            }
            Ok(Some(Outcome::Activate(Action::OnlineChallengeMap))) => {
                if let Some(Err(error)) = challenge.as_mut().map(|c| c.open_online(&menu.map_labels(), in_session)) {
                    warn!("Online Challenge Map: {error}");
                }
                return;
            }
            Ok(Some(Outcome::Activate(Action::ChallengeMap))) => {
                if let Some(Err(error)) = challenge.as_mut().map(|c| c.open(&menu.map_labels())) {
                    warn!("Challenge Map: {error}");
                }
                return;
            }
            Ok(Some(Outcome::Choose(Action::LoadMap, index))) => {
                // The classic panel owns map loading (multiplayer guard, progress, resume).
                crossbar.runtime = None;
                menu.hand_off(7, false, true, Some(index));
                return;
            }
            Ok(Some(Outcome::Choose(Action::Physics, index))) => {
                use crate::skate2_physics::settings::Physics;
                if let Some(&physics) = Physics::ALL.get(index) {
                    if let Err(error) = menu.set_physics(physics, &config.asset_root) {
                        warn!("Physics: {error}");
                    }
                }
                // Off New San Vanelona the row greys or frees with the choice.
                let rows = crossbar.settings_rows(&menu);
                if let Some(runtime) = crossbar.runtime.as_mut() {
                    runtime.bindings.categories = rows;
                }
                let chosen = Physics::ALL.iter().position(|p| *p == menu.physics()).unwrap_or(0);
                if let Some(Err(error)) = crossbar.runtime.as_mut().map(|r| show_choice(r, chosen)) {
                    warn!("Game Settings: {error}");
                }
            }
            Ok(Some(Outcome::Choose(Action::Skate2Gesture(slot), index))) => {
                if let Err(error) = menu.set_skate2_gesture(usize::from(slot), index as u32, &config.asset_root) {
                    warn!("Skate 2 gestures: {error}");
                }
                let shown = menu.skate2_gestures().get(usize::from(slot)).map_or(0, |&g| g as usize);
                if let Some(Err(error)) = crossbar.runtime.as_mut().map(|r| show_choice(r, shown)) {
                    warn!("Skate 2 gestures: {error}");
                }
            }
            Ok(Some(Outcome::Choose(Action::UnlockEverything, index))) => {
                if let Some(career) = career.as_mut() {
                    career.store.set_everything(index == 1);
                    if let Err(error) = career.store.write(&career.settings) {
                        warn!("Skate 3 career unlocks.json: {error}");
                    }
                }
                if let Some(Err(error)) = crossbar.runtime.as_mut().map(|r| show_choice(r, index)) {
                    warn!("Unlock Everything: {error}");
                }
            }
            Ok(Some(Outcome::Choose(Action::Difficulty, index))) => {
                if let Some(&difficulty) = Difficulty::ALL.get(index) {
                    menu.set_difficulty(difficulty, &mut physics, &mut config);
                }
                if settings_page {
                    if let Some(Err(error)) = crossbar.runtime.as_mut().map(|r| show_choice(r, index)) {
                        warn!("Game Settings: {error}");
                    }
                } else if let Some(runtime) = crossbar.runtime.as_mut() {
                    // Play Mode's helper describes the new value.
                    if let Some(item) = runtime.current_mut() {
                        item.help = play_mode_help(menu.difficulty()).into();
                    }
                    if let Err(error) = runtime.refresh() {
                        warn!("Play Mode: {error}");
                    }
                }
            }
            Ok(Some(other)) => warn!("Original Start menu: unhandled {other:?}"),
            Err(error) => {
                // Never strand a paused player: continue in the classic panel.
                error!("Original Start menu stopped: {error}");
                crossbar.runtime = None;
                menu.hand_off(8, false, false, None);
                return;
            }
        }
    }
}

fn resize_target(
    crossbar: Option<ResMut<Crossbar>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut images: ResMut<Assets<Image>>,
    mut composites: ResMut<Assets<HudComposite>>,
) {
    if let Some(mut crossbar) = crossbar {
        crossbar.canvas.resize(&window, &mut images, &mut composites);
    }
}

/// A UI material keeps the GPU texture it was prepared with, and resizing the
/// scene image (startup window size, Render Scale) replaces that texture: the
/// paused-world backdrop sampled a never-drawn one and showed black. Rebind it.
fn rebind_blur(
    mut events: MessageReader<AssetEvent<Image>>,
    scene: Option<Res<crate::graphics_menu::SceneTarget>>,
    crossbar: Option<Res<Crossbar>>,
    mut blurs: ResMut<Assets<PauseBlur>>,
) {
    let resized = scene.is_some_and(|scene| events.read().any(|e| e.is_modified(&scene.0)));
    events.clear();
    if let Some(blur) = crossbar.as_ref().and_then(|c| c.blur.as_ref()).filter(|_| resized) {
        blurs.get_mut(blur);
    }
}

/// Once the outro plays (or times out), the world and the camera get the screen back
/// (`menu.crossbar` freezes the camera online; the open crossbar paused the world offline).
/// A runtime dropped without handing off to the classic panel ends the same way.
fn close_step(crossbar: &mut Crossbar, menu: &mut Menu, time: &mut Time<Virtual>, dt: f32) {
    if let Some(closing) = crossbar.closing.as_mut() {
        *closing += dt;
    }
    let done = match (&crossbar.runtime, crossbar.closing) {
        (Some(runtime), Some(closing)) => runtime.bindings.outro_complete || closing >= CLOSE_TIMEOUT,
        (Some(_), None) => false,
        (None, _) => menu.crossbar,
    };
    if done {
        crossbar.runtime = None;
        // perf 10-18: after an early resume the world is already back; whatever took the menu since keeps it.
        if menu.crossbar || !crate::config::perf_on("SKATE_EARLY_RESUME") {
            give_back(menu, time);
        }
    }
}
fn give_back(menu: &mut Menu, time: &mut Time<Virtual>) {
    menu.open = false;
    menu.crossbar = false;
    time.unpause();
}
/// perf 10-18: the world and the camera come back on the closing press, the outro playing over the live world (as it
/// does online), not at OutroComplete (~183 ms later). The press itself never reaches gameplay
/// (ControllerInput::menu_release), and the pause blur shows the live world as online. SKATE_EARLY_RESUME=0: at the end.
fn early_resume(
    crossbar: Option<Res<Crossbar>>,
    mut menu: ResMut<Menu>,
    mut time: ResMut<Time<Virtual>>,
    input: Option<ResMut<crate::input::ControllerInput>>,
) {
    if crossbar.is_some_and(|c| resumes_early(&c, &menu)) {
        give_back(&mut menu, &mut time);
        if let Some(mut input) = input {
            input.menu_release = true;
        }
    }
}
fn resumes_early(crossbar: &Crossbar, menu: &Menu) -> bool {
    crossbar.closing.is_some() && crossbar.runtime.is_some() && menu.crossbar && crate::config::perf_on("SKATE_EARLY_RESUME")
}

#[allow(clippy::too_many_arguments)]
fn animate(
    mut commands: Commands,
    crossbar: Option<ResMut<Crossbar>>,
    real: Res<Time<Real>>,
    mut time: ResMut<Time<Virtual>>,
    mut menu: ResMut<Menu>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<HudMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut cameras: Query<&mut Camera>,
    mut overlays: Query<&mut Node, With<CrossbarOverlay>>,
    mut dimmers: Query<&mut ImageNode, With<Dimmer>>,
    challenge: Option<Res<ChallengeMap>>,
    online_ui: Option<Res<crate::online_menu::OnlineUi>>,
    music_ui: Option<Res<crate::music_ui::MusicUi>>,
    party: Option<Res<crate::party_play::PartyPlay>>,
    (retail_ui, team_ui): (Option<Res<crate::retail_ui::RetailUi>>, Option<Res<crate::team_select::TeamUi>>),
) {
    let Some(mut crossbar) = crossbar else { return };
    let crossbar = &mut *crossbar;
    if crossbar.runtime.is_some() {
        crossbar.opened += real.delta_secs();
        let alpha = match crossbar.closing {
            Some(closing) => DIMMER_ALPHA * (1. - closing / CLOSE_TIMEOUT).max(0.),
            None => DIMMER_ALPHA * (crossbar.opened / DIMMER_FADE).min(1.),
        };
        for mut dimmer in &mut dimmers {
            dimmer.color.set_alpha(alpha);
        }
    }
    if let Some(runtime) = crossbar.runtime.as_mut() {
        crossbar.accumulated = (crossbar.accumulated + real.delta_secs()).min(4. * FRAME);
        let mut result = Ok(());
        while crossbar.accumulated >= FRAME && result.is_ok() {
            crossbar.accumulated -= FRAME;
            result = runtime.advance();
            if result.is_ok() && runtime.bindings.intro_complete && !crossbar.shown {
                crossbar.shown = true;
                result = runtime.show();
            }
        }
        if crossbar.closing.is_none() {
            if let Err(error) = result {
                error!("Original Start menu stopped: {error}");
                crossbar.runtime = None;
                menu.hand_off(8, false, false, None);
            }
        }
    }
    close_step(crossbar, &mut menu, &mut time, real.delta_secs());
    // The Challenge Map covers the Start menu while it is open; the online confirm popup hides it (ruling 9).
    // Audio Settings / Music Player replace it too (retail screens, not crossbar popups).
    let active = crossbar.runtime.is_some()
        && !challenge.is_some_and(|c| c.active())
        && !online_ui.is_some_and(|u| u.modal)
        && !music_ui.is_some_and(|u| u.modal)
        && !retail_ui.is_some_and(|u| u.modal)
        && !team_ui.is_some_and(|u| u.modal)
        && !party.is_some_and(|p| p.modal);
    crossbar.canvas.set_active(active, &mut cameras);
    for mut node in &mut overlays {
        let display = if active { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
    }
    // Not retail (user 10-26): "Y Search" while Game Settings takes input.
    let hint = crossbar.on_game_settings() && crossbar.shown && crossbar.closing.is_none();
    // Not retail (user 10-26): Mic Test's level bar while it runs (only while a page shows its row, keeps_mic_test), so
    // Open Mic Sensitivity can be set against it.
    let meter = (menu.mic_test && crossbar.shown && crossbar.closing.is_none())
        .then(|| (menu.mic_level_dbfs, skate_voice::dsp::line_dbfs(menu.open_mic_sensitivity())));
    // perf 10-18: rebuilt only when the movie changed (852 us a frame on the menu otherwise).
    let level = meter.map(|(level, line)| (level.round() as i32, line as i32));
    let key = crate::apt_scene::hash_of((crossbar.runtime.as_ref().map(|r| crate::apt_scene::draw_key(&r.bindings.movie, &r.vm)), active, hint, level));
    if crossbar.canvas.up_to_date(key) {
        return;
    }
    let mut draws = match crossbar.runtime.as_ref().map(|r| crate::apt_scene::draw(&r.bindings.movie, &r.vm, &r.bindings.shapes)) {
        Some(Ok(draws)) => draws,
        Some(Err(error)) => {
            error!("Original Start menu geometry: {error}");
            Vec::new()
        }
        None => Vec::new(),
    };
    if let Some(runtime) = crossbar.runtime.as_ref().filter(|_| hint) {
        draws.extend(search_hint(runtime));
    }
    if let Some((level, line)) = meter {
        draws.extend(mic_meter(level, line));
    }
    crossbar.canvas.draw(&draws, &mut commands, &mut images, &mut meshes, &mut materials);
    crossbar.canvas.drawn_as(key);
}

/// [`BlurCache`]: after camera::present decides whether the world holds its image this frame.
fn cache_blur(
    crossbar: Option<ResMut<Crossbar>>,
    world: Query<&Camera, With<crate::camera::GameplayCamera>>,
    mut cameras: Query<&mut Camera, Without<crate::camera::GameplayCamera>>,
    overlays: Query<&Node, With<CrossbarOverlay>>,
    mut blurs: Query<(&mut Node, Has<CachedBlur>), (Or<(With<LiveBlur>, With<CachedBlur>)>, Without<CrossbarOverlay>)>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut images: ResMut<Assets<Image>>,
) {
    let Some(mut crossbar) = crossbar else { return };
    let Some(cache) = crossbar.cache.as_mut() else { return };
    let held = overlays.iter().any(|n| n.display != Display::None) && world.iter().all(|c| !c.is_active);
    cache.state = blur_state(held, cache.state);
    if cache.state == 1 {
        let size = window.physical_size().max(UVec2::ONE);
        if images.get(&cache.image).is_some_and(|i| i.size() != size) {
            if let Some(image) = images.get_mut(&cache.image) {
                image.resize(bevy::render::render_resource::Extent3d { width: size.x, height: size.y, depth_or_array_layers: 1 });
            }
        }
    }
    if let Ok(mut camera) = cameras.get_mut(cache.camera) {
        if camera.is_active != (cache.state == 1) {
            camera.is_active = cache.state == 1;
        }
    }
    for (mut node, cached) in &mut blurs {
        let display = if cached == (cache.state == 2) { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
    }
}
/// Live (0) until the world is held, then one frame caching (1), then shown from the cache (2) until it draws again.
fn blur_state(held: bool, state: u8) -> u8 {
    match (held, state) {
        (false, _) => 0,
        (true, 0) => 1,
        (true, _) => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core_menu_runtime::tests::raster_draws;

    #[test]
    fn the_held_blur_is_cached_once_then_shown() {
        let states: Vec<_> = [false, true, true, true, false, true].iter().scan(0, |s, &held| {
            *s = blur_state(held, *s);
            Some(*s)
        }).collect();
        assert_eq!(states, [0, 1, 2, 2, 0, 1]);
    }

    /// Online safety Q13: a match's Lua shutting the pause menu can't trap a player online (Quit Session stays).
    #[test]
    fn a_mode_never_shuts_the_start_menu_online() {
        assert!(start_blocked_by_mode(true, false));
        assert!(!start_blocked_by_mode(true, true) && !start_blocked_by_mode(false, false));
    }

    #[test]
    fn online_row_opens_the_online_challenge_map_when_available() {
        let menu = crate::graphics_menu::test_menu();
        let cats = categories(&menu, &[], true, false, false, true, false);
        let online = &cats[1];
        assert_eq!(online.items[0].name, "ID_CROSSBAR_ONLINE_CHALLENGE_MAP");
        assert_eq!(online.items[0].help, "ID_CROSSBAR_ONLINE_CHALLENGE_MAP_HELPER_TEXT");
        assert_eq!(online.items[0].action, Action::OnlineChallengeMap);
        // Not retail (user 10-25): the server browser right under it, within the movie's 8 rows.
        assert_eq!((online.items[1].name.as_str(), &online.items[1].action), (crate::server_browser::TITLE, &Action::Online(OnlineAction::ServerBrowser)));
        assert!(online.items.len() <= 8);
        let without = categories(&menu, &[], true, false, false, false, false);
        assert_eq!(without[1].items[0].action, Action::Panel { row: 0, multiplayer: true, perform: false }, "no online.json: classic panel");
    }

    /// Bug 10-17 #23: the Skate 1 map's Learn tab has "Skate 1 Tutorial" right above its Start Over, both or neither.
    #[test]
    fn the_skate1_tutorial_item_shows_with_start_over() {
        let menu = crate::graphics_menu::test_menu();
        let learn = |s1: bool| {
            let cats = categories(&menu, &[], true, false, s1, false, false);
            let learn = cats.into_iter().find(|c| c.name == "ID_CROSSBAR_LEARN").unwrap();
            learn.items.into_iter().map(|i| i.action).filter(|a| matches!(a, Action::Skate1Tutorial | Action::Skate1StartOver)).collect::<Vec<_>>()
        };
        assert_eq!(learn(true), [Action::Skate1Tutorial, Action::Skate1StartOver]);
        assert!(learn(false).is_empty());
    }

    fn test_crossbar() -> Crossbar {
        let world = World::new();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let mut commands = Commands::new(&mut queue, &world);
        let canvas = Canvas::new(&mut commands, &mut Assets::default(), &mut Assets::default(), &Window::default(), PathBuf::new(), 1, 0);
        Crossbar {
            booted: None,
            canvas,
            runtime: None,
            shown: true,
            closing: None,
            accumulated: 0.,
            opened: 0.,
            teleports: Vec::new(),
        watchable: Vec::new(),
        marker_list: false,
            settings: false,
            search: None,
            voice: false,
            capture: None,
            blur: None,
            cache: None,
            sounds: Vec::new(),
            last: None,
            progression: crate::core_menu_runtime::NO_PROGRESSION,
        }
    }
    #[test]
    fn crossbar_reopens_where_it_was_in_the_same_mode() {
        let last = (MAIN_TITLE.to_string(), (4, 2));
        assert_eq!(reopen_at(Some(&last), MAIN_TITLE), (4, 2), "same mode: last menu and row");
        assert_eq!(reopen_at(Some(&last), ONLINE_TITLE), (1, 0), "online freeskate (mode 4) opens on Multiplayer");
        assert_eq!(reopen_at(None, MAIN_TITLE), (0, 0));
        assert_eq!(reopen_at(Some(&last), "#Party Play"), (0, 0));
    }
    #[test]
    fn a_pick_that_does_not_travel_gives_back_the_world_and_camera() {
        // C1: an Online Challenge Map pick without travel closes the crossbar (begin_close); a runtime
        // dropped outright must end the same way. Either way menu.crossbar (camera freeze) and the pause end.
        let mut menu = crate::graphics_menu::test_menu();
        let mut time = Time::<Virtual>::default();
        let mut crossbar = test_crossbar();
        for dropped in [false, true] {
            (menu.open, menu.crossbar) = (true, true);
            time.pause();
            if dropped {
                crossbar.runtime = None;
            } else {
                begin_close(&mut crossbar);
            }
            close_step(&mut crossbar, &mut menu, &mut time, 0.);
            assert!(!menu.crossbar && !menu.open && !time.is_paused(), "dropped: {dropped}");
        }
    }

    #[test]
    fn a_direction_pressed_during_the_intro_is_kept() {
        let mut held = None;
        buffer_intro_nav(&mut held, 0x1000);
        assert_eq!(held, None, "A is not kept");
        buffer_intro_nav(&mut held, 2);
        buffer_intro_nav(&mut held, 0);
        assert_eq!(held, Some(Key::Down), "the last direction stays until played");
        buffer_intro_nav(&mut held, 8);
        assert_eq!(held, Some(Key::Right));
    }

    /// perf 10-18: closing gives the world back at once; the outro's end then leaves alone what took the menu since.
    /// SKATE3_ASSET_ROOT = the install's assets (skipped without).
    #[test]
    fn closing_gives_the_world_back_before_the_outro_ends() {
        let Some(assets) = std::env::var_os("SKATE3_ASSET_ROOT").map(PathBuf::from) else { return };
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(manifest(&assets)).unwrap()).unwrap();
        let mut crossbar = test_crossbar();
        crossbar.runtime = Some(Runtime::load(&json, crate::core_menu_runtime::boot_probe(Action::Resume), String::new()).unwrap());
        let (mut menu, mut time) = (crate::graphics_menu::test_menu(), Time::<Virtual>::default());
        (menu.open, menu.crossbar) = (true, true);
        time.pause();
        assert!(!resumes_early(&crossbar, &menu), "open, not closing");
        begin_close(&mut crossbar);
        assert!(resumes_early(&crossbar, &menu));
        give_back(&mut menu, &mut time);
        // A map load takes the menu during the outro.
        menu.open = true;
        time.pause();
        close_step(&mut crossbar, &mut menu, &mut time, CLOSE_TIMEOUT);
        assert!(crossbar.runtime.is_none() && menu.open && time.is_paused());
    }

    fn names(c: &Category<Action>) -> Vec<&str> {
        c.items.iter().map(|i| i.name.as_str()).collect()
    }
    #[test]
    fn online_freeskate_tables_match_retail() {
        let menu = |host, steam, private, challenge| OnlineMenu { host, steam, private, challenge, no_proposals: false, collisions: false, listed: true, can_start: false, end_ots: false, activities: Vec::new(), active: false, spectate: Vec::new(), edit: None };
        let (guest, title) = online_categories(&menu(false, true, false, None), true);
        assert_eq!(title, ONLINE_TITLE);
        assert_eq!(guest.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
                   ["ID_CROSSBAR_SINGLE_PLAYER", "#Multiplayer", "ID_CROSSBAR_CREATE", "ID_CROSSBAR_LEARN", "ID_CROSSBAR_OPTIONS"]);
        assert_eq!(names(&guest[0]), ["ID_CROSSBAR_PROFILE_CAREER"]);
        // Not retail (user 10-25): the server browser under Online Challenge Map.
        assert_eq!(names(&guest[1]), ["ID_CROSSBAR_ONLINE_CHALLENGE_MAP", crate::server_browser::TITLE, "ID_ONLINE_HUB_PLAYER_LIST", "#Spectate", "ID_CROSSBAR_CONTACTS", "ID_ONLINE_HUB_VIEW_INVITES", "ID_CROSSBAR_QUIT_SESSION"]);
        assert_eq!(guest[1].items[1].action, Action::Online(OnlineAction::ServerBrowser));
        assert!(!guest[1].items[3].enabled, "alone in the session: no one to spectate");
        let (others, _) = online_categories(&OnlineMenu { spectate: vec!["Sam".into(), "Kai".into()], ..menu(false, true, false, None) }, true);
        assert_eq!((others[1].items[3].enabled, &others[1].items[3].choices[..]), (true, &["#Sam".to_string(), "#Kai".to_string()][..]));
        assert_eq!(names(&guest[2]), ["ID_START_HUB_SKATE_REPLAY_EDITOR", "#Edit Skater", "#Custom Models"], "the Replay Editor (retail's label), then the extras");
        assert!(guest[2].items[1].enabled && guest[2].items[1].action == Action::EditSkater);
        let (during, _) = online_categories(&OnlineMenu { edit: edit_blocked(true, false), ..menu(false, true, false, None) }, true);
        assert!(!during[2].items[1].enabled, "greyed while a match is under way");
        assert!(edit_blocked(false, true).is_some() && edit_blocked(false, false).is_none(), "and while a custom model is worn");
        assert_eq!((guest[2].items[0].enabled, &guest[2].items[0].action), (true, &Action::Replay));
        assert!(!guest[2].items[2].enabled, "no model swaps in a session");
        assert_eq!(names(&guest[3]), ["ID_START_HUB_TRICK_GUIDE", "ID_CROSSBAR_ON_SCREEN_HELP"]);
        assert_eq!(names(&guest[4]), ["ID_START_HUB_GAME_SETTINGS", "ID_CROSSBAR_MUSIC_PLAYER", "#Sign out of Steam", "#Quit"]);
        let (host, _) = online_categories(&menu(true, true, false, None), true);
        assert_eq!(names(&host[1]), ["ID_CROSSBAR_ONLINE_CHALLENGE_MAP", crate::server_browser::TITLE, "ID_ONLINE_HUB_HOST_OPTIONS", "ID_ONLINE_HUB_PLAYER_LIST", "#Spectate", "ID_CROSSBAR_CONTACTS", "ID_ONLINE_HUB_VIEW_INVITES", "ID_CROSSBAR_QUIT_SESSION"]);
        let host_actions = &host[1].items[2];
        // Proposals are allowed until the host disables them.
        // ... and players pass through each other until the host turns collisions on (user 10-25).
        // ... and (user 10-25) the session is in the server browser until the host hides it.
        assert_eq!(host_actions.choices, ["ID_CROSSBAR_DENY_PROPOSALS", "ID_ONLINE_HUB_MAKE_PRIVATE_SLOT", "ID_CROSSBAR_SESSION_DIFFICULTY",
            "#Session Weather and Time", "#Session Physics", COLLISIONS_OFF, UNLISTED, SERVER_DETAILS]);
        assert!(host_actions.choices.len() <= crate::core_menu_runtime::MAX_CHOICES, "the picker lists 8 at most");
        assert!(host_actions.enabled && host_actions.disabled_choices.is_empty());
        let (private, _) = online_categories(&OnlineMenu { no_proposals: true, collisions: true, listed: false, ..menu(true, true, true, None) }, true);
        assert_eq!(private[1].items[2].choices[..2], ["ID_CROSSBAR_ALLOW_PROPOSALS", "ID_ONLINE_HUB_MAKE_PUBLIC_SLOT"]);
        assert_eq!(private[1].items[2].choices[5..7], [COLLISIONS_ON, LISTED]);
        let (direct, _) = online_categories(&menu(true, false, false, None), true);
        assert_eq!(direct[1].items[2].disabled_choices, [1, 6, 7], "ruling 4: privacy (and the server browser) are Steam-only");
        assert_eq!(host[1].items[5].choices, ["ID_CROSSBAR_TEAMMATES", "ID_CROSSBAR_FRIENDS", "ID_CROSSBAR_RECENT_PLAYERS", "ID_CROSSBAR_INVITE_FRIEND"]);
        assert!(!host[1].items[6].enabled, "View Invites greyed");
        for page in [&guest, &host] { for c in page.iter() { assert!(!c.items.is_empty() && c.items.len() <= 8, "{}", c.name); } }
    }
    /// Online Challenge mode (vtable 0x82303da4, selector 0x8261ff58, no active challenge), PS3 text.
    #[test]
    fn online_challenge_tables_match_retail() {
        let menu = |host, team| OnlineMenu { host, steam: true, private: false, challenge: Some(team), no_proposals: false, collisions: false, listed: true, can_start: false, end_ots: false, activities: Vec::new(), active: false, spectate: Vec::new(), edit: None };
        let (team_host, title) = online_categories(&menu(true, true), true);
        assert_eq!(title, "ID_CROSSBAR_ONLINE_TEAM_CHALLENGE_MODE_TITLE");
        // Retail's three tabs, and (user 10-25) Create with Edit Skater after Multiplayer, greyed while a match runs.
        assert_eq!(team_host.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["#Multiplayer", "ID_CROSSBAR_CREATE", "ID_CROSSBAR_LEARN", "ID_CROSSBAR_OPTIONS"]);
        assert_eq!(names(&team_host[0]), ["ID_ONLINE_HUB_START_CHALLENGE", "ID_CROSSBAR_ONLINE_CHALLENGE_MAP", "ID_ONLINE_HUB_PLAYER_LIST",
            "ID_ONLINE_HUB_SWITCH_TEAM", "ID_ONLINE_HUB_HOST_OPTIONS", "ID_CROSSBAR_CONTACTS", "ID_ONLINE_HUB_VIEW_INVITES", "ID_CROSSBAR_QUIT_SESSION"]);
        assert_eq!(names(&team_host[1]), ["#Edit Skater"]);
        assert!(team_host[1].items[0].enabled && team_host[1].items[0].action == Action::EditSkater);
        let (between, _) = online_categories(&OnlineMenu { edit: edit_blocked(true, false), ..menu(false, false) }, true);
        assert!(!between[1].items[0].enabled, "greyed while a match is under way");
        assert_eq!(names(&team_host[2]), ["ID_START_HUB_TRICK_GUIDE", "ID_CROSSBAR_ON_SCREEN_HELP"]);
        assert_eq!(names(&team_host[3]), ["ID_START_HUB_GAME_SETTINGS", "#Sign out of Steam", "#Quit"]);
        let (host, title) = online_categories(&menu(true, false), true);
        assert_eq!(title, "ID_CROSSBAR_ONLINE_CHALLENGE_MODE_TITLE");
        // Not retail (user 10-25): the server browser under Online Challenge Map where the 8 rows leave room (not the team host's).
        assert_eq!(names(&host[0]), ["ID_ONLINE_HUB_START_CHALLENGE", "ID_CROSSBAR_ONLINE_CHALLENGE_MAP", crate::server_browser::TITLE,
            "ID_ONLINE_HUB_PLAYER_LIST", "ID_ONLINE_HUB_HOST_OPTIONS", "ID_CROSSBAR_CONTACTS", "ID_ONLINE_HUB_VIEW_INVITES", "ID_CROSSBAR_QUIT_SESSION"]);
        assert!(!host[0].items[0].enabled, "no mode plays this row");
        let (ready, _) = online_categories(&OnlineMenu { can_start: true, ..menu(true, false) }, true);
        assert!(ready[0].items[0].enabled && ready[0].items[0].action == Action::Online(OnlineAction::StartChallenge));
        let (team_guest, _) = online_categories(&menu(false, true), true);
        assert_eq!(names(&team_guest[0]), ["ID_CROSSBAR_ONLINE_CHALLENGE_MAP", crate::server_browser::TITLE, "ID_ONLINE_HUB_PLAYER_LIST",
            "ID_ONLINE_HUB_SWITCH_TEAM", "ID_CROSSBAR_CONTACTS", "ID_ONLINE_HUB_VIEW_INVITES", "ID_CROSSBAR_QUIT_SESSION"]);
        let (guest, _) = online_categories(&menu(false, false), true);
        assert_eq!(names(&guest[0]), ["ID_CROSSBAR_ONLINE_CHALLENGE_MAP", crate::server_browser::TITLE, "ID_ONLINE_HUB_PLAYER_LIST",
            "ID_CROSSBAR_CONTACTS", "ID_ONLINE_HUB_VIEW_INVITES", "ID_CROSSBAR_QUIT_SESSION"]);
        for page in [&team_host, &host, &team_guest, &guest] { for c in page.iter() { assert!(!c.items.is_empty() && c.items.len() <= 8, "{}", c.name); } }
        // An Own the Lot lobby (T52): Select Activity in Start Challenge's place.
        let (lot, _) = online_categories(&OnlineMenu { can_start: true, activities: vec!["#a".into()], ..menu(true, false) }, true);
        assert_eq!(names(&lot[0])[0], crate::skate3_otl::SELECT_ACTIVITY);
        // Case 3, an active challenge: the OTS host's End item first, then Player List, Contacts, Quit Session.
        let (ots, _) = online_categories(&OnlineMenu { active: true, end_ots: true, ..menu(true, false) }, true);
        assert_eq!(names(&ots[0]), ["ID_CROSSBAR_OTS_RETURN_TO_LOBBY", "ID_ONLINE_HUB_PLAYER_LIST", "ID_CROSSBAR_CONTACTS", "ID_CROSSBAR_QUIT_SESSION"]);
        let (running, _) = online_categories(&OnlineMenu { active: true, ..menu(false, false) }, true);
        assert_eq!(names(&running[0]), ["ID_ONLINE_HUB_PLAYER_LIST", "ID_CROSSBAR_CONTACTS", "ID_CROSSBAR_QUIT_SESSION"]);
    }
    #[test]
    fn offline_multiplayer_tab_gains_the_friends_row() {
        let menu = crate::graphics_menu::test_menu();
        let page = categories(&menu, &[], true, false, false, true, false);
        let row = page[1].items.iter().position(|i| i.name == "ID_CROSSBAR_FRIENDS").expect("Friends row");
        assert_eq!(page[1].items[row - 1].name, "ID_CROSSBAR_CONTACTS", "engine extra right after Contacts");
        assert_eq!(page[1].items[row].action, Action::Online(crate::online_menu::OnlineAction::Friends));
    }

    /// Create > Replay Editor and skate.Reel are live (replay/hud.rs): the editor row and My Replays.
    #[test]
    fn pause_menu_replay_item_enabled() {
        let menu = crate::graphics_menu::test_menu();
        let page = categories(&menu, &[], true, false, false, false, false);
        let create = page.iter().find(|c| c.name == "ID_CROSSBAR_CREATE").expect("Create");
        let row = |name: &str| create.items.iter().find(|i| i.name == name).expect(name);
        let replay = row("ID_START_HUB_SKATE_REPLAY_EDITOR");
        assert_eq!((replay.enabled, &replay.action, replay.icon), (true, &Action::Replay, "replay"));
        let reel = row("ID_SKATE_REEL_GENERAL_TITLE");
        assert_eq!((reel.enabled, &reel.action), (true, &Action::SkateReel));
    }

    #[test]
    fn party_play_row_needs_the_party_pack() {
        let menu = crate::graphics_menu::test_menu();
        let row = |installed: bool| {
            let page = with_party(categories(&menu, &[], true, false, false, true, false), installed);
            page[0].items.last().cloned().expect("Main rows")
        };
        let off = row(false);
        assert_eq!((off.name.as_str(), off.enabled), (PARTY_PLAY_ROW, false), "the last Main row, greyed");
        let on = row(true);
        assert_eq!((on.enabled, on.action, on.icon), (true, Action::PartyPlay, "freeskate"));
    }

    #[test]
    fn party_play_crossbar_has_retails_rows_all_enabled_but_unported_ones() {
        let (page, title) = party_categories(false);
        assert_eq!(title, "#Party Play");
        let icons: Vec<&str> = page.iter().map(|c| c.icon).collect();
        assert_eq!(icons, ["singleplayer", "create", "learn", "options"]);
        let main: Vec<(&Action, bool)> = page[0].items.iter().map(|i| (&i.action, i.enabled)).collect();
        assert_eq!(main, [(&Action::RestartChallenge, true), (&Action::ExitPartyPlay, true)], "D8: Restart always enabled");
        assert_eq!((&page[1].items[0].action, page[1].items[0].enabled), (&Action::Replay, true), "7c: the Replay Editor too");
        assert_eq!(page[3].items[0].action, Action::GameSettings);
        assert_eq!(page[3].items[1].action, Action::SignInOut);
    }

    /// Audit 10-27 #8: the pause crossbar's Replay Editor row is greyed until a replay is recorded (0x8261E208 calls
    /// 0x8250b8f8: MIN_BACK_FRAMES, Back's own gate); nothing else changes.
    #[test]
    fn the_replay_editor_row_waits_for_a_recording() {
        let menu = crate::graphics_menu::test_menu();
        let rows = |ready: Option<bool>| {
            let page = categories(&menu, &[], true, false, false, false, false);
            let page = match ready {
                Some(ready) => with_replay(page, ready),
                None => page,
            };
            page.into_iter().flat_map(|c| c.items).map(|i| (i.action, i.enabled)).collect::<Vec<_>>()
        };
        let (built, ready, short) = (rows(None), rows(Some(true)), rows(Some(false)));
        assert_eq!(ready, built, "recorded: as built");
        assert!(built.contains(&(Action::Replay, true)), "the page has the row, enabled as built");
        for ((action, was), (_, now)) in built.iter().zip(&short) {
            assert_eq!(*now, *was && *action != Action::Replay, "{action:?}");
        }
    }

    /// Not retail (user 10-27): a Game Settings pick that turns Performance mode on or off waits for the restart
    /// question; any other pick goes in at once.
    #[test]
    fn a_performance_mode_pick_asks_to_restart_first() {
        use crate::online_menu::OnlineRequest;
        let mut menu = crate::graphics_menu::test_menu();
        let mut requests = Vec::new();
        let revision = menu.revision;
        pick_setting(&mut menu, &mut requests, Setting::Preset, 1);
        assert_eq!(requests, [OnlineRequest::Restart(Setting::Preset, 1)]);
        assert_eq!((menu.choices(Setting::Preset).1, menu.revision), (2, revision), "still Default until the answer");
        pick_setting(&mut menu, &mut requests, Setting::FpsLimit, 1);
        assert_eq!((requests.len(), menu.revision), (1, revision + 1), "anything else goes in at once, unasked");
    }

    #[test]
    fn on_screen_help_is_a_picker_once_installed() {
        let menu = crate::graphics_menu::test_menu();
        let row = |help: Option<&crate::hud_meters::OnScreenHelp>| {
            let page = with_help(categories(&menu, &[], true, false, false, true, false), help);
            page.into_iter().flat_map(|c| c.items).find(|i| i.name == ON_SCREEN_HELP[0]).expect("Learn > On Screen Help")
        };
        assert!(!row(None).enabled);
        assert!(!row(Some(&Default::default())).enabled, "greyed without the prepared movies");
        let installed = crate::hud_meters::OnScreenHelp { available: true, ..Default::default() };
        let item = row(Some(&installed));
        assert!(item.enabled && item.action == Action::OnScreenHelp);
        assert_eq!(item.choices, ["ID_CROSSBAR_TRICK_ANALYZER_ON", "ID_CROSSBAR_MANUAL_METER_ON"]);
    }

    #[test]
    fn the_free_play_row_is_live_and_absent_in_a_challenge() {
        let menu = crate::graphics_menu::test_menu();
        let main = |career| {
            let (page, title) = with_free_play(categories(&menu, &[], true, false, false, true, false), career);
            (page.into_iter().next().unwrap().items, title)
        };
        let (off, _) = main(None);
        assert!(off.iter().any(|i| i.name == "ID_CROSSBAR_SOLO_FREESKATE" && !i.enabled), "greyed with the career off");
        let (live, title) = main(Some((GameMode::SinglePlayer, false)));
        let at = live.iter().position(|i| i.action == Action::FreePlay).unwrap();
        assert_eq!(live[at - 1].name, "ID_CROSSBAR_SKATE_FEED", "after Skate Feed");
        assert_eq!(title, MAIN_TITLE);
        let (busy, _) = main(Some((GameMode::SinglePlayer, true)));
        assert!(busy.iter().all(|i| i.name != "ID_CROSSBAR_SOLO_FREESKATE"));
    }

    #[test]
    fn the_free_play_crossbar_trades_rows_and_title() {
        let menu = crate::graphics_menu::test_menu();
        let (page, title) = with_free_play(categories(&menu, &[], true, false, false, true, false), Some((GameMode::ProfileFreeSkate, false)));
        assert_eq!(title, FREE_PLAY_TITLE);
        assert!(page.iter().all(|c| c.name != "#Multiplayer"));
        let rows = &page[0].items;
        assert!(rows.iter().any(|i| i.action == Action::ResumeCareer));
        for gone in ["ID_CROSSBAR_MY_CAREER_TEAM", "ID_CROSSBAR_SKATE_FEED", PARTY_PLAY_ROW, "ID_CROSSBAR_SOLO_FREESKATE"] {
            assert!(rows.iter().all(|i| i.name != gone), "{gone}");
        }
    }

    #[test]
    fn unlock_everything_follows_play_mode() {
        let menu = crate::graphics_menu::test_menu();
        let options = |page: Vec<Category<Action>>| page.into_iter().find(|c| c.name == "ID_CROSSBAR_OPTIONS").unwrap().items;
        let rows = options(with_unlocks(categories(&menu, &[], true, true, true, true, false), Some(true)));
        let at = rows.iter().position(|i| i.action == Action::Difficulty).unwrap();
        assert_eq!(rows[at + 1].action, Action::UnlockEverything);
        assert_eq!((rows[at + 1].chosen, rows[at + 1].choices.as_slice()), (1, ["#Off".to_string(), "#On".to_string()].as_slice()));
        assert!(rows.len() <= 8, "the movie draws 8");
        let off = options(with_unlocks(categories(&menu, &[], true, true, true, true, false), None));
        assert!(off.iter().all(|i| i.action != Action::UnlockEverything), "no row with the career off");
    }

    /// The main page keeps the movie's 8 rows; Game Settings' pages scroll past them (user 10-26).
    #[test]
    fn every_category_fits_the_movie() {
        let mut menu = crate::graphics_menu::test_menu();
        for category in categories(&menu, &[], true, true, true, true, false) {
            assert!(category.items.len() <= 8, "{} has {} rows; the movie draws 8", category.name, category.items.len());
        }
        // Difficulty: on the Options page offline, in Game Settings only in a session.
        let has_difficulty = |menu: &Menu| settings_categories(menu)[0].items.iter().any(|i| i.action == Action::Difficulty);
        assert!(!has_difficulty(&menu));
        menu.set_session_world(Some([0, 0]));
        assert!(has_difficulty(&menu));
    }
    /// Skate 2 physics (user 10-20, not tested in play): Gameplay's eighth row offline. Online the host's Difficulty row
    /// takes the free slot. User 10-28: online too, the host's (spec 2026-10-20 P4). User 11-02: Skip Intro after it.
    #[test]
    fn the_physics_row_is_gameplays_eighth_offline() {
        use crate::skate2_physics::settings::Physics;
        let gameplay = |menu: &Menu| settings_categories(menu).swap_remove(0).items;
        let mut menu = crate::graphics_menu::test_menu();
        let rows = gameplay(&menu);
        assert_eq!(rows.len(), 9);
        let last = &rows[7];
        assert_eq!(last.name, "#Physics: Skate 3");
        assert_eq!(last.choices, ["Skate 3", "Skate 2"]);
        assert_eq!(last.action, Action::Physics);
        assert_eq!((rows[8].name.as_str(), &rows[8].action), ("#Skip Intro: Off", &Action::Setting(Setting::SkipIntro)));
        menu.choose(Setting::SkipIntro, 1);
        assert_eq!(gameplay(&menu)[8].name, "#Skip Intro: On");
        menu.choose(Setting::SkipIntro, 0);
        // The pick is saved and shown.
        let dir = std::env::temp_dir().join(format!("sk8-physics-row-{}", std::process::id()));
        let root = dir.join("assets");
        std::fs::create_dir_all(&root).unwrap();
        menu.set_physics(Physics::Skate2, &root).unwrap();
        assert_eq!(menu.status(), "Physics saved");
        assert_eq!(gameplay(&menu)[7].name, "#Physics: Skate 2");
        assert_eq!(crate::skate2_physics::settings::Gameplay::load(&root).physics, Physics::Skate2);
        let _ = std::fs::remove_dir_all(&dir);
        // User 10-28: in a session the host's Difficulty row, the seven retail ones, the host's Physics and Skip Intro
        // (the page scrolls); another guest sees the host's greyed.
        menu.set_session_world(Some([0, 0]));
        menu.on_nsv = true;
        let rows = gameplay(&menu);
        assert_eq!(rows.len(), 10);
        assert_eq!((&rows[0].action, &rows[8].action), (&Action::Difficulty, &Action::Physics));
        assert!(rows[8].enabled, "the host's");
        assert_eq!(rows[8].help, "#This session's physics: everyone in it gets yours");
        menu.guest = true;
        let row = gameplay(&menu).swap_remove(8);
        assert!(!row.enabled);
        assert_eq!(row.help, "#The host sets the physics in an online session");
        menu.cohost = true;
        assert!(!gameplay(&menu)[8].enabled, "a co-host sets the world, not the physics");
    }

    /// Skate 2 physics runs only on New San Vanelona in this build: the row greys elsewhere.
    #[test]
    fn the_physics_row_is_greyed_off_nsv() {
        let physics = |menu: &Menu| settings_categories(menu).swap_remove(0).items.into_iter().find(|i| i.action == Action::Physics).unwrap();
        let mut menu = crate::graphics_menu::test_menu();
        assert!(!menu.on_nsv);
        let row = physics(&menu);
        assert!(!row.enabled);
        assert_eq!(row.help, "#Skate 2 physics runs only on New San Vanelona in this build");
        menu.on_nsv = true;
        assert!(physics(&menu).enabled);
    }

    /// The Extras list gains the engine row after the retail seven, and its page is four D-pad slot pickers.
    #[test]
    fn the_gestures_page_has_four_slots() {
        use crate::skate2_physics::settings::GESTURES;
        let mut menu = crate::graphics_menu::test_menu();
        // The Options crossbar still opens Extras and Game Settings, and lists no gesture row of its own.
        let main = categories(&menu, &[], true, true, true, true, false);
        let options = main.last().unwrap();
        assert_eq!(options.icon, "options");
        assert!(options.items.iter().any(|i| i.action == Action::Retail(crate::retail_ui::Panel::Extras)));
        assert!(options.items.iter().any(|i| i.action == Action::GameSettings));
        assert!(main.iter().flat_map(|c| &c.items).all(|i| i.action != Action::Skate2Gestures));
        // Extras: the seven retail rows, then ours (the story plan's "#Start a New Career" comes after it).
        let extras = crate::retail_ui::EXTRAS;
        assert_eq!(extras.len(), 9);
        assert_eq!(extras[..7], ["ID_EXTRA_CHEAT_CODE", "ID_EXTRA_EMERICA_SHOE_CODE", "ID_EXTRA_ETNIES_SHOE_CODE", "ID_EXTRA_PRE_SELL_CODE", "ID_EXTRA_CREDITS", "ID_EXTRA_LICENSES", "ID_SKATE_REEL_VIDEOS"]);
        assert_eq!(extras[7], "#Skate 2 Gestures");
        assert_eq!(extras[8], "#Start a New Career");
        assert_eq!(crate::retail_ui::GESTURES_ROW, 7);
        // The page: one tab, four pickers in the port's slot order (edit_skater_frontend.rs 153-160).
        let page = gesture_categories(&menu);
        assert_eq!(page.len(), 1);
        let rows = &page[0].items;
        assert_eq!(rows.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), [
            "#<B:DPadUp> Up: sky",
            "#<B:DPadDown> Down: what",
            "#<B:DPadLeft> Left: snap",
            "#<B:DPadRight> Right: thumbdown",
        ]);
        assert_eq!(rows.iter().map(|i| i.action.clone()).collect::<Vec<_>>(), (0..4).map(Action::Skate2Gesture).collect::<Vec<_>>());
        for (slot, row) in rows.iter().enumerate() {
            assert!(row.enabled);
            assert_eq!(row.choices, GESTURES);
            assert_eq!(row.chosen, menu.skate2_gestures()[slot] as usize);
        }
        // A pick is saved, shown, and survives a reload; a gesture the sim would refuse is not taken.
        let dir = std::env::temp_dir().join(format!("sk8-gesture-page-{}", std::process::id()));
        let root = dir.join("assets");
        std::fs::create_dir_all(&root).unwrap();
        menu.set_skate2_gesture(1, 34, &root).unwrap();
        assert_eq!(menu.status(), "Gestures saved");
        let page = gesture_categories(&menu);
        assert_eq!((page[0].items[1].name.as_str(), page[0].items[1].chosen), ("#<B:DPadDown> Down: axe", 34));
        assert_eq!(crate::skate2_physics::settings::Gameplay::load(&root).skate2_gestures, [0, 34, 2, 3]);
        assert!(menu.set_skate2_gesture(1, 35, &root).is_err());
        assert!(menu.set_skate2_gesture(4, 0, &root).is_err());
        assert_eq!(menu.skate2_gestures(), [0, 34, 2, 3]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The DLSS row: only where this build and GPU run it (dlss.rs sets dlss_supported; builds without the feature
    /// never do). While a mode is on, Render Scale, Anti-Aliasing and AO grey out.
    #[test]
    fn the_dlss_row_needs_a_supporting_gpu() {
        let rows = |menu: &Menu| settings_categories(menu).into_iter().flat_map(|c| c.items).collect::<Vec<_>>();
        let dlss_row = |menu: &Menu| rows(menu).into_iter().find(|i| i.action == Action::Setting(Setting::Dlss));
        let mut menu = crate::graphics_menu::test_menu();
        // The Physics row greys off New San Vanelona (user 10-20); this test is about DLSS's greyed rows only.
        menu.on_nsv = true;
        assert!(dlss_row(&menu).is_none());
        menu.dlss_supported = true;
        assert_eq!(dlss_row(&menu).unwrap().name, "#DLSS: Off");
        assert!(rows(&menu).iter().all(|i| i.enabled));
        menu.choose(Setting::Dlss, 2);
        assert_eq!(dlss_row(&menu).unwrap().name, "#DLSS: Quality");
        let greyed: Vec<String> = rows(&menu).into_iter().filter(|i| !i.enabled).map(|i| i.name).collect();
        assert_eq!(greyed, ["#Render Scale: DLSS", "#Anti-Aliasing: DLSS", "#Ambient Occlusion: Off"]);
    }

    /// User 10-26: Settings search finds rows by the words of their names and help lines (never their values), names
    /// first, retail rows by their text, and nothing for no words.
    #[test]
    fn settings_search_finds_rows_by_their_words() {
        let menu = crate::graphics_menu::test_menu();
        let say = |text: &str| match text {
            "ID_GAMESETTINGS_VIDEO_CALIBRATION" => "Video Calibration".to_owned(),
            text => text.trim_start_matches('#').to_owned(),
        };
        let found = search(&menu, "fps", say);
        assert_eq!(found.first(), Some(&Action::Setting(Setting::FpsCounter)), "the row named FPS first: {found:?}");
        assert!(found.contains(&Action::Setting(Setting::Preset)), "the Preset's help names FPS: {found:?}");
        assert_eq!(search(&menu, "FRAME  rate", say), [Action::Setting(Setting::FpsLimit)], "any case and spacing, every word");
        assert_eq!(search(&menu, "calibration", say), [Action::Retail(crate::retail_ui::Panel::VideoCalibration)]);
        assert!(search(&menu, "unlimited", say).is_empty(), "Frame Rate Limit's value is not searched");
        assert!(search(&menu, "  ", say).is_empty());
    }

    /// The search page shows the rows it found as they stand now, in the found order, through a pick; nothing found is
    /// one greyed line.
    #[test]
    fn the_search_page_shows_the_rows_it_found() {
        let mut menu = crate::graphics_menu::test_menu();
        let found = [Action::Setting(Setting::FpsCounter), Action::Setting(Setting::FpsLimit)];
        let names = |page: &[Category<Action>]| page[0].items.iter().map(|i| i.name.clone()).collect::<Vec<_>>();
        let page = search_categories(&menu, "fps", &found);
        assert_eq!((page.len(), page[0].name.as_str()), (1, "#Search: fps"));
        assert_eq!(names(&page), ["#FPS Counter: On", "#Frame Rate Limit: Unlimited"]);
        menu.choose(Setting::FpsCounter, 0);
        assert_eq!(names(&search_categories(&menu, "fps", &found)), ["#FPS Counter: Off", "#Frame Rate Limit: Unlimited"]);
        let none = search_categories(&menu, "zzz", &[]);
        assert_eq!((none[0].items.len(), none[0].items[0].enabled), (1, false));
        assert!(none[0].items[0].name.contains("zzz"), "{}", none[0].items[0].name);
    }

    /// User 10-26: the FPS Counter row sits under Frame Rate Limit on the Video page (a ninth row: the page scrolls).
    #[test]
    fn the_fps_counter_row_sits_under_frame_rate_limit() {
        let menu = crate::graphics_menu::test_menu();
        let video = settings_categories(&menu).into_iter().find(|c| c.name == "#Video").unwrap().items;
        let at = video.iter().position(|i| i.action == Action::Setting(Setting::FpsLimit)).unwrap();
        assert_eq!((video[at + 1].name.as_str(), &video[at + 1].action), ("#FPS Counter: On", &Action::Setting(Setting::FpsCounter)));
    }

    fn voice_row(menu: &Menu, setting: Setting) -> Item<Action> {
        voice_categories(menu)[0].items.iter().find(|i| i.action == Action::Setting(setting)).cloned().unwrap()
    }
    /// User 10-26: the Voice Chat page's twelve rows in the spec's order with their defaults; three sliders, a key row
    /// with no picker, every picker within the movie's eight; the pad button's help says what it stops doing.
    #[test]
    fn the_voice_page_has_the_specs_rows_in_order() {
        let mut menu = crate::graphics_menu::test_menu();
        menu.voice_devices = Some((vec!["USB Mic".into()], vec![]));
        let page = voice_categories(&menu);
        assert_eq!((page.len(), page[0].name.as_str()), (1, "#Voice Chat"));
        assert_eq!(names(&page[0]), [
            "#Voice Chat: On",
            "#Voice From: Everyone",
            "#Mic Mode: Push to Talk",
            "#Push to Talk Key: V",
            "#Push to Talk Button: None",
            "#Open Mic Sensitivity: 5",
            "#Microphone: Default (Windows)",
            "#Mic Volume: 100%",
            "#Mic Test: Off",
            "#Noise Suppression: Standard",
            "#Voice Volume: 100%",
            "#Lower Game Sound While Talking: Off",
        ]);
        let sliders: Vec<Action> = page[0].items.iter().filter(|i| i.slider).map(|i| i.action.clone()).collect();
        assert_eq!(sliders, [Setting::OpenMicSensitivity, Setting::MicVolume, Setting::VoiceVolume].map(Action::Setting));
        assert!(page[0].items[3].choices.is_empty(), "Push to Talk Key takes the next key pressed: no picker");
        assert!(page[0].items.iter().all(|i| i.choices.len() <= crate::core_menu_runtime::MAX_CHOICES));
        assert!(voice_row(&menu, Setting::NoiseSuppression).disabled_choices.is_empty(), "High runs without FMA too (DECISIONS.md)");
        menu.choose(Setting::PushToTalkButton, 4);
        let rb = voice_row(&menu, Setting::PushToTalkButton);
        assert_eq!(rb.name, "#Push to Talk Button: RB");
        assert!(rb.help.contains("darkslides"), "what RB stops doing: {}", rb.help);
    }
    /// Settings > Audio: Voice Chat under Chat From opens the page, Output Device under the Music Player; Back from
    /// the page lands on the Voice Chat row.
    #[test]
    fn the_audio_tab_opens_the_voice_page_and_back_returns_to_its_row() {
        let menu = crate::graphics_menu::test_menu();
        let audio = settings_categories(&menu).into_iter().find(|c| c.name == "#Audio").unwrap();
        let at = |action: &Action| audio.items.iter().position(|i| i.action == *action).unwrap();
        assert_eq!(at(&Action::VoiceChat), at(&Action::Setting(Setting::ChatFrom)) + 1, "Voice Chat under Chat From");
        let output = at(&Action::Setting(Setting::OutputDevice));
        assert_eq!(output, at(&Action::Music(crate::music_ui::Panel::Player)) + 1, "Output Device under the Music Player");
        assert_eq!((audio.items[output].name.as_str(), audio.items[output].help.as_str()), ("#Output Device: Default (Windows)", "#Applies when the game restarts"));
        let voice = &audio.items[at(&Action::VoiceChat)];
        assert!(voice.enabled && voice.choices.is_empty(), "Accept opens the page");
        assert_eq!(settings_back(&menu, true, Some(&Action::Setting(Setting::MicVolume))), Action::VoiceChat);
        assert_eq!(settings_back(&menu, true, None), Action::VoiceChat);
    }
    /// A pick on the Voice Chat page rebuilds the page it is on (Crossbar::settings_rows), not Game Settings' tabs.
    #[test]
    fn a_voice_change_keeps_the_voice_page() {
        let mut menu = crate::graphics_menu::test_menu();
        let mut crossbar = test_crossbar();
        crossbar.voice = true;
        menu.choose(Setting::NoiseSuppression, 2);
        let rows = crossbar.settings_rows(&menu);
        assert_eq!(rows.len(), 1, "still the Voice Chat page");
        assert!(names(&rows[0]).contains(&"#Noise Suppression: High"), "{:?}", names(&rows[0]));
        crossbar.voice = false;
        assert_eq!(crossbar.settings_rows(&menu).len(), 6, "Game Settings' tabs");
    }
    /// Settings search covers the Voice Chat page's rows, changed in place on the search page; Back from one lands on
    /// the Audio tab's Voice Chat row (other rows on themselves, as before).
    #[test]
    fn search_finds_voice_rows_and_back_goes_to_voice_chat() {
        let menu = crate::graphics_menu::test_menu();
        let found = search(&menu, "noise", |text| text.trim_start_matches('#').to_owned());
        assert_eq!(found.first(), Some(&Action::Setting(Setting::NoiseSuppression)), "{found:?}");
        let page = search_categories(&menu, "noise", &found);
        assert_eq!(page[0].items[0].name, "#Noise Suppression: Standard");
        assert_eq!(settings_back(&menu, false, Some(&Action::Setting(Setting::NoiseSuppression))), Action::VoiceChat);
        let fps = Action::Setting(Setting::FpsLimit);
        assert_eq!(settings_back(&menu, false, Some(&fps)), fps, "other rows: their own");
    }
    /// Push to Talk Key: after Accept, the next key (or mouse side button) is the key; Esc or the pad's B keeps the old.
    #[test]
    fn push_to_talk_key_is_set_by_the_next_key() {
        assert_eq!(captured_key(&[], &[], false), None, "waits for a press");
        assert_eq!(captured_key(&[KeyCode::KeyV], &[], false), Some(Some("KeyV".into())));
        assert_eq!(captured_key(&[KeyCode::Escape], &[], false), Some(None), "Esc cancels");
        assert_eq!(captured_key(&[], &[], true), Some(None), "so does B");
        assert_eq!(captured_key(&[], &[MouseButton::Back], false), Some(Some("Mouse4".into())));
        assert_eq!(captured_key(&[], &[MouseButton::Forward], false), Some(Some("Mouse5".into())));
        assert_eq!(captured_key(&[], &[MouseButton::Left], false), None, "a click is not a talk button");
        let mut menu = crate::graphics_menu::test_menu();
        menu.set_ptt_key("Mouse5".into());
        assert_eq!(voice_row(&menu, Setting::PushToTalkKey).name, "#Push to Talk Key: Mouse 5");
        use crate::graphics_menu::key_label;
        assert_eq!([key_label("KeyV"), key_label("Digit4"), key_label("ShiftLeft"), key_label("F5")], ["V", "4", "Shift Left", "F5"]);
    }
    #[test]
    fn open_mic_sensitivity_greys_in_push_to_talk() {
        let mut menu = crate::graphics_menu::test_menu();
        assert!(!voice_row(&menu, Setting::OpenMicSensitivity).enabled, "Push to Talk");
        menu.choose(Setting::MicMode, 1);
        assert!(voice_row(&menu, Setting::OpenMicSensitivity).enabled, "Open Mic");
    }
    #[test]
    fn no_microphone_greys_the_row() {
        let mut menu = crate::graphics_menu::test_menu();
        menu.voice_devices = Some((vec![], vec!["Speakers".into()]));
        let row = voice_row(&menu, Setting::Microphone);
        assert_eq!((row.name.as_str(), row.enabled), ("#Microphone: No microphone", false));
        menu.voice_devices = Some((vec!["USB Mic".into()], vec![]));
        let row = voice_row(&menu, Setting::Microphone);
        assert_eq!((row.name.as_str(), row.enabled, row.choices.len()), ("#Microphone: Default (Windows)", true, 2));
        assert!(row.help.contains("pick your headset by name"), "{}", row.help);
        // Audit 10-30: one that won't open says why, instead of sending nothing silently.
        menu.mic_error = Some("access denied".into());
        let row = voice_row(&menu, Setting::Microphone);
        assert!(row.enabled && row.help.contains("Privacy > Microphone"), "{}", row.help);
    }
    /// Mic Test is on only while a page shows its row: Back to Game Settings, another search or the menu closing ends it.
    #[test]
    fn mic_test_turns_off_when_the_page_closes() {
        let shown = [Action::Setting(Setting::MicTest)];
        assert!(keeps_mic_test(true, true, None), "on the Voice Chat page");
        assert!(keeps_mic_test(true, false, Some(&shown)), "on a search page that found it");
        assert!(!keeps_mic_test(true, false, None), "Back to Game Settings");
        assert!(!keeps_mic_test(true, false, Some(&[Action::Setting(Setting::FpsLimit)])), "a search without it");
        assert!(!keeps_mic_test(false, true, None), "the menu closed");
    }
    /// The meter's bar fills with the level; the Open Mic line's mark stays put and moves with the Sensitivity.
    #[test]
    fn the_mic_meter_fills_to_the_level_and_marks_the_line() {
        let right = |d: &crate::apt_scene::Draw| d.vertices.iter().map(|v| v.position[0]).fold(f32::MIN, f32::max);
        let left = |d: &crate::apt_scene::Draw| d.vertices.iter().map(|v| v.position[0]).fold(f32::MAX, f32::min);
        let (quiet, loud) = (mic_meter(-50., -38.), mic_meter(-10., -38.));
        assert_eq!(quiet.len(), 3, "frame, level, line");
        assert!(right(&loud[1]) > right(&quiet[1]), "the level fills further when louder");
        assert!(right(&mic_meter(-120., -38.)[1]) <= left(&quiet[1]) + 0.5, "silence fills nothing");
        assert_eq!(left(&quiet[2]), left(&loud[2]), "the line stays put");
        assert!(left(&mic_meter(-50., -18.)[2]) > left(&quiet[2]), "a higher Sensitivity line sits further right");
    }

    /// Visual check of the Game Settings page against the owned, prepared menu
    /// (SKATE_CORE_MENU_ROOT = private/core-menu).
    #[test]
    #[ignore = "requires the prepared original core menu"]
    fn game_settings_page_renders() {
        let root = PathBuf::from(std::env::var("SKATE_CORE_MENU_ROOT").unwrap());
        let out = PathBuf::from(std::env::var("SKATE_CORE_MENU_OUT").unwrap_or_else(|_| ".".into()));
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("runtime/core_menu.json")).unwrap()).unwrap();
        let snap = |rt: &Runtime<Action>, name: &str| {
            let draws = crate::apt_scene::draw(&rt.bindings.movie, &rt.vm, &rt.bindings.shapes).unwrap();
            raster_draws(&draws, &root, &out.join(format!("{name}.png")));
        };
        let run = |rt: &mut Runtime<Action>, frames: usize| (0..frames).for_each(|_| rt.advance().unwrap());
        let open = |categories, title: &str, focus: (usize, usize)| {
            let mut rt = Runtime::load(&json, categories, title.into()).unwrap();
            rt.focus(focus.0, focus.1);
            run(&mut rt, 30);
            assert!(rt.bindings.intro_complete);
            rt.show().unwrap();
            run(&mut rt, 40);
            rt
        };
        let mut menu = crate::graphics_menu::test_menu();
        // Back from Game Settings lands on its own row in Options.
        let main = open(categories(&menu, &[], true, false, false, true, false), MAIN_TITLE, (4, 0));
        snap(&main, "settings-0-main-options");

        let mut rt = open(settings_categories(&menu), SETTINGS_TITLE, (0, 0));
        for page in 0..6 {
            snap(&rt, &format!("settings-1-page-{page}"));
            rt.input(Key::Right).unwrap();
            run(&mut rt, 30);
        }
        // Video (wrapped back to page 0, then two right): Display Mode picker.
        rt.input(Key::Right).unwrap();
        rt.input(Key::Right).unwrap();
        run(&mut rt, 30);
        assert_eq!(rt.current_mut().unwrap().name, "#Display Mode: Windowed");
        assert_eq!(rt.input(Key::Accept).unwrap(), None, "Display Mode opens its picker");
        run(&mut rt, 20);
        snap(&rt, "settings-2-picker");
        assert_eq!(rt.input(Key::Back).unwrap(), None, "Back closes the picker only");
        run(&mut rt, 20);
        // Resolution (below Monitor): Down picks the next size and the row shows it.
        rt.input(Key::Down).unwrap();
        rt.input(Key::Down).unwrap();
        run(&mut rt, 10);
        assert!(rt.current_mut().unwrap().name.starts_with("#Resolution: "));
        rt.input(Key::Accept).unwrap();
        run(&mut rt, 20);
        rt.input(Key::Down).unwrap();
        run(&mut rt, 10);
        let Some(Outcome::Choose(Action::Setting(Setting::Resolution), index)) = rt.input(Key::Accept).unwrap() else {
            panic!("Resolution picker did not choose");
        };
        assert_eq!(index, 1, "default 1280 x 720 is index 0; Down picks 1280 x 800");
        menu.choose(Setting::Resolution, index);
        show_choice(&mut rt, index).unwrap();
        run(&mut rt, 30);
        snap(&rt, "settings-3-chosen");
        assert_eq!(rt.current_mut().unwrap().name, "#Resolution: 1280 x 800");
        // Controls (Field of View's page since the DLSS row): the slider, third row, moves in place, and Back keeps its value.
        rt.input(Key::Left).unwrap();
        run(&mut rt, 30);
        rt.input(Key::Down).unwrap();
        rt.input(Key::Down).unwrap();
        run(&mut rt, 10);
        assert_eq!(rt.current_mut().unwrap().name, "#Field of View: 67 (Default)");
        assert_eq!(rt.input(Key::Accept).unwrap(), None, "opens the slider");
        run(&mut rt, 20);
        for _ in 0..5 {
            let Some(Outcome::Slide(Action::Setting(setting), step)) = rt.input(Key::Right).unwrap() else { panic!("slider step") };
            assert!(menu.slide(setting, step));
            rt.show_slider(slider_line(&menu, setting)).unwrap();
            run(&mut rt, 5);
        }
        run(&mut rt, 20);
        snap(&rt, "settings-4-fov-slider");
        assert_eq!(rt.input(Key::Back).unwrap(), Some(Outcome::Choose(Action::Setting(Setting::FieldOfView), 0)));
        show_value(&mut rt, 0, &menu.fov_label()).unwrap();
        run(&mut rt, 30);
        snap(&rt, "settings-5-fov-set");
        assert_eq!(rt.current_mut().unwrap().name, "#Field of View: 72 (+5)");
        assert_eq!(rt.input(Key::Back).unwrap(), Some(Outcome::Close));
        // Bug 10-25 #31: a Preset rewrites the rows it covers on the open page, not only after leaving it.
        let mut rt = open(settings_categories(&menu), SETTINGS_TITLE, (4, 0));
        let rows = |c: &[Category<Action>]| c.iter().flat_map(|c| &c.items).map(|i| i.name.clone()).collect::<Vec<_>>();
        let before = rows(&rt.bindings.categories);
        assert_eq!(rt.current_mut().unwrap().action, Action::Setting(Setting::Preset));
        menu.choose(Setting::Preset, 1);
        show_setting(&mut rt, &menu, Setting::Preset, Some(settings_categories(&menu))).unwrap();
        run(&mut rt, 30);
        snap(&rt, "settings-8-preset");
        let after = rows(&rt.bindings.categories);
        assert_eq!(after, rows(&settings_categories(&menu)));
        assert!(before.iter().zip(&after).filter(|(b, a)| b != a).count() > 3, "{after:?}");
        assert_eq!(rt.current_mut().unwrap().name, "#Preset: Performance");
        // User 10-26: the Video page's ninth row scrolls into view, with "Y Search" bottom right; the search page.
        let mut rt = open(settings_categories(&menu), SETTINGS_TITLE, (2, 0));
        for _ in 0..8 {
            rt.input(Key::Down).unwrap();
            run(&mut rt, 8);
        }
        assert_eq!(rt.current_mut().unwrap().action, Action::Setting(Setting::Occlusion));
        let hint = search_hint(&rt);
        assert!(hint.iter().any(|d| d.texture == "runtime/buttons/Y.rgba"), "the Y glyph");
        let mut draws = crate::apt_scene::draw(&rt.bindings.movie, &rt.vm, &rt.bindings.shapes).unwrap();
        draws.extend(hint);
        raster_draws(&draws, &root, &out.join("settings-9-scrolled.png"));
        let found = search(&menu, "fps", |text| rt.bindings.movie.text_assets.localize(text));
        assert_eq!(found.first(), Some(&Action::Setting(Setting::FpsCounter)));
        snap(&open(search_categories(&menu, "fps", &found), SETTINGS_TITLE, (0, 0)), "settings-10-search");
        // A guest's World page: the host's values, greyed.
        menu.guest = true;
        menu.set_session_world(Some([2, 2]));
        let guest = open(settings_categories(&menu), SETTINGS_TITLE, (3, 0));
        snap(&guest, "settings-6-guest-world");
        let world = &guest.bindings.categories[3].items;
        // The host's two world rows; the graphics rows below them stay the guest's own.
        assert_eq!(world[..2].iter().map(|i| (i.name.as_str(), i.enabled)).collect::<Vec<_>>(),
            [("#Weather: Heavy Rain", false), ("#Time of Day: Cycle", false)]);
        // Bug 10-25 #21: each gestures row leads with its own lit D-pad arm.
        snap(&open(gesture_categories(&menu), GESTURES_TITLE, (0, 1)), "settings-7-gestures");
    }

    /// Review 10-27: device lists are read on a thread, so they can come in after the Voice Chat page opened: its rows
    /// take them in place (the saved microphone, not found before, shows by name).
    #[test]
    #[ignore = "requires the prepared original core menu"]
    fn devices_read_after_the_page_opened_show_in_place() {
        let root = PathBuf::from(std::env::var("SKATE_CORE_MENU_ROOT").unwrap());
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("runtime/core_menu.json")).unwrap()).unwrap();
        let mut menu = crate::graphics_menu::test_menu();
        menu.voice_devices = Some((vec!["USB Mic".into()], vec![]));
        menu.choose(Setting::Microphone, 1);
        menu.voice_devices = None;
        let mut rt = Runtime::load(&json, voice_categories(&menu), VOICE_TITLE.into()).unwrap();
        (0..30).for_each(|_| rt.advance().unwrap());
        rt.show().unwrap();
        (0..40).for_each(|_| rt.advance().unwrap());
        let mut crossbar = test_crossbar();
        (crossbar.runtime, crossbar.settings, crossbar.voice) = (Some(rt), true, true);
        let mic = |crossbar: &Crossbar| {
            let rows = &crossbar.runtime.as_ref().unwrap().bindings.categories[0].items;
            rows.iter().find(|i| i.action == Action::Setting(Setting::Microphone)).unwrap().name.clone()
        };
        assert_eq!(mic(&crossbar), "#Microphone: Default (Windows)", "the saved one, not read yet");
        menu.voice_devices = Some((vec!["USB Mic".into()], vec![]));
        show_devices(&mut crossbar, &menu).unwrap();
        assert_eq!(mic(&crossbar), "#Microphone: USB Mic");
    }

    /// Visual check of the Voice Chat page on its Mic Test row, the meter drawn (SKATE_CORE_MENU_ROOT = private/core-menu).
    #[test]
    #[ignore = "requires the prepared original core menu"]
    fn voice_page_renders() {
        let root = PathBuf::from(std::env::var("SKATE_CORE_MENU_ROOT").unwrap());
        let out = PathBuf::from(std::env::var("SKATE_CORE_MENU_OUT").unwrap_or_else(|_| ".".into()));
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("runtime/core_menu.json")).unwrap()).unwrap();
        let mut menu = crate::graphics_menu::test_menu();
        menu.voice_devices = Some((vec!["Headset Microphone (USB Audio)".into()], vec!["Speakers (Realtek)".into()]));
        let mut rt = Runtime::load(&json, voice_categories(&menu), VOICE_TITLE.into()).unwrap();
        rt.focus(0, 8);
        (0..30).for_each(|_| rt.advance().unwrap());
        rt.show().unwrap();
        (0..40).for_each(|_| rt.advance().unwrap());
        assert_eq!(rt.current_mut().unwrap().action, Action::Setting(Setting::MicTest));
        let mut draws = crate::apt_scene::draw(&rt.bindings.movie, &rt.vm, &rt.bindings.shapes).unwrap();
        draws.extend(mic_meter(-24., skate_voice::dsp::line_dbfs(5)));
        raster_draws(&draws, &root, &out.join("voice-1-mic-test.png"));
    }
}
