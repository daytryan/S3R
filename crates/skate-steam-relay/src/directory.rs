use skate_net::directory::{self, Command, Details, Event, Request, Response, Row, Tags};
use std::{
    collections::BTreeSet,
    ffi::{CStr, CString},
    sync::mpsc::{self, Receiver, Sender},
    time::{Duration, Instant},
};
use steamworks::sys::{
    ELobbyType, SteamAPI_ISteamMatchmaking_SetLobbyOwner, SteamAPI_ISteamMatchmaking_SetLobbyType,
    SteamAPI_ISteamNetworkingUtils_ConvertPingLocationToString, SteamAPI_ISteamNetworkingUtils_EstimatePingTimeFromLocalHost,
    SteamAPI_ISteamNetworkingUtils_GetLocalPingLocation, SteamAPI_ISteamNetworkingUtils_ParsePingLocationString,
    SteamAPI_SteamMatchmaking_v009, SteamAPI_SteamNetworkingUtils_SteamAPI_v004, SteamNetworkPingLocation_t,
    k_cchMaxSteamNetworkingPingLocationString,
};
use steamworks::{
    Client, DistanceFilter, LobbyId, LobbyKey, LobbyType, Matchmaking, StringFilter,
    StringFilterKind,
};
enum ResultEvent {
    /// (request, page, map, physics, the browse's session tag, whether it was untagged (the server browser's),
    /// lobbies)
    List(u64, usize, u64, u64, String, bool, Result<Vec<LobbyId>, String>),
    Host(u64, u64, u64, String, Tags, Result<LobbyId, String>),
    Join(u64, u64, u64, Result<LobbyId, String>),
}
pub struct Directory {
    pub lobby: Option<LobbyId>,
    pub owner: u64,
    pub members: BTreeSet<u64>,
    tx: Sender<ResultEvent>,
    rx: Receiver<ResultEvent>,
    request: Option<u64>,
    pending: bool,
    began: Instant,
    cached: Option<Response>,
    last_owner: Instant,
    /// This game's build (`directory::BUILD_KEY`), from the game's command line.
    build: String,
    /// In a lobby whose members have not listed us since then (Steam dropped us: a long drop, the PC asleep).
    alone_since: Option<Instant>,
    /// The ping location we last published to the lobby we own (`directory::PING_KEY`).
    ping_sent: Option<(LobbyId, String)>,
}
/// How long Steam may not list us in our own lobby before we count it lost (its member list lags a moment). A knob.
const LOST_AFTER: Duration = Duration::from_secs(5);
impl Directory {
    pub fn new(build: String) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            build,
            lobby: None,
            owner: 0,
            members: BTreeSet::new(),
            tx,
            rx,
            request: None,
            pending: false,
            began: Instant::now(),
            cached: None,
            last_owner: Instant::now() - Duration::from_secs(1),
            alone_since: None,
            ping_sent: None,
        }
    }
    pub fn leave(&mut self, mm: &Matchmaking) {
        if let Some(l) = self.lobby.take() {
            mm.leave_lobby(l);
        }
        self.owner = 0;
        self.members.clear();
        self.alone_since = None;
    }
    /// Steam listed us among our lobby's members (`member`) at `now`, or not.
    fn seen(&mut self, member: bool, now: Instant) {
        if member {
            self.alone_since = None;
        } else {
            self.alone_since.get_or_insert(now);
        }
    }
    /// Audit 10-24: Steam no longer has us in our lobby, so nobody's traffic reaches us any more (`main` drops what
    /// isn't a member's): the game is told, instead of sitting in the session alone.
    pub fn lost(&self, now: Instant) -> bool {
        self.lobby.is_some() && self.alone_since.is_some_and(|t| now.duration_since(t) > LOST_AFTER)
    }
    /// perf 10-18 (LEAVE): forget the request in flight; its late answer is dropped (a lobby it joined is left, `poll`).
    pub fn cancel(&mut self) {
        self.pending = false;
    }
    fn finish(&mut self, id: u64, event: Event) -> Response {
        let r = Response { request: id, event };
        self.pending = false;
        self.cached = Some(r.clone());
        r
    }
    /// Synchronous answer to Privacy/Owner; the real owner change arrives later from `poll`.
    fn owner_answer(&mut self, id: u64, client: &Client, ok: bool, error: &str) -> Response {
        let own = client.user().steam_id().raw();
        let event = match self.lobby {
            // The validator rejects a zero owner, so fall back to ourselves.
            Some(l) if ok => Event::Owner {
                lobby: l.raw(),
                owner: if self.owner == 0 { own } else { self.owner },
                own,
            },
            _ => Event::Error(error.into()),
        };
        self.finish(id, event)
    }
    pub fn command(&mut self, r: Request, client: &Client) -> Option<Response> {
        if self.request == Some(r.id) {
            return self.cached.clone();
        }
        // Audit 10-24: a newer request replaces one in flight (a cancelled Quick Match's browse refused the next one):
        // its late answer is dropped and a lobby it made left (`poll`). The game resends a request until answered, so
        // an older one's resend arriving late is ignored.
        if self.request.is_some_and(|current| r.id < current) {
            return None;
        }
        self.request = Some(r.id);
        self.pending = true;
        self.cached = None;
        self.began = Instant::now();
        let mm = client.matchmaking();
        let tx = self.tx.clone();
        let id = r.id;
        match r.command {
            Command::Browse {
                page,
                map,
                physics,
                label,
                tags,
            } => {
                let (wanted, untagged) = (tags.session.clone(), tags.is_empty());
                let equal = |key: &str, value: &str| {
                    mm.add_request_lobby_list_string_filter(StringFilter(
                        LobbyKey::new(key),
                        value,
                        StringFilterKind::Equal,
                    ));
                };
                // Only lobbies of this exact build: other Spacewar games, older protocols and other builds of this
                // game stay out of Steam's bounded result set and never show as joinable.
                equal("sk8game", directory::NAMESPACE);
                equal(directory::BUILD_KEY, &self.build);
                if !tags.is_empty() {
                    // Online Challenge Map match: the same map and every non-empty tag.
                    equal("label", &clean(&label));
                    for (key, value) in tags.keys() {
                        if !value.is_empty() {
                            equal(key, value);
                        }
                    }
                    // Audit 10-24: only lobbies with room. Quick Match reads the first page, and full ones (the oldest)
                    // filled it: "No match" while open lobbies existed.
                    mm.set_request_lobby_list_slots_available_filter(1);
                }
                mm.set_request_lobby_list_distance_filter(DistanceFilter::Worldwide);
                mm.set_request_lobby_list_result_count_filter(directory::BROWSE_MAX as u64);
                mm.request_lobby_list(move |result| {
                    let _ = tx.send(ResultEvent::List(
                        id,
                        page,
                        map,
                        physics,
                        wanted,
                        untagged,
                        result.map_err(|e| e.to_string()),
                    ));
                });
            }
            Command::Host {
                map,
                physics,
                label,
                tags,
            } => {
                self.leave(&mm);
                mm.create_lobby(LobbyType::Public, tags.capacity() as u32, move |result| {
                    let _ = tx.send(ResultEvent::Host(
                        id,
                        map,
                        physics,
                        label,
                        tags,
                        result.map_err(|e| e.to_string()),
                    ));
                });
            }
            Command::Join {
                lobby,
                map,
                physics,
            } => {
                self.leave(&mm);
                mm.join_lobby(LobbyId::from_raw(lobby), move |result| {
                    let _ = tx.send(ResultEvent::Join(
                        id,
                        map,
                        physics,
                        result
                            .map_err(|_| "Lobby unavailable, full, or Steam could not join".into()),
                    ));
                });
            }
            Command::Privacy { private } => {
                let kind = if private {
                    ELobbyType::k_ELobbyTypeFriendsOnly
                } else {
                    ELobbyType::k_ELobbyTypePublic
                };
                // SAFETY: Steam is initialized for `client`'s lifetime; the interface pointer is checked.
                let ok = self.lobby.is_some_and(|l| unsafe {
                    let mm = SteamAPI_SteamMatchmaking_v009();
                    !mm.is_null() && SteamAPI_ISteamMatchmaking_SetLobbyType(mm, l.raw(), kind)
                });
                // Audit 10-24: and hidden from every browse while private (a Create Private lobby was made hidden), so
                // one turned public is found by browses again.
                let ok = ok
                    && self.lobby.is_some_and(|l| {
                        mm.set_lobby_data(l, directory::HIDDEN_KEY, if private { "1" } else { "0" })
                    });
                return Some(self.owner_answer(
                    id,
                    client,
                    ok,
                    "Could not change the lobby's privacy (host only)",
                ));
            }
            Command::Owner { member } => {
                // SAFETY: as above.
                let ok = self.lobby.is_some_and(|l| unsafe {
                    let mm = SteamAPI_SteamMatchmaking_v009();
                    !mm.is_null() && SteamAPI_ISteamMatchmaking_SetLobbyOwner(mm, l.raw(), member)
                });
                return Some(self.owner_answer(
                    id,
                    client,
                    ok,
                    "Could not hand the lobby to that player",
                ));
            }
        };
        None
    }
    /// User 10-25: the host's server browser listing (`DETAILS`, unanswered: the game sends it again until it changes).
    /// Steam takes lobby data from the lobby's owner alone and sends only what changed.
    pub fn details(&self, mm: &Matchmaking, d: &Details) {
        if let Some(l) = self.lobby {
            mm.set_lobby_data(l, directory::NAME_KEY, &directory::bounded(&d.name, directory::NAME));
            mm.set_lobby_data(l, directory::DESCRIPTION_KEY, &directory::bounded(&d.description, directory::DESCRIPTION));
            mm.set_lobby_data(l, directory::UNLISTED_KEY, if d.listed { "0" } else { "1" });
        }
    }
    fn owner_event(&mut self, mm: &Matchmaking, own: u64) -> Option<Event> {
        let l = self.lobby?;
        let owner = mm.lobby_owner(l).raw();
        self.members = mm.lobby_members(l).iter().map(|i| i.raw()).collect();
        self.seen(self.members.contains(&own), Instant::now());
        if owner == 0 || !self.members.contains(&own) {
            return None;
        }
        self.owner = owner;
        Some(Event::Owner {
            lobby: l.raw(),
            owner,
            own,
        })
    }
    pub fn poll(&mut self, client: &Client) -> Vec<Response> {
        let mm = client.matchmaking();
        let own = client.user().steam_id().raw();
        let mut output = Vec::new();
        while let Ok(result) = self.rx.try_recv() {
            let id = match &result {
                ResultEvent::List(id, ..)
                | ResultEvent::Host(id, ..)
                | ResultEvent::Join(id, ..) => *id,
            };
            if self.request != Some(id) || !self.pending {
                match result {
                    ResultEvent::Host(_, _, _, _, _, Ok(l)) | ResultEvent::Join(_, _, _, Ok(l)) => {
                        mm.leave_lobby(l)
                    }
                    _ => (),
                };
                continue;
            }
            let event = match result {
                ResultEvent::List(_, page, _map, _physics, wanted, untagged, result) => match result {
                    Err(e) => Event::Error(e),
                    Ok(mut list) => {
                        list.retain(|&l| {
                            mm.lobby_data(l, "sk8game")
                                .is_some_and(|name| directory::is_game_lobby(&name))
                                && mm.lobby_data(l, directory::BUILD_KEY).as_deref() == Some(self.build.as_str())
                                && directory::listed(
                                    mm.lobby_data(l, directory::HIDDEN_KEY).as_deref(),
                                    mm.lobby_data(l, "session").as_deref(),
                                    &wanted,
                                )
                                // A host's Hide from Server Browser (user 10-25): left out of the browser's browse only.
                                && !(untagged && mm.lobby_data(l, directory::UNLISTED_KEY).as_deref() == Some("1"))
                        });
                        list.sort_by_key(LobbyId::raw);
                        let total = list.len();
                        let page = page.min(total.saturating_sub(1) / directory::PAGE_SIZE);
                        let rows = list
                            .into_iter()
                            .skip(page * directory::PAGE_SIZE)
                            .take(directory::PAGE_SIZE)
                            .map(|l| Row {
                                id: l.raw(),
                                map: clean(
                                    &mm.lobby_data(l, "label")
                                        .unwrap_or_else(|| "Unknown map".into()),
                                ),
                                players: mm.lobby_member_count(l).min(skate_net::lobby::MAX_PLAYERS),
                                capacity: mm.lobby_member_limit(l).unwrap_or(10).min(skate_net::lobby::MAX_PLAYERS),
                                compatible: true,
                                ping: mm.lobby_data(l, directory::PING_KEY).and_then(|p| ping_to(&p)),
                                mode: mode(&mm, l),
                                name: directory::bounded(&mm.lobby_data(l, directory::NAME_KEY).unwrap_or_default(), directory::NAME),
                                description: directory::bounded(
                                    &mm.lobby_data(l, directory::DESCRIPTION_KEY).unwrap_or_default(),
                                    directory::DESCRIPTION,
                                ),
                            })
                            .collect();
                        Event::Rows { page, total, rows }
                    }
                },
                ResultEvent::Host(_, map, physics, label, tags, result) => match result {
                    Err(e) => Event::Error(e),
                    Ok(l) => {
                        // Hidden before anything makes it one of ours (sk8game goes last), so no browse sees it unhidden.
                        let okay = (!tags.hidden || mm.set_lobby_data(l, directory::HIDDEN_KEY, "1"))
                            && mm.set_lobby_data(l, "map", &map.to_string())
                            && mm.set_lobby_data(l, "physics", &physics.to_string())
                            && mm.set_lobby_data(l, "label", &clean(&label))
                            && tags
                                .keys()
                                .iter()
                                .all(|(key, value)| value.is_empty() || mm.set_lobby_data(l, key, value))
                            && mm.set_lobby_data(l, directory::BUILD_KEY, &self.build)
                            && mm.set_lobby_data(l, "sk8game", directory::NAMESPACE);
                        if !okay {
                            mm.leave_lobby(l);
                            Event::Error("Steam could not publish lobby details".into())
                        } else {
                            self.lobby = Some(l);
                            self.owner_event(&mm, own)
                                .unwrap_or(Event::Error("Steam lobby owner unavailable".into()))
                        }
                    }
                },
                ResultEvent::Join(_, _map, _physics, result) => match result {
                    Err(e) => Event::Error(e),
                    // An invite or Join Game skips the browse's filter; a different build never gets in.
                    Ok(l) if mm.lobby_data(l, directory::BUILD_KEY).as_deref() != Some(self.build.as_str()) => {
                        mm.leave_lobby(l);
                        Event::Error("That lobby runs a different version of the game. Both players need the same version.".into())
                    }
                    Ok(l) => {
                        self.lobby = Some(l);
                        self.owner_event(&mm, own)
                            .unwrap_or(Event::Error("Steam lobby owner unavailable".into()))
                    }
                },
            };
            output.push(self.finish(id, event));
        }
        if self.pending && self.began.elapsed() > Duration::from_secs(15) {
            output.push(self.finish(
                self.request.unwrap(),
                Event::Error("Steam request timed out; refresh or retry".into()),
            ));
        }
        // Repeated notifications survive loopback datagram loss and track Steam owner election.
        if self.last_owner.elapsed() > Duration::from_millis(250) {
            if let Some(event) = self.owner_event(&mm, own) {
                output.push(Response { request: 0, event });
            }
            if self.owner == own {
                self.publish_ping(&mm);
            }
            self.last_owner = Instant::now();
        }
        output
    }
    /// The lobby we own carries our ping location for the server browser, once Steam has measured it (seconds after
    /// relay access starts) and again when it changes; a migrated lobby's new owner publishes its own.
    fn publish_ping(&mut self, mm: &Matchmaking) {
        let (Some(lobby), Some(here)) = (self.lobby, ping_location()) else { return };
        let sent = (lobby, here);
        if self.ping_sent.as_ref() != Some(&sent) && mm.set_lobby_data(lobby, directory::PING_KEY, &sent.1) {
            self.ping_sent = Some(sent);
        }
    }
}
fn clean(text: &str) -> String {
    directory::label(text)
}
/// The lobby's mode for the browser: its challenge type (an Online Challenge Map filter string id), else its tab. A
/// stranger's lobby data, so anything but a short identifier reads as none.
fn mode(mm: &Matchmaking, l: LobbyId) -> String {
    let id = |v: &String| !v.is_empty() && v.len() <= 64 && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    mm.lobby_data(l, "filter").filter(id).or_else(|| mm.lobby_data(l, "tab").filter(id)).unwrap_or_default()
}
/// This PC's place on Steam's relay network, as Steam's text (none until Steam has measured it).
fn ping_location() -> Option<String> {
    let mut here = SteamNetworkPingLocation_t { m_data: [0; 512] };
    let mut text = [0 as std::os::raw::c_char; k_cchMaxSteamNetworkingPingLocationString as usize];
    // SAFETY: Steam is initialized for the relay's lifetime and the interface pointer is checked; the buffer is the
    // SDK's maximum, which Steam writes a terminated string into.
    unsafe {
        let utils = SteamAPI_SteamNetworkingUtils_SteamAPI_v004();
        if utils.is_null() || SteamAPI_ISteamNetworkingUtils_GetLocalPingLocation(utils, &mut here) < 0. {
            return None;
        }
        SteamAPI_ISteamNetworkingUtils_ConvertPingLocationToString(utils, &here, text.as_mut_ptr(), text.len() as i32);
        CStr::from_ptr(text.as_ptr()).to_str().ok().filter(|s| !s.is_empty()).map(str::to_owned)
    }
}
/// Steam's round-trip estimate in ms from this PC to a host's published ping location (none: Steam can't tell yet, or
/// the text is not one it wrote).
fn ping_to(location: &str) -> Option<u32> {
    if location.len() >= k_cchMaxSteamNetworkingPingLocationString as usize {
        return None;
    }
    let text = CString::new(location).ok()?;
    let mut there = SteamNetworkPingLocation_t { m_data: [0; 512] };
    // SAFETY: as in `ping_location`; Steam only reads `text`, a terminated string.
    unsafe {
        let utils = SteamAPI_SteamNetworkingUtils_SteamAPI_v004();
        if utils.is_null() || !SteamAPI_ISteamNetworkingUtils_ParsePingLocationString(utils, text.as_ptr(), &mut there) {
            return None;
        }
        u32::try_from(SteamAPI_ISteamNetworkingUtils_EstimatePingTimeFromLocalHost(utils, &there)).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_lobby_that_stopped_listing_us_is_lost_after_a_while() {
        let (mut d, now) = (Directory::new("b".into()), Instant::now());
        d.seen(false, now);
        assert!(!d.lost(now + LOST_AFTER * 2), "no lobby: nothing to lose");
        d.lobby = Some(LobbyId::from_raw(5));
        d.seen(true, now);
        d.seen(false, now + Duration::from_secs(1));
        assert!(!d.lost(now + LOST_AFTER), "Steam's member list lags a moment");
        d.seen(false, now + Duration::from_secs(2));
        assert!(d.lost(now + Duration::from_secs(1) + LOST_AFTER * 2), "counted from the first miss");
        d.seen(true, now + Duration::from_secs(9));
        assert!(!d.lost(now + Duration::from_secs(30)), "listed again");
    }
}
