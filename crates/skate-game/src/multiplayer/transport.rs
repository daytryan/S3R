use skate_net::directory::{self, Command as LobbyCommand, Event, Request, Response};
use std::{
    io,
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering::Relaxed},
        mpsc::{Receiver, SyncSender, sync_channel},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// Voice off the game thread (skate-voice): a sender of (endpoint, datagram), and the voice datagrams that come.
pub(crate) type VoiceIo = (Box<dyn FnMut(u64, &[u8]) + Send>, Receiver<(u64, Vec<u8>)>);

/// Datagrams the reader thread holds for `receive()` (the OS buffer held them before); past that they are dropped.
const QUEUE: usize = 16_384;
/// Voice datagrams it holds for the voice thread.
const VOICE_QUEUE: usize = 256;
/// The reader's blocking read gives up this often to see whether its transport is gone.
const READ_TIMEOUT: Duration = Duration::from_millis(50);

/// A transport's socket read on a thread of its own, so voice never waits on a game frame (user 10-26): the voice its
/// `voice_of` picks out goes to the newest voice channel (none open: dropped), the rest to `receive()` through a
/// bounded queue.
struct Reader {
    /// A Mutex only so the transport is Sync (one reader: `receive()`).
    queue: Mutex<Receiver<(SocketAddr, Vec<u8>)>>,
    voice: Arc<Mutex<Option<SyncSender<(u64, Vec<u8>)>>>>,
    stop: Arc<AtomicBool>,
    /// Wakes the blocked read when the transport drops, so the socket is closed at once, not up to 50 ms later.
    wake: (UdpSocket, SocketAddr),
    thread: Option<JoinHandle<()>>,
}
impl Reader {
    /// Reads `socket` (which becomes blocking: only this thread reads it), datagrams of at most `max` bytes.
    fn spawn(socket: &UdpSocket, max: usize, voice_of: impl Fn(&[u8], SocketAddr) -> Option<(u64, Vec<u8>)> + Send + 'static) -> io::Result<Self> {
        let read = socket.try_clone()?;
        read.set_nonblocking(false)?;
        read.set_read_timeout(Some(READ_TIMEOUT))?;
        let local = socket.local_addr()?;
        let wake = (socket.try_clone()?, SocketAddr::new(if local.ip().is_unspecified() { Ipv4Addr::LOCALHOST.into() } else { local.ip() }, local.port()));
        let (to_main, queue) = sync_channel(QUEUE);
        let (voice, stop) = (Arc::<Mutex<Option<SyncSender<_>>>>::default(), Arc::new(AtomicBool::new(false)));
        let thread = std::thread::Builder::new().name("net reader".into()).spawn({
            let (voice, stop) = (voice.clone(), stop.clone());
            move || {
                let mut buffer = vec![0; max + 1];
                while !stop.load(Relaxed) {
                    match read.recv_from(&mut buffer) {
                        Ok((n, from)) if n <= max => match voice_of(&buffer[..n], from) {
                            Some(v) => {
                                if let Some(voice) = &*lock(&voice) {
                                    let _ = voice.try_send(v);
                                }
                            }
                            None => drop(to_main.try_send((from, buffer[..n].to_vec()))),
                        },
                        // 3.7: one bad datagram (too big: WSAEMSGSIZE, or cut short; an ICMP reset) is skipped; a
                        // timeout is the stop check's turn.
                        _ => {}
                    }
                }
            }
        })?;
        Ok(Self { queue: Mutex::new(queue), voice, stop, wake, thread: Some(thread) })
    }
    /// What came for `receive()`: at most 4096 a call, as before.
    fn take(&self) -> Vec<(SocketAddr, Vec<u8>)> {
        lock(&self.queue).try_iter().take(4096).collect()
    }
    /// A new voice channel: from now on the reader feeds this one, not an older one.
    fn voice(&self) -> Receiver<(u64, Vec<u8>)> {
        let (to_voice, voice) = sync_channel(VOICE_QUEUE);
        *lock(&self.voice) = Some(to_voice);
        voice
    }
}
impl Drop for Reader {
    fn drop(&mut self) {
        self.stop.store(true, Relaxed);
        let _ = self.wake.0.send_to(&[], self.wake.1);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
/// The value behind a lock a panicking thread held: each is replaced whole.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}
/// A game datagram of voice's kind.
fn voice(data: &[u8]) -> bool {
    skate_net::packed::envelope(data).is_some_and(|(_, _, kind, _)| kind == skate_net::voice::VOICE)
}

/// Platform adapters move opaque bounded datagrams; session/actors live above this.
pub(super) trait Transport: Send + Sync {
    fn send(&mut self, peer: u64, data: &[u8]) -> io::Result<()>;
    fn receive(&mut self) -> io::Result<Vec<(u64, Vec<u8>)>>;
    fn status(&self) -> String;
    fn command(&mut self, _command: LobbyCommand) -> Result<(), String> {
        Err("Leave local multiplayer before browsing Steam lobbies".into())
    }
    fn events(&mut self) -> Vec<Response> {
        vec![]
    }
    /// User 10-25: the host's server browser listing for the Steam lobby it owns, unanswered (false: not sent).
    fn details(&mut self, _details: &directory::Details) -> bool {
        false
    }
    fn loopback(&self) -> bool {
        false
    }
    /// Endpoints are ip:port (Direct): bans and the per-IP cap key on the IP.
    fn ip_endpoints(&self) -> bool {
        false
    }
    fn metrics(&self) -> String {
        String::new()
    }
    fn congested(&self) -> bool {
        false
    }
    /// The session ended: true if this transport stays for the next one (Steam's relay), false to drop it.
    fn leave(&mut self) -> bool {
        false
    }
    /// Voice for a session: each call opens a new voice channel, and the newest gets the voice. None: no voice here.
    fn voice_io(&mut self) -> Option<VoiceIo> {
        None
    }
}
pub(super) fn endpoint(addr: SocketAddr) -> io::Result<u64> {
    match addr {
        SocketAddr::V4(a) => Ok(((u32::from(*a.ip()) as u64) << 16) | a.port() as u64),
        _ => Err(io::Error::other("Direct testing currently requires IPv4")),
    }
}
/// Where an IPv4 address is: a Direct session only ever sends within its own (online safety P9).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Scope {
    Loopback,
    Lan,
    Internet,
}
/// None: never a peer (multicast, broadcast, unspecified, documentation, "this network").
pub(super) fn scope(ip: Ipv4Addr) -> Option<Scope> {
    if ip.is_loopback() {
        Some(Scope::Loopback)
    } else if ip.is_private() || ip.is_link_local() {
        Some(Scope::Lan)
    } else if ip.is_multicast() || ip.is_broadcast() || ip.is_unspecified() || ip.is_documentation() || ip.octets()[0] == 0 {
        None
    } else {
        Some(Scope::Internet)
    }
}
pub(super) struct Direct {
    socket: UdpSocket,
    /// The session's scope (the host we join, or the address we host on); None: a host on every interface.
    scope: Option<Scope>,
    reader: Reader,
}
impl Direct {
    /// `peer`: the host we join (None when hosting).
    pub fn new(bind: SocketAddr, peer: Option<SocketAddr>) -> io::Result<Self> {
        endpoint(bind)?;
        let scope = match peer {
            Some(p) => {
                let SocketAddr::V4(p) = p else { return Err(io::Error::other("Direct testing currently requires IPv4")) };
                Some(scope(*p.ip()).ok_or_else(|| io::Error::other("Not a player's address"))?)
            }
            None => match bind {
                SocketAddr::V4(b) if !b.ip().is_unspecified() => scope(*b.ip()),
                _ => None,
            },
        };
        let socket = UdpSocket::bind(bind)?;
        skate_net::socket::configure(&socket)?;
        let reader = Reader::spawn(&socket, skate_net::packed::MTU, |data, from| {
            voice(data).then(|| endpoint(from).ok().map(|from| (from, data.to_vec()))).flatten()
        })?;
        Ok(Self { socket, scope, reader })
    }
}
/// P9: a roster can name any address; we never send outside the session's own kind of network.
fn send_within(socket: &UdpSocket, own: Option<Scope>, peer: u64, data: &[u8]) -> io::Result<()> {
    let ip = Ipv4Addr::from((peer >> 16) as u32);
    match scope(ip) {
        Some(s) if own.is_none_or(|own| own == s) => {}
        _ => return Err(io::Error::new(io::ErrorKind::PermissionDenied, "outside the session's network")),
    }
    socket.send_to(data, std::net::SocketAddrV4::new(ip, peer as u16)).map(|_| ())
}
impl Transport for Direct {
    fn loopback(&self) -> bool {
        self.socket.local_addr().is_ok_and(|a| a.ip().is_loopback())
    }
    fn ip_endpoints(&self) -> bool {
        true
    }
    fn send(&mut self, peer: u64, data: &[u8]) -> io::Result<()> {
        send_within(&self.socket, self.scope, peer, data)
    }
    fn receive(&mut self) -> io::Result<Vec<(u64, Vec<u8>)>> {
        // The reader skipped anything over an MTU (3.7) and took the voice.
        Ok(self.reader.take().into_iter().filter_map(|(from, data)| Some((endpoint(from).ok()?, data))).collect())
    }
    fn status(&self) -> String {
        "Direct connection (Steam not required)".into()
    }
    fn voice_io(&mut self) -> Option<VoiceIo> {
        let (socket, own) = (self.socket.try_clone().ok()?, self.scope);
        Some((Box::new(move |peer, data: &[u8]| drop(send_within(&socket, own, peer, data))), self.reader.voice()))
    }
}
/// A game datagram from the relay: (the target endpoint, the payload) when it came from the relay, is 16 to MTU + 16
/// bytes and starts with our cookie.
fn steam_datagram(buf: &[u8], from: SocketAddr, relay: Option<SocketAddr>, cookie: u64) -> Option<(u64, &[u8])> {
    let ours = relay == Some(from) && (16..=skate_net::packed::MTU + 16).contains(&buf.len());
    (ours && u64::from_le_bytes(buf[..8].try_into().unwrap()) == cookie).then(|| (u64::from_le_bytes(buf[8..16].try_into().unwrap()), &buf[16..]))
}
/// A game datagram for the relay: our cookie, the target endpoint, then the datagram.
fn framed(cookie: u64, target: u64, data: &[u8]) -> Vec<u8> {
    let mut packet = Vec::with_capacity(16 + data.len());
    packet.extend(cookie.to_le_bytes());
    packet.extend(target.to_le_bytes());
    packet.extend(data);
    packet
}
pub(super) struct Steam {
    socket: UdpSocket,
    child: Child,
    /// The relay's address once it spoke; `relay` holds the same for the reader thread and the voice sender.
    peer: Option<SocketAddr>,
    relay: Arc<Mutex<Option<SocketAddr>>>,
    reader: Reader,
    cookie: String,
    /// perf 10-18: `cookie` as the number every datagram starts with (parsed once, not per datagram).
    cookie_value: u64,
    /// perf 10-18: the lobby we left with this relay kept (LEAVE), and the one Steam last told us we are in: its
    /// owner notices still in flight never start a session again.
    left: Option<u64>,
    lobby: Option<u64>,
    status: String,
    last_ping: Instant,
    started: Instant,
    metrics: String,
    congested: bool,
    events: Vec<Response>,
    pending: Option<(Request, Instant)>,
    last_command: Instant,
    request_id: u64,
}
impl Steam {
    pub fn new(peer: u64, session: u64) -> Result<Self, String> {
        let socket = UdpSocket::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        skate_net::socket::configure(&socket).map_err(|e| e.to_string())?;
        let dir = std::env::current_exe()
            .map_err(|e| e.to_string())?
            .parent()
            .unwrap()
            .to_path_buf();
        let helper = dir.join("steam-relay/skate-steam-relay.exe");
        if !helper.is_file() || !dir.join("steam-relay/steam_api64.dll").is_file() {
            return Err(
                "Steam relay files missing; solo and direct multiplayer remain available".into(),
            );
        }
        let cookie_value = super::unique();
        let cookie = format!("{cookie_value:016x}");
        let mut command = Command::new(helper);
        command
            .args([
                socket.local_addr().map_err(|e| e.to_string())?.to_string(),
                peer.to_string(),
                session.to_string(),
                cookie,
                super::build_tag(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let child = command
            .spawn()
            .map_err(|e| format!("Could not start Steam relay: {e}"))?;
        Self::with(socket, child, cookie_value).map_err(|e| e.to_string())
    }
    /// The transport for the relay `child` started with `cookie_value`, on `socket` (the relay is stopped if this fails).
    fn with(socket: UdpSocket, mut child: Child, cookie_value: u64) -> io::Result<Self> {
        let relay = Arc::<Mutex<Option<SocketAddr>>>::default();
        let voice_of = {
            let relay = relay.clone();
            move |data: &[u8], from| {
                let (target, payload) = steam_datagram(data, from, *lock(&relay), cookie_value)?;
                voice(payload).then(|| (target, payload.to_vec()))
            }
        };
        // A control answer is up to RESPONSE_MAX after its prefix; a game datagram is at most an MTU and its frame.
        let reader = Reader::spawn(&socket, 2 * directory::RESPONSE_MAX, voice_of).inspect_err(|_| drop(child.kill()))?;
        Ok(Self {
            socket,
            child,
            peer: None,
            relay,
            reader,
            cookie: format!("{cookie_value:016x}"),
            cookie_value,
            left: None,
            lobby: None,
            status: "Checking Steam; open Steam and sign in if needed".into(),
            last_ping: Instant::now(),
            started: Instant::now(),
            metrics: String::new(),
            congested: false,
            events: vec![],
            pending: None,
            last_command: Instant::now() - Duration::from_secs(1),
            request_id: 0,
        })
    }
}
impl Drop for Steam {
    fn drop(&mut self) {
        if let Some(peer) = self.peer {
            let _ = self
                .socket
                .send_to(format!("QUIT {}", self.cookie).as_bytes(), peer);
            for _ in 0..10 {
                if self.child.try_wait().ok().flatten().is_some() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Transport for Steam {
    fn command(&mut self, command: LobbyCommand) -> Result<(), String> {
        // Audit 10-24: a browse still waiting (a cancelled Quick Match's) is replaced by the next browse; nothing else
        // replaces a request, nor drops one (a privacy change would leave a Quick Match to say No match).
        let browse = |c: &LobbyCommand| matches!(c, LobbyCommand::Browse { .. });
        if self.pending.as_ref().is_some_and(|(r, _)| !browse(&r.command) || !browse(&command)) {
            return Err("Steam request in progress; please wait".into());
        }
        self.request_id += 1;
        self.pending = Some((
            Request {
                id: self.request_id,
                command,
            },
            Instant::now(),
        ));
        self.last_command = Instant::now() - Duration::from_secs(1);
        Ok(())
    }
    fn events(&mut self) -> Vec<Response> {
        std::mem::take(&mut self.events)
    }
    fn details(&mut self, details: &directory::Details) -> bool {
        let Some(peer) = self.peer else { return false };
        let text = format!("DETAILS {} {}", self.cookie, directory::encode(details));
        self.socket.send_to(text.as_bytes(), peer).is_ok()
    }
    fn send(&mut self, target: u64, data: &[u8]) -> io::Result<()> {
        let peer = self
            .peer
            .ok_or_else(|| io::Error::new(io::ErrorKind::WouldBlock, "Relay starting"))?;
        self.socket.send_to(&framed(self.cookie_value, target, data), peer).map(|_| ())
    }
    fn receive(&mut self) -> io::Result<Vec<(u64, Vec<u8>)>> {
        let mut packets = vec![];
        let prefix = format!("SK8RELAY {} ", self.cookie);
        // The reader skipped bad datagrams (3.7) and took the voice.
        for (from, data) in self.reader.take() {
            if !from.ip().is_loopback() || self.peer.is_some_and(|p| p != from) {
                continue;
            }
            if let Ok(text) = std::str::from_utf8(&data) {
                if let Some(status) = text.strip_prefix(&prefix) {
                    self.peer = Some(from);
                    *lock(&self.relay) = Some(from);
                    if let Some(json) = status.strip_prefix("EVENT ") {
                        if let Some(response) = directory::response(json) {
                            if (response.request == 0
                                || self
                                    .pending
                                    .as_ref()
                                    .is_some_and(|(r, _)| r.id == response.request))
                                && !stale(&mut self.left, &mut self.lobby, &response)
                            {
                                if response.request != 0 {
                                    self.pending = None;
                                }
                                self.events.push(response);
                            }
                        }
                    } else if let Some(stats) = status.strip_prefix("STATS ") {
                        self.congested = stats.split_whitespace().next() == Some("1");
                        self.metrics = stats.split_once(' ').map_or("", |(_, s)| s).into();
                    } else {
                        self.status = status.into();
                    }
                    continue;
                }
            }
            if let Some((target, payload)) = steam_datagram(&data, from, self.peer, self.cookie_value) {
                packets.push((target, payload.to_vec()));
            }
        }
        if let Some((request, since)) = &self.pending {
            if since.elapsed() > Duration::from_secs(18) {
                self.events.push(Response {
                    request: request.id,
                    event: Event::Error(
                        "Steam request timed out. Open Steam and sign in, then retry.".into(),
                    ),
                });
                self.pending = None;
            } else if self.last_command.elapsed() > Duration::from_millis(500) {
                if let Some(peer) = self.peer {
                    let _ = self.socket.send_to(
                        format!("CMD {} {}", self.cookie, directory::encode(request)).as_bytes(),
                        peer,
                    );
                    self.last_command = Instant::now();
                }
            }
        }
        if self.last_ping.elapsed() > Duration::from_secs(1) {
            if let Some(p) = self.peer {
                let _ = self
                    .socket
                    .send_to(format!("PING {}", self.cookie).as_bytes(), p);
            }
            self.last_ping = Instant::now();
        }
        if self.child.try_wait()?.is_some() && !self.status.starts_with("ERROR") {
            self.status = "Steam relay stopped. Open Steam, sign in, and retry.".into();
        }
        if self.peer.is_none() && self.started.elapsed() > Duration::from_secs(10) {
            self.status = "Steam did not respond. Open Steam, sign in, and retry.".into();
        }
        Ok(packets)
    }
    fn status(&self) -> String {
        self.status.clone()
    }
    fn metrics(&self) -> String {
        self.metrics.clone()
    }
    fn congested(&self) -> bool {
        self.congested
    }
    /// perf 10-18: the relay leaves the Steam lobby (LEAVE) and stays up with Steam for the next session; killing and
    /// starting it again cost up to 100 ms on the main thread and a Steam init per leave or map change.
    /// SKATE_RELAY_REUSE=0: dropped as before.
    fn leave(&mut self) -> bool {
        let Some(peer) = self.peer.filter(|_| skate_net::knob!("SKATE_RELAY_REUSE", 1) != 0) else {
            return false;
        };
        if self.status.starts_with("ERROR") || self.child.try_wait().ok().flatten().is_some() {
            return false;
        }
        let _ = self.socket.send_to(format!("LEAVE {}", self.cookie).as_bytes(), peer);
        self.pending = None;
        self.events.clear();
        self.left = self.lobby.take().or(self.left);
        true
    }
    fn voice_io(&mut self) -> Option<VoiceIo> {
        let (socket, relay, cookie) = (self.socket.try_clone().ok()?, self.relay.clone(), self.cookie_value);
        let send = move |target, data: &[u8]| {
            // Nothing while the relay is not known yet.
            if let Some(relay) = *lock(&relay) {
                let _ = socket.send_to(&framed(cookie, target, data), relay);
            }
        };
        Some((Box::new(send), self.reader.voice()))
    }
}
/// perf 10-18: true for an owner notice (request 0) of the lobby a kept relay left: sent before the relay took LEAVE,
/// it would start that session again. A requested join of that lobby clears it.
fn stale(left: &mut Option<u64>, current: &mut Option<u64>, response: &Response) -> bool {
    let Event::Owner { lobby, .. } = &response.event else {
        return false;
    };
    if response.request == 0 && *left == Some(*lobby) {
        return true;
    }
    if *left == Some(*lobby) {
        *left = None;
    }
    *current = Some(*lobby);
    false
}
#[cfg(test)]
mod tests {
    use super::*;
    use skate_net::{lobby, packed, voice::VOICE};
    fn direct() -> Direct {
        Direct::new("127.0.0.1:0".parse().unwrap(), None).unwrap()
    }
    fn steam(socket: UdpSocket) -> Steam {
        Steam::with(socket, Command::new("cmd").args(["/C", "exit"]).stdout(Stdio::null()).spawn().unwrap(), 9).unwrap()
    }
    /// A game datagram of `kind` (session 7, actor 2).
    fn datagram(kind: u8, seq: u32) -> Vec<u8> {
        let mut d = packed::header(7, 2, kind, seq);
        d.extend(b"payload");
        d
    }
    /// `receive()` until `done` (at most 1 s): the reader thread hands datagrams over a moment after they land.
    fn poll(t: &mut dyn Transport, done: impl Fn(&[(u64, Vec<u8>)]) -> bool) -> Vec<(u64, Vec<u8>)> {
        let (mut got, start) = (vec![], Instant::now());
        while !done(&got) && start.elapsed() < Duration::from_secs(1) {
            got.extend(t.receive().expect("one bad datagram is skipped, not the frame"));
            std::thread::sleep(Duration::from_millis(5));
        }
        got
    }
    #[test]
    fn a_stranger_s_oversized_datagram_does_not_drop_the_frame() {
        let mut direct = direct();
        let to = direct.socket.local_addr().unwrap();
        let stranger = UdpSocket::bind("127.0.0.1:0").unwrap();
        stranger.send_to(&[0; 4000], to).unwrap(); // Windows: WSAEMSGSIZE into a 1500-byte buffer
        stranger.send_to(b"real", to).unwrap();
        let got = poll(&mut direct, |got| got.iter().any(|(_, d)| d == b"real"));
        assert!(got.iter().any(|(_, d)| d == b"real"));
        assert!(got.iter().all(|(_, d)| d.len() <= packed::MTU), "the oversized one is not passed on");
    }
    /// User 10-26 (voice chat): a VOICE datagram goes to the voice channel at once; the rest waits for `receive()`.
    #[test]
    fn voice_goes_to_the_voice_channel_and_the_rest_to_receive() {
        let (mut a, mut b) = (direct(), direct());
        let (_, voice) = a.voice_io().expect("Direct carries voice");
        let to = endpoint(a.socket.local_addr().unwrap()).unwrap();
        let from = endpoint(b.socket.local_addr().unwrap()).unwrap();
        b.send(to, &datagram(VOICE, 1)).unwrap();
        b.send(to, &datagram(lobby::PING, 1)).unwrap();
        assert_eq!(voice.recv_timeout(Duration::from_secs(1)), Ok((from, datagram(VOICE, 1))));
        assert_eq!(poll(&mut a, |got| !got.is_empty()), [(from, datagram(lobby::PING, 1))], "voice is not passed on twice");
        // Our own voice goes out through the sender, within the session's network as `send` keeps it (P9).
        let (mut send, _) = b.voice_io().unwrap();
        send(to, &datagram(VOICE, 2));
        assert_eq!(voice.recv_timeout(Duration::from_secs(1)), Ok((from, datagram(VOICE, 2))));
    }
    /// The game thread hitches for a second (no `receive()`): voice still comes as it is sent.
    #[test]
    fn a_stalled_main_thread_still_gets_voice() {
        let (mut a, mut b) = (direct(), direct());
        let (_, voice) = a.voice_io().unwrap();
        let to = endpoint(a.socket.local_addr().unwrap()).unwrap();
        for seq in 0..50 {
            b.send(to, &datagram(VOICE, seq)).unwrap();
            let got = voice.recv_timeout(Duration::from_millis(100)).expect("voice while the game thread stalls");
            assert_eq!(packed::envelope(&got.1).map(|e| e.3), Some(seq));
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    #[test]
    fn steam_framing_checks_relay_cookie_and_length() {
        let relay: SocketAddr = "127.0.0.1:5000".parse().unwrap();
        let frame = |cookie: u64, payload: &[u8]| [&cookie.to_le_bytes()[..], &42u64.to_le_bytes(), payload].concat();
        let good = frame(9, b"data");
        assert_eq!(steam_datagram(&good, relay, Some(relay), 9), Some((42, &b"data"[..])));
        assert_eq!(steam_datagram(&good, "127.0.0.1:5001".parse().unwrap(), Some(relay), 9), None, "another sender");
        assert_eq!(steam_datagram(&good, relay, None, 9), None, "the relay is not known yet");
        assert_eq!(steam_datagram(&good, relay, Some(relay), 8), None, "the wrong cookie");
        assert_eq!(steam_datagram(&good[..15], relay, Some(relay), 9), None, "15 bytes");
        assert_eq!(steam_datagram(&frame(9, &[0; packed::MTU + 1]), relay, Some(relay), 9), None, "MTU + 17 bytes");
        assert!(steam_datagram(&frame(9, &[0; packed::MTU]), relay, Some(relay), 9).is_some(), "MTU + 16 is fine");
        assert_eq!(steam_datagram(&frame(9, &[]), relay, Some(relay), 9), Some((42, &[][..])), "16: nothing after the frame");
    }
    /// Steam's VOICE comes from the relay as (cookie, target, datagram): it goes to the voice channel as (target,
    /// datagram), the rest to `receive()`; ours goes to the relay framed the same way, and nowhere before it is known.
    #[test]
    fn steam_voice_goes_through_the_relay() {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let ours = socket.local_addr().unwrap();
        let mut steam = steam(socket);
        let (mut send, voice) = steam.voice_io().expect("Steam carries voice");
        send(43, b"too early");
        let relay = UdpSocket::bind("127.0.0.1:0").unwrap();
        relay.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        relay.send_to(b"SK8RELAY 0000000000000009 READY", ours).unwrap();
        let start = Instant::now();
        while steam.peer.is_none() && start.elapsed() < Duration::from_secs(1) {
            steam.receive().unwrap();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(steam.peer, relay.local_addr().ok(), "the relay is known");
        let framed = |target: u64, d: &[u8]| [&9u64.to_le_bytes()[..], &target.to_le_bytes(), d].concat();
        relay.send_to(&framed(42, &datagram(VOICE, 1)), ours).unwrap();
        relay.send_to(&framed(42, &datagram(lobby::PING, 1)), ours).unwrap();
        assert_eq!(voice.recv_timeout(Duration::from_secs(1)), Ok((42, datagram(VOICE, 1))));
        assert_eq!(poll(&mut steam, |got| !got.is_empty()), [(42, datagram(lobby::PING, 1))]);
        send(43, &datagram(VOICE, 2));
        let mut buffer = [0; 2048];
        let first = std::iter::from_fn(|| relay.recv_from(&mut buffer).ok().map(|(n, _)| buffer[..n].to_vec()))
            .find(|d| !d.starts_with(b"PING "))
            .expect("our voice reaches the relay");
        assert_eq!(first, framed(43, &datagram(VOICE, 2)), "framed, and the early one never went");
    }
    /// Audit 10-24: a Quick Match cancelled in a session left its browse waiting here, so the next one was refused
    /// ("Steam request in progress") and showed No match at once. The newest request replaces it; the old one's late
    /// answer is dropped.
    #[test]
    fn a_new_request_replaces_one_still_waiting() {
        let mut steam = steam(UdpSocket::bind("127.0.0.1:0").unwrap());
        let browse = || LobbyCommand::Browse { page: 0, map: 1, physics: 2, label: String::new(), tags: Default::default() };
        assert!(steam.command(browse()).is_ok());
        assert!(steam.command(browse()).is_ok(), "the cancelled one does not block it");
        assert_eq!(steam.pending.as_ref().map(|(r, _)| r.id), Some(2));
        assert!(steam.command(LobbyCommand::Privacy { private: true }).is_err(), "a browse is only replaced by a browse");
        steam.pending = None;
        assert!(steam.command(LobbyCommand::Join { lobby: 5, map: 1, physics: 2 }).is_ok());
        assert!(steam.command(browse()).is_err(), "a join in flight is never dropped");
    }
    /// perf 10-18: a relay kept through a leave: the left lobby's owner notices still in flight are dropped, a new
    /// lobby's pass, and asking to join the left one again takes it back.
    #[test]
    fn a_kept_relay_never_rejoins_the_lobby_it_left() {
        let owner = |request, lobby| Response { request, event: Event::Owner { lobby, owner: 1, own: 2 } };
        let (mut left, mut current) = (None, Some(7));
        left = current.take().or(left); // Steam::leave
        assert!(stale(&mut left, &mut current, &owner(0, 7)), "a notice of the lobby we left");
        assert!(!stale(&mut left, &mut current, &owner(0, 8)));
        assert_eq!(current, Some(8));
        assert!(!stale(&mut left, &mut current, &owner(3, 7)), "our own join of it again");
        assert!(!stale(&mut left, &mut current, &owner(0, 7)));
        assert!(!stale(&mut left, &mut current, &Response { request: 0, event: Event::Error("x".into()) }));
    }
    #[test]
    fn a_roster_cannot_aim_direct_traffic_off_the_session_s_network() {
        let to = |ip: &str| (u32::from(ip.parse::<Ipv4Addr>().unwrap()) as u64) << 16 | 9;
        let mut lan = Direct::new("127.0.0.1:0".parse().unwrap(), Some("192.168.1.20:31030".parse().unwrap())).unwrap();
        for ip in ["8.8.8.8", "127.0.0.1", "224.0.0.1", "255.255.255.255", "0.0.0.0", "192.0.2.1"] {
            assert_eq!(lan.send(to(ip), b"x").unwrap_err().kind(), io::ErrorKind::PermissionDenied, "{ip}");
        }
        let mut local = Direct::new("127.0.0.1:0".parse().unwrap(), None).unwrap();
        assert!(local.send(to("127.0.0.1"), b"x").is_ok());
        assert!(local.send(to("10.0.0.5"), b"x").is_err());
        assert!(local.send(to("1.1.1.1"), b"x").is_err());
        assert_eq!(scope("203.0.114.1".parse().unwrap()), Some(Scope::Internet));
        assert!(Direct::new("127.0.0.1:0".parse().unwrap(), Some("[::1]:1".parse().unwrap())).is_err(), "IPv6 is an error, not a panic");
    }
}
