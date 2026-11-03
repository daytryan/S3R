//! The retail Player List (`online/friends`, class PlayerList): session players, Steam friends, recent players,
//! teammates and friends to invite, each with its per-context actions (spec §5). The movie's own controller runs as
//! shipped; [`PlayerList`] answers what it asks and turns its `Game.OnMenuSelect` calls into [`Effect`]s that
//! online_menu carries out.
use crate::{
    apt_screen::{ONLINE, Screen, Screens},
    core_menu_runtime::Key,
    online_menu::ListKind,
    online_roster::RecentPlayer,
};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use skate_net::{presence::Friend, roster::Member};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        Mutex, MutexGuard, OnceLock,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
};

/// Above the event log and roster screens on the online layer.
const NAME: &str = "3-playerlist";
/// The actions' menu_picker, as friends:Constructor() finds it (`mScreen.mFriends.mContent.mSubmenu`).
const SUBMENU: &str = "mFriends.mContent.mSubmenu";
/// The root list's menu_picker (friends:Constructor's mMenu).
const MENU: &str = "mFriends.mContent.mMenu";
/// The panel whose button bar (panel.AddButtonItem / RefreshButtonHelp) the server browser adds to.
const PANEL: &str = "mFriends.mPanel";
/// voice-players.json keeps this many players at most.
const VOICE_PLAYERS_MAX: usize = 1000;
/// The controller's `SetState` modes, as the game defines them (`friends.dis` SetState).
const MODES: [&str; 4] = ["InitList", "GoToSubMenu", "GoToRootMenu", "HideList"];
const INIT_LIST: u8 = 0;
const SUB_MENU: u8 = 1;
const ROOT_MENU: u8 = 2;
const HIDE_LIST: u8 = 3;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Person {
    pub name: String,
    /// 0: none known (Direct players).
    pub steam: u64,
    /// Session actor id: Kick and Make Host target it, never the name.
    pub actor: Option<u64>,
    /// The session's host (a co-host can't kick them).
    pub host: bool,
    pub friend: bool,
    pub online: bool,
    /// Rich-presence join string: the friend is in Skate3Rust.
    pub connect: Option<String>,
    /// A server browser session's details (its map, its description), greyed under its Join.
    pub about: Vec<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Row {
    pub label: String,
    pub header: bool,
    pub person: Option<Person>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlayerAction {
    SkateWith,
    ViewSkateProfile,
    ViewSteamProfile,
    SendFriendInvite,
    SendInvite,
    Kick,
    Mute,
    InviteToTeam,
    ViewTeamProfile,
    MakeHost,
    SendMessage,
    /// This player's Graphic Creator designs, on this machine only.
    HideGraphics,
    ShowGraphics,
    /// Host only: a co-host moves the session, kicks and sets the world, but makes no co-hosts or host.
    MakeCoHost,
    RemoveCoHost,
    /// Host only, a team lobby: move the player to the other team (decision T1: the host has the final say).
    SwitchTeam,
    /// Not retail (online safety): [`Shield`]'s picks.
    Unmute,
    HidePlayer,
    ShowPlayer,
    Block,
    Unblock,
    /// Not retail (voice chat, user 10-26): this session player's voice, 0-200 %, set in place with Left / Right.
    VoiceVolume,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Context {
    pub kind: ListKind,
    /// We host the session.
    pub host: bool,
    /// The session is a Steam lobby (not Direct).
    pub steam_session: bool,
    /// The Steam presence helper is up.
    pub presence: bool,
    /// We are a co-host (the host made us one).
    pub cohost: bool,
    /// The session's co-hosts' actor ids.
    pub cohosts: Vec<u64>,
    /// The session is a team lobby (Online Team Challenge).
    pub teams: bool,
    /// Who is muted, hidden and blocked ([`snapshot`]).
    pub shield: Shield,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Effect {
    Act(PlayerAction, Person),
    /// Voice Volume moved in place: the player's new volume (online_menu keeps it).
    Volume(Person, u16),
    Closed,
}

pub(crate) fn title(kind: ListKind) -> &'static str {
    match kind {
        ListKind::Session => "ID_PLAYER_LIST_PLAYERS_TITLE",
        ListKind::Friends | ListKind::InviteFriend => "ID_PLAYER_LIST_FRIENDS_TITLE",
        ListKind::Recent => "ID_PLAYER_LIST_RECENT_PLAYERS_TITLE",
        ListKind::Teammates => "ID_PLAYER_LIST_TEAMMATES_TITLE",
        ListKind::Lobbies => crate::server_browser::TITLE,
    }
}
pub(crate) fn empty_text(kind: ListKind) -> &'static str {
    match kind {
        ListKind::Session => "ID_PLAYER_LIST_EMPTY_PLAYER_LIST",
        ListKind::Friends | ListKind::InviteFriend => "ID_PLAYER_LIST_EMPTY_FRIENDS_LIST",
        ListKind::Recent => "ID_PLAYER_LIST_EMPTY_RECENT_PLAYERS_LIST",
        ListKind::Teammates => "ID_PLAYER_LIST_EMPTY_TEAMMATES_LIST",
        ListKind::Lobbies => crate::server_browser::NONE,
    }
}

fn header(label: &str) -> Row {
    Row { label: label.into(), header: true, person: None }
}
fn friend_row(f: &Friend) -> Row {
    // User 10-25: a Steam name can't fake a mark either (skate_net::roster::shown_name, as for session players).
    let name = skate_net::roster::shown_name(&f.name);
    let label = crate::apt_text::literal(&if f.connect.is_some() { format!("{name} - Skate3Rust") } else { name.clone() });
    let person = Person { name, steam: f.id, actor: None, host: false, friend: true, online: f.online, connect: f.connect.clone(), about: Vec::new() };
    Row { label, header: false, person: Some(person) }
}

/// A session player's row: the name and its mark. User 10-25: the verified creator (creator.rs) reads "Name - Creator"
/// (then " - Host" when hosting), the whole row gold (apt_text::gold), which no name can be.
fn session_label(name: &str, host: bool, friend: bool, creator: bool) -> String {
    match (creator, host, friend) {
        (true, true, _) => crate::apt_text::gold(&format!("{name}{} - Host", crate::creator::MARK)),
        (true, false, _) => crate::apt_text::gold(&format!("{name}{}", crate::creator::MARK)),
        (_, true, _) => crate::apt_text::literal(&format!("{name} - Host")),
        (_, _, true) => crate::apt_text::literal(&format!("{name} - Friend")),
        _ => crate::apt_text::literal(name),
    }
}

/// The list's rows; an empty list is one header row with [`empty_text`].
pub(crate) fn rows(kind: ListKind, members: &[Member], local_actor: u64, friends: &[Friend], recent: &[RecentPlayer]) -> Vec<Row> {
    let is_friend = |steam: u64| steam != 0 && friends.iter().any(|f| f.id == steam);
    // In Skate3Rust first, then by name.
    let mut online: Vec<&Friend> = friends.iter().filter(|f| f.online).collect();
    online.sort_by_cached_key(|f| (f.connect.is_none(), f.name.to_lowercase()));
    let rows: Vec<Row> = match kind {
        // C4: a copied name reads "Name #2" and the host and friends carry a mark after the name; a name is a literal,
        // never looked up as a string ID ("ID_ONLINE_EVENT_HOSTLEFT" as a name would read as the game's notice).
        // ponytail: the marks are text; the reserved-word filter (skate-net) keeps "Host" out of names. User 10-25: the
        // verified creator (creator.rs) reads "Name - Creator", the whole row gold, which no name can be.
        ListKind::Session => (members.iter().zip(shown_names(members)).filter(|(m, _)| m.actor != local_actor))
            .map(|(m, name)| Row {
                label: session_label(&name, m.host, is_friend(m.steam), crate::creator::is_creator(m.actor)),
                header: false,
                person: Some(Person {
                    name,
                    steam: m.steam,
                    actor: Some(m.actor),
                    host: m.host,
                    friend: is_friend(m.steam),
                    online: true,
                    connect: None,
                    about: Vec::new(),
                }),
            })
            .collect(),
        ListKind::Friends => {
            let mut offline: Vec<&Friend> = friends.iter().filter(|f| !f.online).collect();
            offline.sort_by_cached_key(|f| f.name.to_lowercase());
            let mut rows = Vec::new();
            for (label, group) in [("ID_MESSENGER_GROUP_NAME_ONLINE_FRIENDS", online), ("ID_MESSENGER_GROUP_NAME_OFFLINE_FRIENDS", offline)] {
                if !group.is_empty() {
                    rows.push(header(label));
                    rows.extend(group.into_iter().map(friend_row));
                }
            }
            rows
        }
        ListKind::InviteFriend => online.into_iter().map(friend_row).collect(),
        // Online safety 10-26: names saved before online_roster::verified filtered them.
        ListKind::Recent => (recent.iter())
            .map(|r| (r, crate::chat_filter::filter(&skate_net::roster::shown_name(&r.name), false)))
            .map(|(r, name)| Row {
                label: crate::apt_text::literal(&name),
                header: false,
                person: Some(Person {
                    name,
                    steam: r.steam,
                    actor: None,
                    host: false,
                    friend: is_friend(r.steam),
                    online: false,
                    connect: None,
                    about: Vec::new(),
                }),
            })
            .collect(),
        // No teams yet. The server browser's rows are its own (server_browser::Browser::rows).
        ListKind::Teammates | ListKind::Lobbies => Vec::new(),
    };
    if rows.is_empty() { vec![header(empty_text(kind))] } else { rows }
}

/// Voice chat (user 10-26): a talking player's row reads with a lit mic before the name (button_icons' MicOn, drawn
/// inline as apt_scene draws `<B:...>` glyphs), so a talker anywhere on the map can be found and muted.
pub(crate) fn mark_talkers(rows: &mut [Row], talking: impl Fn(u64) -> bool) {
    for row in rows.iter_mut().filter(|r| r.person.as_ref().and_then(|p| p.actor).is_some_and(&talking)) {
        row.label = crate::apt_text::glyph_first("MicOn", &row.label);
    }
}

/// The actions shown for `person`, in retail order (PS3 View Profile and Refer Demo never shown), each with whether
/// it can be picked.
pub(crate) fn actions(ctx: &Context, person: &Person) -> Vec<(PlayerAction, bool)> {
    use PlayerAction::*;
    // A server browser session: Join alone (greyed on the one we are in), its details under it (`open_actions`).
    if ctx.kind == ListKind::Lobbies {
        return vec![(SkateWith, ctx.presence && person.connect.is_some())];
    }
    let session = ctx.kind == ListKind::Session;
    let host = session && ctx.host;
    let steam = ctx.presence && person.steam != 0;
    let hidden = person.actor.map(crate::custom_graphic::hidden);
    let their_cohost = person.actor.is_some_and(|a| ctx.cohosts.contains(&a));
    // A co-host kicks guests only; the host checks again.
    let kick = host || session && ctx.cohost && !person.host && !their_cohost;
    let here = session && person.actor.is_some();
    let (muted, hid, blocked) = (ctx.shield.has(&ctx.shield.muted, person), ctx.shield.has(&ctx.shield.hidden, person), ctx.shield.is_blocked(person.steam));
    // (action, shown, enabled)
    [
        (SkateWith, !session && person.connect.is_some(), ctx.presence),
        (ViewSkateProfile, true, false),
        (ViewSteamProfile, true, steam),
        (SendFriendInvite, true, steam && !person.friend),
        // Steam only delivers game invites to friends.
        (SendInvite, true, steam && ctx.steam_session && person.actor.is_none() && person.friend),
        (Kick, kick, true),
        // The host resolves the real endpoint SteamID (Session::endpoint), whatever `@player` says. User 10-29: right
        // under Kick, not after the team rows: the popup shows 10 rows, and Make Co-Host was the 14th.
        (MakeHost, host, ctx.steam_session),
        (MakeCoHost, host && person.actor.is_some() && !their_cohost, ctx.cohosts.len() < skate_net::roster::COHOSTS_MAX),
        (RemoveCoHost, host && their_cohost, true),
        // Chat and skater sounds; a blocked player stays muted and hidden whatever these say.
        (Mute, !muted, here),
        (Unmute, muted, true),
        // Not retail (voice chat, user 10-26): Left / Right on the row; greyed while muted or blocked (no voice).
        (VoiceVolume, here, !muted && !blocked),
        (HidePlayer, here && !hid, true),
        (ShowPlayer, here && hid, true),
        // By Steam ID, saved: Direct players have none.
        (Block, !blocked, person.steam != 0),
        (Unblock, blocked, true),
        (HideGraphics, session && hidden == Some(false), true),
        (ShowGraphics, session && hidden == Some(true), true),
        (InviteToTeam, true, false),
        (ViewTeamProfile, true, false),
        (SwitchTeam, host && ctx.teams && person.actor.is_some(), true),
        (SendMessage, true, steam),
    ]
    .into_iter()
    .filter(|&(_, shown, _)| shown)
    .map(|(action, _, enabled)| (action, enabled))
    .collect()
}

pub(crate) fn label(a: PlayerAction) -> &'static str {
    use PlayerAction::*;
    match a {
        SkateWith => "ID_MESSENGER_MENU_JOIN",
        ViewSkateProfile => "ID_ONLINE_HUB_VIEW_SKATEPROFILE",
        ViewSteamProfile => "#View Steam Profile",
        SendFriendInvite => "ID_ONLINE_HUB_SEND_FRIEND_INVITE",
        SendInvite => "ID_ONLINE_HUB_SEND_INVITE",
        Kick => "ID_ONLINE_HUB_KICK_PLAYER",
        Mute => "ID_COMMON_MUTE",
        InviteToTeam => "ID_PLAYER_LIST_INVITE_TO_TEAM",
        ViewTeamProfile => "ID_PLAYER_LIST_VIEW_TEAM_PROFILE",
        MakeHost => "ID_PLAYER_LIST_MAKE_HOST",
        SendMessage => "ID_PLAYER_LIST_SEND_MESSAGE",
        HideGraphics => "#Hide Graphics",
        ShowGraphics => "#Show Graphics",
        MakeCoHost => "#Make Co-Host",
        RemoveCoHost => "#Remove Co-Host",
        SwitchTeam => "ID_ONLINE_HUB_SWITCH_TEAM",
        Unmute => "#Unmute",
        HidePlayer => "#Hide Player",
        ShowPlayer => "#Show Player",
        Block => "#Block Player",
        Unblock => "#Unblock Player",
        VoiceVolume => "#Voice Volume",
    }
}

/// Menu rows as the movie asks for them: labels, then `OnMenuGetItemEnabled(0, i)` per row (menu id 0: both of
/// the movie's menus use the menu_picker default).
/// Not retail (user 10-27: server browser rows ran off the menu): the most a row spans, in its field's units. The rows'
/// fields are 558 wide; the root list's panel shows 400 of them from a row's start, less a margin
/// (the_server_browser_stays_inside_its_panel: at 400 a full row ended 1.8 px past the panel).
const ROW_ROOM: f64 = 384.;

/// The rows' labels, each shrunk into [`ROW_ROOM`] if it is longer (apt_text::fit).
fn answer_menu(screen: &mut Screen, items: impl Iterator<Item = (String, bool)>) {
    let (labels, enabled): (Vec<String>, Vec<bool>) = items.map(|(label, enabled)| (crate::apt_text::fit(&label), enabled)).unzip();
    for (i, enabled) in enabled.into_iter().enumerate() {
        screen.answer(&format!("Game.OnMenuGetItemEnabled(0,{i})"), Json::from(enabled));
    }
    screen.answer("Game.GetMenuItems", Json::from(labels));
}

/// The open list, owned by `OnlineUi`.
pub(crate) struct PlayerList {
    rows: Vec<Row>,
    ctx: Context,
    /// Level 1: the person whose actions show.
    open: Option<(Person, Vec<(PlayerAction, bool)>)>,
    /// `rows` changed since the movie's list was built (rebuilt once back at the root).
    stale: bool,
    /// The movie's highlighted row (`Game.OnMenuHilight`): Left / Right act on a person's Voice Volume there.
    hilight: usize,
}

impl PlayerList {
    pub fn open(screens: &mut Screens, kind: ListKind, rows: Vec<Row>, ctx: Context) -> Result<Self, String> {
        let list = Self { rows, ctx: Context { kind, ..ctx }, open: None, stale: false, hilight: 0 };
        let screen = screens.layer(ONLINE).open(NAME, "friends", &[])?;
        if let Err(error) = list.start(screen) {
            screens.layer(ONLINE).close(NAME);
            return Err(error);
        }
        Ok(list)
    }
    pub fn kind(&self) -> ListKind {
        self.ctx.kind
    }
    fn start(&self, screen: &mut Screen) -> Result<(), String> {
        for (mode, name) in MODES.iter().enumerate() {
            screen.set_global(&format!("FEPlayerListScreenMode_{name}"), Json::from(mode))?;
        }
        let root = screen.bindings.movie.root;
        screen.vm.set(root, crate::apt_movie::FIT_WIDTH, crate::apt_vm::Value::Number(ROW_ROOM))?;
        self.answer_root(screen);
        screen.call("SetState", &[INIT_LIST.into()])?;
        self.browser_legend(screen)?;
        // The root waits stopped on frame 0 for the screen manager to play it in.
        screen.play("intro")
    }
    fn answer_root(&self, screen: &mut Screen) {
        screen.answer("PlayerList.PlayerListTitle", Json::from(title(self.ctx.kind)));
        screen.answer("PlayerList.PlayerListGetHeaderArray", self.rows.iter().map(|r| r.header).collect());
        answer_menu(screen, self.rows.iter().map(|r| (r.label.clone(), !r.header)));
    }

    /// Each frame while open: the context, and the rows when they may have changed (`None`: unchanged). At the root,
    /// changed rows (players joining or leaving, friends coming online) rebuild the list (in a person's actions they
    /// are kept for the way back); in a person's actions, a changed context (host migrated, Steam lost) rebuilds
    /// those. False when the screen is gone (it stopped; already logged).
    pub fn refresh(&mut self, screens: &mut Screens, rows: Option<Vec<Row>>, ctx: Context) -> bool {
        let Some(screen) = screens.layer(ONLINE).get(NAME) else { return false };
        if let Err(error) = self.update(screen, rows, ctx) {
            error!("Player List stopped: {error}");
            screens.layer(ONLINE).close(NAME);
            return false;
        }
        true
    }
    /// [`PlayerList::refresh`] on its screen.
    fn update(&mut self, screen: &mut Screen, rows: Option<Vec<Row>>, ctx: Context) -> Result<(), String> {
        // Only OnMenuSelect matters, and it comes from `input`.
        screen.take_calls();
        self.ctx = Context { kind: self.ctx.kind, ..ctx };
        let mut relabel = Vec::new();
        if let Some(rows) = rows.filter(|rows| *rows != self.rows) {
            // The same people with other labels (voice chat: a talker's mic) are rewritten in place at the root, the
            // highlight kept; anything else builds the list again.
            let same = rows.len() == self.rows.len() && rows.iter().zip(&self.rows).all(|(a, b)| (a.header, &a.person) == (b.header, &b.person));
            if same && self.open.is_none() && !self.stale {
                relabel = (0..rows.len()).filter(|&i| rows[i].label != self.rows[i].label).collect();
            } else {
                self.stale = true;
            }
            self.rows = rows;
        }
        match &self.open {
            Some((person, shown)) if actions(&self.ctx, person) != *shown => {
                let person = person.clone();
                self.open_actions(screen, person)
            }
            None if self.stale => {
                self.stale = false;
                self.answer_root(screen);
                screen.call("SetState", &[INIT_LIST.into()])?;
                self.browser_legend(screen)
            }
            None if !relabel.is_empty() => {
                // menu_picker's SetItemText(str, index) writes a row's text; UpdateText redraws the rows from it.
                self.answer_root(screen);
                for i in relabel {
                    screen.call_at(MENU, "SetItemText", &[crate::apt_text::fit(&self.rows[i].label).into(), i.into()])?;
                }
                screen.call_at(MENU, "UpdateText", &[]).map(drop)
            }
            _ => Ok(()),
        }
    }

    /// A pad press. A stopped screen reports [`Effect::Closed`].
    pub fn input(&mut self, screens: &mut Screens, key: Key) -> Option<Effect> {
        let Some(screen) = screens.layer(ONLINE).get(NAME) else { return Some(Effect::Closed) };
        match self.step(screen, key) {
            Ok(effect) => effect,
            Err(error) => {
                error!("Player List stopped: {error}");
                screens.layer(ONLINE).close(NAME);
                Some(Effect::Closed)
            }
        }
    }
    fn step(&mut self, screen: &mut Screen, key: Key) -> Result<Option<Effect>, String> {
        match key {
            Key::Back if self.open.is_some() => {
                self.root(screen)?;
                return Ok(None);
            }
            Key::Back => {
                screen.call("SetState", &[HIDE_LIST.into()])?;
                return Ok(Some(Effect::Closed));
            }
            Key::Left | Key::Right => return self.slide(screen, if key == Key::Right { 1 } else { -1 }),
            Key::Up | Key::Down | Key::Accept => {}
        }
        screen.take_calls();
        screen.input(key)?;
        let mut selected = None;
        for (call, args) in screen.take_calls() {
            let index = args.get(1).and_then(Json::as_u64).map(|i| i as usize);
            match call.as_str() {
                "Game.OnMenuHilight" => self.hilight = index.unwrap_or(0),
                "Game.OnMenuSelect" => selected = selected.or(index),
                _ => {}
            }
        }
        let Some(index) = selected else { return Ok(None) };
        if let Some((person, actions)) = &self.open {
            // Greyed actions are skipped by the picker, but it rests on one when every action is greyed.
            let Some(&(action, true)) = actions.get(index) else { return Ok(None) };
            let effect = Effect::Act(action, person.clone());
            // Back to the list after Kick / Make Host (the row or the host rows go at the next refresh); the Steam
            // client actions stay on the person.
            if matches!(action, PlayerAction::Kick | PlayerAction::MakeHost) {
                self.root(screen)?;
            }
            return Ok(Some(effect));
        }
        let Some(person) = self.rows.get(index).and_then(|r| r.person.clone()) else { return Ok(None) };
        self.open_actions(screen, person)?;
        Ok(None)
    }
    /// Level 1: `person`'s actions under their name, then their details greyed (a server browser session's).
    fn open_actions(&mut self, screen: &mut Screen, person: Person) -> Result<(), String> {
        let actions = actions(&self.ctx, &person);
        screen.answer("PlayerList.PlayerListSubMenuTitle", Json::from(crate::apt_text::literal(&person.name)));
        let details = person.about.iter().map(|line| (crate::apt_text::literal(line), false));
        let volume = self.ctx.shield.volume(&person);
        let text = |a| if a == PlayerAction::VoiceVolume { voice_label(volume) } else { label(a).to_owned() };
        answer_menu(screen, actions.iter().map(|&(a, enabled)| (text(a), enabled)).chain(details));
        screen.take_calls();
        screen.call("SetState", &[SUB_MENU.into()])?;
        // The picker rests on the first row it can pick, unless it says otherwise.
        self.hilight = actions.iter().position(|&(_, enabled)| enabled).unwrap_or(0);
        if let Some((_, args)) = screen.take_calls().into_iter().rfind(|(call, _)| call == "Game.OnMenuHilight") {
            self.hilight = args.get(1).and_then(Json::as_u64).map_or(self.hilight, |i| i as usize);
        }
        self.open = Some((person, actions));
        self.browser_legend(screen)
    }
    /// Left / Right on a person's Voice Volume: 10 % a step, the row's text changed where it stands.
    fn slide(&mut self, screen: &mut Screen, step: i32) -> Result<Option<Effect>, String> {
        let Some((person, actions)) = &self.open else { return Ok(None) };
        if actions.get(self.hilight) != Some(&(PlayerAction::VoiceVolume, true)) {
            return Ok(None);
        }
        let person = person.clone();
        let old = self.ctx.shield.volume(&person);
        let volume = step_volume(old, step);
        if volume == old {
            return Ok(None);
        }
        self.ctx.shield.set_volume(&person, volume);
        // menu_picker's SetItemText(str, index) writes the row's text; UpdateText redraws the rows from it.
        screen.call_at(SUBMENU, "SetItemText", &[voice_label(volume).into(), self.hilight.into()])?;
        screen.call_at(SUBMENU, "UpdateText", &[])?;
        Ok(Some(Effect::Volume(person, volume)))
    }
    fn root(&mut self, screen: &mut Screen) -> Result<(), String> {
        self.open = None;
        self.answer_root(screen);
        screen.call("SetState", &[ROOT_MENU.into()])?;
        self.browser_legend(screen)
    }
    /// At the list, not in a person's (a session's) actions.
    pub fn at_root(&self) -> bool {
        self.open.is_none()
    }
    /// Not retail (user 10-27): the server browser's legend, whole: the movie's own B Back and A Select (its list
    /// builder's), then at the list Y Search and X Refresh (online_menu acts on them; LB / RB lead the sort row, as
    /// a fifth button finds no room). The movie keeps a panel's legend through its states (a session's details showed
    /// the list's buttons, render 10-27), so it is built again at each.
    fn browser_legend(&self, screen: &mut Screen) -> Result<(), String> {
        if self.ctx.kind != ListKind::Lobbies {
            return Ok(());
        }
        screen.call_at(PANEL, "ClearButtonItems", &[])?;
        let mut items = vec![("B", "ID_COMMON_BACK".to_owned()), ("A", "ID_COMMON_SELECT".to_owned())];
        if self.at_root() {
            items.extend([("Y", crate::apt_text::literal("Search")), ("X", crate::apt_text::literal("Refresh"))]);
        }
        for (button, words) in items {
            screen.call_at(PANEL, "AddButtonItem", &[button.into(), words.into()])?;
        }
        screen.call_at(PANEL, "RefreshButtonHelp", &[]).map(drop)
    }

    pub fn close(self, screens: &mut Screens) {
        screens.layer(ONLINE).close(NAME);
    }
}

/// C4: names as the session shows them, in roster order (the host first): a name already taken (case and outer
/// spaces aside) reads "Name #2", "Name #3"..., so a copy of the host's or a friend's name never reads as theirs.
pub(crate) fn shown_names(members: &[Member]) -> Vec<String> {
    let mut taken = BTreeSet::new();
    let key = |s: &str| s.trim().to_lowercase();
    (members.iter())
        .map(|m| {
            let (mut name, mut n) = (m.name.clone(), 2);
            while !taken.insert(key(&name)) {
                name = format!("{} #{n}", m.name);
                n += 1;
            }
            name
        })
        .collect()
}
/// `actor`'s [`shown_names`] name; None if the roster doesn't name them yet.
pub(crate) fn shown_name(members: &[Member], actor: u64) -> Option<String> {
    members.iter().zip(shown_names(members)).find(|(m, _)| m.actor == actor).map(|(_, n)| n)
}

// Not retail (online safety 3.12, C6/C7, Q7): Mute drops a player's chat and skater sounds, Hide stops drawing their
// skater and name tag, Block is both, for good (by Steam ID, saved beside the settings). Their chat goes in
// online_chat, their sounds in player_audio::remote, their skater and model in multiplayer::render, their tag in
// online_hud. ponytail: Steam IDs are the players' own `@player` claims until the relay vouches for them (C3).

/// Settings > Audio: Text Chat (and whether it is Filtered: swearing masked; slurs always are, chat_filter), Chat From
/// (friends only), Custom Models (0 everyone, 1 friends, 2 nobody); set by
/// graphics_menu::publish.
pub(crate) static CHAT: AtomicBool = AtomicBool::new(true);
pub(crate) static CHAT_FILTER: AtomicBool = AtomicBool::new(true);
pub(crate) static CHAT_FRIENDS: AtomicBool = AtomicBool::new(false);
pub(crate) static CUSTOM_MODELS: AtomicU8 = AtomicU8::new(0);
/// Settings > Audio > Voice Chat: Voice Chat (On) and Voice From (Friends); set by graphics_menu::publish.
pub(crate) static VOICE: AtomicBool = AtomicBool::new(true);
pub(crate) static VOICE_FRIENDS: AtomicBool = AtomicBool::new(false);

/// A player as Mute and Hide remember them: by Steam ID when known (a rejoin under a new actor id stays muted), else
/// by this session's actor id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Who {
    Steam(u64),
    Actor(u64),
}
fn who(steam: u64, actor: Option<u64>) -> Option<Who> {
    if steam != 0 { Some(Who::Steam(steam)) } else { actor.map(Who::Actor) }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Blocked {
    pub players: Vec<RecentPlayer>,
}
/// voice-players.json (user 10-26): Steam players' Mute and Voice Volume, kept across game runs, least recently changed
/// first.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct VoicePlayers {
    pub players: Vec<VoicePlayer>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct VoicePlayer {
    pub steam: u64,
    pub name: String,
    pub volume: u16,
    pub muted: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Shield {
    /// This game run's picks (actor ones end with the session).
    pub muted: BTreeSet<Who>,
    pub hidden: BTreeSet<Who>,
    pub blocked: Blocked,
    /// Voice Volume picks, 0-200 % (100 unless picked); a Steam player's is also in `voice`.
    pub volumes: BTreeMap<Who, u16>,
    /// What voice-players.json holds ([`VoicePlayers`]).
    pub voice: VoicePlayers,
    /// What [`Shield::judge`] made of the session's players, by actor: muted or blocked, hidden or blocked, friends,
    /// and the voices heard (neither muted nor blocked) at their volumes.
    quiet: BTreeSet<u64>,
    ghosts: BTreeSet<u64>,
    friends: BTreeSet<u64>,
    voices: BTreeMap<u64, u16>,
}
impl Shield {
    const EMPTY: Self = Self {
        muted: BTreeSet::new(),
        hidden: BTreeSet::new(),
        blocked: Blocked { players: Vec::new() },
        volumes: BTreeMap::new(),
        voice: VoicePlayers { players: Vec::new() },
        quiet: BTreeSet::new(),
        ghosts: BTreeSet::new(),
        friends: BTreeSet::new(),
        voices: BTreeMap::new(),
    };
    pub fn has(&self, set: &BTreeSet<Who>, p: &Person) -> bool {
        who(p.steam, p.actor).is_some_and(|w| set.contains(&w))
    }
    pub fn is_blocked(&self, steam: u64) -> bool {
        steam != 0 && self.blocked.players.iter().any(|p| p.steam == steam)
    }
    /// The verdicts for the session's players (`local` is us).
    pub fn judge(&mut self, members: &[Member], local: u64, friends: &[Friend]) {
        let (mut quiet, mut ghosts, mut friendly, mut voices) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new(), BTreeMap::new());
        for m in members.iter().filter(|m| m.actor != local) {
            let w = who(m.steam, Some(m.actor));
            let blocked = self.is_blocked(m.steam);
            if blocked || w.is_some_and(|w| self.muted.contains(&w)) {
                quiet.insert(m.actor);
            } else {
                voices.insert(m.actor, w.and_then(|w| self.volumes.get(&w).copied()).unwrap_or(100));
            }
            if blocked || w.is_some_and(|w| self.hidden.contains(&w)) {
                ghosts.insert(m.actor);
            }
            if m.steam != 0 && friends.iter().any(|f| f.id == m.steam) {
                friendly.insert(m.actor);
            }
        }
        (self.quiet, self.ghosts, self.friends, self.voices) = (quiet, ghosts, friendly, voices);
    }
    /// A Player List pick; true if a saved list changed (the block list; voice-players.json for a Steam player's Mute).
    pub fn pick(&mut self, action: PlayerAction, p: &Person) -> bool {
        let Some(w) = who(p.steam, p.actor) else { return false };
        match action {
            PlayerAction::Mute => {
                self.muted.insert(w);
                return self.note_voice(p);
            }
            PlayerAction::Unmute => {
                self.muted.remove(&w);
                return self.note_voice(p);
            }
            PlayerAction::HidePlayer => drop(self.hidden.insert(w)),
            PlayerAction::ShowPlayer => drop(self.hidden.remove(&w)),
            PlayerAction::Block if p.steam != 0 && !self.is_blocked(p.steam) => {
                self.blocked.players.push(RecentPlayer { name: p.name.clone(), steam: p.steam });
                return true;
            }
            PlayerAction::Unblock if self.is_blocked(p.steam) => {
                self.blocked.players.retain(|b| b.steam != p.steam);
                return true;
            }
            _ => {}
        }
        false
    }
    /// A new session: its actor ids mean nobody now.
    fn new_session(&mut self) {
        self.muted.retain(|w| matches!(w, Who::Steam(_)));
        self.hidden.retain(|w| matches!(w, Who::Steam(_)));
        self.volumes.retain(|w, _| matches!(w, Who::Steam(_)));
    }
    /// Their skater sounds (and chat) are dropped.
    pub fn quiet(&self, actor: u64) -> bool {
        self.quiet.contains(&actor)
    }
    /// Their skater and name tag aren't drawn.
    pub fn ghost(&self, actor: u64) -> bool {
        self.ghosts.contains(&actor)
    }
    /// Their chat shows: Text Chat on, Chat From lets them (a sender the roster doesn't name yet is no friend), and
    /// not muted or blocked.
    pub fn reads(&self, actor: u64, chat: bool, friends_only: bool) -> bool {
        chat && !self.quiet(actor) && (!friends_only || self.friends.contains(&actor))
    }
    /// Their imported or native model shows (`setting`: a [`CUSTOM_MODELS`] value); else their stock skater.
    pub fn models(&self, actor: u64, setting: u8) -> bool {
        !self.ghost(actor) && match setting {
            0 => true,
            1 => self.friends.contains(&actor),
            _ => false,
        }
    }
    /// Their Voice Volume, 0-200 % (100 unless picked).
    pub fn volume(&self, p: &Person) -> u16 {
        who(p.steam, p.actor).and_then(|w| self.volumes.get(&w).copied()).unwrap_or(100)
    }
    /// A Voice Volume pick; true if voice-players.json changed (a Steam player).
    pub fn set_volume(&mut self, p: &Person, volume: u16) -> bool {
        let Some(w) = who(p.steam, p.actor) else { return false };
        match volume.min(200) {
            100 => drop(self.volumes.remove(&w)),
            volume => drop(self.volumes.insert(w, volume)),
        }
        self.note_voice(p)
    }
    /// Their voice plays, at this volume: Voice Chat on, Voice From lets them, and neither muted nor blocked (a speaker
    /// the roster doesn't name yet is not heard).
    pub fn hears(&self, actor: u64, voice_on: bool, friends_only: bool) -> Option<u16> {
        let allowed = voice_on && (!friends_only || self.friends.contains(&actor));
        self.voices.get(&actor).copied().filter(|_| allowed)
    }
    /// voice-players.json's players back into `muted` and `volumes`: Steam players only, each once (the last entry),
    /// the newest 1000, volumes in the row's range and steps.
    pub fn remember(&mut self, saved: VoicePlayers) {
        let mut seen = BTreeSet::new();
        let mut players: Vec<VoicePlayer> = saved.players.into_iter().rev().filter(|v| v.steam != 0 && seen.insert(v.steam)).collect();
        players.truncate(VOICE_PLAYERS_MAX);
        players.reverse();
        for v in &mut players {
            v.volume = v.volume.min(200) / 10 * 10;
            if v.muted {
                self.muted.insert(Who::Steam(v.steam));
            }
            if v.volume != 100 {
                self.volumes.insert(Who::Steam(v.steam), v.volume);
            }
        }
        players.retain(|v| v.muted || v.volume != 100);
        self.voice = VoicePlayers { players };
    }
    /// After a Mute or Voice Volume pick: a Steam player's voice-players.json entry moves last (the most recently
    /// changed), or goes when they are back at 100 % and unmuted; past 1000 the least recently changed go. True if the
    /// file's contents changed.
    fn note_voice(&mut self, p: &Person) -> bool {
        if p.steam == 0 {
            return false;
        }
        let w = Who::Steam(p.steam);
        let (volume, muted) = (self.volumes.get(&w).copied().unwrap_or(100), self.muted.contains(&w));
        let before = self.voice.players.len();
        self.voice.players.retain(|v| v.steam != p.steam);
        let removed = self.voice.players.len() != before;
        if !muted && volume == 100 {
            return removed;
        }
        self.voice.players.push(VoicePlayer { steam: p.steam, name: p.name.clone(), volume, muted });
        let over = self.voice.players.len().saturating_sub(VOICE_PLAYERS_MAX);
        self.voice.players.drain(..over);
        true
    }
}

static SHIELD: Mutex<Shield> = Mutex::new(Shield::EMPTY);
static BLOCKED_PATH: OnceLock<PathBuf> = OnceLock::new();
static VOICE_PATH: OnceLock<PathBuf> = OnceLock::new();
fn shield() -> MutexGuard<'static, Shield> {
    SHIELD.lock().unwrap_or_else(|e| e.into_inner())
}
pub(crate) fn snapshot() -> Shield {
    shield().clone()
}
pub(crate) fn quiet(actor: u64) -> bool {
    shield().quiet(actor)
}
pub(crate) fn ghost(actor: u64) -> bool {
    shield().ghost(actor)
}
/// On the block list (by Steam ID; online_roster kicks them from a lobby we host).
pub(crate) fn blocked(steam: u64) -> bool {
    shield().is_blocked(steam)
}
/// C4: a Steam friend in this session (their chat line and name tag are marked).
pub(crate) fn friend(actor: u64) -> bool {
    shield().friends.contains(&actor)
}
pub(crate) fn reads(actor: u64) -> bool {
    shield().reads(actor, CHAT.load(Ordering::Relaxed), CHAT_FRIENDS.load(Ordering::Relaxed))
}
pub(crate) fn models(actor: u64) -> bool {
    shield().models(actor, CUSTOM_MODELS.load(Ordering::Relaxed))
}
/// online_menu: Mute, Hide or Block picked in the Player List.
pub(crate) fn pick(action: PlayerAction, person: &Person) {
    let mut shield = shield();
    if !shield.pick(action, person) {
        return;
    }
    if let Some(path) = BLOCKED_PATH.get()
        && let Err(e) = save_blocked(&shield.blocked, path)
    {
        warn!("Blocked players: {e}");
    }
    save_voices(&shield);
}
/// online_menu: the Player List's Voice Volume moved; a Steam player's is saved.
pub(crate) fn set_volume(person: &Person, volume: u16) {
    let mut shield = shield();
    if shield.set_volume(person, volume) {
        save_voices(&shield);
    }
}
/// The voice engine (voice.rs): `actor`'s voice volume, None while they are not heard (Settings > Voice Chat, Voice
/// From, Mute, Block).
pub(crate) fn voice_volume(actor: u64) -> Option<u16> {
    shield().hears(actor, VOICE.load(Ordering::Relaxed), VOICE_FRIENDS.load(Ordering::Relaxed))
}
fn save_voices(shield: &Shield) {
    if let Some(path) = VOICE_PATH.get()
        && let Err(e) = save_voice(&shield.voice, path)
    {
        warn!("Voice players: {e}");
    }
}
pub(crate) fn blocked_path(asset_root: &Path) -> PathBuf {
    crate::saves::settings_dir(asset_root).join("blocked-players.json") // Not retail, as recent-players.json
}
/// The saved block list (none if missing or unreadable), kept for [`pick`] to save to.
pub(crate) fn load_blocked(path: PathBuf) {
    shield().blocked = crate::saves::load_json(&path).ok().unwrap_or_default();
    let _ = BLOCKED_PATH.set(path);
}
fn save_blocked(blocked: &Blocked, path: &Path) -> std::io::Result<()> {
    crate::saves::write_atomic(path, &serde_json::to_vec_pretty(blocked).map_err(std::io::Error::other)?)
}
/// Left / Right on Voice Volume: 10 % a step within 0-200 %.
pub(crate) fn step_volume(volume: u16, step: i32) -> u16 {
    (i32::from(volume) + 10 * step).clamp(0, 200) as u16
}
/// The Voice Volume row: "Voice Volume: < 60% >" (the movie's fonts have no U+25C0 / U+25B6; the FOV slider's arrows).
pub(crate) fn voice_label(volume: u16) -> String {
    format!("#Voice Volume: < {volume}% >")
}
pub(crate) fn voice_path(asset_root: &Path) -> PathBuf {
    crate::saves::settings_dir(asset_root).join("voice-players.json") // Not retail, as blocked-players.json
}
/// The saved Mutes and Voice Volumes (none if missing or unreadable), kept for [`pick`] and [`set_volume`] to save to.
pub(crate) fn load_voice(path: PathBuf) {
    shield().remember(crate::saves::load_json(&path).ok().unwrap_or_default());
    let _ = VOICE_PATH.set(path);
}
fn save_voice(voice: &VoicePlayers, path: &Path) -> std::io::Result<()> {
    crate::saves::write_atomic(path, &serde_json::to_vec_pretty(voice).map_err(std::io::Error::other)?)
}
/// Each frame, before the chat, sounds and skaters read them: the session's verdicts.
pub(crate) fn guard(
    roster: Res<crate::online_roster::OnlineRoster>,
    presence: Res<crate::presence::Presence>,
    net: Res<crate::multiplayer::Multiplayer>,
    mut session: Local<Option<(u64, u64)>>,
) {
    let key = net.session().map(|s| (s.session, s.local));
    let mut shield = shield();
    if *session != key {
        *session = key;
        shield.new_session();
    }
    shield.judge(&roster.members, roster.local_actor, &presence.friends);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::online_menu::ListKind;
    use skate_net::{presence::Friend, roster::Member};
    fn p(name: &str, steam: u64, actor: Option<u64>, friend: bool) -> Person {
        Person { name: name.into(), steam, actor, host: false, friend, online: true, connect: None, about: Vec::new() }
    }
    fn ctx(kind: ListKind, host: bool, steam_session: bool, presence: bool) -> Context {
        Context { kind, host, steam_session, presence, cohost: false, cohosts: vec![], teams: false, shield: Shield::default() }
    }
    #[test]
    fn session_actions_in_retail_order_for_the_host() {
        let a = actions(&ctx(ListKind::Session, true, true, true), &p("Ann", 7, Some(2), false));
        use PlayerAction::*;
        assert_eq!(a, vec![(ViewSkateProfile, false), (ViewSteamProfile, true), (SendFriendInvite, true), (SendInvite, false),
                           (Kick, true), (MakeHost, true), (MakeCoHost, true), (Mute, true), (VoiceVolume, true), (HidePlayer, true), (Block, true), (HideGraphics, true),
                           (InviteToTeam, false), (ViewTeamProfile, false), (SendMessage, true)]);
    }
    #[test]
    fn cohosts_kick_guests_only_and_make_no_hosts() {
        use PlayerAction::*;
        let host = Context { cohosts: vec![2], ..ctx(ListKind::Session, true, true, true) };
        let a = actions(&host, &p("Ann", 7, Some(2), false));
        assert!(a.contains(&(RemoveCoHost, true)) && !a.iter().any(|(x, _)| *x == MakeCoHost));
        let full = Context { cohosts: (10..10 + skate_net::roster::COHOSTS_MAX as u64).collect(), ..host.clone() };
        assert!(actions(&full, &p("Bo", 8, Some(3), false)).contains(&(MakeCoHost, false)), "greyed at the limit");
        let cohost = Context { cohost: true, cohosts: vec![2, 5], ..ctx(ListKind::Session, false, true, true) };
        let guest = actions(&cohost, &p("Bo", 8, Some(3), false));
        assert!(guest.contains(&(Kick, true)));
        assert!(!guest.iter().any(|(x, _)| matches!(x, MakeHost | MakeCoHost | RemoveCoHost)));
        let the_host = Person { host: true, ..p("Hal", 9, Some(1), false) };
        assert!(!actions(&cohost, &the_host).iter().any(|(x, _)| *x == Kick), "never the host");
        assert!(!actions(&cohost, &p("Cy", 6, Some(5), false)).iter().any(|(x, _)| *x == Kick), "never another co-host");
        // T1: only the host moves players between a team lobby's teams.
        let teams = Context { teams: true, ..host.clone() };
        assert!(actions(&teams, &p("Ann", 7, Some(2), false)).contains(&(SwitchTeam, true)));
        assert!(!actions(&Context { teams: true, ..cohost }, &p("Bo", 8, Some(3), false)).iter().any(|(x, _)| *x == SwitchTeam));
        assert!(!actions(&host, &p("Ann", 7, Some(2), false)).iter().any(|(x, _)| *x == SwitchTeam), "no teams outside a team lobby");
    }
    #[test]
    fn guests_never_see_host_rows_and_direct_greys_make_host() {
        use PlayerAction::*;
        let guest = actions(&ctx(ListKind::Session, false, true, true), &p("Ann", 7, Some(2), false));
        assert!(!guest.iter().any(|(a, _)| matches!(a, Kick | MakeHost)));
        let direct = actions(&ctx(ListKind::Session, true, false, true), &p("Ann", 0, Some(2), false));
        assert!(direct.contains(&(MakeHost, false)) && direct.contains(&(Kick, true)));
        assert!(direct.contains(&(ViewSteamProfile, false)), "no SteamID in Direct");
        let no_helper = actions(&ctx(ListKind::Session, true, true, true), &p("Ann", 0, Some(2), false));
        assert!(no_helper.contains(&(MakeHost, true)), "the host knows the endpoint without @player.steam");
        let stranger = actions(&ctx(ListKind::Recent, false, true, true), &p("Dee", 9, None, false));
        assert!(stranger.contains(&(SendInvite, false)), "Steam invites reach friends only");
    }
    #[test]
    fn friend_rows_offer_skate_with_and_invite() {
        use PlayerAction::*;
        let mut f = p("Bob", 9, None, true);
        f.connect = Some("sk8:5:University".into());
        let a = actions(&ctx(ListKind::Friends, false, true, true), &f);
        assert_eq!(a[0], (SkateWith, true));
        assert!(a.contains(&(SendInvite, true)) && a.contains(&(SendFriendInvite, false)), "already a friend");
        assert!(!a.iter().any(|(x, _)| matches!(x, Kick | MakeHost)));
    }
    /// User 10-25: a server browser session offers Join alone, greyed on the session we are in.
    #[test]
    fn a_browser_session_offers_join_alone() {
        use PlayerAction::*;
        let mut session = p("Ryan's Park", 0, None, false);
        assert_eq!(actions(&ctx(ListKind::Lobbies, false, true, true), &session), [(SkateWith, false)], "our own");
        session.connect = Some("sk8:5:University".into());
        assert_eq!(actions(&ctx(ListKind::Lobbies, false, true, true), &session), [(SkateWith, true)]);
        assert_eq!(actions(&ctx(ListKind::Lobbies, false, true, false), &session), [(SkateWith, false)], "Steam gone");
    }
    #[test]
    fn no_steam_greys_every_steam_action() {
        let a = actions(&ctx(ListKind::Friends, true, true, false), &p("Bob", 9, None, true));
        // Block is this machine's own list, not a Steam action.
        assert!(a.iter().filter(|(x, _)| !matches!(x, PlayerAction::Block | PlayerAction::Unblock)).all(|(_, enabled)| !enabled));
        assert!(a.contains(&(PlayerAction::Block, true)));
    }
    #[test]
    fn friend_rows_are_grouped_online_first() {
        let friends = vec![
            Friend { id: 1, name: "zed".into(), online: false, connect: None },
            Friend { id: 2, name: "Amy".into(), online: true, connect: None },
            Friend { id: 3, name: "bo".into(), online: true, connect: Some("sk8:4:University".into()) },
        ];
        let r = rows(ListKind::Friends, &[], 0, &friends, &[]);
        let labels: Vec<_> = r.iter().map(|r| (r.label.clone(), r.header)).collect();
        let lit = |s: &str| (crate::apt_text::literal(s), false);
        assert_eq!(labels, [("ID_MESSENGER_GROUP_NAME_ONLINE_FRIENDS".into(), true), lit("bo - Skate3Rust"), lit("Amy"),
                            ("ID_MESSENGER_GROUP_NAME_OFFLINE_FRIENDS".into(), true), lit("zed")], "a friend's name is a literal");
        let invite = rows(ListKind::InviteFriend, &[], 0, &friends, &[]);
        assert_eq!(invite.len(), 2, "online friends only, no headers");
        assert_eq!(rows(ListKind::Friends, &[], 0, &[], &[]), vec![Row { label: empty_text(ListKind::Friends).into(), header: true, person: None }]);
    }
    #[test]
    fn session_rows_skip_self_and_target_actor_ids_even_with_duplicate_names() {
        let members = vec![
            Member { actor: 1, name: "The Legend".into(), steam: 0, host: true },
            Member { actor: 2, name: "The Legend".into(), steam: 0, host: false },
            Member { actor: 3, name: "The Legend".into(), steam: 0, host: false },
        ];
        let r = rows(ListKind::Session, &members, 1, &[], &[]);
        assert_eq!(r.iter().map(|r| r.person.as_ref().unwrap().actor).collect::<Vec<_>>(), [Some(2), Some(3)]);
        assert_eq!(title(ListKind::Session), "ID_PLAYER_LIST_PLAYERS_TITLE");
        assert_eq!(empty_text(ListKind::Session), "ID_PLAYER_LIST_EMPTY_PLAYER_LIST");
    }
    #[test]
    fn recent_rows_know_friends_and_teammates_are_empty() {
        let friends = vec![Friend { id: 5, name: "Cy".into(), online: true, connect: None }];
        let recent = vec![RecentPlayer { name: "Cy".into(), steam: 5 }, RecentPlayer { name: "Dee".into(), steam: 0 }];
        let r = rows(ListKind::Recent, &[], 0, &friends, &recent);
        assert_eq!(r.iter().map(|r| r.person.as_ref().unwrap().friend).collect::<Vec<_>>(), [true, false]);
        assert_eq!(rows(ListKind::Teammates, &[], 0, &friends, &recent)[0].label, "ID_PLAYER_LIST_EMPTY_TEAMMATES_LIST");
    }

    /// The movie as PlayerList drives it (SKATE_ONLINE_UI_ROOT = private/online-ui): the root list, a person's
    /// actions with greyed ones skipped, the picked action, and Back out (SKATE_ONLINE_UI_SHOTS: renders).
    #[test]
    #[ignore = "needs INST\\assets\\private\\online-ui"]
    fn movie_lists_people_and_reports_the_picked_action() {
        let root = std::path::PathBuf::from(std::env::var("SKATE_ONLINE_UI_ROOT").expect("SKATE_ONLINE_UI_ROOT"));
        let mut screen = Screen::load(&root.join("runtime/friends.json"), &[]).unwrap();
        let members = vec![
            Member { actor: 1, name: "Host".into(), steam: 11, host: true },
            Member { actor: 2, name: "The Legend".into(), steam: 22, host: false },
            Member { actor: 3, name: "The Legend".into(), steam: 33, host: false },
        ];
        let context = ctx(ListKind::Session, true, true, true);
        let mut list = PlayerList { rows: rows(ListKind::Session, &members, 1, &[], &[]), ctx: context, open: None, stale: false, hilight: 0 };
        list.start(&mut screen).unwrap();
        let shot = |screen: &mut Screen, name: &str| {
            for _ in 0..30 {
                screen.advance().unwrap();
            }
            let draws = screen.draws().unwrap();
            assert!(!draws.is_empty(), "{name} draws nothing");
            if let Ok(out) = std::env::var("SKATE_ONLINE_UI_SHOTS") {
                crate::core_menu_runtime::tests::raster_draws(&draws, &root, &std::path::Path::new(&out).join(format!("{name}.png")));
            }
        };
        shot(&mut screen, "playerlist-root");
        // The second "The Legend" (actor 3).
        assert_eq!(list.step(&mut screen, Key::Down).unwrap(), None);
        assert_eq!(list.step(&mut screen, Key::Accept).unwrap(), None);
        assert_eq!(list.open.as_ref().map(|(p, _)| p.actor), Some(Some(3)));
        shot(&mut screen, "playerlist-actions");
        // View Steam Profile is the first enabled action (View Skate Profile is greyed).
        let effect = list.step(&mut screen, Key::Accept).unwrap();
        assert!(matches!(&effect, Some(Effect::Act(PlayerAction::ViewSteamProfile, p)) if p.actor == Some(3)), "{effect:?}");
        assert!(list.open.is_some(), "Steam client actions stay on the person");
        // Kick is two enabled rows below (Send Invite is greyed).
        for _ in 0..2 {
            list.step(&mut screen, Key::Down).unwrap();
        }
        let effect = list.step(&mut screen, Key::Accept).unwrap();
        assert!(matches!(&effect, Some(Effect::Act(PlayerAction::Kick, p)) if p.actor == Some(3)), "{effect:?}");
        assert!(list.open.is_none(), "back at the list after Kick");
        assert_eq!(list.step(&mut screen, Key::Back).unwrap(), Some(Effect::Closed));
    }

    /// The server browser (user 10-27: its text ran off the menu, "Your Session" most, and a description could not be
    /// seen): with the longest rows a session makes, every row's text ends inside the list's panel, in the list and in
    /// a session's details (SKATE_ONLINE_UI_ROOT; SKATE_ONLINE_UI_SHOTS: renders).
    #[test]
    #[ignore = "needs INST\\assets\\private\\online-ui"]
    fn the_server_browser_stays_inside_its_panel() {
        let root = std::path::PathBuf::from(std::env::var("SKATE_ONLINE_UI_ROOT").expect("SKATE_ONLINE_UI_ROOT"));
        let mut screen = Screen::load(&root.join("runtime/friends.json"), &[]).unwrap();
        let session = |id, name: &str, players, capacity, ping, mode: &str, description: &str| skate_net::directory::Row {
            id,
            map: "New_San_Vanelona".into(),
            players,
            capacity,
            compatible: true,
            ping,
            mode: mode.into(),
            name: name.into(),
            description: description.into(),
        };
        let browser = crate::server_browser::Browser::with_rows(vec![
            session(1, "", 1, 50, None, "freeskate", ""),
            session(2, "Wwwwwwwwwwwwwwwwwwwwwwww", 10, 10, Some(999), "ID_MAP_FILTER_CHALLENGETYPES_HOMBATTLE", "Hall of Meat all night, bring your best bails and pads"),
            session(3, "Ryan's Park", 3, 50, Some(45), "freeskate", "Chill freeskate at Super Ultra Mega Park, all welcome"),
        ]);
        // The retail English strings (online_menu::lobby_rows looks them up).
        let text = |id: &str| {
            let english = match id {
                "ID_FREESKATE_LOBBY" => "Freeskate",
                "ID_FREESKATE_CHALLENGE_LOBBY" => "Challenge Lobby",
                "ID_MAP_FILTER_CHALLENGETYPES_HOMBATTLE" => "Hall Of Meat Battle",
                _ => return None,
            };
            Some(english.to_owned())
        };
        let context = ctx(ListKind::Lobbies, false, true, true);
        let mut list = PlayerList { rows: browser.rows(Some(1), text), ctx: context, open: None, stale: false, hilight: 0 };
        list.start(&mut screen).unwrap();
        let fonts: Vec<String> = screen.bindings.movie.text_assets.fonts.values().map(|f| f.texture.clone()).collect();
        // The rightmost text and the panel's right edge, in stage pixels: the panel is the biggest box that isn't text
        // or the screen-wide backdrop (the highlight's glow runs as wide as the row's field).
        let check = |screen: &mut Screen, name: &str| {
            for _ in 0..30 {
                screen.advance().unwrap();
            }
            let draws = screen.draws().unwrap();
            if let Ok(out) = std::env::var("SKATE_ONLINE_UI_SHOTS") {
                crate::core_menu_runtime::tests::raster_draws(&draws, &root, &std::path::Path::new(&out).join(format!("{name}.png")));
            }
            let rect = |d: &crate::apt_scene::Draw| {
                (d.vertices.iter()).fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |r, v| {
                    [r[0].min(v.position[0]), r[1].min(v.position[1]), r[2].max(v.position[0]), r[3].max(v.position[1])]
                })
            };
            let (text, boxes): (Vec<_>, Vec<_>) = draws.iter().filter(|d| !d.vertices.is_empty()).partition(|d| fonts.contains(&d.texture));
            let text_right = text.iter().map(|d| rect(d)[2]).fold(f32::MIN, f32::max);
            let text_left = text.iter().map(|d| rect(d)[0]).fold(f32::MAX, f32::min);
            let area = |r: &[f32; 4]| (r[2] - r[0]) * (r[3] - r[1]);
            let panel = boxes.iter().map(|d| rect(d)).filter(|r| r[2] - r[0] < 1200.).max_by(|a, b| area(a).total_cmp(&area(b))).unwrap();
            assert!(text_right <= panel[2], "{name}: text ends at {text_right}, the panel at {}", panel[2]);
            // The button bar (B Back, A Select and, at the list, Y Search and X Refresh) too.
            assert!(text_left >= panel[0], "{name}: text starts at {text_left}, the panel at {}", panel[0]);
            let reach = boxes.iter().map(|d| rect(d)).filter(|r| r[2] - r[0] < 1200.).map(|r| r[2]).fold(f32::MIN, f32::max);
            (panel, reach)
        };
        // The highlight's glow is as long as its row reads (menu_part_text_hilite::SetTitle takes the field's _width): a
        // shrunk row's glow ran on to 1005, past the panel (render 10-27). 16 px for the glow's soft edge.
        let (panel, reach) = check(&mut screen, "browser-root");
        assert!(reach <= panel[2] + 16., "browser-root: a box (the highlight's glow) ends at {reach}, the panel at {}", panel[2]);
        // Wwww's details: the longest name and description (under the sort line: ours, Ryan's Park at 45 ms, Wwww).
        list.step(&mut screen, Key::Down).unwrap();
        list.step(&mut screen, Key::Down).unwrap();
        list.step(&mut screen, Key::Accept).unwrap();
        assert_eq!(list.open.as_ref().map(|(p, _)| p.name.as_str()), Some("Wwwwwwwwwwwwwwwwwwwwwwww"));
        assert!(list.open.is_some(), "A opens a session's details");
        check(&mut screen, "browser-details");
    }

    fn m(actor: u64, name: &str, steam: u64, host: bool) -> Member {
        Member { actor, name: name.into(), steam, host }
    }

    /// Voice chat: a talking player's row reads with a lit mic before the name (the creator's stays gold); nobody
    /// else's changes, and friends and recent players (no actor) never talk.
    #[test]
    fn the_player_list_marks_a_talker() {
        let members = vec![m(1, "Me", 0, false), m(2, "Ann", 22, true), m(3, "Bo", 33, false)];
        let mut r = rows(ListKind::Session, &members, 1, &[], &[]);
        let before = r.clone();
        mark_talkers(&mut r, |actor| actor == 2);
        let literal = crate::apt_text::literal("");
        assert_eq!(r[0].label, format!("{literal}<B:MicOn> Ann - Host"));
        assert_eq!((&r[0].person, &r[1]), (&before[0].person, &before[1]), "Bo is silent");
        let mut gold = vec![Row { label: crate::apt_text::gold("Dee - Creator"), header: false, person: before[0].person.clone() }];
        mark_talkers(&mut gold, |_| true);
        let (rest, lit) = crate::apt_text::split_gold(&gold[0].label[literal.len()..]);
        assert!(lit && rest == "<B:MicOn> Dee - Creator", "{:?}", gold[0].label);
        let mut recent = rows(ListKind::Recent, &[], 0, &[], &[RecentPlayer { name: "Cy".into(), steam: 44 }]);
        let shown = recent.clone();
        mark_talkers(&mut recent, |_| true);
        assert_eq!(recent, shown);
    }

    /// Voice chat: a talker's mic comes and goes in place at the root (menu_picker SetItemText), the highlight kept:
    /// Down still moves on from the row it was on (SKATE_ONLINE_UI_ROOT; SKATE_ONLINE_UI_SHOTS: renders).
    #[test]
    #[ignore = "needs INST\\assets\\private\\online-ui"]
    fn a_talker_is_marked_in_place() {
        let root = std::path::PathBuf::from(std::env::var("SKATE_ONLINE_UI_ROOT").expect("SKATE_ONLINE_UI_ROOT"));
        let mut screen = Screen::load(&root.join("runtime/friends.json"), &[]).unwrap();
        let members = vec![m(1, "Host", 11, true), m(2, "Ann", 22, false), m(3, "Bo", 33, false), m(4, "Cy", 44, false)];
        let context = || ctx(ListKind::Session, false, true, true);
        let shown = rows(ListKind::Session, &members, 1, &[], &[]);
        let mut list = PlayerList { rows: shown.clone(), ctx: context(), open: None, stale: false, hilight: 0 };
        list.start(&mut screen).unwrap();
        let run = |screen: &mut Screen| (0..30).for_each(|_| screen.advance().unwrap());
        // The root picker's row texts (menu_picker's mItemTextArray, which SetItemText writes).
        let texts = |screen: &Screen| -> Vec<String> {
            use crate::apt_vm::Value;
            let picker = screen.clip(MENU).expect("the root picker");
            let Value::Object(array) = screen.vm.get(picker, "mItemTextArray") else { panic!("no mItemTextArray") };
            let Value::Number(n) = screen.vm.get(array, "length") else { panic!("no length") };
            (0..n as usize).map(|i| match screen.vm.get(array, &i.to_string()) { Value::Text(t) => t, _ => String::new() }).collect()
        };
        run(&mut screen);
        list.step(&mut screen, Key::Down).unwrap();
        assert_eq!(list.hilight, 1, "on Bo");
        let mut marked = shown.clone();
        mark_talkers(&mut marked, |actor| actor == 2);
        list.update(&mut screen, Some(marked), context()).unwrap();
        run(&mut screen);
        assert!(texts(&screen)[0].contains("<B:MicOn>"), "{:?}", texts(&screen));
        let draws = screen.draws().unwrap();
        assert!(draws.iter().any(|d| d.texture.ends_with("runtime/buttons/MicOn.rgba")), "the mic drawn in Ann's row");
        if let Ok(out) = std::env::var("SKATE_ONLINE_UI_SHOTS") {
            crate::core_menu_runtime::tests::raster_draws(&draws, &root, &std::path::Path::new(&out).join("playerlist-talker.png"));
        }
        list.step(&mut screen, Key::Down).unwrap();
        assert_eq!(list.hilight, 2, "Down from Bo reaches Cy: the list was not built again");
        list.update(&mut screen, Some(shown), context()).unwrap();
        assert!(!texts(&screen)[0].contains("<B:MicOn>"), "silent again");
    }
    /// C4: a copied name (the host's, in any case) reads "#2" in roster order, never taking a name already shown; the
    /// host and friends carry a mark; a name is a literal, never a string ID.
    #[test]
    fn copied_names_are_numbered_and_marked() {
        let members = [m(1, "Hal", 0, true), m(2, "hal", 0, false), m(3, "Hal #2", 0, false), m(4, "Ann", 7, false), m(5, "ID_COMMON_OK", 0, false)];
        assert_eq!(shown_names(&members), ["Hal", "hal #2", "Hal #2 #2", "Ann", "ID_COMMON_OK"]);
        assert_eq!(shown_name(&members, 2).as_deref(), Some("hal #2"));
        assert_eq!(shown_name(&members, 9), None);
        let friends = [Friend { id: 7, name: "Ann".into(), online: true, connect: None }];
        let r = rows(ListKind::Session, &members, 5, &friends, &[]);
        let labels: Vec<_> = r.iter().map(|r| r.label.as_str()).collect();
        let lit = crate::apt_text::literal;
        assert_eq!(labels, [lit("Hal - Host"), lit("hal #2"), lit("Hal #2 #2"), lit("Ann - Friend")]);
        assert_eq!(r[1].person.as_ref().unwrap().name, "hal #2", "Kick and the sub-menu name the shown name");
    }
    /// User 10-25: the verified creator's row is gold and reads "Name - Creator"; a Steam friend's or recent player's
    /// name can't fake a mark either.
    #[test]
    fn the_creators_row_is_gold_and_names_cannot_fake_it() {
        let gold = crate::apt_text::gold;
        assert_eq!(session_label("Dev", false, true, true), gold("Dev - Creator"));
        assert_eq!(session_label("Dev", true, false, true), gold("Dev - Creator - Host"));
        assert_eq!(session_label("Ann", true, false, false), crate::apt_text::literal("Ann - Host"));
        assert_ne!(crate::apt_text::literal("Dev - Creator"), gold("Dev - Creator"), "plain text is never the gold row");
        assert_eq!(crate::apt_text::split_gold(&gold("x")[2..]), ("x", true), "the marker after the literal one");
        assert_eq!(crate::apt_text::split_gold(&crate::apt_text::literal("\u{2}x")[2..]), ("x", false), "no name carries it");
        let friends = [Friend { id: 8, name: "Bo - Creator".into(), online: true, connect: None }];
        let r = rows(ListKind::Friends, &[], 1, &friends, &[]);
        assert_eq!(r[1].label, crate::apt_text::literal("The Legend"));
        let recent = [RecentPlayer { name: "Bo - Host".into(), steam: 9 }];
        assert_eq!(rows(ListKind::Recent, &[], 1, &[], &recent)[0].label, crate::apt_text::literal("The Legend"));
    }
    /// 3.12: Mute and Hide by Steam ID (surviving a new session) or by actor (ending with it); Block mutes and hides
    /// for good; Block and (user 10-26) a Steam player's Mute are the picks saved; chat and models follow the Settings
    /// rows; unknown senders are no friends.
    #[test]
    fn mute_hide_and_block_reach_the_verdicts() {
        use PlayerAction::*;
        let members = [m(1, "Me", 0, true), m(2, "Ann", 7, false), m(3, "Direct", 0, false), m(4, "Bo", 8, false)];
        let friends = [Friend { id: 8, name: "Bo".into(), online: true, connect: None }];
        let person = |a: &Member| Person { name: a.name.clone(), steam: a.steam, actor: Some(a.actor), host: a.host, friend: false, online: true, connect: None, about: Vec::new() };
        let mut s = Shield::default();
        assert!(s.pick(Mute, &person(&members[1])), "a Steam player's Mute is saved (voice-players.json)");
        assert!(!s.pick(HidePlayer, &person(&members[2])));
        s.judge(&members, 1, &friends);
        assert!(s.quiet(2) && !s.ghost(2) && s.ghost(3) && !s.quiet(3) && !s.quiet(4));
        assert!(!s.reads(2, true, false) && s.reads(3, true, false) && !s.reads(3, false, false), "muted; Text Chat off");
        assert!(s.reads(4, true, true) && !s.reads(3, true, true) && !s.reads(99, true, true), "Chat From: friends");
        assert!(s.models(2, 0) && !s.models(3, 0) && s.models(4, 1) && !s.models(2, 1) && !s.models(4, 2) && !s.models(99, 1));
        s.new_session();
        s.judge(&members, 1, &friends);
        assert!(s.quiet(2) && !s.ghost(3), "a Steam mute stays, an actor's ends with the session");
        assert!(s.pick(Block, &person(&members[3])) && !s.pick(Block, &person(&members[3])));
        assert!(!s.pick(Block, &person(&members[2])), "no Steam ID, nothing to block");
        s.judge(&members, 1, &friends);
        assert!(s.quiet(4) && s.ghost(4) && !s.models(4, 0));
        let ctx = Context { shield: s.clone(), ..ctx(ListKind::Session, false, true, true) };
        let a = actions(&ctx, &person(&members[3]));
        assert!(a.contains(&(Unblock, true)) && !a.iter().any(|(x, _)| *x == Block));
        assert!(actions(&ctx, &person(&members[1])).contains(&(Unmute, true)));
        let dir = std::env::temp_dir().join(format!("blocked-{}", std::process::id()));
        let path = dir.join("blocked-players.json");
        save_blocked(&s.blocked, &path).unwrap();
        let back: Blocked = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(back.players, [RecentPlayer { name: "Bo".into(), steam: 8 }]);
        assert!(s.pick(Unblock, &person(&members[3])));
        s.judge(&members, 1, &friends);
        assert!(!s.quiet(4) && !s.ghost(4));
        assert_eq!(blocked_path(Path::new("C:/x/assets")), Path::new("C:/x/settings/blocked-players.json"));
    }
    /// User 10-26: Voice Volume steps 10 % within 0-200 %, is remembered by Steam ID across game runs; a Direct
    /// player's lasts the session.
    #[test]
    fn voice_volume_steps_and_is_remembered() {
        assert_eq!([step_volume(100, -1), step_volume(200, 1), step_volume(0, -1), step_volume(190, 1)], [90, 200, 0, 200]);
        assert_eq!(voice_label(60), "#Voice Volume: < 60% >");
        let (ann, dee) = (p("Ann", 7, Some(2), false), p("Dee", 0, Some(3), false));
        let mut s = Shield::default();
        assert_eq!((s.volume(&ann), s.volume(&dee)), (100, 100));
        assert!(s.set_volume(&ann, 60), "a Steam player's volume is saved");
        assert!(!s.set_volume(&dee, 150), "a Direct player's is not");
        assert_eq!((s.volume(&ann), s.volume(&dee)), (60, 150));
        let path = std::env::temp_dir().join(format!("voice-{}", std::process::id())).join("voice-players.json");
        save_voice(&s.voice, &path).unwrap();
        let mut back = Shield::default();
        back.remember(crate::saves::load_json(&path).ok().unwrap());
        assert_eq!((back.volume(&p("Ann", 7, Some(9), false)), back.volume(&dee)), (60, 100), "a later run, a new actor id");
        assert_eq!(voice_path(Path::new("C:/x/assets")), Path::new("C:/x/settings/voice-players.json"));
        s.new_session();
        assert_eq!((s.volume(&ann), s.volume(&dee)), (60, 100), "a Direct player's ends with the session");
    }
    /// User decision 10: Mute is remembered by Steam ID across game runs (saved beside the block list); a Direct
    /// player's lasts the session; unmuted at 100 % leaves no entry.
    #[test]
    fn mute_is_remembered_by_steam_id() {
        use PlayerAction::*;
        let (ann, dee) = (p("Ann", 7, Some(2), false), p("Dee", 0, Some(3), false));
        let mut s = Shield::default();
        assert!(s.pick(Mute, &ann), "saved");
        assert!(!s.pick(Mute, &dee), "a Direct player's mute is not");
        assert_eq!(s.voice.players, [VoicePlayer { steam: 7, name: "Ann".into(), volume: 100, muted: true }]);
        let mut back = Shield::default();
        back.remember(s.voice.clone());
        assert!(back.has(&back.muted, &p("Ann", 7, Some(9), false)) && !back.has(&back.muted, &dee));
        assert!(s.pick(Unmute, &ann) && s.voice.players.is_empty(), "unmuted at 100 %: no entry");
    }
    /// Mute and Block silence a voice (its packets are dropped unread); the row greys while muted; session players only.
    #[test]
    fn a_muted_or_blocked_player_has_no_voice_volume() {
        use PlayerAction::*;
        let members = [m(1, "Me", 0, true), m(2, "Ann", 7, false), m(3, "Bo", 8, false), m(4, "Cy", 9, false)];
        let person = |a: &Member| p(&a.name, a.steam, Some(a.actor), false);
        let mut s = Shield::default();
        s.set_volume(&person(&members[3]), 40);
        s.pick(Mute, &person(&members[1]));
        s.pick(Block, &person(&members[2]));
        s.judge(&members, 1, &[]);
        assert_eq!([2, 3, 4, 99].map(|a| s.hears(a, true, false)), [None, None, Some(40), None], "a stranger to the roster too");
        let shielded = Context { shield: s.clone(), ..ctx(ListKind::Session, false, true, true) };
        assert!(actions(&shielded, &person(&members[1])).contains(&(VoiceVolume, false)), "greyed while muted");
        let cy = actions(&shielded, &person(&members[3]));
        assert_eq!(cy.iter().position(|(a, _)| *a == VoiceVolume), cy.iter().position(|(a, _)| *a == Mute).map(|i| i + 1), "after Mute");
        assert!(!actions(&ctx(ListKind::Recent, false, true, true), &p("Cy", 9, None, false)).iter().any(|(a, _)| *a == VoiceVolume), "session players only");
    }
    /// Voice From = Friends: only Steam friends' voices; Voice Chat Off: none.
    #[test]
    fn voice_from_friends_hears_only_friends() {
        let members = [m(1, "Me", 0, true), m(2, "Ann", 7, false), m(3, "Bo", 8, false)];
        let friends = [Friend { id: 8, name: "Bo".into(), online: true, connect: None }];
        let mut s = Shield::default();
        s.judge(&members, 1, &friends);
        assert_eq!([2, 3].map(|a| s.hears(a, true, true)), [None, Some(100)]);
        assert_eq!([2, 3].map(|a| s.hears(a, true, false)), [Some(100), Some(100)]);
        assert_eq!([2, 3].map(|a| s.hears(a, false, false)), [None, None], "Voice Chat off");
    }
    /// Hide Player stops drawing them; their voice stays.
    #[test]
    fn hide_player_keeps_voice() {
        let members = [m(1, "Me", 0, true), m(2, "Ann", 7, false)];
        let mut s = Shield::default();
        s.pick(PlayerAction::HidePlayer, &p("Ann", 7, Some(2), false));
        s.judge(&members, 1, &[]);
        assert!(s.ghost(2));
        assert_eq!(s.hears(2, true, false), Some(100));
    }
    /// voice-players.json holds at most 1000 players, the least recently changed going first; a hand-edited one is
    /// read the same way (no Direct players, the row's range and steps).
    #[test]
    fn the_file_keeps_at_most_1000_players() {
        let mut s = Shield::default();
        for steam in 1..=1001 {
            s.set_volume(&p("P", steam, None, false), 50);
        }
        assert_eq!(s.voice.players.len(), 1000);
        assert_eq!(s.voice.players.first().map(|v| v.steam), Some(2), "the least recently changed went");
        s.set_volume(&p("P", 2, None, false), 60);
        assert_eq!(s.voice.players.last().map(|v| (v.steam, v.volume)), Some((2, 60)), "a change moves it last");
        s.set_volume(&p("P", 2, None, false), 100);
        assert!(!s.voice.players.iter().any(|v| v.steam == 2), "back at 100 % and unmuted: gone");
        let wild = VoicePlayers { players: (0..1200).map(|i| VoicePlayer { steam: i, name: "x".into(), volume: 999, muted: false }).collect() };
        let mut t = Shield::default();
        t.remember(wild);
        assert_eq!(t.voice.players.len(), 1000);
        assert!(t.voice.players.iter().all(|v| v.steam >= 200 && v.volume == 200), "the newest 1000, at most 200 %");
        assert_eq!(t.volume(&p("x", 1199, None, false)), 200);
    }
    /// User 10-29: the host saw no Make Co-Host on a player: every one of the host's 15 actions must be on the movie's
    /// picker and reachable with Down (SKATE_ONLINE_UI_ROOT; SKATE_ONLINE_UI_SHOTS: renders the open popup there).
    #[test]
    #[ignore = "needs INST\\assets\\private\\online-ui"]
    fn the_hosts_every_action_reaches_the_screen() {
        let root = std::path::PathBuf::from(std::env::var("SKATE_ONLINE_UI_ROOT").expect("SKATE_ONLINE_UI_ROOT"));
        let mut screen = Screen::load(&root.join("runtime/friends.json"), &[]).unwrap();
        let members = vec![m(1, "Host", 11, true), m(2, "Ann", 22, false)];
        let mut list = PlayerList { rows: rows(ListKind::Session, &members, 1, &[], &[]), ctx: ctx(ListKind::Session, true, true, true), open: None, stale: false, hilight: 0 };
        list.start(&mut screen).unwrap();
        let run = |screen: &mut Screen| (0..30).for_each(|_| screen.advance().unwrap());
        run(&mut screen);
        assert_eq!(list.step(&mut screen, Key::Accept).unwrap(), None, "Ann's actions");
        run(&mut screen);
        let actions = list.open.as_ref().expect("open").1.clone();
        let at = actions.iter().position(|(a, _)| *a == PlayerAction::MakeCoHost).expect("Make Co-Host for the host");
        assert!(at < 10, "in the popup's first 10 rows, no scrolling");
        let picker = screen.clip("mFriends.mContent.mSubmenu").expect("the sub-menu picker");
        let crate::apt_vm::Value::Object(array) = screen.vm.get(picker, "mItemTextArray") else { panic!("no mItemTextArray") };
        let crate::apt_vm::Value::Number(n) = screen.vm.get(array, "length") else { panic!("no length") };
        assert!(n as usize >= actions.len(), "the picker holds {n} of {} rows", actions.len());
        for _ in 0..actions.len() * 2 {
            if list.hilight == at {
                break;
            }
            list.step(&mut screen, Key::Down).unwrap();
            run(&mut screen);
        }
        if let Ok(out) = std::env::var("SKATE_ONLINE_UI_SHOTS") {
            let draws = screen.draws().unwrap();
            crate::core_menu_runtime::tests::raster_draws(&draws, &root, &std::path::Path::new(&out).join("playerlist-host-cohost.png"));
        }
        assert_eq!(list.hilight, at, "Down reaches Make Co-Host");
    }
    /// User 10-26: Voice Volume on a session player's actions takes Left and Right in place: the movie reports the row
    /// it highlights (Game.OnMenuHilight), the row's text changes where it is and the highlight stays on it
    /// (SKATE_ONLINE_UI_ROOT = private/online-ui; SKATE_ONLINE_UI_SHOTS: renders).
    #[test]
    #[ignore = "needs INST\\assets\\private\\online-ui"]
    fn the_voice_volume_row_takes_left_and_right() {
        let root = std::path::PathBuf::from(std::env::var("SKATE_ONLINE_UI_ROOT").expect("SKATE_ONLINE_UI_ROOT"));
        let mut screen = Screen::load(&root.join("runtime/friends.json"), &[]).unwrap();
        let members = vec![m(1, "Host", 11, true), m(2, "Ann", 22, false)];
        let mut list = PlayerList { rows: rows(ListKind::Session, &members, 1, &[], &[]), ctx: ctx(ListKind::Session, false, true, true), open: None, stale: false, hilight: 0 };
        list.start(&mut screen).unwrap();
        let run = |screen: &mut Screen| (0..30).for_each(|_| screen.advance().unwrap());
        let shot = |screen: &Screen, name: &str| {
            if let Ok(out) = std::env::var("SKATE_ONLINE_UI_SHOTS") {
                let draws = screen.draws().unwrap();
                crate::core_menu_runtime::tests::raster_draws(&draws, &root, &std::path::Path::new(&out).join(format!("{name}.png")));
            }
        };
        // The sub-menu picker's row texts (menu_picker's mItemTextArray, which SetItemText writes).
        let texts = |screen: &Screen| -> Vec<String> {
            use crate::apt_vm::Value;
            let picker = screen.clip("mFriends.mContent.mSubmenu").expect("the sub-menu picker");
            let Value::Object(array) = screen.vm.get(picker, "mItemTextArray") else { panic!("no mItemTextArray") };
            let Value::Number(n) = screen.vm.get(array, "length") else { panic!("no length") };
            (0..n as usize).map(|i| match screen.vm.get(array, &i.to_string()) { Value::Text(t) => t, _ => String::new() }).collect()
        };
        run(&mut screen);
        assert_eq!(list.step(&mut screen, Key::Accept).unwrap(), None, "Ann's actions");
        run(&mut screen);
        let ann = list.open.as_ref().unwrap().0.clone();
        let at = list.open.as_ref().unwrap().1.iter().position(|(a, _)| *a == PlayerAction::VoiceVolume).expect("a Voice Volume row");
        assert_eq!(texts(&screen)[at], voice_label(100));
        for _ in 0..12 {
            if list.hilight == at {
                break;
            }
            list.step(&mut screen, Key::Down).unwrap();
        }
        assert_eq!(list.hilight, at, "the movie reports the highlighted row");
        assert_eq!(list.step(&mut screen, Key::Right).unwrap(), Some(Effect::Volume(ann.clone(), 110)));
        run(&mut screen);
        shot(&screen, "playerlist-voice-volume");
        assert_eq!(texts(&screen)[at], voice_label(110), "the row's text, in place");
        assert_eq!(list.hilight, at, "the highlight stays on the row");
        list.step(&mut screen, Key::Left).unwrap();
        assert_eq!(list.step(&mut screen, Key::Left).unwrap(), Some(Effect::Volume(ann.clone(), 90)));
        assert_eq!(texts(&screen)[at], voice_label(90));
        // Off the row, Left and Right do nothing; Down still moves on.
        list.step(&mut screen, Key::Up).unwrap();
        assert_ne!(list.hilight, at);
        assert_eq!(list.step(&mut screen, Key::Right).unwrap(), None);
        assert_eq!(list.step(&mut screen, Key::Back).unwrap(), None, "back to the list");
        assert!(list.open.is_none());
    }
}
