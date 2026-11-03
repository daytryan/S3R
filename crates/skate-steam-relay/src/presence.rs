//! `skate-steam-relay --presence <parent-addr> <cookie> [<game build>]`: long-lived Steam presence helper, and
//! `--presence --dump`: one-shot diagnostic. The game never links Steamworks; this process reports
//! the persona name, friends, join requests and the players online on its build, and executes rich presence,
//! invites and `steam://` pages.
//!
//! A second process may `init_app(480)` alongside a session relay: the Steam client accepts both,
//! and only this process sets rich presence. A join callback may also reach the session relay, which
//! ignores it.
use skate_net::{
    directory::{self, encode},
    presence::{self, Friend, MAX_FRIENDS, Order, Report},
};
use std::{
    net::{SocketAddr, UdpSocket},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use crate::skaters::{self, Cmd, Event, Item, Skaters};
use std::sync::mpsc::{Sender, channel};
use steamworks::{
    AppIDs, AppId, Client, DistanceFilter, FileType, FriendFlags, FriendState, GameLobbyJoinRequested,
    GameRichPresenceJoinRequested, LobbyId, LobbyKey, LobbyType, Matchmaking, PublishedFileId,
    PublishedFileVisibility, SteamId, StringFilter, StringFilterKind, UGC, UGCType, UserList, UserListOrder,
};

/// How often the players online on this build are counted (the Online Challenge Map shows the count). A knob.
/// User 10-29: 30 s (was 3 minutes, and players joining a session showed up minutes late).
const COUNT_EVERY: Duration = Duration::from_secs(30);
/// The most lobbies one count asks Steam for.
const COUNT_MAX: u64 = 1000;
const TICK: Duration = Duration::from_secs(5);
const SILENCE: Duration = Duration::from_secs(10);
/// How long a Steam lobby join waits for the lobby's map label. A knob.
const LOBBY_DATA_WAIT: Duration = Duration::from_secs(3);

/// Audit 10-24: Steam's own Join Game and its lobby invites name only the lobby, so the game joined on whatever map it
/// was on. Each waits for the lobby's data (asked for at once) and goes with its map label, so the game travels first;
/// one whose data never comes goes without, as before.
#[derive(Default)]
struct LobbyJoins(Vec<(u64, Instant)>);
impl LobbyJoins {
    fn ask(&mut self, lobby: u64, now: Instant) {
        self.0.push((lobby, now));
    }
    /// The connect strings due at `now`; `label` reads a lobby's map label (None: not in yet).
    fn due(&mut self, now: Instant, label: impl Fn(u64) -> Option<String>) -> Vec<String> {
        let mut out = Vec::new();
        self.0.retain(|&(lobby, since)| {
            let map = label(lobby);
            let done = map.is_some() || now.duration_since(since) >= LOBBY_DATA_WAIT;
            if done {
                out.push(presence::connect_string(lobby, &skate_net::directory::label(map.as_deref().unwrap_or(""))));
            }
            !done
        });
        out
    }
}

/// Online first, then case-insensitive name; only our own connect strings survive; capped.
fn friend_list(rows: impl Iterator<Item = (u64, String, bool, Option<String>)>) -> Vec<Friend> {
    let mut friends: Vec<Friend> = rows
        .map(|(id, name, online, connect)| Friend {
            id,
            // As friend_pages: one over-long name would make the game refuse the whole list.
            name: skate_net::roster::truncate(&name.chars().filter(|c| !c.is_control()).collect::<String>(), presence::MAX_NAME).to_owned(),
            online,
            connect: connect.filter(|c| presence::parse_connect(c).is_some()),
        })
        .collect();
    friends.sort_by_cached_key(|f| (!f.online, f.name.to_lowercase()));
    friends.truncate(MAX_FRIENDS);
    friends
}

/// Only our own `steam://` pages, in a charset no shell or URL handler treats specially (3.9).
fn openable(url: &str) -> bool {
    url.starts_with("steam://")
        && url.len() <= 256
        && url.bytes().all(|b| b.is_ascii_alphanumeric() || b"/:._-".contains(&b))
}

fn friend_rows(client: &Client) -> Vec<Friend> {
    friend_list(
        client
            .friends()
            .get_friends(FriendFlags::IMMEDIATE)
            .into_iter()
            .map(|f| {
                let connect = f
                    .game_played()
                    .filter(|g| g.game.app_id() == AppId(480))
                    .and_then(|_| f.rich_presence("connect"));
                (
                    f.id().raw(),
                    f.name(),
                    f.state() != FriendState::Offline,
                    connect,
                )
            }),
    )
}

/// Every friend (offline too) for Friends' Skaters, unsorted: the game sorts.
fn all_friends(client: &Client) -> Vec<(u64, String)> {
    client
        .friends()
        .get_friends(FriendFlags::IMMEDIATE)
        .into_iter()
        .map(|f| (f.id().raw(), f.name()))
        .collect()
}

/// Runs one Friends' Skaters step on Steam; the answer comes back on `events`.
fn execute(ugc: &UGC, cmd: Cmd, events: &Sender<Event>) {
    let app = AppId(480);
    let tx = events.clone();
    match cmd {
        Cmd::Query(user) => {
            let query = ugc.query_user(
                SteamId::from_raw(user).account_id(),
                UserList::Published,
                UGCType::Items,
                UserListOrder::CreationOrderAsc,
                AppIDs::Both { creator: app, consumer: app },
                1,
            );
            match query {
                Ok(q) => q.require_tag(skaters::TAG).set_return_metadata(true).fetch(move |r| {
                    let items = r
                        .map(|r| {
                            (0..r.returned_results())
                                .filter_map(|i| {
                                    let d = r.get(i)?;
                                    Some(Item {
                                        id: d.published_file_id.0,
                                        owner: d.owner.raw(),
                                        tagged: d.tags.iter().any(|t| t == skaters::TAG),
                                        metadata: r.get_metadata(i).and_then(|m| String::from_utf8(m).ok()),
                                    })
                                })
                                .collect()
                        })
                        .map_err(|e| e.to_string());
                    let _ = tx.send(Event::Queried(user, items));
                }),
                Err(e) => {
                    let _ = tx.send(Event::Queried(user, Err(e.to_string())));
                }
            }
        }
        Cmd::Create => ugc.create_item(app, FileType::Community, move |r| {
            let _ = tx.send(Event::Created(r.map(|(id, agreement)| (id.0, agreement)).map_err(|e| e.to_string())));
        }),
        Cmd::Update { item, metadata } => {
            // Fixed ASCII title; the metadata is checked (<= MAX_SKATER, no NUL) so the crate's setters cannot panic.
            let _watch = ugc
                .start_item_update(app, PublishedFileId(item))
                .title("S3R skater")
                .metadata(&metadata)
                .visibility(PublishedFileVisibility::FriendsOnly)
                .tags(vec![skaters::TAG], false)
                .submit(None, move |r| {
                    let _ = tx.send(Event::Updated(r.map(|(_, agreement)| agreement).map_err(|e| e.to_string())));
                });
        }
        Cmd::Delete(item) => ugc.delete_item(PublishedFileId(item), |_| {}),
    }
}

/// ShellExecuteW, not `cmd /C start`: cmd would parse & | ^ < > % in the URL.
fn open(url: &str) {
    if !openable(url) {
        return;
    }
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (verb, file) = (wide("open"), wide(url));
    // SAFETY: both strings are NUL-terminated and outlive the call; the other pointers may be null.
    unsafe {
        windows_sys::Win32::UI::Shell::ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
        );
    }
}

pub fn run(args: &[String]) -> Result<(), String> {
    match args {
        [flag] if flag == "--dump" => dump(),
        [parent, cookie] => serve(parent, cookie, ""),
        [parent, cookie, build] if build.len() <= 64 => serve(parent, cookie, build),
        _ => Err("Expected --dump, or parent-address, IPC cookie and game build".into()),
    }
}

fn dump() -> Result<(), String> {
    let client = Client::init_app(480).map_err(|e| {
        println!("ERROR {e}");
        e.to_string()
    })?;
    // Persona data for friends arrives asynchronously; give Steam a moment.
    for _ in 0..10 {
        client.run_callbacks();
        std::thread::sleep(Duration::from_millis(100));
    }
    let friends = friend_rows(&client);
    println!(
        "name={} id={} friends={} in_skate3rust={}",
        client.friends().name(),
        client.user().steam_id().raw(),
        friends.len(),
        friends.iter().filter(|f| f.connect.is_some()).count()
    );
    Ok(())
}

/// The players-online count's Steam answers.
enum Count {
    /// Our own count lobby, made (or not).
    Made(Option<LobbyId>),
    Found(Vec<LobbyId>),
}

/// A count lobby's size. Steam's lobby search lists only lobbies that are not full (ISteamMatchmaking
/// RequestLobbyList), so a lobby for one was never found and the count said 0 (user 10-27); nothing of ours joins one.
const COUNT_SLOTS: u32 = 2;

/// We count as online on `build` while this helper runs (signed in): an invisible lobby, tagged with
/// [`directory::PLAYERS`] (its own `sk8game` value, so no browse lists it) and the build. Made once, at sign-in, so a
/// session's lobby joined later is never left for it: Steam allows one regular lobby (a session's) and up to two
/// invisible ones at once (ELobbyType).
fn count_lobby(mm: &Matchmaking, tx: Sender<Count>) {
    mm.create_lobby(LobbyType::Invisible, COUNT_SLOTS, move |made| {
        let _ = tx.send(Count::Made(made.ok()));
    });
}
/// Tags our count lobby (the build first: `sk8game` makes it one of ours); one Steam won't tag is left (false).
fn tag_count_lobby(mm: &Matchmaking, lobby: LobbyId, build: &str) -> bool {
    let tagged = mm.set_lobby_data(lobby, directory::BUILD_KEY, build) && mm.set_lobby_data(lobby, "sk8game", directory::PLAYERS);
    if !tagged {
        mm.leave_lobby(lobby);
    }
    tagged
}
/// The players online: the count lobbies `found` that are `ours` (our build's), but our own (`own`), and us: Steam's
/// docs don't say whether a search lists a lobby the searcher is in.
fn players(found: &[LobbyId], ours: impl Fn(&LobbyId) -> bool, own: Option<LobbyId>) -> u32 {
    found.iter().filter(|l| Some(**l) != own && ours(l)).count() as u32 + 1
}
/// Every count lobby on `build`, worldwide.
fn count_players(mm: &Matchmaking, build: &str, tx: Sender<Count>) {
    for (key, value) in [("sk8game", directory::PLAYERS), (directory::BUILD_KEY, build)] {
        mm.add_request_lobby_list_string_filter(StringFilter(LobbyKey::new(key), value, StringFilterKind::Equal));
    }
    mm.set_request_lobby_list_distance_filter(DistanceFilter::Worldwide);
    mm.set_request_lobby_list_result_count_filter(COUNT_MAX);
    // A failed search says nothing (the last count stays).
    mm.request_lobby_list(move |found| {
        if let Ok(found) = found {
            let _ = tx.send(Count::Found(found));
        }
    });
}

fn serve(parent: &str, cookie: &str, build: &str) -> Result<(), String> {
    let parent: SocketAddr = parent.parse().map_err(|_| "Invalid parent")?;
    if !parent.ip().is_loopback() {
        return Err("IPC must be loopback".into());
    }
    let socket = UdpSocket::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    socket.connect(parent).map_err(|e| e.to_string())?;
    skate_net::socket::configure(&socket).map_err(|e| e.to_string())?;
    let status = |r: &Report| {
        let _ = socket.send(format!("SK8RELAY {cookie} PRESENCE {}", encode(r)).as_bytes());
    };
    let (ping, quit, cmd) = (
        format!("PING {cookie}"),
        format!("QUIT {cookie}"),
        format!("CMD {cookie} "),
    );
    // A PublishSkater order is up to MAX_ORDER bytes.
    let mut buffer = vec![0u8; 65536];
    let mut parent_seen = Instant::now();
    // Drains waiting datagrams: (QUIT seen, orders). Foreign or malformed ones are ignored.
    let mut receive = |seen: &mut Instant| -> (bool, Vec<Order>) {
        let mut orders = Vec::new();
        while let Ok(n) = socket.recv(&mut buffer) {
            let Ok(text) = std::str::from_utf8(&buffer[..n]) else {
                continue;
            };
            if text == quit {
                return (true, orders);
            } else if text == ping {
                *seen = Instant::now();
            } else if let Some(json) = text.strip_prefix(&cmd) {
                *seen = Instant::now();
                orders.extend(presence::order(json));
            }
        }
        (false, orders)
    };

    let client = match Client::init_app(480) {
        Ok(c) => c,
        Err(_) => {
            // Stay alive (the game would respawn us) and keep telling it why lists are empty.
            let mut sent = Instant::now() - TICK;
            while parent_seen.elapsed() <= SILENCE {
                if receive(&mut parent_seen).0 {
                    break;
                }
                if sent.elapsed() >= TICK {
                    status(&Report::Error("Open Steam and sign in".into()));
                    sent = Instant::now();
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            return Ok(());
        }
    };
    let friends = client.friends();
    let events = Arc::new(Mutex::new(Vec::<Report>::new()));
    let a = events.clone();
    let _join_rich = client.register_callback(move |r: GameRichPresenceJoinRequested| {
        a.lock()
            .unwrap()
            .push(Report::JoinRequested { connect: r.connect });
    });
    let lobbies = Arc::new(Mutex::new(Vec::<u64>::new()));
    let b = lobbies.clone();
    let _join_lobby = client.register_callback(move |r: GameLobbyJoinRequested| {
        b.lock().unwrap().push(r.lobby_steam_id.raw());
    });
    let mut lobby_joins = LobbyJoins::default();
    let mm = client.matchmaking();

    let id = client.user().steam_id().raw();
    let mut name = friends.name();
    status(&Report::Me {
        id,
        name: name.clone(),
    });
    let mut last_friends: Option<Vec<Friend>> = None;
    let mut connect: Option<String> = None;
    let mut tick = Instant::now() - TICK;
    let ugc = client.ugc();
    let mut skaters = Skaters::new(id);
    let (steam_tx, steam_rx) = channel::<Event>();
    // The players online on our build: counted once our count lobby is up (us included), then every COUNT_EVERY. An
    // older game names no build and counts nobody.
    let (count_tx, count_rx) = channel::<Count>();
    if !build.is_empty() {
        count_lobby(&mm, count_tx.clone());
    }
    let mut count_at: Option<Instant> = None;
    // Our count lobby, left when we stop (signed out, the game gone) so we stop counting at once.
    let mut counted: Option<LobbyId> = None;
    while parent_seen.elapsed() <= SILENCE {
        client.run_callbacks();
        for report in std::mem::take(&mut *events.lock().unwrap()) {
            status(&report);
        }
        let now = Instant::now();
        while let Ok(count) = count_rx.try_recv() {
            match count {
                Count::Made(lobby) => {
                    counted = lobby.filter(|&l| tag_count_lobby(&mm, l, build));
                    count_at = Some(now);
                    count_players(&mm, build, count_tx.clone());
                }
                // Only lobbies really tagged as the filters asked.
                Count::Found(found) => {
                    let ours = |l: &LobbyId| {
                        mm.lobby_data(*l, "sk8game").as_deref() == Some(directory::PLAYERS)
                            && mm.lobby_data(*l, directory::BUILD_KEY).as_deref() == Some(build)
                    };
                    status(&Report::Players(players(&found, ours, counted)));
                }
            }
        }
        if count_at.is_some_and(|at| now.duration_since(at) >= COUNT_EVERY) {
            count_at = Some(now);
            count_players(&mm, build, count_tx.clone());
        }
        for lobby in std::mem::take(&mut *lobbies.lock().unwrap()) {
            // SAFETY: a flat Steam API call, valid for `client`'s lifetime; the interface pointer is checked.
            unsafe {
                let mm = steamworks::sys::SteamAPI_SteamMatchmaking_v009();
                if !mm.is_null() {
                    steamworks::sys::SteamAPI_ISteamMatchmaking_RequestLobbyData(mm, lobby);
                }
            }
            lobby_joins.ask(lobby, now);
        }
        for connect in lobby_joins.due(now, |l| mm.lobby_data(steamworks::LobbyId::from_raw(l), "label")) {
            status(&Report::JoinRequested { connect });
        }
        while let Ok(event) = steam_rx.try_recv() {
            skaters.event(event, now);
        }
        skaters.step(now);
        for cmd in std::mem::take(&mut skaters.cmds) {
            execute(&ugc, cmd, &steam_tx);
        }
        for report in std::mem::take(&mut skaters.reports) {
            status(&report);
        }
        if tick.elapsed() >= TICK {
            tick = Instant::now();
            // SAFETY: a flat Steam API query, valid for `client`'s lifetime.
            if !unsafe { steamworks::sys::SteamAPI_IsSteamRunning() } {
                // Steam closed or is restarting: exit, so the game greys Steam actions and starts a
                // fresh helper (rate-limited) that picks Steam up again.
                status(&Report::Error("Steam closed".into()));
                break;
            }
            let now = friends.name();
            if now != name && !now.is_empty() {
                name = now;
                status(&Report::Me {
                    id,
                    name: name.clone(),
                });
            }
            let rows = friend_rows(&client);
            if last_friends.as_ref() != Some(&rows) {
                status(&Report::Friends(rows.clone()));
                last_friends = Some(rows);
            }
        }
        let (quit, orders) = receive(&mut parent_seen);
        for order in orders {
            match order {
                Order::SetConnect(Some(c)) => {
                    friends.set_rich_presence("connect", Some(&c));
                    connect = Some(c);
                }
                Order::SetConnect(None) => {
                    friends.clear_rich_presence();
                    connect = None;
                }
                // Audit 10-24: the session's own connect first (a private one advertises none).
                Order::Invite { friend, connect: ours } => match ours.as_ref().or(connect.as_ref()) {
                    Some(c) => friends
                        .get_friend(SteamId::from_raw(friend))
                        .invite_user_to_game(c),
                    None => status(&Report::Error("No Steam session to invite to".into())),
                },
                Order::Open(page) => open(&presence::url(&page)),
                Order::SkaterList => {
                    let rows = all_friends(&client);
                    for page in skaters::friend_pages(rows.iter().cloned()) {
                        status(&page);
                    }
                    let ids: Vec<u64> = rows.iter().map(|(id, _)| *id).collect();
                    skaters.list(&ids, Instant::now());
                }
                Order::FetchSkater { friend } => skaters.fetch(friend, Instant::now()),
                Order::PublishSkater(metadata) => skaters.publish(metadata),
            }
        }
        if quit {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if let Some(lobby) = counted {
        mm.leave_lobby(lobby);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn friend_rows_keep_only_our_connect_and_cap() {
        let rows = (0..250u64).map(|i| {
            (
                i + 1,
                format!("f{i}"),
                i % 2 == 0,
                if i == 4 {
                    Some("sk8:55:University".to_string())
                } else if i == 6 {
                    Some("+connect x".into())
                } else {
                    None
                },
            )
        });
        let friends = friend_list(rows);
        assert_eq!(friends.len(), skate_net::presence::MAX_FRIENDS);
        assert_eq!(
            friends.iter().filter(|f| f.connect.is_some()).count(),
            1,
            "foreign connect strings dropped"
        );
        assert!(friends[0].online, "online first");
        let long = friend_list([(1, format!("a\u{202e}\u{7}{}", "é".repeat(100)), true, None)].into_iter());
        assert!(long[0].name.len() <= skate_net::presence::MAX_NAME && !long[0].name.contains(['\u{7}']));
        assert!(skate_net::presence::report(&encode(&Report::Friends(long))).is_some(), "the list still goes through");
    }
    #[test]
    fn a_steam_lobby_join_waits_for_the_lobby_s_map() {
        let (mut joins, now) = (LobbyJoins::default(), Instant::now());
        joins.ask(5, now);
        joins.ask(6, now);
        assert!(joins.due(now, |_| None).is_empty(), "its data not in yet");
        let label = |l| (l == 5).then(|| "University\u{202e}".to_string());
        assert_eq!(joins.due(now, label), ["sk8:5:University"], "with its map, cleaned");
        assert_eq!(joins.due(now + LOBBY_DATA_WAIT, |_| None), ["sk8:6:"], "no answer: as before");
        assert!(joins.due(now + LOBBY_DATA_WAIT * 2, |_| None).is_empty());
    }
    /// Players Online (user 10-27: it said 0 with them online): us once, listed by Steam or not, and our build's others.
    #[test]
    fn the_players_online_count_includes_us_once() {
        let [a, b, own] = [1, 2, 3].map(LobbyId::from_raw);
        let any = |_: &LobbyId| true;
        assert_eq!(players(&[], any, Some(own)), 1, "alone, our own lobby not listed");
        assert_eq!(players(&[a, own, b], any, Some(own)), 3, "our own listed: once");
        assert_eq!(players(&[a, b], |l: &LobbyId| *l == a, None), 2, "another build's lobby left out; no count lobby: still us");
    }
    #[test]
    fn only_our_steam_pages_open() {
        use skate_net::presence::{Page, url};
        for page in [Page::Profile(u64::MAX), Page::Chat(7), Page::AddFriend(7), Page::WorkshopAgreement] {
            assert!(openable(&url(&page)), "{page:?}");
        }
        for bad in ["steam://x&calc", "steam://x|calc", "steam://a b", "steam://%PATH%", "steam://^x", "https://x", "file:///C:/x", ""] {
            assert!(!openable(bad), "{bad}");
        }
    }
}
