//! Online Challenge Map picks → Steam lobby actions (Quick Match, Join, Host).
use bevy::prelude::*;
use skate_net::{
    directory::{Row, Tags},
    roster::ChallengeLobby,
};

use crate::{
    map_library::same_name,
    multiplayer::Multiplayer,
    online_map::{Choice, Pick},
};

const MATCH_TIMEOUT: f32 = 16.;  // outlasts the relay helper's own 15 s lobby search (directory.rs)
/// Online Challenge Map tab order → the `tab` lobby key. Skate 2 and Skate 1 are their own tabs after retail's four
/// (skate2port game_tabs.py), so their lobbies never match Skate 3 ones.
const TABS: [&str; 6] = ["team", "career", "freeskate", "versus", "skate2", "skate1"];
const FREESKATE_TAB: usize = 2;
/// A session move: how long a guest keeps looking for the host's new lobby once its own map is up, and the browse
/// interval. The host's map load may take longer than the guest's. Knobs: user 10-29, a move to a big map lost
/// everyone at 40 s (the host's lobby came later), and a 2 s browse left them waiting after it came.
pub(crate) const FOLLOW_FOR: f32 = 120.;
const RETRY_EVERY: f32 = 1.;

/// A session move the next pick follows (online_moves sets it, core_menu's perform_online takes it). `carry`: the
/// host's session state for its new lobby (default for guests).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Follow {
    pub token: u64,
    pub host: bool,
    pub carry: skate_net::roster::SessionState,
}

/// The players a pick's lobby holds (Tags::capacity: freeskate 50, challenges 6), whichever tab it is on.
pub(crate) fn capacity(pick: &Pick) -> usize {
    let lobby = match intent(pick) {
        Intent::Join(tags) | Intent::Host(tags) => tags.capacity(),
        _ => skate_net::lobby::MAX_PLAYERS,
    };
    // A challenge holds what Start Challenge allows (ID_ONLNE_TOO_MANY_PLAYERS: match_core::row_cap).
    match pick {
        Pick::Challenge { tab, folder, .. } => lobby.min(crate::match_core::row_cap(*tab, folder)),
        _ => lobby,
    }
}

/// A moving session's lobby, found by its token: the host hosts it, the guests join it. A private session's is hidden
/// (P5): the relay lists it to its token's browse only, so no stranger's browse or Quick Match finds it before
/// online_roster makes it private. Guests only stay in a lobby owned by the old host or a co-host ([`owner_ok`]).
pub(crate) fn follow_intent(intent: Intent, follow: &Follow) -> Intent {
    let session = format!("{:x}", follow.token);
    match intent {
        // Audit 10-24: hidden as the session is private (a move's row is a Create Private pick), and with its filter
        // (a Skate 1 / 2 S.K.A.T.E. lobby holds 4).
        Intent::Join(tags) | Intent::Host(tags) if follow.host => Intent::Host(Tags { session, hidden: follow.carry.private, ..tags }),
        Intent::Join(tags) | Intent::Host(tags) => Intent::Join(Tags { tab: tags.tab, session, ..default() }),
        other => other,
    }
}

#[derive(Resource, Default)]
pub(crate) struct OnlineSession {
    pub pending: Option<Intent>,
    pub popup: Option<Popup>,
    browsing_since: Option<f32>,
    browsing: Option<Intent>,
    was_busy: bool,
    /// A WorldChanged arrived during the current map load (it comes at commit, frames before idle).
    saw_change: bool,
    /// The pick started a map load; hold the intent until that world is up.
    pub awaiting_world: bool,
    /// The picked row's spawn slots and online area (online_boundary).
    pub area: Option<crate::online_boundary::Area>,
    /// The lobby the last pick asks for; online_roster takes it into the new session's `@session`.
    pub lobby: Option<ChallengeLobby>,
    /// The location row the last pick asks for (its string id), taken into `@session` with `lobby`.
    pub location: Option<String>,
    /// A Steam host started at this time and has not formed a lobby yet.
    hosting_since: Option<f32>,
    /// The next pick is a session move's (online_moves).
    pub follow: Option<Follow>,
    /// A moving host's session state for its new lobby (bans, proposals setting, privacy); online_roster takes it.
    pub carry: Option<skate_net::roster::SessionState>,
    /// A move's join: browse again at this time, until `follow_until`.
    retry: Option<(f32, Tags)>,
    follow_until: f32,
    /// The way into a session (a join, a host, a move, an online pick's map load) started, or last stepped on, at this
    /// time: the loading screen stays up (tip_loader::JoinCover) until this player is in it on its spot ([`Self::cover_step`]).
    join_cover: Option<f32>,
    /// User 10-26: a same-map pick's spot, reached behind that loading screen (core_menu's perform_online). Retail moves
    /// a player into a session behind it; the pick teleported at once and the screen came after.
    pub arrival: Option<[[f32; 4]; 4]>,
    /// P5: the Steam IDs that may own a session move's new lobby (this Steam session's host and co-hosts), kept
    /// current while in one.
    owners: Vec<u64>,
    /// A guest's move under way: (the old lobby, `owners` then, the move's tags). Its new lobby is checked by owner.
    moving: Option<(Option<u64>, Vec<u64>, Tags)>,
    /// Lobbies this move refused (someone else's lobby carrying the token), or a Quick Match's join refused: never
    /// matched again.
    refused: Vec<u64>,
    /// Audit 10-28 #11: a Quick Match's join under way: (the lobby, the search's tags).
    quick: Option<(u64, Tags)>,
    /// G5 (perf 10-17): a Quick Match / Join pick's browse, started beside its map load: (the target map's lobby
    /// label, started at).
    early: Option<(String, f32)>,
    /// That browse's answer, kept past the load's WorldChanged (which clears `browser_rows`): (label, rows).
    early_rows: Option<(String, Vec<Row>)>,
}

/// The session state a fresh pick's lobby starts with: Create Private's is private (audit 10-24: it was public).
pub(crate) fn fresh_carry(intent: &Intent) -> Option<skate_net::roster::SessionState> {
    matches!(intent, Intent::Host(tags) if tags.hidden).then(|| skate_net::roster::SessionState { private: true, ..default() })
}

impl OnlineSession {
    /// Audit 10-28 #11: the lobby a Quick Match picked refused us (two players took its last place at once): it is not
    /// matched again, and the search runs again (then hosts, as an unmatched search does). It left the player alone
    /// with "error starting".
    fn quick_join_failed(&mut self, now: f32) -> bool {
        let Some((lobby, tags)) = self.quick.take() else { return false };
        self.refused.push(lobby);
        self.retry = Some((now, tags));
        true
    }
    /// A session move is under way: the old session left, the new one not in yet (audit 10-24: Skate 2 physics, off
    /// online, switched on for the move's search and off again at the join). A Steam host still forming counts too.
    pub(crate) fn move_under_way(&self) -> bool {
        let moving = |intent: &Option<Intent>| matches!(intent, Some(Intent::Join(t) | Intent::Host(t)) if !t.session.is_empty());
        self.follow.is_some() || self.moving.is_some() || self.retry.is_some() || moving(&self.pending) || moving(&self.browsing)
            || self.hosting_since.is_some()
    }
    /// User 10-25: a fresh pick on its way into a session counts as online too, from the pick: its map load, its
    /// browse and its host forming (the Skate 1 and 2 tutorials, single player only, started on their maps in
    /// between). Solo is no session.
    pub(crate) fn going_online(&self) -> bool {
        let online = |intent: &Option<Intent>| matches!(intent, Some(Intent::Join(_) | Intent::Host(_) | Intent::JoinLobby { .. }));
        self.move_under_way() || online(&self.pending) || online(&self.browsing) || self.early.is_some()
    }
    /// A guest's move whose join failed or timed out is over (review 10-24: `moving` stayed, so Online stayed on).
    fn end_failed_move(&mut self, engaged: bool) {
        if !engaged && !self.awaiting_world && self.pending.is_none() && self.browsing.is_none() && self.retry.is_none() {
            self.moving = None;
        }
    }
    /// User 10-26: a session move raises the loading screen before it leaves this lobby (online_moves::follow).
    pub(crate) fn cover(&mut self, now: f32) {
        self.join_cover.get_or_insert(now);
    }
    /// The loading screen over the way into a session, this frame (User 10-26: retail moves a player into a session
    /// behind it). An online pick's map load raises it, so it never drops between the load and the join; a step that
    /// ends on its own (the load, a browse, a move's wait, a host forming) keeps it from running out. The pick's spot
    /// is returned once the screen is up, and the screen drops once this player is in on its spot. True: a join that
    /// heard nobody by JOIN_COVER_MAX is given up.
    fn cover_step(&mut self, now: f32, way: Way) -> (Option<[[f32; 4]; 4]>, bool) {
        let on_the_way = way.engaged || self.awaiting_world || self.going_online();
        let stepping = way.busy || self.browsing_since.is_some() || self.retry.is_some() || self.hosting_since.is_some();
        if stepping && (self.join_cover.is_some() || way.busy && self.awaiting_world && self.going_online()) {
            self.join_cover = Some(now);
        }
        let arrive = if way.covered && self.join_cover.is_some() { self.arrival.take() } else { None };
        // Nothing on the way (no match, cancelled, failed): a spot never reached stays behind.
        if !on_the_way {
            self.arrival = None;
        }
        // User 10-26: a joiner is in once it hears someone AND has the host's weather and time on.
        let done = (way.hosting || way.synced && way.world) && way.settled && self.arrival.is_none() && arrive.is_none() && !self.going_online();
        let mut failed = false;
        if let Some(since) = self.join_cover.filter(|&since| !keep_cover(since, now, on_the_way, done)) {
            self.join_cover = None;
            failed = join_timed_out(since, now, way.engaged && !way.hosting, way.synced);
        }
        (arrive, failed)
    }
}

/// What [`OnlineSession::cover_step`] reads each frame.
#[derive(Clone, Copy, Debug, Default)]
struct Way {
    /// A map is loading.
    busy: bool,
    /// The loading screen is up and opaque (tip_loader::Covered, as it last drew).
    covered: bool,
    /// In a session, or a Steam lobby join under way.
    engaged: bool,
    /// Hosting the session this player is in.
    hosting: bool,
    /// Heard from another player in it.
    synced: bool,
    /// On its spot: its area's slot placed (online_boundary) and no teleport in flight.
    settled: bool,
    /// The host's weather and time of day (its `@session` world) are on (online_moves::world).
    world: bool,
}

/// P5: a session move's new lobby is only followed when Steam says the old host or a co-host owns it.
pub(crate) fn owner_ok(owner: u64, owners: &[u64]) -> bool {
    owner != 0 && owners.contains(&owner)
}

/// P5's `owners` follow the session (in one, no move of ours under way) until its host announces a move (`announced`:
/// the `@session`'s `moving`). Bug 10-25 #17: a guest that saw the move over a second late was still in the old lobby
/// when the host left it (guests leave 5 s after they see it, the host 6 s after it announced it); Steam made another
/// member its owner, that one became the only owner allowed, and the host's new lobby was refused until the search ran
/// out: No Match, removed from the session.
fn track_owners(owners: &mut Vec<u64>, current: impl FnOnce() -> Vec<u64>, active: bool, moving: bool, announced: bool) {
    if active && !moving && !announced {
        *owners = current();
    }
}

/// The Steam IDs that may own this session's next lobby: its Steam owner and the co-hosts' real link endpoints.
fn session_owners(net: &Multiplayer, cohosts: &[u64]) -> Vec<u64> {
    let Some(s) = net.session().filter(|_| net.is_steam()) else { return vec![] };
    net.room_owner().into_iter().chain(cohosts.iter().filter_map(|&a| s.endpoint(a))).collect()
}

/// A join that never hears from anyone gives the screen back after this long and is dropped. A knob: retail's join
/// timeout is UNTRACED.
const JOIN_COVER_MAX: f32 = 15.;

/// The join cover stays while the way into a session is under way and not `done` (in, on its spot), at most
/// JOIN_COVER_MAX past its last step.
fn keep_cover(since: f32, now: f32, underway: bool, done: bool) -> bool {
    underway && !done && now - since < JOIN_COVER_MAX
}

/// The join is still unheard when the cover runs out: the host never answered (a version mismatch it refuses quietly,
/// NAT). It is left with retail's "Failed to join or create an online session." (popup 104, online_roster's
/// `join_failed`).
fn join_timed_out(since: f32, now: f32, engaged: bool, synced: bool) -> bool {
    engaged && !synced && now - since >= JOIN_COVER_MAX
}

/// The challenge lobby a pick makes or joins: Quick Match / Join / Create picks on the Team, Career and Versus tabs.
/// Freeskate picks (locations, Freeskate Options, solo, invites) are Online Freeskate.
pub(crate) fn lobby(intent: &Intent) -> Option<ChallengeLobby> {
    let (Intent::Join(tags) | Intent::Host(tags)) = intent else { return None };
    if tags.tab == "freeskate" {
        return None;
    }
    // ponytail: Career (coop) lobbies get the Online Challenge table; retail has a separate Coop Lobby mode.
    let title = if tags.filter.is_empty() {
        if tags.tab == "team" { "ID_CROSSBAR_ONLINE_TEAM_CHALLENGE_MODE_TITLE" } else { "ID_CROSSBAR_ONLINE_CHALLENGE_MODE_TITLE" }
    } else {
        &tags.filter
    };
    let tab = TABS.iter().position(|t| *t == tags.tab).unwrap_or(0);
    // ponytail: a Custom Match's `challenge` tag is its command, which no mode plays (Start Challenge stays greyed).
    let row = (!tags.challenge.is_empty()).then(|| skate_net::roster::Target::new(tab, &tags.challenge, "", ""));
    Some(ChallengeLobby { title: skate_net::roster::truncate(title, skate_net::roster::TITLE_MAX).into(), team: tags.tab == "team", row })
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Intent {
    /// Quick Match / Join Public Game / Join Team Lobby: join the first matching lobby on this map, else
    /// "No match up found".
    Join(Tags),
    /// Create Private Game / Custom Match / Team Freeskate Here: host a lobby carrying these tags.
    Host(Tags),
    Solo,
    /// A Steam join request or invite (friend's rich-presence connect string).
    JoinLobby { lobby: u64 },
}

/// The lobby action for an Online Challenge Map pick (spec §3; tags are the v6 lobby keys,
/// and every browse is also limited to the current map).
pub(crate) fn intent(pick: &Pick) -> Intent {
    let tags = |tab: usize, filter: &str, challenge: &str| Tags {
        tab: TABS.get(tab).copied().unwrap_or("team").into(),
        filter: filter.into(),
        challenge: challenge.into(),
        ..default()
    };
    match pick {
        Pick::Command { command, .. } if command == "online_freeskate_here_solo" => Intent::Solo,
        // From the Team or the Freeskate tab: join any freeskate lobby (team or location) on this map, or start one.
        Pick::Command { command, .. } if command == "online_freeskate_team_join" => Intent::Join(tags(FREESKATE_TAB, "", "")),
        Pick::Command { command, .. } if command == "online_freeskate_here_team" => Intent::Host(tags(FREESKATE_TAB, "", "")),
        // In a challenge-type folder the folder is the filter; at the tab root any type matches.
        Pick::Command { command, tab, folder } if command.ends_with("quickmatch") => Intent::Join(tags(*tab, folder, "")),
        // *custommatch (ranked_custom screen deferred): host.
        Pick::Command { command, tab, folder } => Intent::Host(tags(*tab, folder, command)),
        // A location's lobby is tagged with it: joiners spawn in the same online area as the host. Every location is
        // freeskate, also the Skate 2 / Skate 1 tabs' (50 players, no challenge lobby).
        Pick::Location { name, choice: Choice::JoinPublic, .. } => Intent::Join(tags(FREESKATE_TAB, "", name)),
        // Audit 10-24: hidden from every browse (fresh_carry makes the session private; invites bring friends in).
        Pick::Location { name, choice: Choice::CreatePrivate, .. } => Intent::Host(Tags { hidden: true, ..tags(FREESKATE_TAB, "", name) }),
        // With its folder (bug 10-25 #13): the lobby an unmatched Join hosts takes its mode from it (`lobby`); every
        // lobby of the row carries the same one (Create Private's, a move's: online_map's `find`).
        Pick::Challenge { challenge, choice: Choice::JoinPublic, tab, folder, .. } => Intent::Join(tags(*tab, folder, &challenge.id)),
        Pick::Challenge { challenge, choice: Choice::CreatePrivate, tab, folder, .. } => {
            Intent::Host(Tags { hidden: true, ..tags(*tab, folder, &challenge.id) })
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum JoinPlan {
    /// Join on the current map.
    Here(u64),
    /// Load the map at this `Menu::map_labels` index, then join.
    Travel(u64, usize),
    /// The session's map is not installed.
    Missing,
}

/// What a `sk8:<lobby>:<map>` connect string asks of us; `None` if it is not ours.
pub(crate) fn join_plan(connect: &str, current_map: &str, labels: &[String]) -> Option<JoinPlan> {
    let (lobby, map) = skate_net::presence::parse_connect(connect)?;
    Some(if map.is_empty() || same_name(map, current_map) {
        JoinPlan::Here(lobby)
    // The sender's map name went through directory::label (32 chars), so compare that form.
    } else if let Some(index) = labels.iter().position(|l| same_name(&skate_net::directory::label(l), map)) {
        JoinPlan::Travel(lobby, index)
    } else {
        JoinPlan::Missing
    })
}

/// What the search shows; online_menu draws it as retail's sk8popups (popup table 0x83028050): `Matching` is popup 43
/// (ID_ONLINE_MATCHING_SKATERS, no options, B cancels: [`OnlineSession::cancel_matching`]), `NoMatch` popup 106
/// (ID_SIDEKICK_ONLINE_QUICK_MATCH_NO_MATCH, OK), handed over once shown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Popup {
    Matching,
    NoMatch,
}

impl OnlineSession {
    /// A map load ended (busy → idle) without a WorldChanged during it: the load failed.
    pub fn load_failed(&mut self, busy: bool, changed: bool) -> bool {
        self.saw_change |= changed;
        let failed = self.was_busy && !busy && !self.saw_change;
        if !busy {
            self.saw_change = false;
        }
        failed
    }

    /// Map load finished (`changed`) or ended without a new world (busy → idle, e.g. a failed load).
    pub fn world_ready(&mut self, busy: bool, changed: bool, travel_pending: bool) -> bool {
        if changed || (self.was_busy && !busy) {
            self.awaiting_world = false;
        }
        self.was_busy = busy;
        !self.awaiting_world && !busy && !travel_pending
    }

    pub fn take_ready(&mut self, world_ready: bool) -> Option<Intent> {
        if world_ready { self.pending.take() } else { None }
    }

    /// B on the Matching popup. Dropping the relay aborts the browse; in a session, keep the session (its answer is
    /// ignored).
    pub(crate) fn cancel_matching(&mut self, net: &mut Multiplayer) {
        if !net.active() {
            net.leave();
        }
        self.cancel_browse();
        self.area = None;
    }

    /// Drop an in-flight Quick Match / Join browse and its popup, a Steam host still forming, and the pick's lobby.
    pub(crate) fn cancel_browse(&mut self) {
        self.browsing = None;
        self.browsing_since = None;
        self.popup = None;
        self.hosting_since = None;
        self.lobby = None;
        self.carry = None;
        self.retry = None;
        self.moving = None;
        self.follow = None;
        self.refused.clear();
        self.quick = None;
        self.early = None;
        self.early_rows = None;
        self.arrival = None;
    }

    /// G5: the pick's browse may start beside its map load: a plain Join (a session move's host is loading too, so
    /// its lobby comes later: moves browse after the load, FOLLOW_FOR) whose load is under way, once.
    fn early_browse(&self, busy: bool, engaged: bool) -> Option<&Tags> {
        let Some(Intent::Join(tags)) = &self.pending else { return None };
        (self.awaiting_world && busy && !engaged && tags.session.is_empty() && self.early.is_none() && self.early_rows.is_none()).then_some(tags)
    }

    /// The early browse answered (any status but "Contacting Steam...": rows, an error, or WorldChanged's clear,
    /// which come with no rows) or timed out: keep what it found.
    fn early_answer(&mut self, answered: bool, rows: &[Row], now: f32) {
        let Some((label, _)) = self.early.take_if(|(_, since)| answered || now - *since > MATCH_TIMEOUT) else { return };
        self.early_rows = answered.then(|| (label, rows.to_vec()));
    }

    /// At the world: the early browse's row to join, when it ran for the map that loaded (`map`, the lobby label)
    /// and found one. None: browse now, as before G5 (a no-match is not trusted: a lobby may have formed during the
    /// load). ponytail: a row found at the load's start may have filled since; its join then fails with popup 104,
    /// as a browse-then-join race does; upgrade path: re-browse when the rows are older than a few seconds.
    fn early_match(&mut self, map: &str, own: Option<u64>) -> Option<(Vec<Row>, usize)> {
        self.early = None;
        let (_, rows) = self.early_rows.take().filter(|(label, _)| label == map)?;
        matching_row(&rows, own, map).map(|i| (rows, i))
    }
}

fn browse(net: &mut Multiplayer, session: &mut OnlineSession, tags: Tags, now: f32) {
    net.browser_rows.clear();
    // ponytail: page 0 only; the relay filters by map and tags, so it holds only candidates.
    net.browse_tagged(tags.clone());
    session.browsing = Some(Intent::Join(tags));
    // Behind the loading screen (a move's, a map load's) the screen is the search's progress: no popup under it.
    session.popup = session.join_cover.is_none().then_some(Popup::Matching);
    session.browsing_since = Some(now);
}

/// First joinable row of a tagged browse (the relay matched the tags and the map): not full, not
/// the lobby we are already in, and on this map (`map` is `Multiplayer::map_name`, the lobby label).
pub(crate) fn matching_row(rows: &[Row], own: Option<u64>, map: &str) -> Option<usize> {
    rows.iter().position(|r| r.compatible && r.players < r.capacity && Some(r.id) != own && same_name(&r.map, map))
}

fn host(net: &mut Multiplayer, session: &mut OnlineSession, tags: Tags, now: f32) {
    net.steam_host_tagged(tags);
    session.hosting_since = Some(now);
    // User 10-26: a pick's spot is reached behind the loading screen, up while the lobby forms (the Matching popup
    // would sit under it).
    if session.arrival.is_some() {
        session.join_cover = Some(now);
        session.popup = session.popup.filter(|p| *p != Popup::Matching);
    }
    host_failed(net, session, now);
}

/// Bug 10-25 #13 (user: "it's not supposed to give up, it will make a lobby and you join it and you will have a public
/// session"): a Join (Quick Match, Join Public Game, Join Team Lobby) whose answered browse matched nothing hosts the
/// session it looked for, public, under the Matching popup. SUSPECTED retail: its matchmaking has a create-game phase
/// after the find phase (online config CreateGameDurationMS / FindGameDurationMS, TU3 0x82259618 / 0x82259630, read
/// at 0x8243A1D4); the native fallback itself is not traced. A move's search (by its token) and a browse Steam never
/// answered still end in No Match (popup 106).
/// Only a Join Public Game on one location or challenge (its tags name it): Quick Match and Join Team Lobby never make
/// a lobby, they end in No Match.
fn host_instead(wanted: Option<&Intent>, answered: bool) -> Option<Tags> {
    match wanted {
        Some(Intent::Join(tags)) if answered && tags.session.is_empty() && !tags.challenge.is_empty() => Some(tags.clone()),
        _ => None,
    }
}

/// The Steam lobby did not form, now or once the relay answers: retail's "error starting" line (online_roster logs
/// `join_failed`). No local fallback: online play needs Steam, which the sign-in (online_menu) already checked.
fn host_failed(net: &mut Multiplayer, session: &mut OnlineSession, now: f32) {
    let Some(since) = session.hosting_since else { return };
    let failed = net.browser_status != "Contacting Steam..." || now - since > MATCH_TIMEOUT;
    if net.active() || failed {
        // An unmatched Join's host (host_instead): its Matching popup goes with the forming, either way.
        session.popup = session.popup.filter(|p| *p != Popup::Matching);
    }
    if net.active() {
        session.hosting_since = None;
    } else if failed {
        warn!("Online host failed: {}", net.browser_status);
        session.hosting_since = None;
        session.area = None;
        session.lobby = None;
        session.carry = None;
        // Audit 10-24: and why (Steam offline, the relay files missing, lobby details refused), not the line alone.
        if net.browser_status != "Contacting Steam..." {
            net.join_error = net.browser_status.clone();
        }
        net.leave();
        net.join_failed = true;
    }
}

pub(crate) struct OnlineSessionPlugin;
impl Plugin for OnlineSessionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<OnlineSession>().add_systems(
            Update,
            (join_requests, run, crate::online_boundary::update.run_if(crate::graphics_menu::gameplay_active)).chain(),
        );
    }
}

fn run(
    mut session: ResMut<OnlineSession>,
    mut net: ResMut<Multiplayer>,
    (transition, mut travel, time, mut cover): (
        Res<crate::map_transition::MapTransition>,
        ResMut<crate::teleport_menu::Travel>,
        Res<Time<Real>>,
        ResMut<crate::tip_loader::JoinCover>,
    ),
    mut changed: MessageReader<crate::map_transition::WorldChanged>,
    (roster, menu): (Option<Res<crate::online_roster::OnlineRoster>>, Res<crate::graphics_menu::Menu>),
    (covered, mut skater): (Res<crate::tip_loader::Covered>, ResMut<crate::physics::SkaterRuntime>),
) {
    let now = time.elapsed_secs();
    // P5: a move's new lobby (another room than the old one) must be the old host's or a co-host's.
    if let Some((old, owners, tags)) = session.moving.clone()
        && let (Some(room), Some(owner)) = (net.room_id(), net.room_owner())
        && Some(room) != old
    {
        if owner_ok(owner, &owners) {
            session.moving = None;
        } else {
            warn!("Session move: the lobby found by the move's token is not the host's; left it");
            session.refused.push(room);
            net.leave();
            if now < session.follow_until {
                session.retry = Some((now + RETRY_EVERY, tags));
            } else {
                session.moving = None;
                session.popup = Some(Popup::NoMatch);
            }
        }
    }
    let cohosts = roster.as_ref().map_or(&[][..], |r| &r.state.cohosts[..]);
    let announced = roster.as_ref().is_some_and(|r| r.state.moving.is_some());
    let (active, moving) = (net.active(), session.moving.is_some());
    track_owners(&mut session.owners, || session_owners(&net, cohosts), active, moving, announced);
    let (busy, changed) = (transition.busy(), changed.read().count() > 0);
    if session.load_failed(busy, changed) {
        // Its post-load teleport will never run.
        travel.pending = None;
        travel.pending_matrix = None;
        session.area = None;
    }
    // The classic panel can leave and host again without a gameplay frame between (online_boundary::update
    // is gameplay-gated), so the picked row's area ends here once its lobby is gone.
    if session.area.as_ref().is_some_and(|a| a.placed()) && !net.mod_identity().0 {
        session.area = None;
    }
    // G5: browse for the pick's map while it loads (the classic panel has requested it: `selected_map` is it). The
    // lobby label is the file stem (multiplayer::world_changed); the menu shows its `_` as spaces (map_library).
    if let Some(tags) = session.early_browse(busy, net.engaged()).cloned()
        && let Some(label) = menu.map_labels().get(menu.selected_map()).map(|l| skate_net::directory::label(&l.replace(' ', "_")))
    {
        // ponytail: browse_tagged filters on `map_name`; swapped for the call, as the load will set it.
        let here = std::mem::replace(&mut net.map_name, label.clone());
        net.browser_rows.clear();
        net.browse_tagged(tags);
        net.map_name = here;
        session.early = Some((label, now));
    }
    let answered = net.browser_status != "Contacting Steam...";
    session.early_answer(answered, &net.browser_rows, now);
    let ready = session.world_ready(busy, changed, travel.pending.is_some());
    match session.take_ready(ready) {
        Some(Intent::Join(tags)) => {
            session.follow_until = now + FOLLOW_FOR;
            if !tags.session.is_empty() {
                session.moving = Some((net.room_id(), session.owners.clone(), tags.clone()));
            }
            let own = net.room_id();
            match session.early_match(&net.map_name, own) {
                Some((rows, i)) => {
                    net.browser_rows = rows;
                    net.join_row(i);
                    session.popup = None;
                    session.join_cover = Some(now);
                }
                None => browse(&mut net, &mut session, tags, now),
            }
        }
        Some(Intent::Host(tags)) => host(&mut net, &mut session, tags, now),
        Some(Intent::Solo) => net.leave(),
        Some(Intent::JoinLobby { lobby }) => {
            // A browse answering later would leave this lobby for a matched one.
            session.cancel_browse();
            net.join_lobby(lobby);
            session.join_cover = Some(now);
        }
        None => {}
    }
    host_failed(&mut net, &mut session, now);
    // Audit 10-28 #11: in the lobby, or its join is over without one (refused, full): searched again without it.
    if net.active() {
        session.quick = None;
    } else if !net.engaged() {
        session.quick_join_failed(now);
    }
    if session.retry.as_ref().is_some_and(|(at, _)| now >= *at) {
        let (_, tags) = session.retry.take().expect("checked above");
        browse(&mut net, &mut session, tags, now);
    }
    if let Some(since) = session.browsing_since {
        let answered = net.browser_status != "Contacting Steam...";
        if answered || now - since > MATCH_TIMEOUT {
            session.browsing_since = None;
            let wanted = session.browsing.take();
            let refused = std::mem::take(&mut session.refused);
            net.browser_rows.retain(|r| !refused.contains(&r.id));
            session.refused = refused;
            let found = matching_row(&net.browser_rows, net.room_id(), &net.map_name).filter(|_| answered && wanted.is_some());
            match (found, wanted) {
                (Some(i), wanted) => {
                    // Audit 10-28 #11: a Quick Match's (not a move's) lobby, searched again without it if it refuses us.
                    session.quick = match wanted {
                        Some(Intent::Join(tags)) if tags.session.is_empty() => Some((net.browser_rows[i].id, tags)),
                        _ => None,
                    };
                    net.join_row(i);
                    session.popup = None;
                    session.join_cover = Some(now);
                }
                // A move's host may still be loading its map: keep looking (the Matching popup stays up).
                (None, Some(Intent::Join(tags))) if !tags.session.is_empty() && now < session.follow_until => {
                    session.retry = Some((now + RETRY_EVERY, tags));
                }
                (None, wanted) => match host_instead(wanted.as_ref(), answered) {
                    // The Matching popup stays up until the lobby forms (host_failed), or a pick's loading screen
                    // takes its place (host).
                    Some(tags) => host(&mut net, &mut session, tags, now),
                    None => {
                        // No lobby will form for the picked row: its challenge and settings never reach a later session.
                        session.area = None;
                        session.lobby = None;
                        session.carry = None;
                        session.moving = None;
                        session.popup = Some(Popup::NoMatch);
                        // 3.3: nothing found, so the Steam relay started for the search ends too (a session stays).
                        if !net.active() {
                            net.leave();
                        }
                    }
                },
            }
        }
    }
    let (in_lobby, _, hosts) = net.mod_identity();
    // Its slot is placed by online_boundary::update, which waits out an open menu (gameplay-gated): so does this.
    let placed = session.area.as_ref().is_none_or(|a| a.placed()) || menu.open;
    let way = Way {
        busy,
        covered: covered.0,
        engaged: net.engaged(),
        hosting: in_lobby && hosts,
        synced: net.synced(),
        settled: placed && !crate::match_core::teleport_blocked(&skater),
        world: roster.as_ref().and_then(|r| r.state.world).is_some_and(|w| {
            menu.session_world() == Some(crate::graphics_menu::Menu::clamp_world([w.weather.into(), w.time.into()]))
        }),
    };
    let (arrive, failed) = session.cover_step(now, way);
    if let Some(at) = arrive
        && let Err(e) = skater.travel_and_mark(at)
    {
        warn!("Online spot: {e}");
    }
    if failed {
        warn!("Online join: no answer from the host in {JOIN_COVER_MAX} s; leaving");
        net.leave();
        session.area = None;
        net.join_failed = true;
    }
    session.end_failed_move(net.engaged());
    cover.set_if_neq(crate::tip_loader::JoinCover(session.join_cover.is_some()));
}

/// Steam join requests (friend "Join Game", accepted invites): join here, or load the
/// session's map first as an Online Challenge Map pick does.
#[allow(clippy::too_many_arguments)]
fn join_requests(
    mut presence: ResMut<crate::presence::Presence>,
    mut session: ResMut<OnlineSession>,
    mut net: ResMut<Multiplayer>,
    mut menu: ResMut<crate::graphics_menu::Menu>,
    mut roster: ResMut<crate::online_roster::OnlineRoster>,
    (transition, mut travel, time): (
        Res<crate::map_transition::MapTransition>,
        ResMut<crate::teleport_menu::Travel>,
        Res<Time<Real>>,
    ),
    (mut crossbar, mut challenge, mut online_ui): (
        Option<ResMut<crate::core_menu::Crossbar>>,
        Option<ResMut<crate::challenge_map::ChallengeMap>>,
        Option<ResMut<crate::online_menu::OnlineUi>>,
    ),
) {
    // Mid-load, `map_name` is still the map being left and a queued load would discard the
    // join: keep the requests until the world is up. Signed out, online_menu asks to sign in first.
    let signed_in = online_ui.as_ref().is_some_and(|ui| ui.signed_in);
    if presence.join_requests.is_empty() || transition.busy() || session.awaiting_world || !signed_in {
        return;
    }
    let labels = menu.map_labels();
    for connect in std::mem::take(&mut presence.join_requests) {
        let plan = join_plan(&connect, &net.map_name, &labels);
        info!("Steam join request {connect:?}: {plan:?}");
        match plan {
            Some(JoinPlan::Here(lobby)) if net.room_id() != Some(lobby) => {
                session.cancel_browse();
                session.area = None; // an invite's lobby is not the picked row's
                session.pending = Some(Intent::JoinLobby { lobby });
            }
            Some(JoinPlan::Travel(lobby, index)) => {
                // As perform_online: close the Start menu screens (a live crossbar pick would
                // cancel this join), drop any queued teleport, then let the classic panel
                // (which refuses map loads while connected) load the map.
                if let Some(c) = crossbar.as_mut() {
                    c.runtime = None;
                }
                if let Some(c) = challenge.as_mut() {
                    c.dismiss();
                }
                // The Player List and its popup too (they would draw over the load and keep the pad).
                if let Some(ui) = online_ui.as_mut() {
                    ui.dismiss = true;
                }
                travel.pending = None;
                travel.pending_matrix = None;
                session.cancel_browse();
                session.area = None;
                net.leave();
                menu.hand_off(7, false, true, Some(index));
                session.awaiting_world = true;
                session.pending = Some(Intent::JoinLobby { lobby });
                // One cover from this map load to the joined lobby (the load's own ends before the join starts).
                session.join_cover = Some(time.elapsed_secs());
            }
            Some(JoinPlan::Missing) => roster
                .lines
                .push(("ID_ONLINE_EVENT_CANNOT_CHANGE_LOCATION", String::new())),
            // Already in that lobby, or not one of our connect strings.
            Some(JoinPlan::Here(_)) | None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skate_net::directory::Row;
    /// Audit 10-28 #11: two players took a Quick Match lobby's last place at once; the one refused was left alone. That
    /// lobby is not matched again and the search runs again.
    #[test]
    fn a_quick_match_refused_by_its_lobby_searches_again() {
        let mut s = OnlineSession::default();
        assert!(!s.quick_join_failed(1.), "no Quick Match join under way");
        let tags = Tags { tab: "2".into(), ..default() };
        s.quick = Some((7, tags.clone()));
        assert!(s.quick_join_failed(2.));
        assert_eq!((s.refused.as_slice(), s.retry.clone()), (&[7][..], Some((2., tags))));
        assert!(!s.quick_join_failed(3.), "once");
    }
    fn row(id: u64, map: &str, players: usize, compatible: bool) -> Row {
        Row { id, map: map.into(), players, capacity: 10, compatible, ping: None, mode: String::new(), name: String::new(), description: String::new() }
    }
    fn tags(tab: &str, filter: &str, challenge: &str) -> Tags {
        Tags { tab: tab.into(), filter: filter.into(), challenge: challenge.into(), ..default() }
    }
    #[test]
    fn moves_find_the_hosts_new_lobby_by_token() {
        let spot = "ID_MAP_FILTER_CHALLENGETYPES_SPOTBATTLE";
        let picked = Intent::Host(tags("versus", spot, "hom_dwtn_01"));
        let follow = |host, private| {
            let carry = skate_net::roster::SessionState { private, ..default() };
            follow_intent(picked.clone(), &Follow { token: 0xab, host, carry })
        };
        let token = |t: Tags| Tags { session: "ab".into(), ..t };
        assert_eq!(follow(true, false), Intent::Host(token(tags("versus", spot, "hom_dwtn_01"))));
        // Audit 10-24: it keeps its filter (a Skate 1 / 2 S.K.A.T.E. lobby held 6, not 4).
        assert_eq!(follow(true, true), Intent::Host(Tags { hidden: true, ..token(tags("versus", spot, "hom_dwtn_01")) }), "P5: hidden from strangers' browses");
        // A public session's move to a row (whose pick is a Create Private one) stays public.
        let hidden = Intent::Host(Tags { hidden: true, ..tags("versus", spot, "hom_dwtn_01") });
        assert_eq!(follow_intent(hidden, &Follow { token: 0xab, host: true, carry: default() }), Intent::Host(token(tags("versus", spot, "hom_dwtn_01"))));
        assert_eq!(follow(false, false), Intent::Join(token(tags("versus", "", ""))), "guests match the token only");
        assert_eq!(follow_intent(Intent::Solo, &Follow { token: 1, host: false, carry: default() }), Intent::Solo);
        // Spot battles and every other challenge hold 6 (retail's max_players); freeskate the protocol ceiling, on any tab.
        let challenge = |tab| Pick::Challenge { challenge: default(), name: "x".into(), choice: Choice::CreatePrivate, tab, folder: spot.into() };
        let location = |tab| Pick::Location { location: default(), tab, name: "x".into(), choice: Choice::CreatePrivate };
        let max = skate_net::lobby::MAX_PLAYERS;
        assert_eq!([0, 3, 4, 5].map(|t| capacity(&challenge(t))), [6; 4]);
        assert_eq!([2, 4, 5].map(|t| capacity(&location(t))), [max; 3]);
        // A row's start locator's slots don't change it (Skate 1 / 2 rows have 8): 6, as retail.
        let mut six = crate::online_map::Challenge::default();
        six.spawn.start = Some(crate::online_map::Start { matrix: [[0.; 4]; 4], slots: vec![[[0.; 4]; 4]; 8] });
        let pick = Pick::Challenge { challenge: six, name: "x".into(), choice: Choice::CreatePrivate, tab: 3, folder: spot.into() };
        assert_eq!(capacity(&pick), 6, "ID_ONLNE_TOO_MANY_PLAYERS: only supports 6 players");
        assert_eq!((TABS[crate::match_core::TEAM_TAB], TABS[crate::match_core::CAREER_TAB]), ("team", "career"));
    }
    /// P5: a member who opens a lobby with the move's token first does not get the session.
    #[test]
    fn a_move_follows_only_the_old_host_or_a_cohost() {
        let (host, cohost, hijacker) = (76561198000000001, 76561198000000002, 76561198000000003);
        assert!(owner_ok(host, &[host, cohost]) && owner_ok(cohost, &[host, cohost]));
        assert!(!owner_ok(hijacker, &[host, cohost]));
        assert!(!owner_ok(0, &[0]) && !owner_ok(host, &[]), "unknown owners: nobody");
        // The refusal is remembered, so the next browse does not pick the same lobby.
        let mut s = OnlineSession { refused: vec![3], ..default() };
        let rows = vec![row(3, "University", 1, true), row(4, "University", 1, true)];
        let left: Vec<Row> = rows.into_iter().filter(|r| !s.refused.contains(&r.id)).collect();
        assert_eq!(matching_row(&left, None, "University").map(|i| left[i].id), Some(4));
        s.cancel_browse();
        assert!(s.refused.is_empty() && s.moving.is_none(), "a new pick starts clean");
    }
    /// Bug 10-25 #17 (build 10): 4+ players, the host switches the mode (a session move); a guest that saw the move
    /// late is still in the old lobby when the host leaves it, and Steam elects another member its owner. The guest
    /// still follows the host that announced the move, frame by frame as `run` tracks it, into its new lobby.
    #[test]
    fn a_late_follower_still_follows_the_host_that_moved() {
        let (host, late, other) = (76561198000000001, 76561198000000002, 76561198000000003);
        let mut owners = Vec::new();
        // In the session (frames), then the move's `@session` arrives, then the host leaves the old lobby.
        for (steam_owner, announced) in [(host, false), (host, false), (host, true), (other, true), (late, true)] {
            track_owners(&mut owners, || vec![steam_owner], true, false, announced);
        }
        assert!(owner_ok(host, &owners), "the moved lobby is the announcing host's: {owners:?}");
        assert!(!owner_ok(other, &owners), "a member Steam elected since owns no follow");
        // Its own follow under way (`moving`) or out of the session: nothing changes; a new session's owners again.
        track_owners(&mut owners, || vec![other], false, false, false);
        track_owners(&mut owners, || vec![other], true, true, false);
        assert_eq!(owners, [host]);
        track_owners(&mut owners, || vec![other], true, false, false);
        assert_eq!(owners, [other]);
    }
    #[test]
    fn skate2_and_skate1_tabs_keep_their_own_lobbies() {
        let skate = "ID_MAP_FILTER_CHALLENGETYPES_SKATE";
        let challenge = crate::online_map::Challenge { id: "ol_skate_01".into(), ..default() };
        let pick = |tab| Pick::Challenge { challenge: challenge.clone(), name: "x".into(), choice: Choice::CreatePrivate, tab, folder: skate.into() };
        let private = |t: Tags| Intent::Host(Tags { hidden: true, ..t });
        assert_eq!(intent(&pick(4)), private(tags("skate2", skate, "ol_skate_01")), "never matches a Skate 3 lobby");
        let made = lobby(&intent(&pick(4))).unwrap();
        assert_eq!((made.title.as_str(), made.team, made.row.unwrap().tab), (skate, false, 4), "the row a match finds on tab 4");
        assert_eq!(intent(&pick(5)), private(tags("skate1", skate, "ol_skate_01")));
        // user 10-12: a Skate 2 S.K.A.T.E. lobby holds 4 (retail throwdownresults draws 4 rows; a move past it gets
        // ID_ONLNE_TOO_MANY_PLAYERS); Skate 1's too (its skatehud draws 4 players).
        assert_eq!((capacity(&pick(4)), capacity(&pick(5))), (4, 4));
        // Audit 10-24: a private session's move there too.
        let carry = skate_net::roster::SessionState { private: true, ..default() };
        let moved = follow_intent(intent(&pick(4)), &Follow { token: 1, host: true, carry });
        assert!(matches!(&moved, Intent::Host(t) if t.capacity() == 4), "{moved:?}");
        let location = Pick::Location { location: default(), tab: 4, name: "#Waterfront".into(), choice: Choice::JoinPublic };
        assert_eq!(intent(&location), Intent::Join(tags("freeskate", "", "#Waterfront")));
        assert_eq!(lobby(&intent(&location)), None, "Skate 2 freeskate is Online Freeskate");
        assert_eq!(location.target().unwrap().tab, 4, "a move finds it on its own tab");
    }
    #[test]
    fn matches_stay_on_this_map_and_out_of_our_own_lobby() {
        // The relay already matched the tags; the rows are that browse's answer.
        let rows = [row(1, "DownTown", 2, true), row(2, "University", 10, true), row(3, "University", 3, false),
                    row(4, "University", 3, true), row(5, "university", 3, true)];
        assert_eq!(matching_row(&rows, None, "University"), Some(3), "other map (I2), full and incompatible skipped");
        assert_eq!(matching_row(&rows, Some(4), "University"), Some(4), "our own lobby skipped (I3)");
        assert_eq!(matching_row(&rows, Some(5), "Sanatorium"), None);
    }
    #[test]
    fn picks_carry_the_spec_lobby_keys() {
        let command = |command: &str, tab, folder: &str| intent(&Pick::Command { command: command.into(), tab, folder: folder.into() });
        let spot = "ID_MAP_FILTER_CHALLENGETYPES_SPOTBATTLE";
        // I2: separate keys; a long id is not squeezed into the 32-char label.
        assert_eq!(command("online_versus_unranked_challengetypequickmatch", 3, spot), Intent::Join(tags("versus", spot, "")));
        assert_eq!(command("online_versus_ranked_quickmatch", 3, ""), Intent::Join(tags("versus", "", "")));
        assert_eq!(command("online_versus_ranked_custommatch", 3, ""), Intent::Host(tags("versus", "", "online_versus_ranked_custommatch")));
        assert_eq!(command("online_freeskate_here_solo", 2, ""), Intent::Solo);
        let challenge = crate::online_map::Challenge { id: "hom_dwtn_01".into(), ..default() };
        let pick = |choice| intent(&Pick::Challenge { challenge: challenge.clone(), name: "x".into(), choice, tab: 3, folder: spot.into() });
        let private = |t: Tags| Intent::Host(Tags { hidden: true, ..t });
        assert_eq!(pick(Choice::JoinPublic), Intent::Join(tags("versus", spot, "hom_dwtn_01")), "with its folder (#13)");
        assert_eq!(pick(Choice::CreatePrivate), private(tags("versus", spot, "hom_dwtn_01")));
        // Freeskate: Join Team Lobby (from either tab) joins, Team Freeskate Here hosts; a location asks which.
        for tab in [0, 2] {
            assert_eq!(command("online_freeskate_team_join", tab, ""), Intent::Join(tags("freeskate", "", "")));
        }
        assert_eq!(command("online_freeskate_here_team", 2, ""), Intent::Host(tags("freeskate", "", "")));
        let memorial = "ID_ONLINE_FS_LOCATION_DWTN_MEMORIAL";
        let location = |choice| intent(&Pick::Location { location: default(), tab: 2, name: memorial.into(), choice });
        assert_eq!(location(Choice::JoinPublic), Intent::Join(tags("freeskate", "", memorial)));
        assert_eq!(location(Choice::CreatePrivate), private(tags("freeskate", "", memorial)));
    }
    /// Audit 10-24: Create Private Game / Freeskate made a public lobby that Quick Match, Join Public and Join Team
    /// Lobby walked into. It is hidden from every browse and starts private; the public picks stay public.
    #[test]
    fn create_private_makes_a_private_session() {
        let challenge = crate::online_map::Challenge { id: "hom_dwtn_01".into(), ..default() };
        let pick = |choice| intent(&Pick::Challenge { challenge: challenge.clone(), name: "x".into(), choice, tab: 3, folder: "".into() });
        let location = |choice| intent(&Pick::Location { location: default(), tab: 2, name: "x".into(), choice });
        for private in [pick(Choice::CreatePrivate), location(Choice::CreatePrivate)] {
            assert!(matches!(&private, Intent::Host(t) if t.hidden && t.session.is_empty()), "{private:?}");
            assert!(fresh_carry(&private).is_some_and(|s| s.private));
        }
        let team = intent(&Pick::Command { command: "online_freeskate_here_team".into(), tab: 2, folder: "".into() });
        for public in [pick(Choice::JoinPublic), location(Choice::JoinPublic), team] {
            assert!(!matches!(&public, Intent::Host(t) if t.hidden) && fresh_carry(&public).is_none(), "{public:?}");
        }
    }
    /// Bug 10-25 #13 (build 10): Join Public Game, Quick Match and Join Team Lobby gave up with No Match when no session
    /// was open. They host the one they looked for, public, and the player is in it; a challenge row's lobby plays its
    /// mode (Start Challenge reads it from the lobby's title). A move's token search still ends in No Match (only the
    /// host's new lobby may be followed), and so does a browse Steam never answered.
    #[test]
    fn an_unmatched_join_hosts_a_public_session() {
        let spot = "ID_MAP_FILTER_CHALLENGETYPES_SPOTBATTLE";
        let challenge = crate::online_map::Challenge { id: "ots_dwtn_01_ol".into(), ..default() };
        let joined = intent(&Pick::Challenge { challenge, name: "x".into(), choice: Choice::JoinPublic, tab: 3, folder: spot.into() });
        let hosted = host_instead(Some(&joined), true).expect("an empty answer hosts");
        assert_eq!(hosted, tags("versus", spot, "ots_dwtn_01_ol"));
        assert!(fresh_carry(&Intent::Host(hosted.clone())).is_none(), "public: Join Public finds it");
        let made = lobby(&Intent::Host(hosted)).unwrap();
        assert_eq!(crate::match_core::Mode::from_filter(&made.title), Some(crate::match_core::Mode::SpotBattle));
        assert_eq!(made.row.map(|r| r.id), Some("ots_dwtn_01_ol".into()), "its row: Start Challenge plays it");
        for command in ["online_versus_unranked_challengetypequickmatch", "online_freeskate_team_join"] {
            let quick = intent(&Pick::Command { command: command.into(), tab: 3, folder: spot.into() });
            assert!(host_instead(Some(&quick), true).is_none(), "{command} never makes a lobby");
        }
        let location = intent(&Pick::Location { location: default(), tab: 2, name: "#Memorial".into(), choice: Choice::JoinPublic });
        assert_eq!(host_instead(Some(&location), true), Some(tags("freeskate", "", "#Memorial")));
        let moved = Intent::Join(Tags { session: "ab".into(), ..tags("versus", "", "") });
        assert_eq!(host_instead(Some(&moved), true), None, "a move follows its host");
        assert_eq!(host_instead(Some(&joined), false), None, "Steam never answered");
        assert_eq!(host_instead(None, true), None);
    }
    /// User 10-25: joining or creating a session on the Skate 1 / 2 maps started their tutorial while the pick's map
    /// loaded (Online was off until the lobby formed).
    #[test]
    fn a_fresh_pick_counts_as_online_from_the_pick() {
        let s = |pending| OnlineSession { pending, ..default() };
        assert!(s(Some(Intent::Join(tags("freeskate", "", "")))).going_online(), "Join Public Game's map load");
        assert!(s(Some(Intent::Host(tags("freeskate", "", "")))).going_online(), "Create Private Game's");
        assert!(s(Some(Intent::JoinLobby { lobby: 7 })).going_online(), "a Steam invite's");
        assert!(!s(Some(Intent::Solo)).going_online(), "single player is no session");
        assert!(!s(None).going_online());
        let browsing = OnlineSession { browsing: Some(Intent::Join(tags("freeskate", "", ""))), ..default() };
        assert!(browsing.going_online(), "its browse after the load");
    }
    /// Audit 10-24: a session move leaves the old session and searches up to FOLLOW_FOR for the new one; Skate 2 physics
    /// (off online) switched on for that search, then off again at the join. The move counts as online throughout.
    #[test]
    fn a_session_move_counts_as_online_until_its_new_session() {
        let moving = |t: Tags| Tags { session: "ab".into(), ..t };
        let s = |pending| OnlineSession { pending, ..default() };
        assert!(s(Some(Intent::Join(moving(tags("versus", "", ""))))).move_under_way(), "loading the move's map");
        assert!(s(Some(Intent::Host(moving(tags("versus", "", ""))))).move_under_way());
        let mut retrying = s(None);
        retrying.retry = Some((1., moving(tags("versus", "", ""))));
        assert!(retrying.move_under_way(), "between browses for the host's new lobby");
        let follow = OnlineSession { follow: Some(Follow { token: 1, host: false, carry: default() }), ..default() };
        assert!(follow.move_under_way());
        assert!(!s(Some(Intent::Join(tags("versus", "", "")))).move_under_way(), "a Quick Match is no move");
        assert!(!s(None).move_under_way());
        // Review 10-24: a move's join that failed or timed out ends it; one still joining or retrying does not.
        let mut joining = s(None);
        joining.moving = Some((None, vec![7], moving(tags("versus", "", ""))));
        joining.end_failed_move(true);
        assert!(joining.move_under_way(), "still joining");
        joining.retry = Some((1., moving(tags("versus", "", ""))));
        joining.end_failed_move(false);
        assert!(joining.moving.is_some(), "between browses");
        joining.retry = None;
        joining.end_failed_move(false);
        assert!(!joining.move_under_way(), "the join failed: offline again");
        let mut picked = OnlineSession { follow: Some(Follow { token: 1, host: true, carry: default() }), ..default() };
        picked.cancel_browse();
        assert!(!picked.move_under_way(), "a cancelled move");
    }
    #[test]
    fn challenge_picks_make_challenge_lobbies() {
        let spot = "ID_MAP_FILTER_CHALLENGETYPES_SPOTBATTLE";
        let lobby_of = |command: &str, tab, folder: &str| lobby(&intent(&Pick::Command { command: command.into(), tab, folder: folder.into() }));
        let row = |tab, id: &str| Some(skate_net::roster::Target::new(tab, id, "", ""));
        let custom = "online_team_ranked_custommatch";
        assert_eq!(lobby_of(custom, 0, spot), Some(ChallengeLobby { title: spot.into(), team: true, row: row(0, custom) }));
        assert_eq!(lobby_of("online_versus_ranked_quickmatch", 3, ""),
                   Some(ChallengeLobby { title: "ID_CROSSBAR_ONLINE_CHALLENGE_MODE_TITLE".into(), team: false, row: None }));
        assert_eq!(lobby_of("online_team_ranked_quickmatch", 0, "").unwrap().title, "ID_CROSSBAR_ONLINE_TEAM_CHALLENGE_MODE_TITLE");
        let challenge = crate::online_map::Challenge { id: "hom_dwtn_01".into(), ..default() };
        let pick = Pick::Challenge { challenge, name: "x".into(), choice: Choice::CreatePrivate, tab: 0, folder: spot.into() };
        assert_eq!(lobby(&intent(&pick)), Some(ChallengeLobby { title: spot.into(), team: true, row: row(0, "hom_dwtn_01") }), "the row a match plays");
        // Freeskate stays Online Freeskate.
        assert_eq!(lobby_of("online_freeskate_team_join", 0, ""), None);
        assert_eq!(lobby_of("online_freeskate_here_team", 2, ""), None);
        assert_eq!(lobby_of("online_freeskate_here_solo", 2, ""), None);
        let location = Pick::Location { location: default(), tab: 2, name: "x".into(), choice: Choice::CreatePrivate };
        assert_eq!(lobby(&intent(&location)), None);
        assert_eq!(lobby(&Intent::JoinLobby { lobby: 5 }), None);
    }
    /// User 10-09: joining showed the player alone on the map before the session came in. The loading screen stays.
    #[test]
    fn the_join_cover_holds_until_someone_is_heard_from() {
        assert!(keep_cover(0., 1., true, false), "joining, nobody heard yet");
        assert!(!keep_cover(0., 1., true, true), "the host's packets arrived: in");
        assert!(!keep_cover(0., 1., false, false), "the join failed or was left");
        assert!(!keep_cover(0., JOIN_COVER_MAX, true, false), "never stuck on the loading screen");
    }
    /// User 10-26: joining or a session move teleported first and loaded after; retail loads and moves at once. A
    /// same-map pick's spot waits for the loading screen, which stays up until the player is in on its spot.
    #[test]
    fn the_spot_is_reached_behind_the_loading_screen() {
        let spot = [[2.; 4]; 4];
        let search = Intent::Join(tags("versus", "", "x"));
        let mut s = OnlineSession { browsing: Some(search.clone()), browsing_since: Some(0.), arrival: Some(spot), ..default() };
        assert_eq!(s.cover_step(1., Way::default()), (None, false), "the search: no screen, nobody moves");
        assert!(s.join_cover.is_none() && s.arrival.is_some());
        // Found: the join raises the screen (run), not drawn yet.
        (s.browsing, s.browsing_since, s.join_cover) = (None, None, Some(2.));
        let joining = Way { engaged: true, ..default() };
        assert_eq!(s.cover_step(2., joining), (None, false), "not up yet: nobody moves");
        let up = Way { covered: true, ..joining };
        assert_eq!(s.cover_step(2.1, up), (Some(spot), false), "up: the spot, behind it");
        let heard = Way { synced: true, ..up };
        s.cover_step(3., heard);
        assert!(s.join_cover.is_some(), "in, its slot not placed or its teleport in flight: still up");
        s.cover_step(3.1, Way { settled: true, world: true, ..heard });
        assert!(s.join_cover.is_none(), "in on its spot, the host's weather and time on: down");
        // No match, cancelled, failed: the spot stays behind.
        let mut s = OnlineSession { arrival: Some(spot), ..default() };
        assert_eq!(s.cover_step(1., Way::default()), (None, false));
        assert!(s.arrival.is_none());
        // A host: up while its lobby forms, down once it has.
        let mut s = OnlineSession { hosting_since: Some(0.), join_cover: Some(0.), arrival: Some(spot), ..default() };
        assert_eq!(s.cover_step(0.5, Way { covered: true, ..default() }), (Some(spot), false));
        s.hosting_since = None;
        let formed = Way { engaged: true, hosting: true, covered: true, ..default() };
        assert_eq!(s.cover_step(1., formed), (None, false));
        assert!(s.join_cover.is_some(), "its slot not placed yet");
        assert_eq!(s.cover_step(1.1, Way { settled: true, ..formed }), (None, false));
        assert!(s.join_cover.is_none());
        // Alone in its lobby a host is never an unanswered join.
        let mut s = OnlineSession { join_cover: Some(0.), ..default() };
        assert_eq!(s.cover_step(JOIN_COVER_MAX, formed), (None, false));
        assert!(s.join_cover.is_none(), "never stuck on the loading screen");
        assert_eq!(OnlineSession { join_cover: Some(0.), ..default() }.cover_step(JOIN_COVER_MAX, joining), (None, true), "a join nobody answered");
        // An online pick's map load raises it and keeps it fresh however long it takes, so it never drops between the
        // load and the join.
        let mut s = OnlineSession { pending: Some(search), awaiting_world: true, ..default() };
        s.cover_step(0., Way { busy: true, ..default() });
        s.cover_step(30., Way { busy: true, ..default() });
        assert_eq!(s.join_cover, Some(30.));
        // A move: up from leaving for it (online_moves::follow) while still in the old lobby.
        let mut s = OnlineSession { follow: Some(Follow { token: 1, host: false, carry: default() }), ..default() };
        s.cover(0.);
        s.cover_step(0.1, Way { engaged: true, synced: true, settled: true, ..default() });
        assert!(s.join_cover.is_some());
    }
    /// User 10-26: a joiner loaded in on Clear and Day, then the host's weather and time came on. The screen stays up
    /// until they are on (online_moves::world); a host's that never comes gives it back at JOIN_COVER_MAX, the join
    /// standing. A host sets its own.
    #[test]
    fn the_join_cover_waits_for_the_hosts_weather_and_time() {
        let heard = Way { engaged: true, covered: true, synced: true, settled: true, ..default() };
        let mut s = OnlineSession { join_cover: Some(0.), ..default() };
        assert_eq!(s.cover_step(1., heard), (None, false));
        assert!(s.join_cover.is_some(), "in on its spot, the host's world not on yet: still up");
        assert_eq!(s.cover_step(1.1, Way { world: true, ..heard }), (None, false));
        assert!(s.join_cover.is_none(), "the host's world on: down");
        let mut s = OnlineSession { join_cover: Some(0.), ..default() };
        assert_eq!(s.cover_step(JOIN_COVER_MAX, heard), (None, false), "never stuck, and the join stands");
        assert!(s.join_cover.is_none());
        let mut s = OnlineSession { join_cover: Some(0.), ..default() };
        s.cover_step(1., Way { hosting: true, ..heard });
        assert!(s.join_cover.is_none(), "a host: down");
    }
    /// Item 30: a host that never answers (version mismatch, NAT) no longer leaves the joiner alone in a dead session.
    #[test]
    fn an_unanswered_join_is_dropped_when_the_cover_runs_out() {
        assert!(!join_timed_out(0., 1., true, false), "still waiting");
        assert!(join_timed_out(0., JOIN_COVER_MAX, true, false), "nobody answered: leave with popup 104");
        assert!(!join_timed_out(0., JOIN_COVER_MAX, true, true), "joined");
        assert!(!join_timed_out(0., JOIN_COVER_MAX, false, false), "a map load, or already left");
    }
    #[test]
    fn intent_runs_once_after_the_world_changes() {
        let mut s = OnlineSession::default();
        s.pending = Some(Intent::Host(tags("team", "", "x")));
        assert_eq!(s.take_ready(false), None, "map still loading");
        assert_eq!(s.take_ready(true), Some(Intent::Host(tags("team", "", "x"))));
        assert_eq!(s.take_ready(true), None, "only once");
    }
    #[test]
    fn failed_load_does_not_strand_the_intent() {
        let mut s = OnlineSession { awaiting_world: true, ..default() };
        assert!(!s.world_ready(false, false, false), "load not started yet");
        assert!(!s.world_ready(true, false, false), "loading");
        assert!(s.world_ready(false, false, false), "busy -> idle without WorldChanged clears the wait");
        let mut s = OnlineSession { awaiting_world: true, ..default() };
        assert!(s.world_ready(false, true, false), "WorldChanged clears the wait");
        assert!(!s.world_ready(false, false, true), "teleport still pending");
        // One frame per tuple, as run() calls them: (busy, WorldChanged).
        let frames = |s: &mut OnlineSession, frames: &[(bool, bool)]| {
            frames.iter().map(|&(busy, changed)| {
                let failed = s.load_failed(busy, changed);
                s.world_ready(busy, changed, false);
                failed
            }).collect::<Vec<_>>()
        };
        // I1: WorldChanged at commit (frame N), Publishing frames, idle at N+4: a success.
        let mut s = OnlineSession::default();
        assert_eq!(frames(&mut s, &[(true, false), (true, true), (true, false), (true, false), (true, false), (false, false)]), [false; 6]);
        // The next load fails: busy -> idle with no WorldChanged.
        assert_eq!(frames(&mut s, &[(true, false), (false, false)]), [false, true]);
    }
    /// G5 (perf 10-17): a Quick Match pick's browse runs during its map load; the join waits for the world.
    #[test]
    fn a_join_pick_browses_during_its_map_load() {
        let join = tags("versus", "", "");
        let mut s = OnlineSession { pending: Some(Intent::Join(join.clone())), awaiting_world: true, ..default() };
        assert_eq!(s.early_browse(false, false), None, "the load is not under way yet");
        assert_eq!(s.early_browse(true, true), None, "in a session: no");
        assert_eq!(s.early_browse(true, false), Some(&join));
        s.early = Some(("University".into(), 0.));
        assert_eq!(s.early_browse(true, false), None, "once");
        s.early_answer(false, &[], 1.);
        assert!(s.early.is_some(), "still contacting Steam");
        let rows = [row(1, "DownTown", 2, true), row(4, "University", 3, true)];
        s.early_answer(true, &rows, 2.);
        assert_eq!(s.early_browse(true, false), None, "answered: not again");
        // The world is up (its WorldChanged cleared `browser_rows`): the kept row is joined.
        assert_eq!(s.early_match("University", None).map(|(r, i)| r[i].id), Some(4));
        assert!(s.early_match("University", None).is_none(), "taken once");
        // A load that failed, or ended on another map: browse there as before.
        s.early_rows = Some(("University".into(), rows.to_vec()));
        assert!(s.early_match("DownTown", None).is_none());
        // Nothing found, an error or the load's clear (no rows): browse after the load as before (no early No Match).
        s.early = Some(("University".into(), 0.));
        s.early_answer(true, &[], 3.);
        assert!(s.early_match("University", None).is_none());
        // Timed out: nothing kept.
        s.early = Some(("University".into(), 0.));
        s.early_answer(false, &rows, MATCH_TIMEOUT + 1.);
        assert!(s.early.is_none() && s.early_rows.is_none());
        // The lobby we are already in is never the match.
        s.early_rows = Some(("University".into(), rows.to_vec()));
        assert!(s.early_match("University", Some(4)).is_none());
        // A session move's join (FOLLOW_FOR, P5 owner checks) browses after the load only.
        let moving = OnlineSession { pending: Some(Intent::Join(Tags { session: "ab".into(), ..join.clone() })), awaiting_world: true, ..default() };
        assert_eq!(moving.early_browse(true, false), None);
        let host = OnlineSession { pending: Some(Intent::Host(join)), awaiting_world: true, ..default() };
        assert_eq!(host.early_browse(true, false), None, "hosts don't browse");
        // A new pick (perform_online) starts clean.
        s.early = Some(("University".into(), 0.));
        s.early_rows = Some(("University".into(), rows.to_vec()));
        s.cancel_browse();
        assert!(s.early.is_none() && s.early_rows.is_none());
    }
    #[test]
    fn join_plan_travels_only_to_installed_maps() {
        let labels = vec!["University".to_string(), "DownTown".to_string()];
        assert_eq!(join_plan("sk8:5:University", "University", &labels), Some(JoinPlan::Here(5)));
        assert_eq!(join_plan("sk8:5:", "University", &labels), Some(JoinPlan::Here(5)), "lobby-only request");
        assert_eq!(join_plan("sk8:5:downtown", "University", &labels), Some(JoinPlan::Travel(5, 1)), "same_name is case-insensitive");
        assert_eq!(join_plan("sk8:5:Sanatorium", "University", &labels), Some(JoinPlan::Missing));
        assert_eq!(join_plan("garbage", "University", &labels), None);
        // A 40-char user map: the sender's name is directory::label'd to 32 chars.
        let long = "a".repeat(40);
        let labels = vec!["University".to_string(), long.clone()];
        let connect = skate_net::presence::connect_string(5, &skate_net::directory::label(&long));
        assert_eq!(join_plan(&connect, "University", &labels), Some(JoinPlan::Travel(5, 1)));
    }
}
