//! Steam identity and friends without linking Steamworks: a `skate-steam-relay --presence` helper over
//! cookie-checked loopback UDP, or the `SKATE_FAKE_FRIENDS` JSON fixture.
//!
//! Offline means offline (3.3): the helper (and its socket) exists only while something needs Steam: signed in, a
//! Steam session, a sign-in attempt or an open Friends' Skaters list ([`Presence::hold`]). Otherwise it is stopped.
use bevy::prelude::*;
use serde::Deserialize;
use skate_net::{
    presence::{Friend, Order, Report},
    roster::{NAME_MAX, truncate},
};
use std::{
    io,
    net::{SocketAddr, UdpSocket},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const PING: Duration = Duration::from_secs(2);
/// `error` when steam-relay is not next to the exe: Steam itself may be fine.
pub(crate) const NO_HELPER: &str = "The Steam helper (steam-relay) is not installed";
/// A helper that exits (Steam closed, crash on bad input) is restarted at most this often.
const RESPAWN: Duration = Duration::from_secs(30);
/// How long a [`Presence::hold`] keeps the helper wanted; past online_menu's 30 s sign-in timeout. A knob.
const HOLD: Duration = Duration::from_secs(40);

#[derive(Resource)]
pub(crate) struct Presence {
    pub me: Option<(u64, String)>,
    pub friends: Vec<Friend>,
    /// Connect strings; consumers drain.
    pub join_requests: Vec<String>,
    /// The helper answered `Me`, or the fixture loaded.
    pub available: bool,
    /// Not retail (user 10-25): the players signed in to online on this build, as the helper last counted them (every
    /// 3 minutes); the Online Challenge Map shows it. None until counted.
    pub players: Option<u32>,
    /// Bumped when `friends` changes (the Player List rebuilds its rows then).
    pub generation: u64,
    /// Why Steam is unreachable (no helper, Steam closed or signed out); cleared by [`Presence::connect`].
    pub error: Option<String>,
    fixture: bool,
    helper: Option<Helper>,
    /// Last rich-presence connect ordered; re-sent when a (re)started helper reports `Me`.
    connect: Option<String>,
    warned: Vec<String>,
    /// Friends' Skaters reports; friend_skaters drains them.
    pub skater_reports: Vec<Report>,
    /// The installed helper; None: never started (not installed, the fixture, the launcher's clip run).
    exe: Option<PathBuf>,
    /// A sign-in attempt or an open Friends' Skaters list wants Steam until then.
    held_until: Option<Instant>,
}

struct Helper {
    socket: UdpSocket,
    exe: PathBuf,
    cookie: String,
    child: Option<Child>,
    spawned: Instant,
    peer: Option<SocketAddr>,
    last_ping: Instant,
    /// One report datagram (friend lists are large); kept, not zero-filled every frame.
    buffer: Vec<u8>,
}
impl Helper {
    fn spawn(&mut self) -> io::Result<()> {
        self.spawned = Instant::now();
        self.peer = None;
        let mut command = Command::new(&self.exe);
        command
            .args([
                "--presence".to_string(),
                self.socket.local_addr()?.to_string(),
                self.cookie.clone(),
                crate::multiplayer::build_tag(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        self.child = Some(command.spawn()?);
        Ok(())
    }
    fn send(&self, text: &str) {
        if let Some(peer) = self.peer {
            let _ = self.socket.send_to(text.as_bytes(), peer);
        }
    }
}
impl Drop for Helper {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        if self.peer.is_some() {
            self.send(&format!("QUIT {}", self.cookie));
            for _ in 0..10 {
                if child.try_wait().ok().flatten().is_some() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// S5 (release review 10-18): never the Windows account name: it went to every player in a Direct session.
pub(crate) fn display_name(steam: Option<&str>) -> String {
    let name = steam.filter(|n| !n.is_empty()).unwrap_or("The Legend");
    truncate(name, NAME_MAX).to_owned()
}

#[derive(Deserialize)]
struct Fixture {
    me: Option<FixtureMe>,
    #[serde(default)]
    friends: Vec<Friend>,
}
#[derive(Deserialize)]
struct FixtureMe {
    id: u64,
    name: String,
}
pub(crate) fn load_fixture(text: &str) -> Result<(Option<(u64, String)>, Vec<Friend>), String> {
    let f: Fixture = serde_json::from_str(text).map_err(|e| e.to_string())?;
    Ok((f.me.map(|m| (m.id, m.name)), f.friends))
}

impl Presence {
    pub fn offline() -> Self {
        Presence {
            me: None,
            friends: vec![],
            join_requests: vec![],
            available: false,
            players: None,
            generation: 0,
            error: None,
            fixture: false,
            helper: None,
            connect: None,
            warned: vec![],
            skater_reports: vec![],
            exe: None,
            held_until: None,
        }
    }

    /// The fixture, or the installed helper, not started: nothing touches the network until [`Presence::want`].
    pub fn new() -> Self {
        if let Ok(path) = std::env::var("SKATE_FAKE_FRIENDS") {
            let mut p = Self::offline();
            match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| load_fixture(&t)) {
                Ok((me, friends)) => {
                    info!("Presence: fixture {path} ({} friends)", friends.len());
                    (p.me, p.friends, p.available, p.fixture) = (me, friends, true, true);
                }
                Err(e) => warn!("Presence: fixture {path}: {e}"),
            }
            return p;
        }
        let dir = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf));
        Self::idle(dir.filter(|d| d.join("steam-relay/steam_api64.dll").is_file()).map(|d| d.join("steam-relay/skate-steam-relay.exe")))
    }

    pub(crate) fn idle(exe: Option<PathBuf>) -> Self {
        Self { exe: exe.filter(|e| e.is_file()), ..Self::offline() }
    }

    /// Something needs Steam now (a sign-in attempt, the Friends' Skaters list): keep the helper for [`HOLD`].
    pub fn hold(&mut self) {
        self.held_until = Some(Instant::now() + HOLD);
    }

    /// Run the helper while `needed` (signed in, a Steam session) or held; stop it otherwise. Stopping ends the
    /// process (QUIT, then kill) and closes its socket.
    pub(crate) fn want(&mut self, needed: bool) {
        let wanted = needed || self.held_until.is_some_and(|t| Instant::now() < t);
        match (wanted, self.helper.is_some()) {
            (true, false) if self.exe.is_some() => self.start_helper(),
            (false, true) => self.stop(),
            _ => {}
        }
    }

    fn start_helper(&mut self) {
        let Some(exe) = self.exe.clone() else { return };
        let socket = match UdpSocket::bind("127.0.0.1:0").and_then(|s| skate_net::socket::configure(&s).map(|_| s)) {
            Ok(s) => s,
            Err(e) => {
                self.error = Some(format!("could not start the Steam helper: {e}"));
                return;
            }
        };
        let mut helper = Helper {
            socket,
            exe,
            cookie: format!("{:016x}", crate::multiplayer::unique()),
            child: None,
            spawned: Instant::now(),
            peer: None,
            last_ping: Instant::now(),
            buffer: vec![0; 65536],
        };
        if let Err(e) = helper.spawn() {
            warn!("Presence: could not start Steam helper: {e}");
            self.error = Some(format!("could not start the Steam helper: {e}"));
        }
        self.helper = Some(helper);
    }

    fn stop(&mut self) {
        // Helper's Drop: QUIT, then kill and reap; its socket closes with it.
        self.helper = None;
        self.available = false;
        self.friends.clear();
        self.generation += 1;
        // ponytail: `me` stays (our own name for offline score tables); a new helper reports it again.
        (self.connect, self.error) = (None, None);
        self.join_requests.clear();
        self.skater_reports.clear();
    }

    pub(crate) fn running(&self) -> bool {
        self.helper.is_some()
    }

    /// False when Steam presence is unavailable (callers grey the action).
    pub fn order(&mut self, order: Order) -> bool {
        if let Order::SetConnect(c) = &order {
            self.connect = c.clone();
        }
        if self.fixture {
            info!("Presence (fixture): {order:?}");
            return true;
        }
        let Some(h) = &self.helper else {
            return false;
        };
        if self.available {
            h.send(&format!("CMD {} {}", h.cookie, skate_net::directory::encode(&order)));
        }
        self.available
    }

    /// A sign-in attempt: restart a helper that has not reached Steam now (not after the respawn delay), so a
    /// Steam client opened since the last try is picked up. Answers arrive as `available` or `error`.
    pub fn connect(&mut self) {
        self.hold();
        if self.available {
            return;
        }
        self.error = None;
        if self.exe.is_none() {
            self.error = Some(NO_HELPER.into());
            return;
        }
        let Some(h) = &mut self.helper else {
            return self.start_helper();
        };
        if let Some(mut child) = h.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Err(e) = h.spawn() {
            self.error = Some(format!("could not start the Steam helper: {e}"));
        }
    }

    pub fn display_name(&self) -> String {
        display_name(self.me.as_ref().map(|(_, n)| n.as_str()))
    }

    fn warn_once(&mut self, text: String) {
        if !self.warned.contains(&text) {
            warn!("Steam presence: {text}");
            self.warned.push(text);
        }
    }

    fn poll(&mut self) {
        let Some(h) = &mut self.helper else {
            return;
        };
        let exited = h.child.as_mut().is_none_or(|c| !matches!(c.try_wait(), Ok(None)));
        if exited {
            if h.child.take().is_some() {
                h.peer = None;
                if !self.available {
                    self.error.get_or_insert_with(|| "the Steam helper stopped".into());
                }
                self.available = false;
                self.friends.clear();
                self.generation += 1;
            }
            if h.spawned.elapsed() >= RESPAWN {
                if let Err(e) = h.spawn() {
                    let e = format!("could not restart the helper: {e}");
                    self.warn_once(e);
                }
            }
            return;
        }
        let prefix = format!("SK8RELAY {} PRESENCE ", h.cookie);
        let mut reports = vec![];
        for _ in 0..64 {
            let Ok((n, from)) = h.socket.recv_from(&mut h.buffer) else {
                break;
            };
            let Some(json) = std::str::from_utf8(&h.buffer[..n]).ok().and_then(|t| t.strip_prefix(&prefix)) else {
                continue;
            };
            if !from.ip().is_loopback() {
                continue;
            }
            h.peer = Some(from);
            reports.extend(skate_net::presence::report(json));
        }
        if h.peer.is_some() && h.last_ping.elapsed() >= PING {
            h.send(&format!("PING {}", h.cookie));
            h.last_ping = Instant::now();
        }
        for r in reports {
            match r {
                Report::Me { id, name } => {
                    let fresh = !self.available;
                    // Steam can answer "" while it is going away; keep the last good name.
                    let name = match self.me.take() {
                        Some((old_id, old)) if name.is_empty() && old_id == id => old,
                        _ => name,
                    };
                    self.me = Some((id, name));
                    self.available = true;
                    self.error = None;
                    // A restarted helper lost the rich presence; restore it.
                    if let (true, Some(c)) = (fresh, self.connect.clone()) {
                        self.order(Order::SetConnect(Some(c)));
                    }
                }
                Report::Friends(f) => {
                    if f != self.friends {
                        self.friends = f;
                        self.generation += 1;
                    }
                }
                Report::JoinRequested { connect } => self.join_requests.push(connect),
                Report::Players(n) => self.players = Some(n),
                r @ (Report::SkaterFriends { .. } | Report::Skater { .. } | Report::SkaterPublished { .. }) => {
                    self.skater_reports.push(r)
                }
                Report::Error(e) => {
                    // ponytail: once this helper said `Me`, its only error is a refused invite; Steam itself still works.
                    // A helper started after Steam closed (the old one exits) has not, even if `me` is remembered.
                    if !self.available {
                        self.error = Some(e.clone());
                        // The helper never retries Steam init: restart it (rate-limited) so a
                        // Steam client started after the game is picked up.
                        if let Some(h) = self.helper.as_mut().filter(|h| h.spawned.elapsed() >= RESPAWN) {
                            if let Some(c) = h.child.as_mut() {
                                let _ = c.kill();
                                let _ = c.wait();
                            }
                        }
                    }
                    self.warn_once(e);
                }
            }
        }
    }
}

fn poll(mut presence: ResMut<Presence>) {
    presence.poll();
}

/// Signed in to Steam, or in (or joining) a Steam session: the helper runs. A Direct session does not need it.
fn demand(
    mut presence: ResMut<Presence>,
    ui: Option<Res<crate::online_menu::OnlineUi>>,
    net: Option<Res<crate::multiplayer::Multiplayer>>,
) {
    let steam = net.is_some_and(|n| n.is_steam() || (n.engaged() && !n.active()));
    presence.want(steam || ui.is_some_and(|u| u.signed_in));
}

pub(crate) struct PresencePlugin;
impl Plugin for PresencePlugin {
    fn build(&self, app: &mut App) {
        // the launcher's hidden clip run starts no Steam relay
        let presence = if crate::hero_clip::active(app) { Presence::offline() } else { Presence::new() };
        app.insert_resource(presence).add_systems(Update, (demand, poll).chain());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn display_name_falls_back_and_truncates() {
        assert_eq!(display_name(Some("Rich")), "Rich");
        assert_eq!(display_name(Some("")), "The Legend", "S5: no Steam name is the placeholder, never %USERNAME%");
        assert_eq!(display_name(None), "The Legend");
        assert_eq!(display_name(Some(&"é".repeat(20))).len(), 32);
    }
    #[test]
    fn fixture_loads_me_and_friends() {
        let (me, friends) = load_fixture(r#"{"me":{"id":5,"name":"Sam"},"friends":[
            {"id":7,"name":"Ann","online":true,"connect":"sk8:99:University"},
            {"id":8,"name":"Bob","online":false,"connect":null}]}"#).unwrap();
        assert_eq!(me, Some((5, "Sam".into())));
        assert_eq!(friends.len(), 2);
        assert!(load_fixture("{").is_err());
    }
    /// 3.3: offline (menus, career, Edit Skater) the helper never starts and no socket opens; signing in starts it,
    /// signing out stops it (process gone, socket closed); a sign-in attempt or Friends' Skaters holds it a while.
    #[test]
    fn offline_flow_opens_no_socket_and_starts_no_helper() {
        let dir = std::env::temp_dir().join(format!("presence-offline-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Installed, but not a program: a start binds the socket and the spawn fails, so no process is left behind.
        let exe = dir.join("skate-steam-relay.exe");
        std::fs::write(&exe, b"not a program").unwrap();
        let mut app = App::new();
        app.insert_resource(Presence::idle(Some(exe)))
            .insert_resource(crate::online_menu::OnlineUi::default())
            .add_systems(Update, (demand, poll).chain());
        for _ in 0..300 {
            app.update();
        }
        let presence = app.world().resource::<Presence>();
        assert!(!presence.running() && !presence.available, "offline: no helper, no socket");
        app.world_mut().resource_mut::<crate::online_menu::OnlineUi>().signed_in = true;
        app.update();
        assert!(app.world().resource::<Presence>().running(), "signed in: started on demand");
        app.world_mut().resource_mut::<crate::online_menu::OnlineUi>().signed_in = false;
        app.update();
        assert!(!app.world().resource::<Presence>().running(), "signed out: stopped");
        // A sign-in attempt holds it until it expires.
        app.world_mut().resource_mut::<Presence>().connect();
        app.update();
        assert!(app.world().resource::<Presence>().running());
        app.world_mut().resource_mut::<Presence>().held_until = Some(Instant::now());
        app.update();
        assert!(!app.world().resource::<Presence>().running(), "the hold ran out");
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn no_helper_means_unavailable_and_orders_refused() {
        let mut p = Presence::offline();
        assert!(!p.available);
        assert!(!p.order(skate_net::presence::Order::Invite { friend: 7, connect: None }));
        assert!(p.friends.is_empty());
    }
}
