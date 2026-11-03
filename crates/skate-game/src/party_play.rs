//! Skate 3's Party Play front end (San Van Party Pack, prepared by tools/prepare_party_play.py into
//! private/party-play): the setup screen (`partyplay`, retail FE slot 31 / screen 67), the "Player N ready! Press A to
//! begin." card (`partyplay_start`, slot 32 / screen 68) and the Exit Party Play / Rematch confirm popups (retail
//! popups 213 / 217, the disc's sk8popup). The movies' ActionScript runs as shipped on the shared APT runtime (layer
//! PARTY_PLAY); what retail's natives do (setup input 0x8260f0b8, card input 0x8260fda0, the FEPartyPlay bindings at
//! 0x825ba9a8) is done here. Research: .superpowers/sdd/2026-10-04-party-play/research.md.
use crate::{
    apt_screen::{PARTY_PLAY, Screen, Screens},
    config::Config,
    core_menu_runtime::{Key, SOUND_A_BUTTON, SOUND_B_BUTTON, SOUND_NAV_DOWN, SOUND_NAV_UP, fe_sound_id},
    custom_models::CustomModels,
    customiser::Navigation,
    match_core::{Match, Mode},
    multiplayer::appearance::{LocalPuppets, Look, Puppet, puppet_id},
    online_map::{Challenge, OnlineMap},
    popup_runtime,
    skate_audio::FrontEndSoundRequest,
};
use bevy::prelude::*;
use serde_json::{Value as Json, json};

const SOUND_NAV_LEFT: u64 = 0xAD8B_F9A0_B499_7B49;
const SOUND_NAV_RIGHT: u64 = 0xD8EE_38A6_1C45_EA28;
/// One authored frame (the popup advances on it).
const FRAME: f32 = 1. / 60.;
/// Outro fallback if a screen never reports OutroComplete.
const CLOSE_TIMEOUT: f32 = 0.6;
/// The asset folder (apt_screen's PARTY_PLAY layer root).
pub(crate) const FOLDER: &str = "private/party-play";

/// The retail character list's picks (vector 0x830c1220 minus its invalid records), in retail order: (characters
/// key, the movie's display name id). The user's own skaters come first (decision D1).
pub(crate) const PROS: [(&str, &str); 33] = [
    ("mike_carroll", "ID_COMMON_MIKE_CARROLL"),
    ("michael_burnette", "ID_COMMON_MICHAEL_BURNETTE"),
    ("pat_duffy", "ID_COMMON_PAT_DUFFY"),
    ("ray_barbee", "ID_COMMON_RAY_BARBEE"),
    ("pj_ladd", "ID_COMMON_PJ_LADD"),
    ("mark_appleyard", "ID_COMMON_MARK_APPLEYARD"),
    ("john_rattray", "ID_COMMON_JOHN_RATTRAY"),
    ("joey_brezinski", "ID_COMMON_BREZINKSI"),
    ("john_cardiel", "ID_COMMON_JOHN_CARDIEL"),
    ("josh_kalis", "ID_COMMON_JOSH_KALIS"),
    ("lucas_puig", "ID_COMMON_LUCAS_PUIG"),
    ("lizard_king", "ID_COMMON_LIZARDKING"),
    ("terry_kennedy", "ID_COMMON_TERRY_KENNEDY"),
    ("ryan_gallant", "ID_COMMON_RYAN_GALLANT"),
    ("rob_dyrdek", "ID_COMMON_ROB_DYRDEK"),
    ("ryan_smith", "ID_COMMON_RYAN_SMITH"),
    ("seb", "ID_COMMON_SEB"),
    ("danny_way", "ID_COMMON_DANNY_WAY"),
    ("dan_drehobl", "ID_COMMON_DREHOBL"),
    ("cuz", "ID_COMMON_CUZ"),
    ("andrew_reynolds", "ID_COMMON_ANDREW_REYNOLDS"),
    ("benny_fairfax", "ID_COMMON_BENNY_FAIRFAX"),
    ("attiba_jefferson", "ID_COMMON_ATTIBA_JEFFERSON"),
    ("brayden_szafranski", "ID_COMMON_BRAYDEN_SZAFRANSKI"),
    ("colin_mckay", "ID_COMMON_COLIN_MCKAY"),
    ("chris_cole", "ID_COMMON_CHRIS_COLE"),
    ("chris_haslam", "ID_COMMON_CHRIS_HASLAM"),
    ("eric_koston", "ID_COMMON_ERIC_KOSTON"),
    ("jason_dill", "ID_COMMON_JASON_DILL"),
    ("jerry_hsu", "ID_COMMON_JERRY_HSU"),
    ("deerman", "ID_COMMON_DEERMAN"),
    ("darren_navarette", "ID_COMMON_DARREN_NAVARETTE"),
    ("dennis_busenitz", "ID_COMMON_DENNIS_BUSENTIZ"),
];

/// One entry of the setup's character list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Character {
    /// The player's own skater (settings/character.json).
    MySkater,
    /// An imported model (custom_models), by id.
    Imported { id: String, name: String },
    /// A retail pick (native roster key).
    Pro { key: &'static str, name: &'static str },
}
impl Character {
    /// What GetPlayerName answers: retail's display name id, or literal text for the user's own skaters (no retail
    /// string names them).
    pub fn label(&self) -> String {
        match self {
            Character::MySkater => "#My Skater".into(),
            Character::Imported { name, .. } => format!("#{name}"),
            Character::Pro { name, .. } => (*name).into(),
        }
    }
}

/// The setup's list: My Skater, each imported model, then the 33 retail picks (D1).
pub(crate) fn characters<'a>(imported: impl IntoIterator<Item = (&'a str, &'a str)>) -> Vec<Character> {
    std::iter::once(Character::MySkater)
        .chain(imported.into_iter().map(|(id, name)| Character::Imported { id: id.into(), name: name.into() }))
        .chain(PROS.iter().map(|&(key, name)| Character::Pro { key, name }))
        .collect()
}

/// The setup's challenge types (mgr+12, 0..=3), as the movie's mPartyPlayChallenge_* number them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    SpotBattle,
    Skate,
    HallOfMeat,
    OneUp,
}
pub(crate) const KINDS: [Kind; 4] = [Kind::SpotBattle, Kind::Skate, Kind::HallOfMeat, Kind::OneUp];
impl Kind {
    /// The Online Challenge Map's Versus folder holding this type's rows (retail buckets X+80 / +48 / +64 / +32 by
    /// ChallengeType 53 / 4 / 33 / 32).
    pub fn folder(self) -> &'static str {
        match self {
            Kind::SpotBattle => "ID_MAP_FILTER_CHALLENGETYPES_SPOTBATTLE",
            Kind::Skate => "ID_MAP_FILTER_CHALLENGETYPES_SKATE",
            Kind::HallOfMeat => "ID_MAP_FILTER_CHALLENGETYPES_HOMBATTLE",
            Kind::OneUp => "ID_MAP_FILTER_CHALLENGETYPES_1UP",
        }
    }
}

/// A location row: its title (a string id or "#literal"), its challenge and the Online Challenge Map tab it is on.
#[derive(Clone, Debug, Default)]
pub(crate) struct Place {
    pub title: String,
    pub challenge: Challenge,
    pub tab: usize,
}

/// Not retail: the row the location page shows the photo of (its tab, its challenge id); none off that page.
pub(crate) fn shown(setup: &Setup) -> Option<(usize, &str)> {
    let place = setup.places.get(setup.kind)?.get(setup.location).filter(|_| setup.page == 3)?;
    Some((place.tab, place.challenge.id.as_str()))
}

/// Not retail: where the location page's photo sits on the 1280x720 stage (2:1, under the settings panel, as wide).
const PHOTO: [f32; 4] = [460., 458., 820., 638.];

/// The location page's photo (an absolute .rgba of `size`), filling [`PHOTO`]: a picture of another shape shows its
/// middle (a square map picture its middle half).
pub(crate) fn photo(path: &str, [w, h]: [u32; 2]) -> Vec<crate::apt_scene::Draw> {
    let [x0, y0, x1, y1] = PHOTO;
    let (slot, image) = ((x1 - x0) / (y1 - y0), w as f32 / h.max(1) as f32);
    let (u, v) = if image < slot { (0., (1. - image / slot) / 2.) } else { ((1. - slot / image) / 2., 0.) };
    vec![crate::graphic_board::quad_uv(path, [w, h], PHOTO, [u, v, 1. - u, 1. - v], [1.; 4])]
}

const VERSUS: &str = "ID_MAP_FILTER_VERSUS";
/// The Online Challenge Map's Skate 2 tab: its Spot Battle, S.K.A.T.E. and Hall of Meat rows follow Skate 3's, played
/// on the Skate 2 ports' party path (Skate 3's rules, their own HUD). Skate 2 had no 1-Up (online_map leaves its rows
/// out).
const SKATE_2: &str = "#Skate 2";
/// The Skate 1 tab: its Spot Battle (Own The Spot) and S.K.A.T.E. rows follow Skate 2's, played by skate1_online's
/// Party (every player's console here, one pad). Skate 1 has no Hall of Meat or 1-Up.
const SKATE_1: &str = "#Skate 1";
/// Each type's locations: the challenge rows of its Versus folder on the installed maps, then the Skate 2 and Skate 1
/// tabs' folders of the same name (titled with the game: their place names repeat Skate 3's).
/// ponytail: online.json order; retail lists its registry order (UNKNOWN), and keeps rows online.json lacks.
/// Except LATE_1UP, which retail's OnAdded skips (0x82565d28..0x82565d7c) and adds last, when the 1-UP bucket is first
/// read, behind the content gate 0x82503ae0 on world record 03D1852F5F9C102B (0x82565b54..0x82565cac); the gate passes
/// with every DLC installed (D4).
pub(crate) fn places(map: &OnlineMap) -> [Vec<Place>; 4] {
    let tab = |name: &str| map.tabs.iter().position(|t| t.name == name);
    let (versus, skate2, skate1) = (tab(VERSUS), tab(SKATE_2), tab(SKATE_1));
    KINDS.map(|kind| {
        let rows = |tab: Option<usize>, suffix: &str| -> Vec<Place> {
            let folder = tab.and_then(|t| map.tabs[t].rows.iter().find(|e| e.folder && e.name == kind.folder()));
            let mut list: Vec<Place> = (folder.map_or(&[][..], |f| &f.rows[..]).iter())
                .filter(|e| !e.header)
                .filter_map(|e| Some(Place { title: format!("{}{suffix}", e.name), challenge: e.challenge.clone()?, tab: tab? }))
                .collect();
            // Stable: everything else keeps its order.
            list.sort_by_key(|p| p.challenge.id == LATE_1UP);
            list
        };
        let mut list = rows(versus, "");
        list.extend(rows(skate2, " (Skate 2)"));
        list.extend(rows(skate1, " (Skate 1)"));
        list
    })
}
const LATE_1UP: &str = "1up_bbpk_01";

/// What a press did: the FE sounds (in order), the movie modes to SetState (in order), and whether setup ended.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Step {
    pub sounds: Vec<u64>,
    pub states: Vec<i64>,
    pub exit: Option<Exit>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Exit {
    /// B on the first page: back to the crossbar.
    Back,
    /// A on a location: start the match.
    Launch,
}

/// The movie's modes (partyplay.as mMode_*).
const SELECT_PLAYERS: i64 = 1;
const REFRESH_SELECT_PLAYERS: i64 = 2;
const PLAYER_SELECT_CHARACTER: i64 = 3;
const REFRESH_SELECT_CHARACTER: i64 = 4;
const REFRESH_SELECT_CHARACTER_INDEX: i64 = 5;
const SETUP_CHALLENGE: i64 = 6;
const REFRESH_CHALLENGE: i64 = 7;
const SETUP_SELECT_LOCATION: i64 = 8;
const REFRESH_SELECT_LOCATION: i64 = 9;
const MAX_PLAYERS: usize = 4;
const MIN_PLAYERS: usize = 2;

/// Retail's setup state (vtable 0x82303710). Enter (0x8260eab8) sets every field, so Change settings reopens on
/// these defaults too (D11).
#[derive(Clone, Debug)]
pub(crate) struct Setup {
    pub characters: Vec<Character>,
    pub places: [Vec<Place>; 4],
    /// +100 (2..=4).
    pub players: usize,
    /// +96: 0 players, 1 characters, 2 challenge type, 3 location, 4 launched.
    pub page: u8,
    /// +120: the player whose character Left/Right changes.
    pub active: usize,
    /// +64..+76: each player's character index. D9: retail's index 0 is an invalid record and its launch with it is
    /// unproven, so every player starts on the list's first entry (My Skater).
    pub picks: [usize; MAX_PLAYERS],
    /// mgr+12.
    pub kind: usize,
    /// mgr+20.
    pub location: usize,
    /// +124: a launch is under way; input is ignored.
    busy: bool,
}

impl Setup {
    pub fn new(characters: Vec<Character>, places: [Vec<Place>; 4]) -> Self {
        Self { characters, places, players: 2, page: 0, active: 0, picks: [0; MAX_PLAYERS], kind: 0, location: 0, busy: false }
    }

    fn location_count(&self) -> usize {
        self.places[self.kind].len()
    }

    /// One press, as retail's input handler 0x8260f0b8 takes it (table in research.md 1.3). Keys a page does not
    /// handle do nothing and play nothing.
    pub fn input(&mut self, key: Key) -> Step {
        let mut step = Step::default();
        if self.busy {
            return step;
        }
        let characters = self.characters.len().max(1);
        let locations = self.location_count();
        let (sounds, states) = (&mut step.sounds, &mut step.states);
        match (self.page, key) {
            (0, Key::Right) if self.players < MAX_PLAYERS => {
                sounds.push(SOUND_NAV_RIGHT);
                self.players += 1;
                states.push(REFRESH_SELECT_PLAYERS);
            }
            (0, Key::Left) if self.players > MIN_PLAYERS => {
                sounds.push(SOUND_NAV_LEFT);
                self.players -= 1;
                states.push(REFRESH_SELECT_PLAYERS);
            }
            (0, Key::Accept) => {
                self.page = 1;
                states.push(PLAYER_SELECT_CHARACTER);
                sounds.push(SOUND_A_BUTTON);
            }
            (0, Key::Back) => {
                sounds.push(SOUND_B_BUTTON);
                step.exit = Some(Exit::Back);
            }
            (1, Key::Back) => {
                self.page = 0;
                states.push(SELECT_PLAYERS);
                sounds.push(SOUND_B_BUTTON);
            }
            // 0x8260fab0: the next valid character (wrapping), with nav_right before and again as it lands.
            // ponytail: every entry here is valid, so retail's skip-invalid loop never skips.
            (1, Key::Right) => {
                sounds.push(SOUND_NAV_RIGHT);
                let pick = &mut self.picks[self.active.min(MAX_PLAYERS - 1)];
                *pick = (*pick + 1) % characters;
                sounds.push(SOUND_NAV_RIGHT);
                states.push(REFRESH_SELECT_CHARACTER);
            }
            // 0x8260fbc8: retail plays nav_right first here too (0x8260f33c), then nav_left as it lands.
            (1, Key::Left) => {
                sounds.push(SOUND_NAV_RIGHT);
                let pick = &mut self.picks[self.active.min(MAX_PLAYERS - 1)];
                *pick = (*pick + characters - 1) % characters;
                sounds.push(SOUND_NAV_LEFT);
                states.push(REFRESH_SELECT_CHARACTER);
            }
            (1, Key::Down) => {
                sounds.push(SOUND_NAV_DOWN);
                self.active += 1;
                if self.active >= self.players {
                    self.active = 0;
                }
                states.push(REFRESH_SELECT_CHARACTER_INDEX);
            }
            (1, Key::Up) => {
                sounds.push(SOUND_NAV_UP);
                self.active = self.active.checked_sub(1).unwrap_or(self.players - 1);
                states.push(REFRESH_SELECT_CHARACTER_INDEX);
            }
            (1, Key::Accept) => {
                self.page = 2;
                states.push(SETUP_CHALLENGE);
                sounds.push(SOUND_A_BUTTON);
            }
            // ponytail: retail calls SetState(3) on two objects here (vt+20 twice); the movie gets it once.
            (2, Key::Back) => {
                states.push(PLAYER_SELECT_CHARACTER);
                self.page = 1;
                sounds.push(SOUND_B_BUTTON);
            }
            (2, Key::Accept) => {
                self.page = 3;
                // 0x82518e50 rebuilds the list for the type: the first location again.
                self.location = 0;
                states.push(SETUP_SELECT_LOCATION);
                sounds.push(SOUND_A_BUTTON);
            }
            (2, Key::Right) => {
                self.kind = (self.kind + 1) % KINDS.len();
                sounds.push(SOUND_NAV_RIGHT);
                states.push(REFRESH_CHALLENGE);
            }
            (2, Key::Left) => {
                self.kind = (self.kind + KINDS.len() - 1) % KINDS.len();
                sounds.push(SOUND_NAV_LEFT);
                states.push(REFRESH_CHALLENGE);
            }
            (3, Key::Back) => {
                self.page = 2;
                states.push(SETUP_CHALLENGE);
                sounds.push(SOUND_B_BUTTON);
            }
            (3, Key::Right) => {
                sounds.push(SOUND_NAV_RIGHT);
                self.location = if self.location + 1 >= locations { 0 } else { self.location + 1 };
                sounds.push(SOUND_NAV_RIGHT);
                states.push(REFRESH_SELECT_LOCATION);
            }
            (3, Key::Left) => {
                sounds.push(SOUND_NAV_LEFT);
                self.location = self.location.checked_sub(1).unwrap_or(locations.saturating_sub(1));
                sounds.push(SOUND_NAV_LEFT);
                states.push(REFRESH_SELECT_LOCATION);
            }
            // ponytail: an empty list (no installed map has the type) refuses A; retail would launch "#Invalid Title".
            (3, Key::Accept) if locations > 0 => {
                self.busy = true;
                self.page = 4;
                sounds.push(SOUND_A_BUTTON);
                step.exit = Some(Exit::Launch);
            }
            _ => {}
        }
        step
    }

    /// The FEPartyPlay bindings the movie reads (research.md 4.1). GetPartyPlayLargeImage stays unanswered (D5): mode 10
    /// loads it into an mImage clip the movie doesn't have, so the location's photo is drawn over the page ([`photo`]).
    /// ponytail (D3): nothing calls the movie's UpdatePlayerPorts / SetActivePlayerPort, as in retail: the TU3 image has
    /// no SetActivePlayerPort, UpdatePlayerPorts or PlayerPort string (ASCII or UTF-16), so its port clips keep their
    /// authored default frame.
    pub fn answers(&self) -> Vec<(String, Json)> {
        let location = (self.places[self.kind].get(self.location)).map_or("#Invalid Title", |p| p.title.as_str());
        let mut answers = vec![
            ("FEPartyPlay.GetPlayerCount".to_owned(), json!(self.players)),
            ("FEPartyPlay.GetPlayerIndex".to_owned(), json!(self.active)),
            ("FEPartyPlay.GetPartyPlayLocation".to_owned(), json!(location)),
            ("FEPartyPlay.GetPartyPlayChallenge".to_owned(), json!(self.kind)),
        ];
        for (i, &pick) in self.picks.iter().enumerate() {
            let name = self.characters.get(pick).map_or_else(String::new, Character::label);
            answers.push((format!("FEPartyPlay.GetPlayerName({i})"), json!(name)));
        }
        answers
    }

    /// The launched setup: the type, the location and each player's character.
    pub fn launch(&self) -> Option<Launch> {
        Some(Launch {
            kind: KINDS[self.kind],
            place: self.places[self.kind].get(self.location)?.clone(),
            players: self.picks[..self.players].iter().map(|&i| self.characters.get(i).cloned().unwrap_or(Character::MySkater)).collect(),
        })
    }
}

/// A launched setup, for the party session (match_core).
#[derive(Clone, Debug)]
pub(crate) struct Launch {
    pub kind: Kind,
    pub place: Place,
    /// Player 1.. in order, each its own skater.
    pub players: Vec<Character>,
}

/// The confirm popups (retail popup table; both No/Yes, not cancelable).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ask {
    /// 213: Exit Party Play.
    Exit,
    /// 217: Rematch.
    Rematch,
}
impl Ask {
    fn message(self) -> &'static str {
        match self {
            Ask::Exit => "ID_QUIT_GAME_MESSAGE_NO_DATA_LOSS",
            Ask::Rematch => "ID_REMATCH_CONFIRM_MESSAGE",
        }
    }
}
const NO_YES: &[&str] = &["ID_COMMON_NO", "ID_COMMON_YES"];

#[derive(Resource, Default)]
pub(crate) struct PartyPlay {
    /// The Party Pack's screens are installed: the crossbar's Party Play row is enabled (D4).
    pub installed: bool,
    /// core_menu's Party Play pick (or the results' Change settings): open the setup.
    pub open_setup: bool,
    /// A setup, card or popup owns input; the crossbar hides.
    pub modal: bool,
    /// One closed this frame; core_menu ignores that press.
    pub closed_this_frame: bool,
    /// A launch or a confirmed Exit: core_menu closes the crossbar.
    pub close_crossbar: bool,
    /// The setup's launch, taken by the party session.
    pub launch: Option<Launch>,
    /// A party session runs: the crossbar shows the Party Play page.
    pub active: bool,
    /// The crossbar's Restart Challenge (the graphs' `restartchallenge`), taken by the party session.
    pub restart: bool,
    /// Open the Press-A card for (player 1-based, round) (FrontEnd.OpenPressADialog).
    pub card: Option<(usize, u32)>,
    /// A on the card (retail message 0x3AFB7BB6, the graphs' `hudcomplete`), taken by the party session.
    pub hud_complete: bool,
    /// Ask a confirm popup.
    pub ask: Option<Ask>,
    /// The popup answered Yes (taken by the session: Rematch is `retry`, Exit ends the session).
    pub confirmed: Option<Ask>,
    /// The session's players' names ("Player N"), the modes' names offline.
    pub players: Vec<String>,
    /// The player the pad drives (ChallengeManager.SetPlayer), from the mode.
    pub controlled: usize,
    /// Each player's TurnBasedWaitingLocation slot while waiting (None: not placed, or the pad's player).
    pub waiting: Vec<Option<[[f32; 4]; 4]>>,
    /// The mode's results dialog answers (OpenContinueRetryDialog) while it waits on Rematch / Quit.
    pub results: Option<Vec<(String, Json)>>,
    /// Rematch confirmed (the graphs' `hudretry`), taken by the mode.
    pub retry: bool,
    /// The results dialog is up.
    results_open: bool,
    /// Each player's custom_models entry to wear (None: My Skater).
    wears: Vec<Option<String>>,
    /// The player whose look the skater wears.
    worn: Option<usize>,
    /// custom_models' selection before the session, worn again when it ends.
    restore: Option<Option<String>>,
    /// A launch from Change settings, started the frame after the running match was dropped.
    next: Option<Launch>,
    setup: Option<Setup>,
    /// Seconds since B played the setup's outro.
    closing: Option<f32>,
    /// The card is up; Some(seconds) once A played its outro.
    card_shown: Option<Option<f32>>,
    popup: Option<(Ask, popup_runtime::Runtime)>,
    accumulated: f32,
    /// The row whose photo the location page shows ([`shown`]).
    photo: Option<(usize, String)>,
}
/// The modes' names offline: the session's players, else two (`SKATE_MODE` solo).
pub(crate) fn names(world: &World) -> Vec<String> {
    match world.get_resource::<PartyPlay>().map(|p| p.players.clone()).filter(|p| !p.is_empty()) {
        Some(players) => players,
        None => vec!["Player 1".into(), "Player 2".into()],
    }
}

/// A Party Play session is under way (the setup launched a match): the Skate 2 ports take its turn path.
pub(crate) fn in_session(world: &World) -> bool {
    world.get_resource::<PartyPlay>().is_some_and(|p| p.active)
}

/// The results dialog's standings for a mode without Skate 3's results data (the Skate 2 ports): `rows` best first,
/// each (player, name, rank, the score cell's number), on PlaceNameInt (0) under `score`'s column; `throwdown` is
/// endchallenge_online's eThrowDownType (None 0, OneUp 1, Skate 2). Player 1's row is the current one, as Skate 3's
/// party modes pass it (their LocalPlayerID).
pub(crate) fn standings(title: &str, rows: &[(usize, String, i64, i64)], score: &str, throwdown: i64) -> Vec<(String, Json)> {
    let block: Vec<Json> = (rows.iter().enumerate())
        .flat_map(|(i, (_, name, rank, value))| {
            [json!(rank), json!(format!("#{value}")), json!(0), json!(""), json!(""), json!(false), json!(i), json!(format!("#{name}"))]
        })
        .collect();
    let n = rows.len();
    let position = rows.iter().position(|r| r.0 == 0).map_or(0, |i| i + 1);
    vec![
        ("Mission.GetResultsTitle".to_owned(), json!(title)),
        ("Mission.GetResultsStatusText".into(), json!("")),
        ("Mission.GetResultsStatusTimer".into(), json!(0)),
        ("Mission.GetCurrentPlayerPosition".into(), json!(position)),
        ("HUDComponents.Results_GetNumPlayers".into(), json!(n)),
        ("HUDComponents.Results_GetLayoutType".into(), json!(0)),
        ("HUDComponents.Results_GetLeaderboardDataBlock".into(), Json::Array(block)),
        ("HUDComponents.Results_GetLeaderboardSubHeaderDataBlock".into(), json!([false, 0, "", ""])),
        ("HUDComponents.Results_GetLeaderboardColTitle(0)".into(), json!("ID_LEADERBOARD_HEADER_RANK")),
        ("HUDComponents.Results_GetLeaderboardColTitle(1)".into(), json!("ID_LEADERBOARD_HEADER_SKATER")),
        ("HUDComponents.Results_GetLeaderboardColTitle(2)".into(), json!(score)),
        ("HUDComponents.Results_GetThrowdownType".into(), json!(throwdown)),
        ("FELeaderboard.EndChallengeOnline_GetLeaderboardNavData".into(), json!([n, position.saturating_sub(1), position])),
        // No gamercard on Steam: no platform matches, no A button (as skate3_skate_game's results).
        ("global.GetPlatform".into(), json!(-1)),
    ]
}

/// What an offline mode sends each frame (its Lua's PartyPlay calls).
pub(crate) struct Turn {
    /// ChallengeManager.SetPlayer: the player the pad drives.
    pub controlled: usize,
    /// RepositionAI + PinAI to TurnBasedWaitingLocation, per player (None: not waiting).
    pub waiting: Vec<Option<[[f32; 4]; 4]>>,
    /// FrontEnd.OpenPressADialog(player 1-based, round), once.
    pub card: Option<(usize, i64)>,
    /// OpenContinueRetryDialog: the results screen's answers while the dialog is up.
    pub results: Option<Vec<(String, Json)>>,
}

/// What comes back: the card's A (`hudcomplete`), Rematch (`hudretry`) and Restart Challenge (`restartchallenge`).
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Answers {
    pub hud_complete: bool,
    pub retry: bool,
    pub restart: bool,
}
impl Answers {
    pub fn messages(&self) -> Vec<crate::skate3_modes::Msg> {
        use crate::skate3_modes::Msg;
        [(self.hud_complete, Msg::HudComplete), (self.retry, Msg::HudRetry), (self.restart, Msg::Restart)]
            .into_iter()
            .filter_map(|(on, msg)| on.then_some(msg))
            .collect()
    }
}

/// A mode's frame of Party Play: its calls in, the player's answers out. Without the front end the card is answered
/// at once.
pub(crate) fn exchange(world: &mut World, turn: Turn) -> Answers {
    let Some(mut party) = world.get_resource_mut::<PartyPlay>() else {
        return Answers { hud_complete: turn.card.is_some(), ..default() };
    };
    let party = &mut *party;
    (party.controlled, party.waiting) = (turn.controlled, turn.waiting);
    if let Some((player, round)) = turn.card {
        party.card = Some((player, round.max(0) as u32));
    }
    // Opened once (the dialog's answers are read as it opens); None takes it down.
    if turn.results.is_none() || party.results.is_none() {
        party.results = turn.results;
    }
    let answers = Answers {
        hud_complete: std::mem::take(&mut party.hud_complete),
        retry: std::mem::take(&mut party.retry),
        restart: std::mem::take(&mut party.restart),
    };
    if answers.retry || answers.restart {
        party.results = None;
    }
    answers
}

pub(crate) struct PartyPlayPlugin;
impl Plugin for PartyPlayPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PartyPlay>()
            .add_systems(Startup, detect)
            .add_systems(PreUpdate, interact.after(crate::customiser::navigation).before(crate::core_menu::interact))
            .add_systems(PreUpdate, open_requested.after(crate::core_menu::interact).before(crate::graphics_menu::MenuInput))
            .add_systems(Update, (session, draw.before(crate::apt_screen::render)));
    }
}

/// The session (D1, the lead's plan A): a launch starts the match on its row, travels there and spawns every player's
/// puppet (each look loaded up front, hidden); each handover wears the next player's look and resets what the turn's
/// teleport leaves; Exit (crossbar or results) ends it with My Skater's look back and every puppet gone.
fn session(world: &mut World) {
    // An online lobby or invite join begins: the session ends (its puppets, worn look and Match are offline only).
    let online = world.get_resource::<crate::multiplayer::Multiplayer>().is_some_and(|n| n.engaged());
    let Some(mut party) = world.get_resource_mut::<PartyPlay>() else { return };
    let party = &mut *party;
    let next = party.next.take();
    let launch = party.launch.take();
    let exit = match party.confirmed.take() {
        // Also with no session (`SKATE_MODE` solo): the match and the dialog go.
        Some(Ask::Exit) => true,
        Some(Ask::Rematch) => {
            party.retry = true;
            false
        }
        None => party.active && online,
    };
    if exit {
        end(world);
    }
    if let Some(launch) = next.filter(|_| !online) {
        begin(world, launch);
    }
    if let Some(launch) = launch.filter(|_| !online) {
        if world.resource::<PartyPlay>().active {
            // Change settings' launch aborts the running challenge (0x8260eb78 case 4): the match is dropped now, so
            // its mode ends, and the new one starts next frame. The session (and the look to restore) carries on.
            let mut party = world.resource_mut::<PartyPlay>();
            (party.next, party.results) = (Some(launch), None);
            *world.resource_mut::<Match>() = Match::default();
        } else {
            begin(world, launch);
        }
    }
    hand_over(world);
}

fn begin(world: &mut World, launch: Launch) {
    let now = crate::match_core::match_now(world);
    let challenge = launch.place.challenge.clone();
    let my_skater = my_skater(world.get_resource::<crate::customiser::Customiser>().map(|c| &c.draft));
    let models = world.resource::<CustomModels>();
    let active = models.active.clone();
    let (wears, looks): (Vec<Option<String>>, Vec<Look>) = (launch.players.iter())
        .map(|character| match character {
            Character::MySkater => (None, my_skater.clone()),
            Character::Imported { id, .. } => (Some(id.clone()), models.entry_path(id).map_or(Look::Stock, Look::Imported)),
            Character::Pro { key, .. } => {
                let entry = models.native_entry(key);
                if entry.is_none() {
                    warn!("Party Play: {key} is not converted (Custom Models); the stock skater stands in");
                }
                (entry, Look::Native((*key).into()))
            }
        })
        .unzip();
    let owned = world.get_resource::<crate::match_core::Owned>().is_some_and(|o| o.0.contains(&challenge.id));
    let mode = match launch.kind {
        Kind::SpotBattle => Mode::SpotBattle,
        Kind::Skate => Mode::Skate,
        Kind::HallOfMeat => Mode::HallOfMeat,
        Kind::OneUp => Mode::OneUp,
    };
    {
        let mut m = world.resource_mut::<Match>();
        m.start(mode, &challenge.id, now);
        m.owned = owned;
    }
    // To the row's map (the mode waits until it is loaded) and its start.
    if let Some(mut map) = world.get_resource_mut::<crate::challenge_map::ChallengeMap>() {
        map.picked = Some(crate::challenge_map::Spot {
            map: challenge.map.clone(),
            id: challenge.teleport.clone(),
            matrix: challenge.spawn.start.as_ref().map(|s| s.slot(0)),
        });
    }
    if let Some(mut puppets) = world.get_resource_mut::<LocalPuppets>() {
        puppets.0 = looks.into_iter().enumerate().map(|(p, look)| (puppet_id(p), Puppet { look, at: None })).collect();
    }
    let mut party = world.resource_mut::<PartyPlay>();
    if !party.active {
        party.restore = Some(active);
    }
    info!("Party Play: {:?} on {} ({}), {} players", launch.kind, challenge.id, challenge.map, wears.len());
    party.players = (1..=wears.len()).map(|n| format!("Player {n}")).collect();
    (party.wears, party.worn, party.controlled, party.active) = (wears, None, 0, true);
    party.waiting.clear();
    party.results = None;
    // One-shot answers meant for the previous match (a Restart Challenge picked during the load) do not carry over.
    (party.retry, party.restart, party.hud_complete, party.card) = (false, false, false, None);
}

/// My Skater's puppet: the customiser's profile (settings/character.json), which the renderer resolves as for a remote
/// outfit. Not `Parts.applied`: only written while no custom model is worn, and Null after a stock restore.
fn my_skater(draft: Option<&Json>) -> Look {
    match draft.filter(|d| d["selections"].as_object().is_some_and(|s| !s.is_empty())) {
        Some(draft) => Look::Outfit(draft.clone()),
        None => Look::Stock,
    }
}

/// Quit / Exit Party Play: back to free skate as My Skater (0x825f1b08 / 0x8261cc60 clear the skater slots and re-add
/// skater 0); every puppet goes.
fn end(world: &mut World) {
    let mut party = world.resource_mut::<PartyPlay>();
    let party = &mut *party;
    let restore = party.restore.take();
    info!("Party Play: ended");
    (party.active, party.worn, party.controlled, party.results, party.card, party.next) = (false, None, 0, None, None, None);
    (party.retry, party.restart, party.hud_complete) = (false, false, false);
    (party.players, party.wears, party.waiting) = Default::default();
    // Left with the card up (the Start menu's Exit, an online join): the card goes with the session, or it waited on
    // an A no turn would take.
    let card_up = party.card_shown.take().is_some();
    if let (true, Some(mut screens)) = (card_up, world.get_resource_mut::<Screens>()) {
        screens.layer(PARTY_PLAY).close("2-card");
    }
    *world.resource_mut::<Match>() = Match::default();
    if let Some(restore) = restore {
        world.resource_mut::<CustomModels>().wear(restore);
    }
    if let Some(mut puppets) = world.get_resource_mut::<LocalPuppets>() {
        puppets.0.clear();
    }
}

/// The waiting players' puppets onto their slots; at a handover the skater wears the next player's look and starts
/// fresh.
fn hand_over(world: &mut World) {
    let party = world.resource::<PartyPlay>();
    if !party.active {
        return;
    }
    let p = party.controlled;
    let physics = world.get_resource::<crate::physics::GamePhysics>();
    let places: Vec<Option<Transform>> = (0..party.wears.len())
        .map(|i| {
            let at = party.waiting.get(i).copied().flatten().filter(|_| i != p);
            at.map(|m| {
                let m = physics.map_or(m, |physics| waiting_place(physics.world(), m));
                Transform::from_matrix(crate::animation::native_matrix(m))
            })
        })
        .collect();
    let (worn, wear) = (party.worn, party.wears.get(p).cloned().flatten());
    if let Some(mut puppets) = world.get_resource_mut::<LocalPuppets>() {
        for (i, at) in places.into_iter().enumerate() {
            if let Some(puppet) = puppets.0.get_mut(&puppet_id(i)).filter(|q| q.at != at) {
                puppet.at = at;
            }
        }
    }
    if worn == Some(p) {
        return;
    }
    world.resource_mut::<PartyPlay>().worn = Some(p);
    // ponytail: one skater wears each player's look in turn (the puppets hold the same assets, so the swap loads
    // nothing); retail spawns a skater entity per player.
    world.resource_mut::<CustomModels>().wear(wear);
    if world.contains_resource::<crate::physics::SkaterRuntime>() {
        world.resource_scope(|world, mut skater: Mut<crate::physics::SkaterRuntime>| {
            reset_turn(&mut skater, world.get_resource_mut::<crate::camera::CameraRuntime>().map(Mut::into_inner));
        });
    }
}

/// A settled skater's presented root above the face it stands on (its board resting flat, idle): the board's rest
/// height from retail tuning, WheelRadius 0.031 (physicswheels) + the wheel's 0.0565 under the deck origin
/// (physicstrucks TruckYPos). `mode_harness::b3_waiting_puppets_stand_where_a_settled_skater_would` checks it against
/// the physics settings and against skaters settled on every University waiting slot. The suspension's static sag
/// under the rider is under 1 mm there.
// ponytail: one height for every slot (flat waiting areas); a slot on a slope would need the board's tilt too.
pub(crate) const SETTLED_ROOT: f32 = 0.0875;

/// Where a waiting player stands: retail's waiting skaters are physical. RepositionAI teleports each to its
/// TurnBasedWaitingLocation slot: the ground probe (0x82598428: from 0.75 m above the slot, down) puts it on the
/// face under the slot, horizontal_spawn (TU3 0x82DB8998) flattens its At, and it settles on its board there;
/// AI.PinAI (0x8284adc8) only holds its input.
pub(crate) fn waiting_place(world: &skate_core::physics::board_world::BoardWorld, slot: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut m = skate_core::physics::skeleton_animation_record::IDENTITY;
    let (x, z) = (slot[2][0], slot[2][2]);
    let length = x.hypot(z);
    if length > 0.001 {
        m[2] = [x / length, 0., z / length, 0.];
        m[0] = [m[2][2], 0., -m[2][0], 0.];
    }
    m[3] = slot[3];
    if let (_, Some(face)) = crate::skate_world::probe_ground(world, slot) {
        m[3][1] = face + SETTLED_ROOT;
    }
    m
}

/// The handover's reset beyond the turn's teleport (which resets the board, contacts, player state, bail and the
/// scorer's line): a fresh scorer, the rumble, and the camera rig behind the skater.
pub(crate) fn reset_turn(skater: &mut crate::physics::SkaterRuntime, camera: Option<&mut crate::camera::CameraRuntime>) {
    skater.scoring = skater.scoring.fresh();
    crate::skate_audio::player::RUMBLE_RESET.store(true, std::sync::atomic::Ordering::Relaxed);
    if let Some(camera) = camera {
        camera.reset_rig();
    }
}

/// The results dialog's answers: the mode's results, then its end options (A Rematch, B Quit, X Change settings).
// ponytail: GetWinState / Results_GetSelfRank / Results_GetLeaderboardType natives are not traced: a win, no
// highlighted row, type 0.
fn results_answers(mut answers: Vec<(String, Json)>) -> Vec<(String, Json)> {
    let title = answers.iter().find(|(k, _)| k == "Mission.GetResultsTitle").map_or(json!(""), |(_, v)| v.clone());
    answers.extend([
        ("Mission.GetNumEndMissionOptions".to_owned(), json!(3)),
        ("Mission.GetEndMissionOptionText(0)".to_owned(), json!("ID_PARTYPLAY_REMATCH")),
        ("Mission.GetEndMissionOptionText(1)".to_owned(), json!("ID_PARTYPLAY_EXIT_GAME")),
        ("Mission.GetEndMissionOptionText(2)".to_owned(), json!("ID_PARTYPLAY_CHANGE_SETTINGS")),
        ("Mission.GetWinState".to_owned(), json!(true)),
        ("Mission.GetTitle".to_owned(), title),
        ("HUDComponents.Results_GetSelfRank".to_owned(), json!(0)),
        ("HUDComponents.Results_GetLeaderboardType".to_owned(), json!(0)),
    ]);
    answers
}

fn detect(config: Res<Config>, mut party: ResMut<PartyPlay>) {
    party.installed = config.asset_root.join(FOLDER).join("runtime/partyplay.json").is_file();
}

fn answer_all(screen: &mut Screen, answers: Vec<(String, Json)>) {
    for (key, value) in answers {
        screen.answer(&key, value);
    }
}

/// The card's two lines: ID_PARTY_PLAY_ROUND with the round, ID_PARTY_PLAY_PLAYER_READY with the player (the
/// GetRoundText / GetPlayerReadyText bindings format them natively; the movie assigns the result as is).
fn card_answers(language: &Json, player: usize, round: u32) -> [(&'static str, Json); 2] {
    let text = |id: &str, n: String| language[id].as_str().unwrap_or(id).replace("{0:d}", &n);
    [
        ("FEPartyPlay.GetRoundText", json!(text("ID_PARTY_PLAY_ROUND", round.to_string()))),
        ("FEPartyPlay.GetPlayerReadyText", json!(text("ID_PARTY_PLAY_PLAYER_READY", player.to_string()))),
    ]
}

/// perf 10-18: the crossbar's picks (the setup, Exit's popup) open on its own frame; `interact` runs before the
/// crossbar, so they waited a frame there. SKATE_SAME_FRAME_OPEN=0: they do again.
fn open_requested(
    mut party: ResMut<PartyPlay>,
    config: Res<Config>,
    (menu, playable): (Res<crate::graphics_menu::Menu>, Res<crate::match_core::Playable>),
    models: Res<CustomModels>,
    challenge: Option<Res<crate::challenge_map::ChallengeMap>>,
    screens: Option<ResMut<Screens>>,
) {
    if let Some(mut screens) = screens
        && (party.open_setup || party.ask.is_some())
        && crate::config::perf_on("SKATE_SAME_FRAME_OPEN")
    {
        let root = config.asset_root.join(FOLDER);
        open_requests(&mut party, screens.layer(PARTY_PLAY), &root, (&menu, &playable, &models), challenge.as_deref());
    }
}
fn open_requests(
    party: &mut PartyPlay,
    layer: &mut crate::apt_screen::Layer,
    root: &std::path::Path,
    (menu, playable, models): (&crate::graphics_menu::Menu, &crate::match_core::Playable, &CustomModels),
    challenge: Option<&crate::challenge_map::ChallengeMap>,
) {
    if std::mem::take(&mut party.open_setup) {
        // ponytail: only rows a mode plays here (retail lists every registered row).
        let playable = |map: &OnlineMap| places(map).map(|mut list| {
            list.retain(|p| playable.0.contains(&p.challenge.id));
            list
        });
        let places = match challenge.map(|c| c.online_map(&menu.map_labels())) {
            Some(Ok(map)) => playable(&map),
            Some(Err(error)) => {
                warn!("Party Play locations: {error}");
                Default::default()
            }
            None => Default::default(),
        };
        let setup = Setup::new(characters(models.imported()), places);
        let answers = setup.answers();
        let borrowed: Vec<(&str, Json)> = answers.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        // The constructor sets mode 1 itself.
        match layer.open("1-setup", "partyplay", &borrowed).and_then(|s| s.play("intro")) {
            Ok(()) => (party.setup, party.closing) = (Some(setup), None),
            Err(error) => {
                error!("Party Play setup: {error} (prepare it with the launcher's party-play stage)");
                layer.close("1-setup");
            }
        }
    }
    if let Some(ask) = party.ask.take() {
        // perf 10-18: sk8popup parsed once, not on every ask.
        let opened = crate::apt_screen::kept_json(&root.join("runtime/sk8popup.json"))
            .and_then(|json| popup_runtime::Runtime::load(&json, ask.message(), NO_YES, false));
        match opened {
            Ok(runtime) => (party.popup, party.accumulated) = (Some((ask, runtime)), 0.),
            Err(error) => {
                // ponytail: without the popup the pick acts at once (online_menu does the same).
                warn!("Party Play popup unavailable ({error}); acting without asking");
                confirm(party, ask);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn interact(
    mut party: ResMut<PartyPlay>,
    (keys, nav, config): (Res<ButtonInput<KeyCode>>, Res<Navigation>, Res<Config>),
    (menu, playable): (Res<crate::graphics_menu::Menu>, Res<crate::match_core::Playable>),
    models: Res<CustomModels>,
    challenge: Option<Res<crate::challenge_map::ChallengeMap>>,
    screens: Option<ResMut<Screens>>,
    mut sounds: MessageWriter<FrontEndSoundRequest>,
    real: Res<Time<Real>>,
) {
    let party = &mut *party;
    party.closed_this_frame = false;
    let Some(mut screens) = screens else {
        // No front end: a card is answered at once.
        party.hud_complete |= party.card.take().is_some();
        (party.open_setup, party.ask) = (false, None);
        return;
    };
    let layer = screens.layer(PARTY_PLAY);
    let root = config.asset_root.join(FOLDER);
    let pressed: Vec<Key> = [
        (nav.pressed & 1 != 0, Key::Up),
        (nav.pressed & 2 != 0, Key::Down),
        (nav.pressed & 4 != 0, Key::Left),
        (nav.pressed & 8 != 0, Key::Right),
        (keys.just_pressed(KeyCode::Enter) || nav.pressed & 0x1000 != 0, Key::Accept),
        (keys.just_pressed(KeyCode::Backspace) || nav.pressed & 0x2000 != 0, Key::Back),
    ]
    .into_iter()
    .filter_map(|(down, key)| down.then_some(key))
    .collect();

    // Requests.
    open_requests(party, layer, &root, (&menu, &playable, &models), challenge.as_deref());
    if let Some((player, round)) = party.card.take() {
        // perf 10-18: parsed once (each round's card read it twice: here and the open).
        let opened = crate::apt_screen::kept_json(&root.join("runtime/partyplay_start.json")).and_then(|json| {
            let answers = card_answers(&json["language"], player, round);
            let screen = layer.open("2-card", "partyplay_start", &answers)?;
            screen.play("intro")
        });
        match opened {
            Ok(()) => party.card_shown = Some(None),
            Err(error) => {
                // Never strand a turn: the card is answered at once.
                error!("Party Play card: {error}");
                layer.close("2-card");
                party.hud_complete = true;
            }
        }
    }

    // The results dialog (OpenContinueRetryDialog: endchallengeoptions, FE slot 4) while the mode's results wait.
    match (party.results.as_ref(), party.results_open) {
        (Some(answers), false) => {
            let answers = results_answers(answers.clone());
            let borrowed: Vec<(&str, Json)> = answers.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
            // The constructor plays its intro and sets mode 6 itself.
            if let Err(error) = layer.open("0-results", "endchallengeoptions", &borrowed) {
                // Its options still answer (never strand the session).
                error!("Party Play results: {error} (prepare it with the launcher's party-play stage)");
                layer.close("0-results");
            }
            party.results_open = true;
        }
        (None, true) => {
            layer.close("0-results");
            party.results_open = false;
        }
        _ => {}
    }
    let change_settings = keys.just_pressed(KeyCode::KeyX) || nav.pressed & 0x4000 != 0;

    // Input: the popup, else the card, else the setup, else the results dialog.
    if let Some((ask, runtime)) = party.popup.as_mut() {
        let ask = *ask;
        let answer = pressed.iter().find_map(|&key| {
            let input = runtime.input(key);
            sounds.write_batch(runtime.bindings.sounds.drain(..).map(FrontEndSoundRequest));
            input.transpose()
        });
        match answer {
            Some(Ok(1)) => {
                party.popup = None;
                party.closed_this_frame = true;
                confirm(party, ask);
            }
            Some(Ok(_)) => (party.popup, party.closed_this_frame) = (None, true),
            Some(Err(error)) => {
                error!("Party Play popup stopped: {error}");
                (party.popup, party.closed_this_frame) = (None, true);
            }
            None => {}
        }
    } else if let Some(shown) = party.card_shown.as_mut() {
        match (shown.as_mut(), layer.get("2-card")) {
            (_, None) => {
                // The screen stopped (apt_screen logged it): answer the card.
                (party.card_shown, party.hud_complete, party.closed_this_frame) = (None, true, true);
            }
            (Some(closing), Some(screen)) => {
                *closing += real.delta_secs();
                if screen.bindings.outro_complete || *closing >= CLOSE_TIMEOUT {
                    layer.close("2-card");
                    (party.card_shown, party.closed_this_frame) = (None, true);
                }
            }
            // 0x8260fda0: only A, which plays core_a_button, pops the card and posts hudcomplete. B is ignored.
            (None, Some(screen)) => {
                if pressed.contains(&Key::Accept) {
                    sounds.write(FrontEndSoundRequest(SOUND_A_BUTTON));
                    party.hud_complete = true;
                    *shown = Some(0.);
                    if let Err(error) = screen.play("outro") {
                        warn!("Party Play card: {error}");
                    }
                }
            }
        }
    } else if party.setup.is_some() {
        setup_input(party, layer, &pressed, &mut sounds, real.delta_secs());
    } else if party.results_open {
        // 0x825f13f8: Rematch (2) asks popup 217, ExitPartyPlay (16) popup 213, AdjustSettings (17) pushes the setup
        // over the results (on its defaults, D11; B there comes back here).
        // ponytail: the dialog's own button sounds are not traced; core_a_button / core_b_button.
        if pressed.contains(&Key::Accept) {
            sounds.write(FrontEndSoundRequest(SOUND_A_BUTTON));
            party.ask = Some(Ask::Rematch);
        } else if pressed.contains(&Key::Back) {
            sounds.write(FrontEndSoundRequest(SOUND_B_BUTTON));
            party.ask = Some(Ask::Exit);
        } else if change_settings {
            sounds.write(FrontEndSoundRequest(SOUND_A_BUTTON));
            party.open_setup = true;
        }
    }
    party.modal = party.setup.is_some() || party.card_shown.is_some() || party.popup.is_some() || party.results_open;
    layer.modal = party.modal;
}

/// The popup's Yes.
fn confirm(party: &mut PartyPlay, ask: Ask) {
    party.confirmed = Some(ask);
    // Retail's Exit closes the crossbar (0x8261afa0).
    party.close_crossbar |= ask == Ask::Exit;
}

fn setup_input(
    party: &mut PartyPlay,
    layer: &mut crate::apt_screen::Layer,
    pressed: &[Key],
    sounds: &mut MessageWriter<FrontEndSoundRequest>,
    dt: f32,
) {
    let Some(screen) = layer.get("1-setup") else {
        (party.setup, party.closing, party.closed_this_frame) = (None, None, true);
        return;
    };
    if let Some(closing) = party.closing.as_mut() {
        *closing += dt;
        if screen.bindings.outro_complete || *closing >= CLOSE_TIMEOUT {
            layer.close("1-setup");
            (party.setup, party.closing, party.closed_this_frame) = (None, None, true);
        }
        return;
    }
    let setup = party.setup.as_mut().expect("checked by the caller");
    let mut exit = None;
    let result = (|| -> Result<(), String> {
        for &key in pressed {
            let step = setup.input(key);
            sounds.write_batch(step.sounds.iter().copied().map(FrontEndSoundRequest));
            answer_all(screen, setup.answers());
            for state in &step.states {
                screen.call("SetState", &[json!(state)])?;
            }
            if step.exit.is_some() {
                exit = step.exit;
                break;
            }
        }
        Ok(())
    })();
    sounds.write_batch(screen.bindings.sounds.drain(..).map(|key| FrontEndSoundRequest(fe_sound_id(&key))));
    match (result, exit) {
        (Err(error), _) => {
            error!("Party Play setup stopped: {error}");
            layer.close("1-setup");
            (party.setup, party.closed_this_frame) = (None, true);
        }
        // Retail pops the state; the movie plays its outro.
        (Ok(()), Some(Exit::Back)) => {
            party.closing = Some(0.);
            if let Err(error) = screen.play("outro") {
                warn!("Party Play setup: {error}");
            }
        }
        // 0x8260eb78 case 4: the FE closes and the match starts.
        (Ok(()), Some(Exit::Launch)) => {
            party.launch = setup.launch();
            layer.close("1-setup");
            (party.setup, party.closed_this_frame, party.close_crossbar) = (None, true, true);
        }
        (Ok(()), None) => {}
    }
}

/// The location page's photo, and advances and draws the confirm popup (on this layer, after its screens).
fn draw(
    mut party: ResMut<PartyPlay>,
    screens: Option<ResMut<Screens>>,
    real: Res<Time<Real>>,
    (menu, challenge): (Res<crate::graphics_menu::Menu>, Option<Res<crate::challenge_map::ChallengeMap>>),
) {
    let Some(mut screens) = screens else { return };
    let layer = screens.layer(PARTY_PLAY);
    let party = &mut *party;
    // Not retail: the highlighted location's photo (its online proposal card's), looked up when the row changes.
    let row = (party.setup.as_ref().filter(|_| party.closing.is_none()).and_then(shown)).map(|(tab, id)| (tab, id.to_owned()));
    if row != party.photo {
        let found = row.as_ref().and_then(|(tab, id)| challenge.as_ref()?.proposal_card(&menu.map_labels(), *tab, id)?.photo);
        match found {
            Some((path, size)) => drop(layer.extra.insert("y-photo", photo(&path, size))),
            None => drop(layer.extra.remove("y-photo")),
        }
        party.photo = row;
    }
    let Some((_, runtime)) = party.popup.as_mut() else {
        layer.extra.remove("z-popup");
        return;
    };
    party.accumulated = (party.accumulated + real.delta_secs()).min(4. * FRAME);
    let mut result = Ok(());
    while party.accumulated >= FRAME && result.is_ok() {
        party.accumulated -= FRAME;
        result = runtime.advance();
    }
    match result.and_then(|()| crate::apt_scene::draw(&runtime.bindings.movie, &runtime.vm, &runtime.bindings.shapes)) {
        Ok(draws) => {
            layer.extra.insert("z-popup", draws);
        }
        Err(error) => {
            error!("Party Play popup stopped: {error}");
            party.popup = None;
            layer.extra.remove("z-popup");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn place(title: &str) -> Place {
        Place { title: title.into(), challenge: Challenge { id: title.to_lowercase(), ..Default::default() }, ..Default::default() }
    }
    fn setup() -> Setup {
        let places = [vec![place("A0"), place("A1"), place("A2")], vec![place("B0")], vec![], vec![place("D0"), place("D1")]];
        Setup::new(characters([("m1", "Robot"), ("m2", "Alien")]), places)
    }
    fn press(setup: &mut Setup, key: Key) -> Step {
        setup.input(key)
    }

    #[test]
    fn own_skaters_come_first_then_the_retail_picks() {
        let list = characters([("m1", "Robot")]);
        assert_eq!(list.len(), 2 + 33);
        assert_eq!(list[0], Character::MySkater);
        assert_eq!(list[1].label(), "#Robot");
        assert_eq!(list[2].label(), "ID_COMMON_MIKE_CARROLL");
        assert_eq!(list[34].label(), "ID_COMMON_DENNIS_BUSENTIZ");
    }

    #[test]
    fn page_0_counts_players_between_2_and_4() {
        let mut s = setup();
        assert_eq!((s.players, s.page, s.active, s.kind, s.location, s.picks), (2, 0, 0, 0, 0, [0; 4]));
        assert_eq!(press(&mut s, Key::Left), Step::default(), "2 is the minimum: silent");
        assert_eq!(press(&mut s, Key::Right), Step { sounds: vec![SOUND_NAV_RIGHT], states: vec![2], exit: None });
        press(&mut s, Key::Right);
        assert_eq!(s.players, 4);
        assert_eq!(press(&mut s, Key::Right), Step::default(), "4 is the maximum: silent");
        assert_eq!(press(&mut s, Key::Left), Step { sounds: vec![SOUND_NAV_LEFT], states: vec![2], exit: None });
        assert_eq!(press(&mut s, Key::Up), Step::default());
        assert_eq!(press(&mut s, Key::Accept), Step { sounds: vec![SOUND_A_BUTTON], states: vec![3], exit: None });
        assert_eq!(s.page, 1);
        let mut back = setup();
        assert_eq!(press(&mut back, Key::Back), Step { sounds: vec![SOUND_B_BUTTON], states: vec![], exit: Some(Exit::Back) });
    }

    #[test]
    fn page_1_picks_characters_per_player() {
        let mut s = setup();
        press(&mut s, Key::Accept);
        // Right: nav_right twice. Left: retail's nav_right then nav_left.
        assert_eq!(press(&mut s, Key::Right), Step { sounds: vec![SOUND_NAV_RIGHT, SOUND_NAV_RIGHT], states: vec![4], exit: None });
        assert_eq!(s.picks, [1, 0, 0, 0]);
        press(&mut s, Key::Left);
        assert_eq!(press(&mut s, Key::Left), Step { sounds: vec![SOUND_NAV_RIGHT, SOUND_NAV_LEFT], states: vec![4], exit: None });
        assert_eq!(s.picks[0], s.characters.len() - 1, "wraps to the last entry");
        assert_eq!(press(&mut s, Key::Down), Step { sounds: vec![SOUND_NAV_DOWN], states: vec![5], exit: None });
        assert_eq!(s.active, 1);
        press(&mut s, Key::Right);
        assert_eq!(s.picks[1], 1, "player 2's own pick");
        press(&mut s, Key::Down);
        assert_eq!(s.active, 0, "wraps at the player count");
        assert_eq!(press(&mut s, Key::Up), Step { sounds: vec![SOUND_NAV_UP], states: vec![5], exit: None });
        assert_eq!(s.active, 1);
        let names = s.answers();
        assert!(names.contains(&("FEPartyPlay.GetPlayerName(1)".to_owned(), json!("#Robot"))));
        assert!(names.contains(&("FEPartyPlay.GetPlayerIndex".to_owned(), json!(1))));
        // Duplicates are allowed: player 1 steps from the last entry round to player 2's.
        press(&mut s, Key::Up);
        press(&mut s, Key::Right);
        press(&mut s, Key::Right);
        assert_eq!(&s.picks[..2], &[1, 1]);
        assert_eq!(press(&mut s, Key::Back), Step { sounds: vec![SOUND_B_BUTTON], states: vec![1], exit: None });
        assert_eq!(s.page, 0);
    }

    #[test]
    fn page_2_cycles_the_four_types_and_page_3_the_locations() {
        let mut s = setup();
        press(&mut s, Key::Accept);
        assert_eq!(press(&mut s, Key::Accept), Step { sounds: vec![SOUND_A_BUTTON], states: vec![6], exit: None });
        assert_eq!(press(&mut s, Key::Left), Step { sounds: vec![SOUND_NAV_LEFT], states: vec![7], exit: None });
        assert_eq!(s.kind, 3, "0 wraps to 1-UP");
        assert_eq!(press(&mut s, Key::Right), Step { sounds: vec![SOUND_NAV_RIGHT], states: vec![7], exit: None });
        assert_eq!(s.kind, 0);
        assert_eq!(press(&mut s, Key::Back), Step { sounds: vec![SOUND_B_BUTTON], states: vec![3], exit: None });
        press(&mut s, Key::Accept);
        assert_eq!(press(&mut s, Key::Accept), Step { sounds: vec![SOUND_A_BUTTON], states: vec![8], exit: None });
        assert_eq!(s.page, 3);
        assert_eq!(press(&mut s, Key::Left), Step { sounds: vec![SOUND_NAV_LEFT, SOUND_NAV_LEFT], states: vec![9], exit: None });
        assert_eq!(s.location, 2, "wraps to the last location");
        assert_eq!(press(&mut s, Key::Right), Step { sounds: vec![SOUND_NAV_RIGHT, SOUND_NAV_RIGHT], states: vec![9], exit: None });
        assert_eq!(s.location, 0);
        press(&mut s, Key::Right);
        assert!(s.answers().contains(&("FEPartyPlay.GetPartyPlayLocation".to_owned(), json!("A1"))));
        // B, another type, A: the list restarts at its first location.
        assert_eq!(press(&mut s, Key::Back), Step { sounds: vec![SOUND_B_BUTTON], states: vec![6], exit: None });
        press(&mut s, Key::Left);
        press(&mut s, Key::Accept);
        assert_eq!((s.kind, s.location), (3, 0));
        assert_eq!(press(&mut s, Key::Accept), Step { sounds: vec![SOUND_A_BUTTON], states: vec![], exit: Some(Exit::Launch) });
        assert_eq!(press(&mut s, Key::Back), Step::default(), "busy after launch");
        let launch = s.launch().unwrap();
        assert_eq!((launch.kind, launch.place.title.as_str(), launch.players.len()), (Kind::OneUp, "D0", 2));
        assert_eq!(launch.players[0], Character::MySkater, "D9: every player starts on the first entry");
    }

    #[test]
    fn an_empty_location_list_reads_invalid_and_refuses_a() {
        let mut s = setup();
        s.kind = 2;
        s.page = 3;
        assert!(s.answers().contains(&("FEPartyPlay.GetPartyPlayLocation".to_owned(), json!("#Invalid Title"))));
        assert_eq!(press(&mut s, Key::Accept), Step::default());
        press(&mut s, Key::Right);
        press(&mut s, Key::Left);
        assert_eq!(s.location, 0);
    }

    #[test]
    fn card_text_is_formatted() {
        let language = json!({"ID_PARTY_PLAY_ROUND": "Round {0:d}", "ID_PARTY_PLAY_PLAYER_READY": "Player {0:d} ready! Press <B:A> to begin."});
        let [round, ready] = card_answers(&language, 3, 2);
        assert_eq!(round.1, json!("Round 2"));
        assert_eq!(ready.1, json!("Player 3 ready! Press <B:A> to begin."));
    }

    // ------------------------------------------------------------------------------------------- the session
    fn world() -> World {
        let mut world = World::new();
        world.init_resource::<PartyPlay>();
        world.init_resource::<Match>();
        world.init_resource::<LocalPuppets>();
        let mut models = CustomModels::default();
        models.active = Some("mine".into());
        world.insert_resource(models);
        world
    }
    fn launch(kind: Kind, id: &str) -> Launch {
        let mut place = place(id);
        place.challenge.map = "University".into();
        let players = vec![
            Character::MySkater,
            Character::Pro { key: "mike_carroll", name: "ID_COMMON_MIKE_CARROLL" },
            Character::Imported { id: "m1".into(), name: "Robot".into() },
        ];
        Launch { kind, place, players }
    }
    fn party(world: &World) -> &PartyPlay {
        world.resource::<PartyPlay>()
    }

    #[test]
    fn exchange_carries_the_card_and_the_results_and_brings_back_the_answers() {
        let turn = |card, results| Turn { controlled: 1, waiting: vec![], card, results };
        // No front end: the card is answered at once.
        assert_eq!(exchange(&mut World::new(), turn(Some((1, 1)), None)), Answers { hud_complete: true, ..default() });
        let mut world = world();
        assert_eq!(exchange(&mut world, turn(Some((2, 3)), None)), Answers::default());
        assert_eq!((party(&world).card, party(&world).controlled), (Some((2, 3)), 1));
        world.resource_mut::<PartyPlay>().hud_complete = true;
        let answers = exchange(&mut world, turn(None, None));
        assert_eq!(answers.messages(), [crate::skate3_modes::Msg::HudComplete]);
        // The results open once; Rematch (or Restart Challenge) takes them down.
        let first = vec![("Mission.GetResultsTitle".to_owned(), json!("one"))];
        exchange(&mut world, turn(None, Some(first.clone())));
        exchange(&mut world, turn(None, Some(vec![])));
        assert_eq!(party(&world).results, Some(first));
        world.resource_mut::<PartyPlay>().confirmed = Some(Ask::Rematch);
        session(&mut world);
        let answers = exchange(&mut world, turn(None, Some(vec![])));
        assert_eq!(answers.messages(), [crate::skate3_modes::Msg::HudRetry]);
        assert_eq!(party(&world).results, None);
        world.resource_mut::<PartyPlay>().restart = true;
        assert_eq!(exchange(&mut world, turn(None, None)).messages(), [crate::skate3_modes::Msg::Restart]);
    }

    /// v12 list 2: leaving the session with "Press A to begin your turn" up took the card down with it; it stayed,
    /// waiting on an A no turn would take.
    #[test]
    fn leaving_with_the_card_up_takes_the_card_down() {
        let mut world = world();
        world.resource_mut::<PartyPlay>().launch = Some(launch(Kind::Skate, "skate_univ_02"));
        session(&mut world);
        world.resource_mut::<PartyPlay>().card_shown = Some(None);
        world.resource_mut::<PartyPlay>().confirmed = Some(Ask::Exit);
        session(&mut world);
        assert!(!party(&world).active);
        assert_eq!(party(&world).card_shown, None, "the card went with the session");
    }

    #[test]
    fn a_session_preloads_every_look_hands_over_and_quit_restores_my_skater() {
        let mut world = world();
        assert_eq!(names(&world), ["Player 1", "Player 2"]);
        world.resource_mut::<PartyPlay>().launch = Some(launch(Kind::Skate, "skate_univ_02"));
        session(&mut world);
        let m = world.resource::<Match>();
        assert_eq!((m.mode, m.row.as_str()), (Some(Mode::Skate), "skate_univ_02"));
        assert!(party(&world).active);
        assert_eq!(names(&world), ["Player 1", "Player 2", "Player 3"]);
        // Every player's puppet, hidden, from the start (their looks load now, not at a handover).
        let puppets = &world.resource::<LocalPuppets>().0;
        assert_eq!(puppets.len(), 3);
        assert!(puppets.values().all(|p| p.at.is_none()));
        assert!(matches!(&puppets[&puppet_id(1)].look, Look::Native(key) if key == "mike_carroll"));
        // Player 1 is My Skater: the skater wears it, not saved.
        assert_eq!((party(&world).worn, world.resource::<CustomModels>().requested()), (Some(0), Some(None)));
        world.resource_mut::<CustomModels>().active = None; // the wear applied

        // Player 2's turn: player 1 and 3 wait on their slots.
        let slot = |x: f32| {
            let mut m = [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [0., 0., 0., 1.]];
            m[3][0] = x;
            m
        };
        exchange(&mut world, Turn { controlled: 1, waiting: vec![Some(slot(4.)), None, Some(slot(6.))], card: Some((2, 1)), results: None });
        session(&mut world);
        assert_eq!(party(&world).worn, Some(1));
        let puppets = &world.resource::<LocalPuppets>().0;
        assert_eq!(puppets[&puppet_id(0)].at.map(|t| t.translation.x), Some(4.));
        assert_eq!(puppets[&puppet_id(1)].at, None, "the pad's player is the skater itself");
        assert_eq!(puppets[&puppet_id(2)].at.map(|t| t.translation.x), Some(6.));

        // Quit (the results' B or the crossbar's Exit Party Play, both popup 213's Yes).
        world.resource_mut::<PartyPlay>().confirmed = Some(Ask::Exit);
        session(&mut world);
        assert!(world.resource::<LocalPuppets>().0.is_empty(), "every puppet removed");
        assert!(!party(&world).active && party(&world).players.is_empty());
        assert_eq!(world.resource::<Match>().mode, None);
        assert_eq!(world.resource::<CustomModels>().requested(), Some(Some("mine".into())), "the look before the session");
    }

    #[test]
    fn change_settings_relaunches_and_backing_out_then_quitting_still_restores() {
        let mut world = world();
        world.resource_mut::<PartyPlay>().launch = Some(launch(Kind::Skate, "skate_univ_02"));
        session(&mut world);
        world.resource_mut::<CustomModels>().active = Some("pro-entry".into()); // a player's look worn
        exchange(&mut world, Turn { controlled: 0, waiting: vec![], card: None, results: Some(vec![]) });
        // Change settings, then a launch: the running match is dropped first, so its mode ends.
        world.resource_mut::<PartyPlay>().launch = Some(launch(Kind::SpotBattle, "spot_univ_01"));
        session(&mut world);
        assert_eq!(world.resource::<Match>().mode, None);
        assert_eq!(party(&world).results, None);
        world.resource_mut::<PartyPlay>().restart = true; // Restart Challenge picked while nothing exchanged (M4)
        session(&mut world);
        assert!(!party(&world).restart, "not carried into the new match");
        let m = world.resource::<Match>();
        assert_eq!((m.mode, m.row.as_str()), (Some(Mode::SpotBattle), "spot_univ_01"));
        assert_eq!(world.resource::<LocalPuppets>().0.len(), 3);
        // Change settings and B back to the results (retail), then Quit: the look from before the first launch.
        world.resource_mut::<PartyPlay>().confirmed = Some(Ask::Exit);
        session(&mut world);
        assert!(world.resource::<LocalPuppets>().0.is_empty());
        assert_eq!(world.resource::<CustomModels>().requested(), Some(Some("mine".into())));
    }

    /// I2: My Skater's puppet is the saved profile, whatever custom model is worn (Parts.applied is Null then).
    #[test]
    fn my_skaters_puppet_wears_the_saved_outfit() {
        let draft = json!({"gender": "male", "selections": {"hat": {"asset_id": "h1", "material_id": "m1"}}});
        assert!(matches!(my_skater(Some(&draft)), Look::Outfit(d) if d == draft));
        assert!(matches!(my_skater(Some(&json!({"selections": {}}))), Look::Stock), "nothing chosen: the stock skater");
        assert!(matches!(my_skater(None), Look::Stock));
    }

    /// M2: Quit with no session (`SKATE_MODE` solo) still drops the match and the dialog, and wears nothing.
    #[test]
    fn quit_without_a_session_drops_the_match() {
        let mut world = world();
        world.resource_mut::<Match>().start(Mode::Skate, "skate_univ_02", 0.);
        exchange(&mut world, Turn { controlled: 0, waiting: vec![], card: None, results: Some(vec![]) });
        world.resource_mut::<PartyPlay>().confirmed = Some(Ask::Exit);
        session(&mut world);
        assert_eq!((world.resource::<Match>().mode, party(&world).results.is_none()), (None, true));
        assert_eq!(world.resource::<CustomModels>().requested(), None, "the worn model stays");
    }

    #[test]
    fn the_results_dialog_offers_rematch_quit_and_change_settings() {
        let answers = results_answers(vec![("Mission.GetResultsTitle".to_owned(), json!("Spot"))]);
        let get = |k: &str| answers.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone());
        assert_eq!(get("Mission.GetNumEndMissionOptions"), Some(json!(3)));
        assert_eq!(get("Mission.GetEndMissionOptionText(0)"), Some(json!("ID_PARTYPLAY_REMATCH")));
        assert_eq!(get("Mission.GetEndMissionOptionText(1)"), Some(json!("ID_PARTYPLAY_EXIT_GAME")));
        assert_eq!(get("Mission.GetEndMissionOptionText(2)"), Some(json!("ID_PARTYPLAY_CHANGE_SETTINGS")));
        assert_eq!(get("Mission.GetTitle"), Some(json!("Spot")));
    }

    #[test]
    fn versus_folders_feed_the_types() {
        let file = json!({"format": "skate3-online-map", "version": 1, "tabs": [
            {"name": "ID_TAG_CAREERMODE", "rows": []},
            {"name": VERSUS, "rows": [
                {"name": "ID_MAP_FILTER_CHALLENGETYPES_SKATE", "folder": true, "rows": [
                    {"name": "ID_MAP_HEADER_VERSUS_OPTIONS", "header": true},
                    {"name": "ID_MAP_COMMAND_QUICKMATCH", "command": "x"},
                    {"name": "ID_SK8", "challenge": {"id": "skate_univ_02", "map": "University"}},
                    {"name": "#Gone", "challenge": {"id": "skate_x", "map": "Elsewhere"}}
                ]},
                {"name": "ID_MAP_FILTER_CHALLENGETYPES_1UP", "folder": true, "rows": [
                    {"name": "ID_BBPK", "challenge": {"id": "1up_bbpk_01", "map": "University"}},
                    {"name": "ID_A", "challenge": {"id": "1up_a", "map": "University"}},
                    {"name": "ID_B", "challenge": {"id": "1up_b", "map": "University"}}
                ]}
            ]}
        ]});
        let map = OnlineMap::from_json(&file.to_string(), &["University".to_owned()]).unwrap();
        let places = places(&map);
        assert!(places[0].is_empty() && places[2].is_empty());
        assert_eq!(places[1].iter().map(|p| p.challenge.id.as_str()).collect::<Vec<_>>(), ["skate_univ_02"], "installed maps only");
        let ids: Vec<&str> = places[3].iter().map(|p| p.challenge.id.as_str()).collect();
        assert_eq!(ids, ["1up_a", "1up_b", "1up_bbpk_01"], "I3: retail adds the BBPK 1-UP last");
    }

    /// Not retail: the location page shows the highlighted row's photo; the other pages and an empty list show none.
    #[test]
    fn the_location_page_shows_the_highlighted_rows_photo() {
        let mut s = setup();
        for (i, list) in s.places.iter_mut().enumerate() {
            list.iter_mut().for_each(|p| p.tab = i + 3);
        }
        assert_eq!(shown(&s), None, "page 0");
        press(&mut s, Key::Accept);
        press(&mut s, Key::Accept);
        assert_eq!(shown(&s), None, "the type page");
        press(&mut s, Key::Accept);
        assert_eq!(shown(&s), Some((3, "a0")));
        press(&mut s, Key::Right);
        assert_eq!(shown(&s), Some((3, "a1")), "follows the highlight");
        press(&mut s, Key::Back);
        press(&mut s, Key::Left);
        press(&mut s, Key::Accept);
        assert_eq!(shown(&s), Some((6, "d0")), "the type's own rows, its own tab");
        press(&mut s, Key::Back);
        press(&mut s, Key::Left);
        press(&mut s, Key::Accept);
        assert_eq!(shown(&s), None, "no rows: no photo");
        // A 2:1 photo keeps its shape; a square map picture fills the slot from its middle.
        let wide = photo("C:/p.rgba", [512, 256]);
        let [x0, y0, x1, y1] = bounds(&wide[0]);
        assert!(((x1 - x0) / (y1 - y0) - 2.).abs() < 0.01, "{:?}", (x0, y0, x1, y1));
        let square = photo("C:/p.rgba", [256, 256]);
        assert_eq!(bounds(&square[0]), [x0, y0, x1, y1], "the same slot");
        assert!(square[0].vertices.iter().any(|v| (v.uv[1] - 0.25).abs() < 1e-4), "cropped to its middle half");
        assert_eq!(square[0].texture, "C:/p.rgba");
    }
    fn bounds(d: &crate::apt_scene::Draw) -> [f32; 4] {
        let xs = d.vertices.iter().map(|v| v.position[0]);
        let ys = d.vertices.iter().map(|v| v.position[1]);
        [xs.clone().fold(f32::MAX, f32::min), ys.clone().fold(f32::MAX, f32::min), xs.fold(f32::MIN, f32::max), ys.fold(f32::MIN, f32::max)]
    }

    #[test]
    fn skate_1_and_2_rows_follow_skate_3s_under_each_type() {
        let folder = |kind: Kind, rows: serde_json::Value| json!({"name": kind.folder(), "folder": true, "rows": rows});
        let row = |name: &str, id: &str, map: &str| json!({"name": name, "challenge": {"id": id, "map": map}});
        let file = json!({"format": "skate3-online-map", "version": 1, "tabs": [
            {"name": VERSUS, "rows": [
                folder(Kind::SpotBattle, json!([row("ID_OTS", "ots_univ_01_ol", "University")])),
                folder(Kind::OneUp, json!([row("ID_BBPK", "1up_bbpk_01", "University"), row("ID_A", "1up_a", "University")]))
            ]},
            {"name": SKATE_2, "rows": [
                folder(Kind::SpotBattle, json!([
                    {"name": "ID_MAP_HEADER_VERSUS_OPTIONS", "header": true},
                    {"name": "ID_MAP_COMMAND_QUICKMATCH", "command": "x"},
                    row("#Books", "f_1a_01_ots_pj01_books", "New_San_Vanelona")
                ])),
                folder(Kind::Skate, json!([row("#Alphabet Yard", "t_1b_01_skate_alphabet", "New_San_Vanelona")])),
                folder(Kind::HallOfMeat, json!([row("#Fence", "ol_hom_01_pj05_fence", "New_San_Vanelona")])),
                folder(Kind::OneUp, json!([row("#Mountain Up", "ol_1up_01_svm_mountainup", "New_San_Vanelona")]))
            ]},
            {"name": SKATE_1, "rows": [
                folder(Kind::SpotBattle, json!([row("#Junk Jiving", "ol_fp_u1_ownspot01", "Skate_1_San_Vanelona")])),
                folder(Kind::Skate, json!([row("#Parkade", "skate_financial_parkade", "Skate_1_San_Vanelona")])),
                {"name": "ID_MAP_FILTER_CHALLENGETYPES_JAM", "folder": true, "rows": [row("#Jam", "jam_financial_parkade", "Skate_1_San_Vanelona")]}
            ]}
        ]});
        let installed = ["University", "New San Vanelona", "Skate 1 San Vanelona"].map(String::from);
        let map = OnlineMap::from_json(&file.to_string(), &installed).unwrap();
        let places = places(&map);
        let list = |k: usize| places[k].iter().map(|p| (p.title.as_str(), p.challenge.id.as_str())).collect::<Vec<_>>();
        assert_eq!(
            list(0),
            [("ID_OTS", "ots_univ_01_ol"), ("#Books (Skate 2)", "f_1a_01_ots_pj01_books"), ("#Junk Jiving (Skate 1)", "ol_fp_u1_ownspot01")]
        );
        assert_eq!(list(1), [("#Alphabet Yard (Skate 2)", "t_1b_01_skate_alphabet"), ("#Parkade (Skate 1)", "skate_financial_parkade")]);
        assert_eq!(list(2), [("#Fence (Skate 2)", "ol_hom_01_pj05_fence")], "Skate 1 has no Hall of Meat");
        let ids: Vec<&str> = list(3).into_iter().map(|(_, id)| id).collect();
        assert_eq!(ids, ["1up_a", "1up_bbpk_01"], "the BBPK 1-UP last of Skate 3's; Skate 2 had no 1-Up (user 11-01), nor Skate 1");
        // Each row knows its tab (the photo's lookup).
        let tab = |name: &str| map.tabs.iter().position(|t| t.name == name).unwrap();
        assert_eq!(places[0].iter().map(|p| p.tab).collect::<Vec<_>>(), [tab(VERSUS), tab(SKATE_2), tab(SKATE_1)]);
        assert!(map.tabs[tab(SKATE_2)].rows.iter().all(|e| e.name != Kind::OneUp.folder()), "nor the map's Skate 2 tab");
        let no_skate2 = OnlineMap::from_json(&file.to_string(), &["University".to_owned()]).unwrap();
        assert_eq!(super::places(&no_skate2)[1].len(), 0, "an uninstalled map's rows are pruned");
    }

    #[test]
    fn standings_rank_the_players_and_point_at_player_1() {
        let rows = [(1, "Player 2".to_owned(), 1, 0), (0, "Player 1".to_owned(), 2, 3)];
        let answers = standings("#Books", &rows, "ID_COMMON_SCORE", 2);
        let get = |k: &str| answers.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone());
        assert_eq!(get("Mission.GetResultsTitle"), Some(json!("#Books")));
        assert_eq!(get("HUDComponents.Results_GetNumPlayers"), Some(json!(2)));
        assert_eq!(get("Mission.GetCurrentPlayerPosition"), Some(json!(2)));
        assert_eq!(get("HUDComponents.Results_GetThrowdownType"), Some(json!(2)));
        let block = get("HUDComponents.Results_GetLeaderboardDataBlock").unwrap();
        assert_eq!(block[0], json!(1));
        assert_eq!(block[7], json!("#Player 2"));
        assert_eq!((block[8].clone(), block[9].clone(), block[15].clone()), (json!(2), json!("#3"), json!("#Player 1")));
        // Dialog-ready: the party options go on top of it.
        assert_eq!(results_answers(answers).iter().find(|(k, _)| k == "Mission.GetTitle").map(|(_, v)| v.clone()), Some(json!("#Books")));
    }

    // ------------------------------------------------- the prepared screens (SKATE3_ASSET_ROOT/private/party-play)
    fn root() -> Option<PathBuf> {
        let root = PathBuf::from(std::env::var_os("SKATE3_ASSET_ROOT")?).join(FOLDER);
        root.join("runtime/partyplay.json").is_file().then_some(root)
    }
    fn run(screen: &mut Screen, frames: usize) {
        (0..frames).for_each(|_| screen.advance().unwrap());
    }
    fn texts(screen: &Screen) -> Vec<String> {
        let mut out: Vec<String> = (screen.bindings.movie.instances.keys())
            .filter(|&&id| screen.vm.get(id, "_visible") != crate::apt_vm::Value::Bool(false))
            .filter_map(|&id| match screen.vm.get(id, "_displayText") {
                crate::apt_vm::Value::Text(t) if !t.is_empty() => Some(t),
                _ => None,
            })
            .collect();
        out.sort();
        out
    }
    /// SKATE_PARTY_SHOTS=<dir>: PNGs of each step.
    fn shoot(draws: &[crate::apt_scene::Draw], root: &std::path::Path, name: &str) {
        if let Some(out) = std::env::var_os("SKATE_PARTY_SHOTS").map(PathBuf::from) {
            crate::core_menu_runtime::tests::raster_draws(draws, root, &out.join(format!("{name}.png")));
        }
    }
    fn drive(screen: &mut Screen, s: &mut Setup, key: Key) {
        let step = s.input(key);
        answer_all(screen, s.answers());
        for state in step.states {
            screen.call("SetState", &[json!(state)]).unwrap();
        }
        run(screen, 20);
    }
    fn has(shown: &[String], want: &str) -> bool {
        shown.iter().any(|t| t == want)
    }

    #[test]
    fn setup_screen_runs_its_four_pages() {
        let Some(root) = root() else { return };
        let mut s = setup();
        let answers = s.answers();
        let borrowed: Vec<(&str, Json)> = answers.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        let mut screen = Screen::load(&root.join("runtime/partyplay.json"), &borrowed).unwrap();
        screen.play("intro").unwrap();
        run(&mut screen, 40);
        let shown = texts(&screen);
        for want in ["Party Play Settings", "Select number of players", "2"] {
            assert!(has(&shown, want), "{want} missing from {shown:?}");
        }
        shoot(&screen.draws().unwrap(), &root, "1-players");
        drive(&mut screen, &mut s, Key::Right);
        assert!(has(&texts(&screen), "3"), "{:?}", texts(&screen));
        drive(&mut screen, &mut s, Key::Accept);
        let shown = texts(&screen);
        for want in ["Player 1", "Player 2", "Player 3", "My Skater"] {
            assert!(has(&shown, want), "{want} missing from {shown:?}");
        }
        shoot(&screen.draws().unwrap(), &root, "2-characters");
        drive(&mut screen, &mut s, Key::Down);
        drive(&mut screen, &mut s, Key::Left);
        assert!(has(&texts(&screen), "Dennis Busenitz"), "{:?}", texts(&screen));
        shoot(&screen.draws().unwrap(), &root, "2-characters-player2");
        drive(&mut screen, &mut s, Key::Accept);
        let shown = texts(&screen);
        for want in ["Select challenge type", "Spot Battle", "Get the highest scoring trick to own the spot!"] {
            assert!(has(&shown, want), "{want} missing from {shown:?}");
        }
        shoot(&screen.draws().unwrap(), &root, "3-type");
        drive(&mut screen, &mut s, Key::Right);
        assert!(has(&texts(&screen), "S.K.A.T.E"), "{:?}", texts(&screen));
        drive(&mut screen, &mut s, Key::Left);
        drive(&mut screen, &mut s, Key::Accept);
        assert!(has(&texts(&screen), "Select location"), "{:?}", texts(&screen));
        shoot(&screen.draws().unwrap(), &root, "4-location");
        // Not retail: the row's photo over the page (a Skate 3 location photo here).
        let assets = root.parent().and_then(std::path::Path::parent).unwrap();
        let file = assets.join("private/challenge-map/images/assets/data/fe/source/images/locations/uptown_promenade/0000_uptown_promenade.Texture.rgba");
        if file.is_file() {
            let mut draws = screen.draws().unwrap();
            draws.extend(photo(&file.to_string_lossy(), [512, 256]));
            shoot(&draws, &root, "4-location-photo");
        }
        drive(&mut screen, &mut s, Key::Back);
        drive(&mut screen, &mut s, Key::Back);
        drive(&mut screen, &mut s, Key::Back);
        assert!(has(&texts(&screen), "Select number of players"), "{:?}", texts(&screen));
        screen.play("outro").unwrap();
        run(&mut screen, 30);
        assert!(screen.bindings.outro_complete);
    }

    #[test]
    fn press_a_card_shows_the_round_and_the_player() {
        let Some(root) = root() else { return };
        let json = crate::apt_screen::kept_json(&root.join("runtime/partyplay_start.json")).unwrap();
        let answers = card_answers(&json["language"], 2, 1);
        let mut screen = Screen::load(&root.join("runtime/partyplay_start.json"), &answers).unwrap();
        screen.play("intro").unwrap();
        run(&mut screen, 30);
        assert!(screen.bindings.intro_complete);
        let shown = texts(&screen);
        assert!(has(&shown, "Round 1"), "{shown:?}");
        assert!(shown.iter().any(|t| t.starts_with("Player 2 ready! Press")), "{shown:?}");
        shoot(&screen.draws().unwrap(), &root, "5-card");
        screen.play("outro").unwrap();
        run(&mut screen, 30);
        assert!(screen.bindings.outro_complete);
    }

    #[test]
    fn exit_and_rematch_popups_answer_no_and_yes() {
        let Some(root) = root() else { return };
        let json = crate::apt_screen::kept_json(&root.join("runtime/sk8popup.json")).unwrap();
        for ask in [Ask::Exit, Ask::Rematch] {
            let mut popup = popup_runtime::Runtime::load(&json, ask.message(), NO_YES, false).unwrap();
            (0..40).for_each(|_| popup.advance().unwrap());
            let draws = crate::apt_scene::draw(&popup.bindings.movie, &popup.vm, &popup.bindings.shapes).unwrap();
            shoot(&draws, &root, &format!("6-popup-{ask:?}"));
            assert_eq!(popup.input(Key::Back).unwrap(), None, "not cancelable");
            assert_eq!(popup.input(Key::Accept).unwrap(), Some(0), "No is highlighted first");
            popup.input(Key::Down).unwrap();
            assert_eq!(popup.input(Key::Accept).unwrap(), Some(1), "Yes");
        }
    }

    #[test]
    fn results_dialog_shows_the_standings_and_the_party_options() {
        let Some(root) = root().filter(|r| r.join("runtime/endchallengeoptions.json").is_file()) else { return };
        let row = |rank: i64, score: &str, i: i64, name: &str| {
            [json!(rank), json!(score), json!(0.), json!(""), json!(""), json!(false), json!(i), json!(name)]
        };
        let block: Vec<Json> = row(1, "#1200", 0, "#Player 2").into_iter().chain(row(2, "#800", 1, "#Player 1")).collect();
        let answers = results_answers(vec![
            ("Mission.GetResultsTitle".to_owned(), json!("#Spot Battle")),
            ("HUDComponents.Results_GetNumPlayers".to_owned(), json!(2)),
            ("HUDComponents.Results_GetLayoutType".to_owned(), json!(0)),
            ("HUDComponents.Results_GetLeaderboardDataBlock".to_owned(), Json::Array(block)),
            ("HUDComponents.Results_GetThrowdownType".to_owned(), json!(0)),
            ("HUDComponents.Results_GetLeaderboardColTitle(0)".to_owned(), json!("ID_LEADERBOARD_HEADER_RANK")),
            ("HUDComponents.Results_GetLeaderboardColTitle(1)".to_owned(), json!("ID_LEADERBOARD_NAME_HEADER")),
            ("HUDComponents.Results_GetLeaderboardColTitle(2)".to_owned(), json!("ID_LEADERBOARD_POINTS_HEADER")),
        ]);
        let borrowed: Vec<(&str, Json)> = answers.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        let mut screen = Screen::load(&root.join("runtime/endchallengeoptions.json"), &borrowed).unwrap();
        run(&mut screen, 40);
        let shown = texts(&screen);
        for want in ["Rematch", "Quit", "Change settings", "Player 1", "Player 2", "1200"] {
            assert!(shown.iter().any(|t| t.contains(want)), "{want} missing from {shown:?}");
        }
        shoot(&screen.draws().unwrap(), &root, "7-results");
    }
}
