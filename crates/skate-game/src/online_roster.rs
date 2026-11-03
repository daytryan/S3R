//! Session roster: publishes our `@player` (and `@session` when hosting), turns roster changes
//! into retail event-log lines, keeps recent players, advertises the rich-presence connect string
//! and leaves when the host kicks us.
use crate::{multiplayer::Multiplayer, presence::Presence};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use skate_net::{
    lobby::Session,
    presence::Order,
    roster::{self, Player, RosterEvent},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

const RECENT_MAX: usize = 50;
/// The creator tag's work (creator.rs) per actor, at most once per this many ms however often a record changes.
const CREATOR_CHECK_MS: u64 = 500;

#[derive(Resource, Default)]
pub(crate) struct OnlineRoster {
    pub members: Vec<roster::Member>,
    /// Authoritative copy: ours when hosting, else the host's last `@session`.
    pub state: roster::SessionState,
    /// Event-log lines: string ID + `{0:s}` argument (the online HUD drains these).
    pub lines: Vec<(&'static str, String)>,
    /// A central message for this player alone (online_hud::central): the host's kick went through (0x8262cf80).
    pub central: Option<String>,
    /// Not retail: a notice for the top-middle box (online_hud::chyron), [title, body] as text or string IDs; where
    /// the HUD has no box it goes to `central`.
    pub chyron: Option<[String; 2]>,
    /// The central message stays up until this clears (the boundary warning while outside the area).
    pub central_held: bool,
    pub recent: RecentPlayers,
    pub local_actor: u64,
    /// Bumped when `members` or `recent` change (the Player List rebuilds its rows then).
    pub generation: u64,
    tracker: roster::Tracker,
    /// (session id, local actor) of the session being tracked.
    key: Option<(u64, u64)>,
    /// [`members_key`] of the session `members` was built from.
    members_key: Option<u64>,
    /// The Steam identity (`Presence::me`) our `@player` was last published for.
    published: Option<Option<(u64, String)>>,
    /// The `@session` we last published as host.
    published_state: Option<roster::SessionState>,
    /// The host's `@session` record (host actor, seq) last decoded, and what it decoded to.
    inherited: Option<((u64, u32), Option<roster::SessionState>)>,
    /// The Steam lobby our rich-presence connect string advertises.
    connect: Option<u64>,
    /// Actors already added to `recent` this session.
    met: BTreeSet<u64>,
    path: PathBuf,
    /// A private session's move: its new lobby turns private at this time, once the guests found it by browse.
    private_at: Option<u64>,
    /// User 10-25, the creator tag (creator.rs): our challenge for the tracked session, and whether it is published.
    nonce: Option<(crate::creator::Nonce, bool)>,
    /// The challenges our answers last covered (this PC holds the creator key), and when they were signed (ms).
    answered: Option<(Vec<(u64, crate::creator::Nonce)>, u64)>,
    /// Per actor: its answer records' seqs and Steam ID when last checked, when (ms), and whether they proved it.
    creator_checks: BTreeMap<u64, (([u32; 4], u64), u64, bool)>,
}

/// A moved private session stays public this long (ms): Steam lists no friends-only lobby, and the guests find the
/// host's new one by browsing (online_session::FOLLOW_FOR after their own map is up). User 10-29: it was 45 s, so a
/// move to a big map took only the players whose load and search came in time; now the whole search and 30 s more
/// (the lobby stays hidden: only the move's token finds it).
const PRIVATE_AFTER_MS: u64 = (crate::online_session::FOLLOW_FOR as u64 + 30) * 1000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct RecentPlayer {
    pub name: String,
    pub steam: u64,
}

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct RecentPlayers {
    pub players: Vec<RecentPlayer>,
}
impl RecentPlayers {
    /// Newest first; one entry per SteamID (per name for Direct players). False if nothing changed.
    pub fn add(&mut self, name: &str, steam: u64) -> bool {
        let entry = RecentPlayer { name: name.to_owned(), steam };
        if self.players.first() == Some(&entry) {
            return false;
        }
        self.players
            .retain(|p| if steam != 0 { p.steam != steam } else { p.steam != 0 || p.name != name });
        self.players.insert(0, entry);
        self.players.truncate(RECENT_MAX);
        true
    }
    pub fn load(path: &Path) -> Self {
        crate::saves::load_json(path).ok().unwrap_or_default()
    }
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        crate::saves::write_atomic(path, &serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)
    }
}

/// `under_way`: a match is running, so a joiner only watches it (retail's event-log handler 0x825e5f78, join type 9
/// with the observer flag +488 set: 0x825e5b88 formats ID_ONLINE_EVENT_JOIN_OBSERVER).
/// None: retail logs nothing (a kick has no line in TU3; the kicker gets a central message, 0x8262cf80).
pub(crate) fn line(event: &RosterEvent, under_way: bool) -> Option<(&'static str, String)> {
    Some(match event {
        RosterEvent::Joined(n) if under_way => ("ID_ONLINE_EVENT_JOIN_OBSERVER", n.clone()),
        RosterEvent::Joined(n) => ("ID_ONLINE_EVENT_JOIN_SKATER", n.clone()),
        RosterEvent::Left(n) => ("ID_ONLINE_EVENT_LEAVE", n.clone()),
        RosterEvent::NewHost(n) => ("ID_ONLINE_EVENT_NEWHOST", n.clone()),
        // 0x825e5f78's event 25: Make Host's hand-off, "{0:s} is now the host".
        RosterEvent::AdminTransfer(n) => ("ID_ONLINE_EVENT_ADMIN_TRANSFER", n.clone()),
        RosterEvent::HostLeft => ("ID_ONLINE_EVENT_HOSTLEFT", String::new()),
        RosterEvent::Kicked(_) => return None,
        RosterEvent::Privacy { by, private: true } => ("ID_ONLINE_EVENT_PRIVATE_SESSION", by.clone()),
        RosterEvent::Privacy { by, private: false } => ("ID_ONLINE_EVENT_PUBLIC_SESSION", by.clone()),
    })
}

/// The retail lines for a session that has ended under us, or `None` while it lives (the caller
/// leaves it). Kicked; refused (3: full); or, in a Direct session, the host gone (a Steam lobby
/// elects a new host instead, and the tracker logs that).
pub(crate) fn end_lines(session: &Session, steam: bool) -> Option<&'static [&'static str]> {
    Some(match session.rejected {
        _ if session.kicked => &["ID_ONLINE_KICKED_BY_GAME_HOST"],
        Some(3) => &["ID_JOIN_ONLINE_GAME_FAILED_POPUP_MESSAGE"],
        Some(_) => &["ID_ONLINE_PLAYNOW_ERROR_STARTING"],
        None if session.host_lost && !steam => &["ID_ONLINE_EVENT_HOSTLEFT", "ID_ONLINE_GAME_LEFT"],
        None => return None,
    })
}

/// User 10-25, the creator tag (creator.rs). Publishes our challenge for this session; with the creator key on this PC
/// and Steam saying we are the creator, signs every player's challenge; and returns the members whose answers to our
/// challenge check out (ourselves, when we are the creator). A Steam-verified ID ([`verified`]: a host has one for every
/// player, a guest for the host) must be the creator's too.
/// `now` is the session clock (Multiplayer::now_ms); `signer` the creator key if this PC holds it, `public` the key
/// answers must verify under (both creator.rs; a test passes its own).
fn creator_tags(
    tracked: &mut OnlineRoster,
    session: &mut Session,
    members: &[roster::Member],
    own: Option<u64>,
    now: u64,
    (signer, public): (Option<&ring::signature::Ed25519KeyPair>, &[u8]),
) -> BTreeSet<u64> {
    let local = session.local;
    let (nonce, published) = tracked.nonce.get_or_insert_with(|| (crate::creator::nonce(), false));
    if !*published {
        *published = session.publish_application(roster::NONCE_KEY, nonce.to_vec(), now);
    }
    let nonce = *nonce;
    let signer = signer.filter(|_| own == Some(crate::creator::CREATOR_STEAM_ID));
    if let Some(signer) = signer {
        let mut viewers: Vec<(u64, crate::creator::Nonce)> = (session.actors.iter())
            .filter(|&(&actor, _)| actor != local)
            .filter_map(|(&actor, a)| Some((actor, crate::creator::parse_nonce(&a.application.get(roster::NONCE_KEY)?.value)?)))
            .collect();
        viewers.sort_unstable();
        // Signed again at most every CREATOR_CHECK_MS, however often players change their challenges.
        let due = tracked.answered.as_ref().is_none_or(|(answered, at)| *answered != viewers && now >= at + CREATOR_CHECK_MS);
        let records = due.then(|| crate::creator::answers(signer, session.session, local, &viewers));
        if records.is_some_and(|r| roster::CREATOR_KEYS.iter().zip(&r).all(|(k, v)| session.publish_application(k, v.clone(), now))) {
            tracked.answered = Some((viewers, now));
        }
    }
    let mut creators = BTreeSet::new();
    for m in members {
        let proven = if m.actor == local {
            signer.is_some()
        } else if let Some(a) = session.actors.get(&m.actor) {
            let stamp = (roster::CREATOR_KEYS.map(|k| a.application.get(k).map_or(0, |r| r.seq)), m.steam);
            match tracked.creator_checks.get(&m.actor) {
                Some(&(seen, _, ok)) if seen == stamp => ok,
                // Checked again at most every CREATOR_CHECK_MS, however often its records change.
                Some(&(_, at, ok)) if now < at + CREATOR_CHECK_MS => ok,
                _ => {
                    let records = roster::CREATOR_KEYS.iter().filter_map(|k| a.application.get(*k)).map(|r| r.value.as_slice());
                    // Online safety 10-26: the Steam-verified ID, so never in a Direct session (none there).
                    let ok = m.steam == crate::creator::CREATOR_STEAM_ID
                        && crate::creator::verify(public, records, session.session, m.actor, local, &nonce);
                    tracked.creator_checks.insert(m.actor, (stamp, now, ok));
                    ok
                }
            }
        } else {
            false
        };
        if proven {
            creators.insert(m.actor);
        }
    }
    creators
}

/// Audit 10-28 #14: what the member list is built from (who is here, their `@player` record, how we reach them, who
/// hosts), so it is decoded and filtered again only when that changes, not every frame (50 names in a full lobby).
fn members_key(session: &Session, steam: bool, own: Option<u64>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::hash::DefaultHasher::new();
    (session.session, session.local, session.host_actor(), steam, own).hash(&mut h);
    for (&actor, a) in &session.actors {
        (actor, a.application.get(roster::PLAYER_KEY).map(|r| r.seq), session.endpoint(actor)).hash(&mut h);
    }
    h.finish()
}

/// C3: the Steam ID Steam vouches for, never the self-reported `@player` one: a Steam session's link endpoint (ours
/// from Presence), none in a Direct session (no friend badge or profile there).
pub(crate) fn verified(mut members: Vec<roster::Member>, session: &Session, steam: bool, own: Option<u64>) -> Vec<roster::Member> {
    for m in &mut members {
        // Online safety 10-26: every display, the event log and Recent Players read these names (chat_filter: slurs
        // never show, whoever sends them).
        m.name = crate::chat_filter::filter(&m.name, false);
        m.steam = match (steam, m.actor == session.local) {
            (false, _) => 0,
            (true, true) => own.unwrap_or(0),
            (true, false) => session.endpoint(m.actor).unwrap_or(0),
        };
    }
    members
}

/// A new session's state: the pick's lobby, and a moved session's bans and settings.
fn new_state(carry: roster::SessionState, challenge: Option<roster::ChallengeLobby>, location: Option<String>) -> roster::SessionState {
    roster::SessionState {
        challenge,
        location,
        private: carry.private,
        kicked: carry.kicked,
        no_proposals: carry.no_proposals,
        collisions: carry.collisions,
        world: carry.world,
        cohost_endpoints: carry.cohost_endpoints,
        ..default()
    }
}

/// Q4: the lobby our rich presence offers to "Join Game": none while the session is private (or a moved private
/// lobby waits to turn private).
pub(crate) fn advertised(room: Option<u64>, private: bool) -> Option<u64> {
    room.filter(|_| !private)
}

pub(crate) fn recent_path(asset_root: &Path) -> PathBuf {
    crate::saves::settings_dir(asset_root).join("recent-players.json")  // Not retail (user 10-13)
}

pub(crate) struct OnlineRosterPlugin;
impl Plugin for OnlineRosterPlugin {
    fn build(&self, app: &mut App) {
        let path = recent_path(&app.world().resource::<crate::config::Config>().asset_root);
        let recent = RecentPlayers::load(&path);
        // Multiplayer::receive runs in PreUpdate, so `track` sees this frame's packets.
        app.insert_resource(OnlineRoster { recent, path, ..default() })
            .add_systems(Update, track);
    }
}

pub(crate) fn track(
    mut roster: ResMut<OnlineRoster>,
    mut net: ResMut<Multiplayer>,
    mut presence: ResMut<Presence>,
    mut online: ResMut<crate::online_session::OnlineSession>,
    time: Res<Time<Real>>,
) {
    let now = (time.elapsed_secs_f64() * 1000.) as u64;
    let roster = &mut *roster;
    // A Steam join (friend, invite, browser row) that never became a session.
    if std::mem::take(&mut net.join_failed) {
        roster.lines.push(("ID_ONLINE_PLAYNOW_ERROR_STARTING", String::new()));
        // Not retail (public release review 10-22): the retail line alone never says why (version, full, Steam).
        let why = std::mem::take(&mut net.join_error);
        if !why.is_empty() {
            // The relay's own status lines start "ERROR " (multiplayer::relay_down); players read the reason.
            roster.lines.push(("{0:s}", why.trim_start_matches("ERROR ").to_owned()));
        }
    }
    // Audit 10-24: the Steam relay stopped mid-session (Steam closed, the lobby lost); multiplayer left it.
    if std::mem::take(&mut net.steam_lost) {
        roster.lines.push(("ID_ONLINE_DISCONNECT_FROM_EA_SERVER", String::new()));
    }
    let Some(session) = net.session() else {
        if roster.key.take().is_some() {
            roster.members.clear();
            roster.members_key = None;
            roster.state = default();
            roster.generation += 1;
        }
        crate::creator::set_verified(BTreeSet::new());
        if roster.connect.take().is_some() {
            presence.order(Order::SetConnect(None));
        }
        return;
    };
    if let Some(ids) = end_lines(session, net.is_steam()) {
        net.leave();
        roster.lines.extend(ids.iter().map(|&id| (id, String::new())));
        return;
    }
    let key = (session.session, session.local);
    let host = session.is_host();
    let connected = session.connected();
    let own = presence.me.as_ref().map(|(id, _)| *id);
    // Audit 10-28 #14: decoded and filtered again only when the players change, else last frame's list (put back below).
    let built = members_key(session, net.is_steam(), own);
    let fresh = roster.members_key != Some(built);
    let members = if fresh {
        roster.members_key = Some(built);
        verified(roster::members(session), session, net.is_steam(), own)
    } else {
        std::mem::take(&mut roster.members)
    };
    let version = session
        .host_actor()
        .and_then(|h| Some((h, session.actors.get(&h)?.application.get(roster::SESSION_KEY)?.seq)));
    if roster.key != Some(key) {
        roster.key = Some(key);
        roster.tracker.reset(now);
        roster.published = None;
        roster.published_state = None;
        roster.inherited = None;
        // The Online Challenge Map pick that made this session (a guest's is replaced by the host's `@session`),
        // and a moved session's bans and settings.
        let carry = online.carry.take().unwrap_or_default();
        net.seed_bans(&carry);
        roster.private_at = carry.private.then_some(now + PRIVATE_AFTER_MS);
        roster.state = new_state(carry, online.lobby.take(), online.location.take());
        roster.met.clear();
        // A new session gets a new challenge: nothing answered before passes in it.
        roster.nonce = None;
        roster.answered = None;
        roster.creator_checks.clear();
    }
    // The state is private from the start (new_state); this turns the Steam lobby private, again a second later when
    // the order could not be sent; not once the host made it public meanwhile, nor handed on (review 10-24: Make Host
    // within the 45 s kept ours, which hid our Join Game for the rest of the session).
    if !host {
        roster.private_at = None;
    } else if roster.private_at.is_some_and(|at| now >= at) {
        roster.private_at = (roster.state.private && net.is_steam() && !net.set_private(true)).then_some(now + 1000);
    }
    roster.local_actor = key.1;
    // Decoded once per new record (host actor, seq), not every frame.
    if roster.inherited.as_ref().map(|(v, _)| *v) != version {
        let state = net.session().and_then(roster::session_state);
        if let Some(state) = &state {
            // Every client, so a promoted host already holds the earlier bans.
            net.seed_bans(state);
        }
        roster.inherited = version.map(|v| (v, state));
    }
    let inherited = roster.inherited.as_ref().and_then(|(_, s)| s.as_ref());
    // Never adopt from the record while hosting: ours is authoritative and may hold an
    // unpublished kick. A promoted host keeps the copy it last saw from the old host.
    if let (false, Some(state)) = (host, inherited) {
        if roster.state != *state {
            roster.state = state.clone();
        }
    }
    if roster.published.as_ref() != Some(&presence.me) {
        let me = Player {
            name: presence.display_name(),
            // 3.10: a Direct session never learns our Steam ID.
            steam: own.filter(|_| net.is_steam()).unwrap_or(0),
        };
        if net.publish_player(&me) {
            roster.published = Some(presence.me.clone());
        }
    }
    if host && roster.published_state.as_ref() != Some(&roster.state) && net.publish_session(&roster.state) {
        roster.published_state = Some(roster.state.clone());
    }
    // Online safety 10-26 (review 10-24 R6): Block was our own side only, so a player we blocked could follow us into
    // each lobby we host. Hosting, we kick them (which bans their Steam ID for the session, as the Player List's Kick).
    if host {
        for m in members.iter().filter(|m| m.actor != key.1 && crate::player_list::blocked(m.steam)) {
            net.kick(&mut roster.state, m.actor);
        }
    }
    let state = if host { Some(&roster.state) } else { inherited };
    let events = if connected {
        roster.tracker.update(&members, state, now)
    } else {
        // Joining, or a migration waiting for the new host's ROSTER (I1).
        roster.tracker.pause(now);
        Vec::new()
    };
    let (clock, public) = (net.now_ms(), crate::creator::public_key());
    let keys = (crate::creator::key(), &public[..]);
    let creators = net.session_mut().map_or_else(BTreeSet::new, |s| creator_tags(roster, s, &members, own, clock, keys));
    if crate::creator::set_verified(creators) {
        roster.generation += 1;
    }
    // Everyone met (not only "joined" lines: members present when we joined count too).
    let mut changed = false;
    for m in members.iter().filter(|m| m.actor != key.1) {
        if roster.met.insert(m.actor) {
            changed |= roster.recent.add(&m.name, m.steam);
        }
    }
    if changed {
        if let Err(e) = roster.recent.save(&roster.path) {
            warn!("Recent players: {e}");
        }
    }
    let under_way = roster.state.game.as_ref().is_some_and(|g| !g.results);
    roster.lines.extend(events.iter().filter_map(|e| line(e, under_way)));
    if changed || (fresh && roster.members != members) {
        roster.generation += 1;
    }
    roster.members = members;
    let room = advertised(net.room_id(), roster.state.private || roster.private_at.is_some());
    if room != roster.connect {
        let connect = room.map(|lobby| skate_net::presence::connect_string(lobby, &net.map_name));
        presence.order(Order::SetConnect(connect));
        roster.connect = room;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skate_net::roster::RosterEvent;
    /// Audit 10-28 #14: the member list was decoded and filtered again every frame (50 names in a full lobby). It is
    /// rebuilt when its key changes: someone joins, leaves or renames.
    #[test]
    fn the_member_list_is_rebuilt_only_when_its_players_change() {
        let info = skate_net::lobby::Info { id: 10, map: 1, rig: 2, physics: 3, appearance: 4 };
        let mut s = Session::new(7, info, None);
        let player = |name: &str| roster::Player { name: name.into(), steam: 0 }.encode();
        assert!(s.publish_application(roster::PLAYER_KEY, player("ann"), 0));
        let key = members_key(&s, true, None);
        assert_eq!(members_key(&s, true, None), key, "nothing changed");
        assert!(s.publish_application(roster::PLAYER_KEY, player("bea"), 1));
        assert_ne!(members_key(&s, true, None), key, "renamed");
        assert_ne!(members_key(&s, false, None), members_key(&s, true, None), "Steam IDs shown or not");
    }
    /// Audit 10-24: a moved (or Create Private) session's state said public until its Steam lobby turned private 45 s
    /// in; a host quitting before then handed a public session on. It is private from the start.
    #[test]
    fn a_private_carry_starts_the_new_session_private() {
        let mut carry = roster::SessionState { private: true, no_proposals: true, collisions: true, ..default() };
        carry.record_kick(5, "Griefer", 77);
        let state = new_state(carry.clone(), None, None);
        assert!(state.private && state.no_proposals && state.collisions && state.kicked == carry.kicked, "a move keeps them");
        assert!(!new_state(default(), None, None).private && !new_state(default(), None, None).collisions);
    }
    /// User 10-29: a move to a big map took only some players: the host's new lobby went friends only (unlisted by
    /// Steam) 45 s in, while the others still loaded or searched. It stays findable past the guests' whole search.
    #[test]
    fn a_moved_lobby_stays_findable_for_the_whole_search() {
        assert!(PRIVATE_AFTER_MS as f32 / 1000. > crate::online_session::FOLLOW_FOR);
    }
    #[test]
    fn events_map_to_retail_lines() {
        let line = |e: RosterEvent, under_way| line(&e, under_way);
        let some = |id, arg: &str| Some((id, arg.to_string()));
        assert_eq!(line(RosterEvent::Joined("A".into()), false), some("ID_ONLINE_EVENT_JOIN_SKATER", "A"));
        assert_eq!(line(RosterEvent::Joined("A".into()), true), some("ID_ONLINE_EVENT_JOIN_OBSERVER", "A"), "I1: mid-match");
        assert_eq!(line(RosterEvent::Left("A".into()), false), some("ID_ONLINE_EVENT_LEAVE", "A"));
        assert_eq!(line(RosterEvent::NewHost("A".into()), false), some("ID_ONLINE_EVENT_NEWHOST", "A"));
        assert_eq!(line(RosterEvent::AdminTransfer("A".into()), false), some("ID_ONLINE_EVENT_ADMIN_TRANSFER", "A"), "item 30: Make Host");
        assert_eq!(line(RosterEvent::HostLeft, false), some("ID_ONLINE_EVENT_HOSTLEFT", ""));
        // Item 30: ID_ONLINE_EVENT_ADMIN_PLAYER_KICKED is not in TU3; retail logs no line for a kick.
        assert_eq!(line(RosterEvent::Kicked("A".into()), false), None);
        assert_eq!(line(RosterEvent::Privacy { by: "H".into(), private: true }, false), some("ID_ONLINE_EVENT_PRIVATE_SESSION", "H"));
        assert_eq!(line(RosterEvent::Privacy { by: "H".into(), private: false }, false), some("ID_ONLINE_EVENT_PUBLIC_SESSION", "H"));
    }
    /// I2: a full lobby, a host that quits and a timed-out host end a Direct session with retail lines;
    /// in a Steam lobby a lost host is a migration, not the end.
    #[test]
    fn dead_sessions_end_with_retail_lines() {
        use skate_net::lobby::{Info, REJECT, Session};
        let info = |id| Info { id, map: 1, rig: 2, physics: 3, appearance: 4 };
        // Host endpoint 1, client endpoint 2.
        let pump = |host: &mut Session, client: &mut Session, now: &mut u64| {
            for _ in 0..10 {
                *now += 50;
                for p in host.service(*now) {
                    client.receive(1, &p.data, *now);
                }
                for p in client.service(*now) {
                    host.receive(2, &p.data, *now);
                }
            }
        };
        let (mut host, mut client) = (Session::new(7, info(10), None), Session::new(7, info(20), Some(1)));
        assert_eq!(end_lines(&client, false), None, "joining");
        let mut now = 0;
        pump(&mut host, &mut client, &mut now);
        assert!(client.connected());
        assert_eq!(end_lines(&client, false), None, "connected");
        // The host quits: GOODBYE.
        for p in host.goodbye() {
            client.receive(1, &p.data, now);
        }
        assert_eq!(end_lines(&client, false), Some(&["ID_ONLINE_EVENT_HOSTLEFT", "ID_ONLINE_GAME_LEFT"][..]));
        assert_eq!(end_lines(&client, true), None, "Steam elects a new host");
        // The host goes silent instead.
        let (mut host, mut client) = (Session::new(7, info(10), None), Session::new(7, info(21), Some(1)));
        let mut now = 0;
        pump(&mut host, &mut client, &mut now);
        client.service(now + skate_net::lobby::LINK_TIMEOUT_MS + 1_000);
        assert_eq!(end_lines(&client, false), Some(&["ID_ONLINE_EVENT_HOSTLEFT", "ID_ONLINE_GAME_LEFT"][..]));
        // A host that never answered may still be starting: keep waiting.
        let mut lonely = Session::new(7, info(22), Some(1));
        lonely.service(skate_net::lobby::LINK_TIMEOUT_MS + 1_000);
        assert_eq!(end_lines(&lonely, false), None);
        // The lobby is full (REJECT 3), in Direct and Steam alike.
        let mut full = Session::new(7, info(23), Some(1));
        let mut reject = skate_net::packed::header(7, 10, REJECT, 0);
        reject.push(3);
        full.receive(1, &reject, 0);
        assert_eq!(end_lines(&full, true), Some(&["ID_JOIN_ONLINE_GAME_FAILED_POPUP_MESSAGE"][..]));
        // Kicked wins over everything else.
        let mut kicked = Session::new(7, info(24), Some(1));
        let mut reject = skate_net::packed::header(7, 10, REJECT, 0);
        reject.push(skate_net::lobby::REJECT_KICKED);
        kicked.receive(1, &reject, 0);
        assert_eq!(end_lines(&kicked, true), Some(&["ID_ONLINE_KICKED_BY_GAME_HOST"][..]));
    }
    /// C3: a stranger claiming a friend's Steam ID in `@player` gets their real link's ID (Steam) or none (Direct).
    #[test]
    fn steam_ids_come_from_the_link_not_the_claim() {
        use skate_net::lobby::{Info, Session};
        const FRIEND: u64 = 76561198000000001;
        let info = |id| Info { id, map: 1, rig: 2, physics: 3, appearance: 4 };
        // Host endpoint 1, the stranger's endpoint 2.
        let (mut host, mut stranger) = (Session::new(7, info(10), None), Session::new(7, info(20), Some(1)));
        let claim = Player { name: "Friend".into(), steam: FRIEND };
        let mut now = 0;
        for _ in 0..20 {
            now += 50;
            stranger.publish_application(roster::PLAYER_KEY, claim.encode(), now);
            for p in host.service(now) {
                stranger.receive(1, &p.data, now);
            }
            for p in stranger.service(now) {
                host.receive(2, &p.data, now);
            }
        }
        let claimed = roster::members(&host).into_iter().find(|m| m.actor == 20).expect("the stranger's @player arrived");
        assert_eq!(claimed.steam, FRIEND, "the record claims the friend");
        let steam = verified(roster::members(&host), &host, true, Some(5));
        assert_eq!(steam.iter().find(|m| m.actor == 20).unwrap().steam, 2, "Steam: the real link");
        assert!(verified(roster::members(&host), &host, false, Some(5)).iter().all(|m| m.steam == 0), "Direct: no Steam IDs");
    }
    /// Online safety 10-26: a name gets the chat's slur filter, so no display (tags, Player List, event log, results,
    /// Recent Players) shows one.
    #[test]
    fn names_never_show_a_slur() {
        use skate_net::lobby::{Info, Session};
        // ROT13, as chat_filter's list: the source never spells it out.
        let slur: String = "ergneq".chars().map(|c| (b'a' + (c as u8 - b'a' + 13) % 26) as char).collect();
        let session = Session::new(7, Info { id: 10, map: 1, rig: 2, physics: 3, appearance: 4 }, None);
        let members = vec![roster::Member { actor: 20, name: format!("ann {slur}"), steam: 0, host: false }];
        let name = &verified(members, &session, false, None)[0].name;
        assert!(name.starts_with("ann ") && !name.to_lowercase().contains(&slur), "{name}");
    }
    /// User 10-25, the creator tag over the real wire: the creator's game (a test key standing in for the real one, on
    /// the creator's Steam account) answers the host's challenge and the host shows the tag; the key on another Steam
    /// account, or the creator's account without the key, shows nothing.
    #[test]
    fn only_the_creators_own_answers_show_the_creator_tag() {
        use ring::signature::{Ed25519KeyPair, KeyPair};
        use skate_net::lobby::{Info, Session};
        let rng = ring::rand::SystemRandom::new();
        let key = Ed25519KeyPair::from_pkcs8(Ed25519KeyPair::generate_pkcs8(&rng).unwrap().as_ref()).unwrap();
        let public = key.public_key().as_ref().to_vec();
        let info = |id| Info { id, map: 1, rig: 2, physics: 3, appearance: 4 };
        // `endpoint`: the guest's Steam ID as the host's link has it (Steam vouches for it).
        let run = |endpoint: u64, own: u64, signer: Option<&Ed25519KeyPair>| {
            let (mut host, mut guest) = (Session::new(7, info(10), None), Session::new(7, info(20), Some(1)));
            let (mut at_host, mut at_guest) = (OnlineRoster::default(), OnlineRoster::default());
            let (mut now, mut seen) = (0, (BTreeSet::new(), BTreeSet::new()));
            for (s, name) in [(&mut host, "Host"), (&mut guest, "Dev")] {
                s.publish_application(roster::PLAYER_KEY, Player { name: name.into(), steam: 0 }.encode(), 0);
            }
            for _ in 0..60 {
                now += 50;
                for p in host.service(now) {
                    guest.receive(1, &p.data, now);
                }
                for p in guest.service(now) {
                    host.receive(endpoint, &p.data, now);
                }
                let members = verified(roster::members(&guest), &guest, true, Some(own));
                seen.1 = creator_tags(&mut at_guest, &mut guest, &members, Some(own), now, (signer, &public));
                let members = verified(roster::members(&host), &host, true, Some(5));
                seen.0 = creator_tags(&mut at_host, &mut host, &members, Some(5), now, (None, &public));
            }
            seen
        };
        let creator = crate::creator::CREATOR_STEAM_ID;
        assert_eq!(run(creator, creator, Some(&key)), ([20].into(), [20].into()), "the host sees the tag, the creator its own");
        assert_eq!(run(2, creator, Some(&key)).0, BTreeSet::new(), "a valid answer from another Steam account");
        assert_eq!(run(creator, creator, None), (BTreeSet::new(), BTreeSet::new()), "the account without the key");
        assert_eq!(run(2, 2, Some(&key)), (BTreeSet::new(), BTreeSet::new()), "the key never signs for another account");
    }
    #[test]
    fn private_sessions_advertise_no_join() {
        assert_eq!(advertised(Some(9), false), Some(9));
        assert_eq!(advertised(Some(9), true), None);
        assert_eq!(advertised(None, false), None);
    }
    #[test]
    fn recent_players_dedupe_cap_and_survive_corruption() {
        let mut r = RecentPlayers::default();
        for i in 0..60 { r.add(&format!("p{i}"), 1000 + i); }
        assert_eq!(r.players.len(), 50);
        assert_eq!(r.players[0].name, "p59", "newest first");
        assert!(r.add("renamed", 1059), "same SteamID moves to the front with the new name");
        assert_eq!((r.players.len(), r.players[0].name.as_str()), (50, "renamed"));
        r.add("Direct", 0); r.add("Direct", 0);
        assert_eq!(r.players.iter().filter(|p| p.name == "Direct").count(), 1);
        let dir = std::env::temp_dir().join(format!("recent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("recent-players.json");
        r.save(&path).unwrap();
        assert_eq!(RecentPlayers::load(&path).players, r.players);
        std::fs::write(&path, b"{not json").unwrap();
        assert_eq!(RecentPlayers::load(&path).players, r.players, "a damaged list comes back from its last good copy");
        std::fs::write(&path, b"{not json").unwrap();
        std::fs::remove_file(crate::saves::backup(&path)).unwrap();
        assert!(RecentPlayers::load(&path).players.is_empty(), "and with none, it starts empty");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(recent_path(std::path::Path::new("C:/x/assets")), std::path::Path::new("C:/x/settings/recent-players.json"));
    }
}
