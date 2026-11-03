//! `FrontEndState_OnlineHubMenu` (FE state 44, screen 92 `onlinehub/onlineHubMenu.swf`): the hub
//! Back opens (`StateManagerFrontEnd::CheckForOnlineHubMenu` 0x82656428). Its `HubTree` stacks
//! menus. In an online game: the host's (9, `SetupOnlineHostMenu` 0x82683e50) or a client's (10,
//! `SetupOnlineClientMenu` 0x82683fc8), the player list (14), a player's menu (4,
//! `SetupHostPlayerTemplate` 0x82684200 and `SetupClientPlayerTemplate` 0x82684350), Host
//! Options (11, `SetupHostOptionsMenu` 0x82688f90) and session access (17,
//! `GoToSetupToggleSlotAccessMenu` 0x826889e0). In a single player game: the single player menu
//! (0, `SetupSinglePlayerMenu` 0x82688b38) with Freeskate (5, `SetupFreeskateMenu` 0x82688d00)
//! and Unranked (7, `SetupUnrankedOnlineMenu` 0x82688e48), or before signing in the offline menu
//! (15, `SetupOfflineMainMenu` 0x82689068). The screen's `menu_picker`s keep the selection as
//! `NextValueCalc` and `PrevValueCalc` step it: over disabled items, wrapping.

use super::FeSoundRequest;
use super::pause_menu::PauseMenuInput;
use std::collections::BTreeMap;

pub const SINGLE_PLAYER: u32 = 0;
pub const FRIENDS: u32 = 1;
pub const PLAYER: u32 = 4;
pub const FREESKATE: u32 = 5;
pub const UNRANKED: u32 = 7;
pub const HOST: u32 = 9;
pub const CLIENT: u32 = 10;
pub const HOST_OPTIONS: u32 = 11;
pub const PLAYER_LIST: u32 = 14;
pub const OFFLINE: u32 = 15;
pub const SESSION_ACCESS: u32 = 17;
/// The sound `CheckForOnlineHubMenu` plays as it opens the hub and `ReceiveInput` 0x8266c578 as
/// B leaves a submenu.
pub const HUB_SOUND: u64 = 0x1A53_83D3_0000_0000;
/// `"#%s (%d/%d)"`'s total of the player list (`HubStateOnlineHost::OnEvent` 0x8268b940): the
/// other players a full game holds. The original prints a literal 5; a game here holds
/// `MAX_PLAYERS`.
const PLAYER_LIST_TOTAL: usize = skate2_net::protocol::MAX_PLAYERS - 1;

/// A menu or item title as the hub's strings hold it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Title {
    /// A HAL id.
    Hal(&'static str),
    /// `"#%s"` of a name.
    Literal(String),
    /// `"#%s (%d/%d)"` of a HAL id's packed string.
    Count { label: &'static str, count: usize, total: usize },
}

/// The hub `PickCurrentState` 0x82688470 sets up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HubKind {
    /// In an online game: the host's or a client's menu.
    #[default]
    Online,
    /// Signed in, in a single player or free skate game (`IsSinglePlayerHubMode` 0x82645170):
    /// the single player menu, titled with the player's name (`SetSinglePlayerHubMenu`
    /// 0x82683cc8).
    SinglePlayer,
    /// Not signed in (`Fesl::HubSingle` not `CONNECTION_READY`): the offline menu, titled
    /// `ID_ONLINE_HUB_OFFLINE_TITLE`.
    Offline,
}

/// What the game knows of the session as the hub sets its menus up.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lobby {
    pub kind: HubKind,
    /// `GetLocalUser()->GetUsername()`.
    pub nickname: String,
    /// `GetGameHostPeerID() == GetLocalUser()->GetUserId()`.
    pub host: bool,
    /// Attribute 6, `Attribute_IsRanked`.
    pub ranked: bool,
    /// Attribute 4: the session is private.
    pub private: bool,
    /// The game's challenge type is `Challenge_OnlineFreeSkate`.
    pub free_skate_type: bool,
    /// `sGameInfo.gameMode == GM_OnlineFreeSkate`: lobby skate.
    pub lobby_mode: bool,
    /// `Net::IsChallengeInProgress`.
    pub challenge_in_progress: bool,
    /// `GameManagerGame::IsGameInProgress`.
    pub game_in_progress: bool,
    /// `mIsChangingLocation`: the host's change of challenge is counting down.
    pub changing_location: bool,
    /// `OE_PlayersSkatingWith`: the other players, by user id and name.
    pub players: Vec<(u64, String)>,
    /// `GetPlayerTypeCount(GAME_PLAYER_TYPE_PARTICIPANT)`.
    pub participants: usize,
}

/// What an item does when selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    Menu(u32),
    /// A `PeerHubMenuItem` of a player, to their menu.
    Peer(u64),
    StartChallenge,
    FreeskateActivities,
    MatchSetup,
    ChangeLocation,
    SessionAccess,
    ToggleSlotAccess,
    SendFriendInvite,
    ViewGamercard,
    KickPlayer,
    PublicFreeskate,
    CustomFreeskate,
    QuickMatch,
    CustomMatch,
    SignIn,
    /// Skate.Reel, Create-a-Spot, Ranked and Skate Profile in a game: screens the port does not
    /// open there, so these items stay off.
    NotPorted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Item {
    title: Title,
    enabled: bool,
    target: Target,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Menu {
    title: Option<Title>,
    items: Vec<Item>,
    selected: usize,
}

/// A menu as the screen's `HubTree` calls read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuView {
    pub id: u32,
    /// `GetHubMenuTitle`.
    pub title: Title,
    /// `GetHubMenuDataForID` and `OnMenuGetItemEnabled`.
    pub items: Vec<(Title, bool)>,
    /// `GetSelectedItemForHubMenuID`.
    pub selected: usize,
}

/// `SetScreenState` of the hub's screen: `SetState(0)` (`HubTree.Initialize`), 1
/// (`OnMenuPush`, "0-1") or 2 (`OnMenuPop`, "1-0").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenCall {
    Initialize,
    Push,
    Pop,
}

/// `GoToKickPlayer` 0x826722e8's confirmation: Yes or No, Yes first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KickPopup {
    pub user_id: u64,
    pub name: String,
    pub selected: usize,
}

/// A popup of the online front end (sk8popup, FE state 34).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Popup {
    Kick(KickPopup),
    /// `OnlineBackEnd::OpenLoadingPopup` 0x825665e8 while matchmaking searches: popup 38,
    /// `ID_ONLINE_MATCHING_SKATERS` with no options, not cancellable.
    Matching,
    /// `StateOnlineSignIn::ConnectToPlasma` 0x8275df08 while the sign-in connects: popup 33,
    /// `ID_ONLINE_SIGNING_IN` with no options; B cancels.
    SigningIn,
    /// What ended a search or the sign-in (`OnlineBackEnd::ShowErrorDialog` 0x825735d0), and OK.
    Notice(&'static str),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// The hub tree's stack, root first.
    pub stack: Vec<MenuView>,
}

/// What the hub asks of the game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    /// `OnlineFeManager::Exit` 0x8266c418.
    Exit,
    /// `GoToStartChallenge` 0x826617a0's `GameRequest(1)`.
    StartChallenge,
    /// `GoToNewChallenge` 0x82661970: FE state 69 in host mode.
    MatchSetup,
    /// `GoToChangeLocation` 0x826618f0: FE state 68 in host mode.
    ChangeLocation,
    /// `GoToToggleSlotAccess` 0x8266c998's `GameRequest(3)` of the new privacy, then Exit.
    SetPrivate(bool),
    /// `KickPlayerCallback` 0x8266c908's `Net::KickPlayer`, central message and Exit.
    Kick(u64),
    /// `GoToFreeSkateHere` 0x8267bee8: a public free skate where the skater is
    /// (`StateOnlineFreeskateHere`).
    PublicFreeskate,
    /// `GoToCustomFreeSkate` 0x82661540: FE state 68.
    CustomFreeskate,
    /// `GoToUnrankedQuickMatch` 0x826616e0: `StartUnrankedCustomMatch(Challenge_None, quick)`.
    QuickMatch,
    /// `GoToUnrankedCustomMatch` 0x82661720: FE state 69.
    CustomMatch,
    /// `GoToSignIn` 0x8267c158: `OnlineFeManager::Exit`, then the sign-in.
    SignIn,
}

pub struct Controller {
    lobby: Lobby,
    menus: BTreeMap<u32, Menu>,
    stack: Vec<u32>,
    /// `FrontEndState_SkateProfile::m_rPeerID`: the player whose menu is set up.
    peer: Option<u64>,
    popup: Option<Popup>,
    calls: Vec<ScreenCall>,
    sounds: Vec<FeSoundRequest>,
    /// The screen's intro completed: `StateManagerFrontEnd::ReceiveInput` passes input on.
    pub ready: bool,
}

impl Controller {
    /// `OnEnter` 0x8268a3c0: `PickCurrentState` pushes the kind's root menu; `HubState*::OnLoaded`
    /// asks for the friend list (and online the players), whose events (`OnEvent`) set the menus
    /// up and `SetScreenState(0)`.
    #[must_use]
    pub fn open(lobby: Lobby) -> Self {
        let root = Self::root(&lobby);
        let mut hub = Self { lobby, menus: BTreeMap::new(), stack: vec![root], peer: None, popup: None, calls: Vec::new(), sounds: Vec::new(), ready: false };
        hub.setup_menus();
        hub.calls.push(ScreenCall::Initialize);
        hub
    }

    const fn root(lobby: &Lobby) -> u32 {
        match lobby.kind {
            HubKind::Online if lobby.host => HOST,
            HubKind::Online => CLIENT,
            HubKind::SinglePlayer => SINGLE_PLAYER,
            HubKind::Offline => OFFLINE,
        }
    }

    /// Matchmaking searches under popup 38.
    pub fn matching(&mut self) {
        self.sounds.push(FeSoundRequest::Play("FE_SOUND_CORE_POPUP"));
        self.popup = Some(Popup::Matching);
    }

    /// `OnlineBackEnd::CloseLoadingPopup` 0x82566650: the search ended, and the searching popup
    /// closes as its notice opens.
    pub fn search_ended(&mut self) {
        if self.popup == Some(Popup::Matching) {
            self.popup = None;
        }
    }

    /// The popup the hub opened over its menus.
    #[must_use]
    pub const fn popup(&self) -> Option<&Popup> {
        self.popup.as_ref()
    }

    /// `OnEvent(OE_PlayersSkatingWith)` with the session as it is now.
    pub fn refresh(&mut self, lobby: Lobby) {
        if self.lobby == lobby {
            return;
        }
        self.lobby = lobby;
        self.setup_menus();
        self.calls.push(ScreenCall::Initialize);
    }

    pub fn take_calls(&mut self) -> Vec<ScreenCall> {
        std::mem::take(&mut self.calls)
    }

    pub fn take_sounds(&mut self) -> Vec<FeSoundRequest> {
        std::mem::take(&mut self.sounds)
    }

    fn players_count(&self) -> usize {
        self.lobby.players.len()
    }

    fn peer_name(&self, user_id: u64) -> Option<&str> {
        self.lobby.players.iter().find(|(id, _)| *id == user_id).map(|(_, name)| name.as_str())
    }

    /// `SetupMenus` 0x8268a808's items and the menus the events rebuild.
    fn setup_menus(&mut self) {
        match self.lobby.kind {
            HubKind::Online => self.setup_online_menus(),
            HubKind::SinglePlayer => self.setup_single_player_menus(),
            HubKind::Offline => self.setup_offline_menu(),
        }
        let root_id = Self::root(&self.lobby);
        // A menu the stack no longer leads to closes back to its parent.
        while self.stack.len() > 1 && self.stack.last().is_some_and(|id| !self.menus.contains_key(id)) {
            self.stack.pop();
        }
        if self.stack.first() != Some(&root_id) {
            self.stack = vec![root_id];
        }
    }

    /// `OnEvent(OE_FriendList)`: the port's server keeps no friends, so the item counts none.
    fn friends() -> Item {
        Item { title: Title::Count { label: "ID_ONLINE_HUB_FRIENDS", count: 0, total: 0 }, enabled: false, target: Target::Menu(FRIENDS) }
    }

    /// `SetupSinglePlayerMenu` 0x82688b38, `SetupFreeskateMenu` 0x82688d00 and
    /// `SetupUnrankedOnlineMenu` 0x82688e48. `HubStateSinglePlayer::OnEvent` 0x82688010 selects
    /// the root's first enabled item.
    fn setup_single_player_menus(&mut self) {
        let off = |title| Item { title: Title::Hal(title), enabled: false, target: Target::NotPorted };
        let items = vec![
            Self::friends(),
            Item { title: Title::Hal("ID_ONLINE_HUB_FREESKATE"), enabled: true, target: Target::Menu(FREESKATE) },
            off("ID_ONLINE_HUB_SKATEREEL"),
            off("ID_ONLINE_HUB_CREATEASPOT"),
            off("ID_ONLINE_HUB_RANKED"),
            Item { title: Title::Hal("ID_ONLINE_HUB_UNRANKED"), enabled: true, target: Target::Menu(UNRANKED) },
            off("ID_ONLINE_HUB_SKATEPROFILE"),
        ];
        let selected = items.iter().position(|item| item.enabled).unwrap_or(0);
        self.menus.insert(SINGLE_PLAYER, Menu { title: Some(Title::Literal(self.lobby.nickname.clone())), items, selected });
        self.menus.insert(FRIENDS, Menu { title: Some(Title::Hal("ID_ONLINE_HUB_FRIENDS")), items: Vec::new(), selected: 0 });
        self.menus.insert(
            FREESKATE,
            Menu {
                title: Some(Title::Hal("ID_ONLINE_HUB_FREESKATE")),
                items: vec![
                    Item { title: Title::Hal("ID_ONLINE_HUB_PUBLIC_FREESKATE"), enabled: true, target: Target::PublicFreeskate },
                    Item { title: Title::Hal("ID_ONLINE_HUB_CUSTOM_FREESKATE"), enabled: true, target: Target::CustomFreeskate },
                ],
                selected: 0,
            },
        );
        self.menus.insert(
            UNRANKED,
            Menu {
                title: Some(Title::Hal("ID_ONLINE_HUB_UNRANKED")),
                items: vec![
                    Item { title: Title::Hal("ID_ONLINE_HUB_QUICK_MATCH"), enabled: true, target: Target::QuickMatch },
                    Item { title: Title::Hal("ID_ONLINE_HUB_CUSTOM_MATCH"), enabled: true, target: Target::CustomMatch },
                ],
                selected: 0,
            },
        );
    }

    /// `SetupOfflineMainMenu` 0x82689068, titled `ID_ONLINE_HUB_OFFLINE_TITLE` by
    /// `PickCurrentState`: Sign In, Skate.Reel and Create-a-Spot.
    fn setup_offline_menu(&mut self) {
        let items = vec![
            Item { title: Title::Hal("ID_ONLINE_HUB_SIGN_IN"), enabled: true, target: Target::SignIn },
            Item { title: Title::Hal("ID_ONLINE_HUB_SKATEREEL"), enabled: false, target: Target::NotPorted },
            Item { title: Title::Hal("ID_ONLINE_HUB_CREATEASPOT"), enabled: false, target: Target::NotPorted },
        ];
        self.menus.insert(OFFLINE, Menu { title: Some(Title::Hal("ID_ONLINE_HUB_OFFLINE_TITLE")), items, selected: 0 });
    }

    /// The host's or a client's menu and the menus they lead to.
    fn setup_online_menus(&mut self) {
        let lobby = &self.lobby;
        let players = self.players_count();
        let friends = Self::friends();
        // OnEvent(OE_PlayersSkatingWith): "#Player List (n/7)", enabled with anyone to list.
        let player_list = Item {
            title: Title::Count { label: "ID_ONLINE_HUB_PLAYER_LIST", count: players, total: PLAYER_LIST_TOTAL },
            enabled: players > 0,
            target: Target::Menu(PLAYER_LIST),
        };
        // SetupStartChallengeMenuItem 0x82661db0.
        let start_challenge = Item {
            title: Title::Hal("ID_ONLINE_HUB_START_CHALLENGE"),
            enabled: lobby.lobby_mode && !lobby.ranked && !lobby.free_skate_type && players + 1 > 1 && !lobby.challenge_in_progress && !lobby.changing_location,
            target: Target::StartChallenge,
        };
        // SetupFreeskateActivitiesMenuItem 0x8268a710: enabled only in an online free skate with
        // others; the port has no freeskate activities, so it stays off.
        let activities = Item { title: Title::Hal("ID_ONLINE_HUB_FREESTYLE_ACTIVITES"), enabled: false, target: Target::FreeskateActivities };
        // HubStateOnlineHost::OnEvent: Host Options only while no game, proposal or change is on.
        let host_options = Item {
            title: Title::Hal("ID_ONLINE_HUB_HOST_OPTIONS"),
            enabled: lobby.host && !lobby.game_in_progress && !lobby.changing_location,
            target: Target::Menu(HOST_OPTIONS),
        };
        let mut root = Menu { title: Some(Title::Literal(lobby.nickname.clone())), items: vec![friends], selected: 0 };
        if lobby.host {
            if !lobby.ranked {
                root.items.extend([start_challenge, activities]);
            }
            root.items.push(player_list);
            if !lobby.ranked {
                root.items.push(host_options);
            }
        } else {
            if !lobby.ranked {
                root.items.push(activities);
            }
            root.items.push(player_list);
        }
        // OnEvent selects the root's first enabled item.
        root.selected = root.items.iter().position(|item| item.enabled).unwrap_or(0);
        let root_id = if lobby.host { HOST } else { CLIENT };
        self.menus.remove(&if lobby.host { CLIENT } else { HOST });
        self.menus.insert(root_id, root);
        self.menus.insert(FRIENDS, Menu { title: Some(Title::Hal("ID_ONLINE_HUB_FRIENDS")), items: Vec::new(), selected: 0 });
        let list = lobby.players.iter().map(|(user_id, name)| Item { title: Title::Literal(name.clone()), enabled: true, target: Target::Peer(*user_id) }).collect();
        let selected = self.menus.get(&PLAYER_LIST).map_or(0, |menu| menu.selected.min(players.saturating_sub(1)));
        self.menus.insert(PLAYER_LIST, Menu { title: Some(Title::Hal("ID_ONLINE_HUB_PLAYER_LIST")), items: list, selected });
        if !self.menus.contains_key(&HOST_OPTIONS) {
            self.menus.insert(
                HOST_OPTIONS,
                Menu {
                    title: Some(Title::Hal("ID_ONLINE_HUB_HOST_OPTIONS")),
                    items: vec![
                        Item { title: Title::Hal("ID_ONLINE_HUB_NEW_CHALLENGE"), enabled: true, target: Target::MatchSetup },
                        Item { title: Title::Hal("ID_ONLINE_HUB_CHANGE_LOCATION"), enabled: true, target: Target::ChangeLocation },
                        Item { title: Title::Hal("ID_ONLINEHUB_SESSIONACCESS"), enabled: true, target: Target::SessionAccess },
                    ],
                    selected: 0,
                },
            );
        }
        self.setup_player_menu();
        self.setup_session_access();
    }

    /// `Setup{Host,Client}PlayerTemplate`: the player's name; Send Friend Request and View
    /// Gamercard (Xbox LIVE's, which the port has not, so both off); the host's Kick Player in an
    /// unranked game with no challenge on.
    fn setup_player_menu(&mut self) {
        let name = self.peer.and_then(|peer| self.peer_name(peer)).map(str::to_owned);
        let mut items = vec![
            Item { title: Title::Hal("ID_ONLINE_HUB_SEND_FRIEND_INVITE"), enabled: false, target: Target::SendFriendInvite },
            Item { title: Title::Hal("ID_ONLINE_HUB_VIEW_GAMERCARD"), enabled: false, target: Target::ViewGamercard },
        ];
        if self.lobby.host && !self.lobby.ranked && name.is_some() && !self.lobby.challenge_in_progress {
            items.push(Item { title: Title::Hal("ID_ONLINE_HUB_KICK_PLAYER"), enabled: true, target: Target::KickPlayer });
        }
        let selected = items.iter().position(|item| item.enabled).unwrap_or(0);
        let title = name.map_or(Title::Hal("ID_ONLINE_HUB_PLAYER_LIST"), Title::Literal);
        self.menus.insert(PLAYER, Menu { title: Some(title), items, selected });
    }

    /// `GoToSetupToggleSlotAccessMenu`: Make Public when private, Make Private when public, the
    /// header naming the current access and the enabled item selected.
    fn setup_session_access(&mut self) {
        let private = self.lobby.private;
        self.menus.insert(
            SESSION_ACCESS,
            Menu {
                title: Some(Title::Hal(if private { "ID_ONLINEHUB_SESSIONACCESS_HEADER_PRIVATE" } else { "ID_ONLINEHUB_SESSIONACCESS_HEADER_PUBLIC" })),
                items: vec![
                    Item { title: Title::Hal("ID_ONLINE_HUB_MAKE_PUBLIC_SLOT"), enabled: private, target: Target::ToggleSlotAccess },
                    Item { title: Title::Hal("ID_ONLINE_HUB_MAKE_PRIVATE_SLOT"), enabled: !private, target: Target::ToggleSlotAccess },
                ],
                selected: usize::from(!private),
            },
        );
    }

    fn menu_title(&self, id: u32, menu: &Menu) -> Title {
        menu.title.clone().unwrap_or(match id {
            HOST | CLIENT => Title::Literal(self.lobby.nickname.clone()),
            _ => Title::Literal(String::new()),
        })
    }

    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        let stack = self
            .stack
            .iter()
            .filter_map(|&id| {
                let menu = self.menus.get(&id)?;
                Some(MenuView {
                    id,
                    title: self.menu_title(id, menu),
                    items: menu.items.iter().map(|item| (item.title.clone(), item.enabled)).collect(),
                    selected: menu.selected,
                })
            })
            .collect();
        Snapshot { stack }
    }

    /// `NextValueCalc` and `PrevValueCalc`: the next enabled item past the selection, wrapping;
    /// the selection itself when none is.
    fn step(menu: &Menu, forward: bool) -> usize {
        let count = menu.items.len();
        for offset in 1..=count {
            let index = if forward { (menu.selected + offset) % count } else { (menu.selected + count * offset - offset) % count };
            if menu.items[index].enabled {
                return index;
            }
        }
        menu.selected
    }

    /// `ReceiveInput` 0x8266c578 and the top menu's `menu_picker.UpdateInput`.
    pub fn update(&mut self, input: PauseMenuInput) -> Action {
        if !self.ready {
            return Action::None;
        }
        match self.popup.take() {
            Some(Popup::Kick(mut popup)) => {
                // sk8popup's menu_picker: two options, wrapping, not cancellable.
                if input.up {
                    popup.selected = (popup.selected + 1) % 2;
                    self.sounds.push(FeSoundRequest::Play("FE_SOUND_CORE_NAV_UP"));
                }
                if input.down {
                    popup.selected = (popup.selected + 1) % 2;
                    self.sounds.push(FeSoundRequest::Play("FE_SOUND_CORE_NAV_DOWN"));
                }
                if !input.select {
                    self.popup = Some(Popup::Kick(popup));
                    return Action::None;
                }
                self.sounds.push(FeSoundRequest::Play("FE_SOUND_CORE_A_BUTTON"));
                return if popup.selected == 0 { Action::Kick(popup.user_id) } else { Action::None };
            }
            // The searching popup takes no input: the search ends in a game or a notice.
            Some(popup) => {
                self.popup = Some(popup);
                return Action::None;
            }
            None => {}
        }
        if input.cancel {
            if self.stack.len() == 1 {
                return Action::Exit;
            }
            self.sounds.push(FeSoundRequest::PlayKey(HUB_SOUND));
            self.stack.pop();
            self.calls.push(ScreenCall::Pop);
            return Action::None;
        }
        if input.back {
            return Action::Exit;
        }
        let Some(&id) = self.stack.last() else { return Action::None };
        let Some(menu) = self.menus.get_mut(&id) else { return Action::None };
        if menu.items.is_empty() {
            return Action::None;
        }
        for (pressed, forward) in [(input.up, false), (input.down, true)] {
            if !pressed {
                continue;
            }
            let next = Self::step(menu, forward);
            if next != menu.selected {
                menu.selected = next;
                self.sounds.push(FeSoundRequest::Play(if forward { "FE_SOUND_CORE_NAV_DOWN" } else { "FE_SOUND_CORE_NAV_UP" }));
            }
        }
        if !input.select {
            return Action::None;
        }
        // menu_picker::OnMenuSelect sounds A before Game.OnMenuSelect, which acts on an enabled
        // item only.
        self.sounds.push(FeSoundRequest::Play("FE_SOUND_CORE_A_BUTTON"));
        let item = menu.items[menu.selected].clone();
        if !item.enabled {
            return Action::None;
        }
        self.select(item.target)
    }

    /// `OnMenuSelect` 0x82688570: a submenu's push, or the item's function.
    fn select(&mut self, target: Target) -> Action {
        match target {
            Target::Menu(id) => {
                self.stack.push(id);
                self.calls.push(ScreenCall::Push);
                Action::None
            }
            Target::Peer(user_id) => {
                self.peer = Some(user_id);
                self.setup_player_menu();
                self.stack.push(PLAYER);
                self.calls.push(ScreenCall::Push);
                Action::None
            }
            Target::StartChallenge => {
                if self.lobby.participants >= 2 { Action::StartChallenge } else { Action::None }
            }
            Target::MatchSetup => Action::MatchSetup,
            Target::ChangeLocation => Action::ChangeLocation,
            Target::SessionAccess => {
                self.setup_session_access();
                self.stack.push(SESSION_ACCESS);
                self.calls.push(ScreenCall::Push);
                Action::None
            }
            Target::ToggleSlotAccess => Action::SetPrivate(!self.lobby.private),
            Target::KickPlayer => {
                let Some(user_id) = self.peer else { return Action::None };
                let name = self.peer_name(user_id).unwrap_or_default().to_owned();
                // FrontEndState_Popup::OnEnter sounds core_popup.
                self.sounds.push(FeSoundRequest::Play("FE_SOUND_CORE_POPUP"));
                self.popup = Some(Popup::Kick(KickPopup { user_id, name, selected: 0 }));
                Action::None
            }
            Target::PublicFreeskate => Action::PublicFreeskate,
            Target::CustomFreeskate => Action::CustomFreeskate,
            Target::QuickMatch => Action::QuickMatch,
            Target::CustomMatch => Action::CustomMatch,
            Target::SignIn => Action::SignIn,
            Target::FreeskateActivities | Target::SendFriendInvite | Target::ViewGamercard | Target::NotPorted => Action::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lobby(host: bool, others: usize) -> Lobby {
        Lobby {
            nickname: "SkaterA".into(),
            host,
            lobby_mode: true,
            players: (0..others).map(|index| (index as u64 + 2, format!("Skater{index}"))).collect(),
            participants: others + 1,
            ..Lobby::default()
        }
    }

    fn press(f: impl FnOnce(&mut PauseMenuInput)) -> PauseMenuInput {
        let mut input = PauseMenuInput::default();
        f(&mut input);
        input
    }

    fn titles(view: &MenuView) -> Vec<Title> {
        view.items.iter().map(|(title, _)| title.clone()).collect()
    }

    #[test]
    fn host_root_lists_native_items_and_selects_first_enabled() {
        let hub = Controller::open(lobby(true, 1));
        let snapshot = hub.snapshot();
        let root = &snapshot.stack[0];
        assert_eq!(root.id, HOST);
        assert_eq!(root.title, Title::Literal("SkaterA".into()));
        assert_eq!(
            titles(root),
            vec![
                Title::Count { label: "ID_ONLINE_HUB_FRIENDS", count: 0, total: 0 },
                Title::Hal("ID_ONLINE_HUB_START_CHALLENGE"),
                Title::Hal("ID_ONLINE_HUB_FREESTYLE_ACTIVITES"),
                Title::Count { label: "ID_ONLINE_HUB_PLAYER_LIST", count: 1, total: 7 },
                Title::Hal("ID_ONLINE_HUB_HOST_OPTIONS"),
            ]
        );
        assert_eq!(root.items.iter().map(|(_, enabled)| *enabled).collect::<Vec<_>>(), vec![false, true, false, true, true]);
        assert_eq!(root.selected, 1);
    }

    #[test]
    fn start_challenge_needs_another_player_and_a_challenge_type() {
        let alone = Controller::open(lobby(true, 0)).snapshot();
        assert!(!alone.stack[0].items[1].1);
        let mut free_skate = lobby(true, 1);
        free_skate.free_skate_type = true;
        assert!(!Controller::open(free_skate).snapshot().stack[0].items[1].1);
        let mut hub = Controller::open(lobby(true, 1));
        hub.ready = true;
        assert_eq!(hub.update(press(|input| input.select = true)), Action::StartChallenge);
    }

    #[test]
    fn client_root_has_no_host_items() {
        let snapshot = Controller::open(lobby(false, 1)).snapshot();
        assert_eq!(snapshot.stack[0].id, CLIENT);
        assert_eq!(snapshot.stack[0].items.len(), 3);
        assert_eq!(snapshot.stack[0].selected, 2);
    }

    #[test]
    fn navigation_skips_disabled_items_and_b_pops_or_exits() {
        let mut hub = Controller::open(lobby(true, 1));
        hub.ready = true;
        hub.update(press(|input| input.down = true));
        assert_eq!(hub.snapshot().stack[0].selected, 3);
        hub.update(press(|input| input.select = true));
        assert_eq!(hub.take_calls(), vec![ScreenCall::Initialize, ScreenCall::Push]);
        assert_eq!(hub.snapshot().stack[1].id, PLAYER_LIST);
        hub.update(press(|input| input.select = true));
        let snapshot = hub.snapshot();
        assert_eq!(snapshot.stack[2].id, PLAYER);
        assert_eq!(snapshot.stack[2].title, Title::Literal("Skater0".into()));
        assert_eq!(snapshot.stack[2].selected, 2);
        hub.update(press(|input| input.select = true));
        assert!(matches!(hub.popup(), Some(Popup::Kick(KickPopup { user_id: 2, .. }))));
        assert_eq!(hub.update(press(|input| input.select = true)), Action::Kick(2));
        hub.update(press(|input| input.cancel = true));
        hub.update(press(|input| input.cancel = true));
        assert_eq!(hub.snapshot().stack.len(), 1);
        assert_eq!(hub.update(press(|input| input.cancel = true)), Action::Exit);
    }

    #[test]
    fn session_access_offers_the_other_access() {
        let mut hub = Controller::open(lobby(true, 1));
        hub.ready = true;
        hub.update(press(|input| input.down = true));
        hub.update(press(|input| input.down = true));
        hub.update(press(|input| input.select = true));
        assert_eq!(hub.snapshot().stack[1].id, HOST_OPTIONS);
        hub.update(press(|input| input.up = true));
        hub.update(press(|input| input.select = true));
        let access = &hub.snapshot().stack[2];
        assert_eq!(access.title, Title::Hal("ID_ONLINEHUB_SESSIONACCESS_HEADER_PUBLIC"));
        assert_eq!(access.selected, 1);
        assert_eq!(hub.update(press(|input| input.select = true)), Action::SetPrivate(true));
    }

    fn single_player(kind: HubKind) -> Controller {
        let mut hub = Controller::open(Lobby { kind, nickname: "SkaterA".into(), ..Lobby::default() });
        hub.ready = true;
        hub
    }

    #[test]
    fn single_player_hub_offers_free_skate_and_unranked_matches() {
        let mut hub = single_player(HubKind::SinglePlayer);
        let root = &hub.snapshot().stack[0];
        assert_eq!(root.id, SINGLE_PLAYER);
        assert_eq!(root.title, Title::Literal("SkaterA".into()));
        assert_eq!(root.items.iter().map(|(_, enabled)| *enabled).collect::<Vec<_>>(), vec![false, true, false, false, false, true, false]);
        assert_eq!(root.selected, 1);
        hub.update(press(|input| input.select = true));
        assert_eq!(hub.snapshot().stack[1].id, FREESKATE);
        assert_eq!(hub.update(press(|input| input.select = true)), Action::PublicFreeskate);
        hub.update(press(|input| input.down = true));
        assert_eq!(hub.update(press(|input| input.select = true)), Action::CustomFreeskate);
        hub.update(press(|input| input.cancel = true));
        hub.update(press(|input| input.down = true));
        assert_eq!(hub.snapshot().stack[0].selected, 5);
        hub.update(press(|input| input.select = true));
        assert_eq!(hub.snapshot().stack[1].id, UNRANKED);
        assert_eq!(hub.update(press(|input| input.select = true)), Action::QuickMatch);
        hub.update(press(|input| input.down = true));
        assert_eq!(hub.update(press(|input| input.select = true)), Action::CustomMatch);
    }

    #[test]
    fn offline_hub_signs_in_and_signing_in_changes_the_menu() {
        let mut hub = single_player(HubKind::Offline);
        let root = &hub.snapshot().stack[0];
        assert_eq!((root.id, root.title.clone()), (OFFLINE, Title::Hal("ID_ONLINE_HUB_OFFLINE_TITLE")));
        assert_eq!(hub.update(press(|input| input.select = true)), Action::SignIn);
        hub.refresh(Lobby { kind: HubKind::SinglePlayer, nickname: "SkaterA".into(), ..Lobby::default() });
        assert_eq!(hub.snapshot().stack[0].id, SINGLE_PLAYER);
    }

    #[test]
    fn the_searching_popup_holds_the_input_until_the_search_ends() {
        let mut hub = single_player(HubKind::SinglePlayer);
        hub.matching();
        for key in [press(|input| input.back = true), press(|input| input.cancel = true), press(|input| input.select = true)] {
            assert_eq!(hub.update(key), Action::None);
        }
        assert_eq!(hub.popup(), Some(&Popup::Matching));
        hub.search_ended();
        assert_eq!(hub.popup(), None);
        assert_eq!(hub.update(press(|input| input.cancel = true)), Action::Exit);
    }
}
