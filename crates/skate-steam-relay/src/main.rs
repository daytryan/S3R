//! Optional Steam byte relay. The game never links this SDK or calls Steam Input.
mod directory;
mod presence;
mod skaters;
use std::{
    collections::BTreeMap,
    net::{SocketAddr, UdpSocket},
    time::{Duration, Instant},
};
use steamworks::{
    Client, SteamId,
    networking_types::{NetworkingIdentity, SendFlags},
};
const CHANNEL: u32 = 38;
/// Steam networking settings for every session, set before any opens (a failure ends the relay: never a session
/// without them).
/// - ICE off (online safety 10-26): never a direct route, so every packet goes through Valve's relays and no player
///   learns another's IP address. Left at -1 Steam follows each player's own setting, and its "Default" shares the
///   address when the relay ping is high.
/// - No throughput ceiling of ours, without forcing a minimum send rate: Steam still controls the connection, the
///   game bounds in-flight data.
const NETWORKING: [(steamworks::sys::ESteamNetworkingConfigValue, i32); 2] = [
    (steamworks::sys::ESteamNetworkingConfigValue::k_ESteamNetworkingConfig_P2P_Transport_ICE_Enable, 0),
    (steamworks::sys::ESteamNetworkingConfigValue::k_ESteamNetworkingConfig_SendRateMax, i32::MAX),
];
fn main() {
    if let Err(e) = run() {
        eprintln!("Steam relay: {e}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--presence") {
        return presence::run(&args[1..]);
    }
    if args.len() != 5 || args[4].is_empty() || args[4].len() > 64 {
        return Err(
            "Expected parent-address, peer Steam ID (0 to host), session, IPC cookie, game build".into(),
        );
    }
    let parent: SocketAddr = args[0].parse().map_err(|_| "Invalid parent")?;
    if !parent.ip().is_loopback() {
        return Err("IPC must be loopback".into());
    }
    let mut peer = args[1].parse::<u64>().map_err(|_| "Invalid peer")?;
    let mut session = args[2].parse::<u64>().map_err(|_| "Invalid session")?;
    let cookie = &args[3];
    let socket = UdpSocket::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    socket.connect(parent).map_err(|e| e.to_string())?;
    skate_net::socket::configure(&socket).map_err(|e| e.to_string())?;
    let status = |s: &str| {
        let _ = socket.send(format!("SK8RELAY {cookie} {s}").as_bytes());
    };
    status("Starting Steam");
    let client = match Client::init_app(480) {
        Ok(c) => c,
        Err(e) => {
            status(&format!("ERROR Open Steam and sign in, then retry. {e}"));
            return Err(e.to_string());
        }
    };
    let own = client.user().steam_id().raw();
    if peer == own {
        status(
            "ERROR Two Steam players need different Steam accounts. Use local testing on one PC.",
        );
        return Err("Same Steam identity".into());
    }
    client.networking_utils().init_relay_network_access();
    // SAFETY: Steam is initialized and owns this interface for `client`'s lifetime.
    unsafe {
        let utils = steamworks::sys::SteamAPI_SteamNetworkingUtils_SteamAPI_v004();
        if utils.is_null()
            || !NETWORKING.iter().all(|&(key, value)| {
                steamworks::sys::SteamAPI_ISteamNetworkingUtils_SetGlobalConfigValueInt32(utils, key, value)
            })
        {
            return Err("Could not configure Steam networking".into());
        }
    }
    let messages = client.networking_messages();
    // Peer-to-peer movement: every lobby member opens a session to every other one; strangers never (Q3).
    let gate = std::sync::Arc::new(std::sync::Mutex::new(Gate::default()));
    let requests = gate.clone();
    messages.session_request_callback(move |request| {
        let id = request.remote().steam_id().map(|id| id.raw()).unwrap_or(0);
        let mut gate = requests.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(request) = gate.offer(id, request, own, Instant::now()) {
            request.accept();
        }
    });
    // The Steam lobby size and game admission own the player limit; this adapter only routes bytes.
    let mut peers: BTreeMap<u64, (Instant, i64)> = BTreeMap::new();
    if peer != 0 {
        peers.insert(peer, (Instant::now(), 0));
    }
    let cookie_value = u64::from_str_radix(cookie, 16).map_err(|_| "Invalid cookie")?;
    let ping = format!("PING {cookie}");
    // perf 10-18: built once, not per datagram.
    let (quit, leave, cmd) = (format!("QUIT {cookie}"), format!("LEAVE {cookie}"), format!("CMD {cookie} "));
    let details = format!("DETAILS {cookie} ");
    let coalesce = skate_net::knob!("SKATE_RELAY_COALESCE", 1) != 0;
    // This loop's datagrams for Steam, (target, bytes in `arena`, [`no_delay`]), sent together after it (see `flags`).
    let (mut batch, mut arena): (Vec<(u64, std::ops::Range<usize>, bool)>, Vec<u8>) = (vec![], vec![]);
    let mut dropped = 0u64;
    let mut failures = 0u64;
    let mut heartbeat = Instant::now() - Duration::from_secs(2);
    let mut parent_seen = Instant::now();
    // Audit 10-28 #5: the game that started us. When it ends (a crash too) we leave within a second; we waited for 15 s
    // of silence, holding its lobby seat (a crashed host held the whole session up that long).
    let game = parent_pid().and_then(watch);
    let mut buffer = [0u8; 1500];
    let mut directory = directory::Directory::new(args[4].clone());
    let mut route = None;
    'running: loop {
        client.run_callbacks();
        for response in directory.poll(&client) {
            status(&format!(
                "EVENT {}",
                skate_net::directory::encode(&response)
            ));
        }
        if let Some(lobby) = directory.lobby {
            let next = (lobby.raw(), directory.owner);
            if directory.owner != 0 && route != Some(next) {
                session = lobby.raw();
                peer = if directory.owner == own {
                    0
                } else {
                    directory.owner
                };
                peers.clear();
                if peer != 0 {
                    peers.insert(peer, (Instant::now(), 0));
                }
                route = Some(next);
            }
        }
        {
            let mut gate = gate.lock().unwrap_or_else(|e| e.into_inner());
            // perf 10-18: the member set is copied only when it changed, not every loop.
            match directory.lobby {
                Some(_) => gate.allow(&directory.members),
                None => gate.allow(&(peer != 0).then_some(peer).into_iter().collect()),
            }
            for request in gate.due(own, Instant::now()) {
                request.accept();
            }
        }
        if heartbeat.elapsed() > Duration::from_secs(1) {
            // Audit 10-24: Steam gone (closed, restarting) or our lobby lost (a long drop, the PC asleep): say so and
            // end, so the game leaves the session (a fresh relay starts next time) instead of sitting in it alone.
            // SAFETY: a flat Steam API query, valid for `client`'s lifetime.
            if !unsafe { steamworks::sys::SteamAPI_IsSteamRunning() } {
                status(skate_net::directory::STEAM_CLOSED);
                break 'running;
            }
            if directory.lost(Instant::now()) {
                status("ERROR Lost the Steam lobby. Check the connection, then retry.");
                break 'running;
            }
            if game.is_some_and(ended) {
                break 'running;
            }
            status(&format!("READY {own}"));
            let mut up = 0f32;
            let mut down = 0f32;
            let mut queue = 0i64;
            let mut loss = 0f32;
            let mut ping_ms = 0;
            // NetworkingMessages expires idle sessions; bound our routing/metrics cache too.
            peers.retain(|id, (seen, _)| seen.elapsed() <= Duration::from_secs(10) || *id == peer);
            for (&id, (_, queued)) in &mut peers {
                let (_, _, realtime) = messages.get_session_connection_info(
                    &NetworkingIdentity::new_steam_id(SteamId::from_raw(id)),
                );
                if let Some(r) = realtime {
                    up += r.out_bytes_per_sec();
                    down += r.in_bytes_per_sec();
                    ping_ms = ping_ms.max(r.ping());
                    // steamworks-rs calls this queued_send_bytes, but the SDK field is m_usecQueueTime.
                    *queued = r.queued_send_bytes().max(0) / 1000;
                    queue = queue.max(*queued);
                    for quality in [r.connection_quality_local(), r.connection_quality_remote()] {
                        if quality >= 0. {
                            loss = loss.max((1. - quality) * 100.);
                        }
                    }
                }
            }
            status(&format!(
                "STATS {} Steam up {:.1} / down {:.1} kB/s | ping {} ms | loss {:.1}% | queue {} ms | drops {} | errors {}",
                u8::from(queue > 20),
                up / 1000.,
                down / 1000.,
                ping_ms,
                loss,
                queue,
                dropped,
                failures
            ));
            heartbeat = Instant::now();
        }
        // Audit 10-28 #6: QUIT (true) or LEAVE (false) waits for this loop's sends: the game's GOODBYEs come just before
        // it, and went nowhere (the others saw a frozen skater for 5 s).
        let mut ending = None;
        for _ in 0..4096 {
            match socket.recv(&mut buffer) {
                Ok(n) => {
                    if buffer[..n] == *ping.as_bytes() {
                        parent_seen = Instant::now();
                        continue;
                    }
                    if buffer[..n] == *quit.as_bytes() {
                        ending = Some(true);
                        break;
                    }
                    // perf 10-18: the game left its session but keeps this relay (and Steam) for the next one: no
                    // process restart and Steam init per leave or map change.
                    if buffer[..n] == *leave.as_bytes() {
                        parent_seen = Instant::now();
                        ending = Some(false);
                        break;
                    }
                    if let Ok(text) = std::str::from_utf8(&buffer[..n]) {
                        // User 10-25: the host's server browser listing, unanswered.
                        if let Some(json) = text.strip_prefix(&details) {
                            if let Some(d) = skate_net::directory::details(json) {
                                parent_seen = Instant::now();
                                directory.details(&client.matchmaking(), &d);
                            }
                            continue;
                        }
                        if let Some(json) = text.strip_prefix(&cmd) {
                            if let Some(request) = skate_net::directory::request(json) {
                                parent_seen = Instant::now();
                                if let Some(response) = directory.command(request, &client) {
                                    status(&format!(
                                        "EVENT {}",
                                        skate_net::directory::encode(&response)
                                    ));
                                }
                            }
                            continue;
                        }
                    }
                    if n < 16 || u64::from_le_bytes(buffer[..8].try_into().unwrap()) != cookie_value
                    {
                        continue;
                    }
                    let target = u64::from_le_bytes(buffer[8..16].try_into().unwrap());
                    let data = &buffer[16..n];
                    let Some((s, _, kind, _)) = skate_net::packed::envelope(data) else {
                        continue;
                    };
                    // In a lobby, any member (peer-to-peer movement); without one, only the host.
                    if s != session
                        || target == 0
                        || (directory.lobby.is_none() && peer != 0 && peer != target)
                        || (directory.lobby.is_some() && !directory.members.contains(&target))
                    {
                        continue;
                    }
                    parent_seen = Instant::now();
                    let state = no_delay(kind);
                    let start = arena.len();
                    arena.extend_from_slice(data);
                    batch.push((target, start..arena.len(), state));
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                // One bad datagram (WSAEMSGSIZE, an ICMP reset) ends this batch, never the relay.
                Err(e) => {
                    eprintln!("relay: game socket: {e}");
                    break;
                }
            }
        }
        // Do not discard packets based on the once-per-second
        // queue sample. That creates long artificial loss bursts.
        // Steam handles backpressure; state remains NoDelay, and
        // bulk retries/ACK windows remain owned by skate-net.
        for ((target, range, _), flags) in batch.iter().zip(flags(&batch, coalesce)) {
            if let Err(e) = messages.send_message_to_user(
                NetworkingIdentity::new_steam_id(SteamId::from_raw(*target)),
                flags,
                &arena[range.clone()],
                CHANNEL,
            ) {
                if matches!(e, steamworks::SteamError::Ignored) {
                    dropped += 1;
                } else {
                    failures += 1;
                }
            }
        }
        batch.clear();
        arena.clear();
        if let Some(quitting) = ending {
            // Steam sends from its own thread: a moment for those last datagrams before the lobby goes (the game waits
            // 100 ms for us to end).
            std::thread::sleep(Duration::from_millis(40));
            directory.leave(&client.matchmaking());
            if quitting {
                break 'running;
            }
            directory.cancel();
            (peer, session, route) = (0, 0, None);
            peers.clear();
        }
        for message in messages.receive_messages_on_channel(CHANNEL, 4096) {
            let id = message
                .identity_peer()
                .steam_id()
                .map(|id| id.raw())
                .unwrap_or(0);
            let Some((s, _, kind, _)) = skate_net::packed::envelope(message.data()) else {
                // A peer on another protocol version: say so once (the game shows its own notice).
                static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                if let Some(v) = skate_net::packed::other_version(message.data()) {
                    if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                        eprintln!("relay: dropping datagrams from a peer on protocol version {}{}", v[0] as char, v[1] as char);
                    }
                }
                continue;
            };
            if s != session
                || id == 0
                || id == own
                || (directory.lobby.is_none() && peer != 0 && id != peer)
                || (directory.lobby.is_some() && !directory.members.contains(&id))
            {
                continue;
            }
            if !peers.contains_key(&id) {
                // A lobby member may open with movement; outside a lobby only a HELLO starts a route.
                if (directory.lobby.is_none() && kind != skate_net::lobby::HELLO)
                    || peers.len() >= skate_net::lobby::MAX_PLAYERS + 2
                {
                    continue;
                }
                peers.insert(id, (Instant::now(), 0));
            }
            peers.get_mut(&id).unwrap().0 = Instant::now();
            let mut packet = cookie_value.to_le_bytes().to_vec();
            packet.extend(id.to_le_bytes());
            packet.extend(message.data());
            if socket.send(&packet).is_err() {
                failures += 1;
            }
        }
        if parent_seen.elapsed() > Duration::from_secs(15) {
            break;
        }
        // perf 10-18: 1 ms (was 2): up to a millisecond less on every datagram each way.
        std::thread::sleep(Duration::from_millis(1));
    }
    directory.leave(&client.matchmaking());
    Ok(())
}

/// The process that started us (the game).
fn parent_pid() -> Option<u32> {
    use windows_sys::Win32::{Foundation::CloseHandle, System::Diagnostics::ToolHelp::*};
    let own = std::process::id();
    // SAFETY: a snapshot we close, read into a zeroed entry with its size set.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap.is_null() || snap as isize == -1 {
            return None;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut parent = None;
        let mut more = Process32FirstW(snap, &mut entry) != 0;
        while more && parent.is_none() {
            if entry.th32ProcessID == own {
                parent = Some(entry.th32ParentProcessID);
            }
            more = Process32NextW(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
        parent
    }
}
/// A handle to process `pid` that tells when it ends ([`ended`]); held for the relay's life, so the pid is never reused
/// under it.
fn watch(pid: u32) -> Option<windows_sys::Win32::Foundation::HANDLE> {
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE};
    // SAFETY: a plain open; a null handle is checked.
    let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    (!process.is_null()).then_some(process)
}
fn ended(process: windows_sys::Win32::Foundation::HANDLE) -> bool {
    // SAFETY: a valid process handle from `watch`; a zero wait never blocks.
    unsafe { windows_sys::Win32::System::Threading::WaitForSingleObject(process, 0) == windows_sys::Win32::Foundation::WAIT_OBJECT_0 }
}

/// Movement and voice go UNRELIABLE_NO_DELAY when last for their target ([`flags`]): late, both are useless.
fn no_delay(kind: u8) -> bool {
    matches!(kind, skate_net::packed::BODY | skate_net::packed::POSE | skate_net::voice::VOICE)
}

/// Each message's send flags, in order (`state`: [`no_delay`]). perf 10-18: within one batch every message but a
/// target's last goes Nagled (plain UNRELIABLE) and that last one with the usual flag, whose no-Nagle flushes the queue:
/// one Steam packet per target per batch, not one per message. All stay unreliable, as before.
/// SKATE_RELAY_COALESCE=0: each with its usual flag.
fn flags<R>(batch: &[(u64, R, bool)], coalesce: bool) -> Vec<SendFlags> {
    let mut last = BTreeMap::new();
    for (i, (target, _, _)) in batch.iter().enumerate() {
        last.insert(*target, i);
    }
    batch
        .iter()
        .enumerate()
        .map(|(i, (target, _, state))| {
            let class = match (coalesce && last[target] != i, state) {
                (true, _) => SendFlags::UNRELIABLE,
                (false, true) => SendFlags::UNRELIABLE_NO_DELAY,
                (false, false) => SendFlags::UNRELIABLE_NO_NAGLE,
            };
            // Ledger #54: a session Steam marked broken (a long drop) fails every SendMessageToUser with ConnectFailed
            // until Steam frees it minutes later; with this flag the send opens a new session instead.
            class | SendFlags::AUTO_RESTART_BROKEN_SESSION
        })
        .collect()
}

/// Q3: who may open a Steam P2P session to us. Only lobby members (or the host we are joining), a few times each
/// per window, so a stranger can neither talk to us nor use up the allowance real members need.
struct Gate<R> {
    /// Lobby members, or the expected host before joining; empty: nobody.
    allowed: std::collections::BTreeSet<u64>,
    window: Option<Instant>,
    total: usize,
    per: BTreeMap<u64, u8>,
    /// Requests from players not in `allowed` yet: Steam's member list can lag a new member's first packet.
    /// Dropping a steamworks SessionRequest rejects it.
    parked: Vec<(Instant, u64, R)>,
}
impl<R> Default for Gate<R> {
    fn default() -> Self {
        Self { allowed: Default::default(), window: None, total: 0, per: BTreeMap::new(), parked: vec![] }
    }
}
const GATE_WINDOW: Duration = Duration::from_secs(10);
const PER_USER: u8 = 4;
const PARK_FOR: Duration = Duration::from_secs(3);
const PARK_MAX: usize = 16;
impl<R> Gate<R> {
    fn allow(&mut self, allowed: &std::collections::BTreeSet<u64>) {
        if self.allowed != *allowed {
            self.allowed = allowed.clone();
        }
    }
    fn admit(&mut self, id: u64, own: u64, now: Instant) -> bool {
        if self.window.is_none_or(|w| now.duration_since(w) > GATE_WINDOW) {
            (self.window, self.total) = (Some(now), 0);
            self.per.clear();
        }
        if id == 0 || id == own || !self.allowed.contains(&id) || self.total >= 2 * skate_net::lobby::MAX_PLAYERS {
            return false;
        }
        let n = self.per.entry(id).or_default();
        if *n >= PER_USER {
            return false;
        }
        *n += 1;
        self.total += 1;
        true
    }
    /// A new session request: Some to accept now; else parked for a while (an unknown player) or dropped.
    fn offer(&mut self, id: u64, request: R, own: u64, now: Instant) -> Option<R> {
        if self.admit(id, own, now) {
            return Some(request);
        }
        if id != 0 && id != own && !self.allowed.contains(&id) && self.parked.len() < PARK_MAX {
            self.parked.push((now, id, request));
        }
        None
    }
    /// Parked requests whose player is now allowed; the expired ones are dropped.
    fn due(&mut self, own: u64, now: Instant) -> Vec<R> {
        let mut out = vec![];
        for (at, id, request) in std::mem::take(&mut self.parked) {
            if self.allowed.contains(&id) {
                out.extend(self.admit(id, own, now).then_some(request));
            } else if now.duration_since(at) < PARK_FOR {
                self.parked.push((at, id, request));
            }
        }
        out
    }
}

#[cfg(test)]
mod gate_tests {
    use super::*;
    #[test]
    fn only_members_open_sessions_and_each_only_a_few_times() {
        let (own, member, stranger) = (1, 2, 3);
        let now = Instant::now();
        let mut gate = Gate::<u32>::default();
        assert_eq!(gate.offer(member, 0, own, now), None, "no lobby yet: nobody");
        gate.allow(&[own, member].into());
        assert_eq!(gate.due(own, now), [0], "a member whose lobby join was still arriving gets in once listed");
        assert_eq!(gate.offer(own, 1, own, now), None, "ourselves");
        // A stranger flooding requests never gets in and never uses up a member's allowance.
        for i in 0..1000 {
            assert_eq!(gate.offer(stranger, i, own, now), None);
        }
        assert!(gate.parked.len() <= PARK_MAX);
        assert_eq!(gate.due(own, now + PARK_FOR), Vec::<u32>::new(), "expired strangers are dropped");
        assert!(gate.parked.is_empty());
        // A member: a few per window, then refused until the window passes.
        let accepted = (0..10).filter(|&i| gate.offer(member, i, own, now).is_some()).count();
        assert_eq!(accepted, PER_USER as usize - 1, "one was spent by the parked request");
        assert_eq!(gate.offer(member, 0, own, now + GATE_WINDOW + Duration::from_millis(1)), Some(0));
        // Before a lobby, only the host we are joining.
        gate.allow(&[9].into());
        assert_eq!(gate.offer(member, 0, own, now + GATE_WINDOW * 3), None);
        assert_eq!(gate.offer(9, 0, own, now + GATE_WINDOW * 3), Some(0));
    }
    /// Online safety 10-26: Steam never makes a direct route between players, so nobody learns another's IP address.
    #[test]
    fn steam_never_connects_players_directly() {
        use steamworks::sys::ESteamNetworkingConfigValue::k_ESteamNetworkingConfig_P2P_Transport_ICE_Enable as ICE;
        assert_eq!(NETWORKING.iter().find(|(key, _)| *key == ICE).map(|&(_, value)| value), Some(0));
    }
}

#[cfg(test)]
mod game_tests {
    use super::*;
    /// Audit 10-28 #5: the relay watches the process that started it and sees it end.
    #[test]
    fn a_watched_process_is_seen_to_end() {
        assert!(parent_pid().is_some_and(|pid| pid != std::process::id()), "our parent");
        let mut child = std::process::Command::new("cmd").args(["/C", "ping -n 2 127.0.0.1 >nul"]).spawn().unwrap();
        let process = watch(child.id()).expect("a handle");
        assert!(!ended(process), "still running");
        child.wait().unwrap();
        assert!(ended(process), "ended");
    }
}

#[cfg(test)]
mod flag_tests {
    use super::*;
    /// perf 10-18: a target's last message in a batch flushes the ones before it; the class stays unreliable.
    #[test]
    fn a_batch_goes_out_as_one_flush_per_target() {
        let batch = [(1, (), true), (2, (), false), (1, (), false), (1, (), true), (2, (), true)];
        let r = SendFlags::AUTO_RESTART_BROKEN_SESSION;
        let u = SendFlags::UNRELIABLE | r;
        let no_delay = SendFlags::UNRELIABLE_NO_DELAY | r;
        assert_eq!(flags(&batch, true), [u, u, u, no_delay, no_delay]);
        let old = [no_delay, SendFlags::UNRELIABLE_NO_NAGLE | r, SendFlags::UNRELIABLE_NO_NAGLE | r, no_delay, no_delay];
        assert_eq!(flags(&batch, false), old);
        assert!(flags(&batch, true).iter().all(|f| !f.contains(SendFlags::RELIABLE)));
    }
    /// Ledger #54 (user 10-26): a Steam session that failed (a long drop) refused every send with ConnectFailed until
    /// Steam freed it minutes later; with AUTO_RESTART_BROKEN_SESSION the next send opens a new one.
    #[test]
    fn every_send_restarts_a_broken_session() {
        let batch = [(1u64, 0..1, true), (1, 1..2, false), (2, 2..3, true)];
        for coalesce in [true, false] {
            assert!(flags(&batch, coalesce).iter().all(|f| f.contains(SendFlags::AUTO_RESTART_BROKEN_SESSION)));
        }
    }
    /// Voice (user 10-26) is sent like movement; every other kind keeps no-Nagle.
    #[test]
    fn voice_is_sent_like_movement() {
        let fast: Vec<u8> = (0..=u8::MAX).filter(|&kind| no_delay(kind)).collect();
        assert_eq!(fast, [skate_net::packed::BODY, skate_net::packed::POSE, skate_net::voice::VOICE]);
    }
}
