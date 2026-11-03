//! 10-12 online audit (freeskate50): a 50-player free-skate lobby through the real `Session`s, in one process, over an
//! injectable lossy / late / reordering / duplicating link. Endpoint = index + 1 (a SteamID in Steam sessions).
//! Never starts the game or Steam.
use skate_net::{
    Body, Bone, Pose,
    lobby::{Info, MAX_PLAYERS, Session},
    packed::{self, BodyState, Packed, PoseState},
    roster::{self, Player, SessionState},
};

fn info(id: u64) -> Info {
    Info { id, map: 1, rig: 2, physics: 3, appearance: 4 }
}
fn pose(p: [f32; 3], t: f32) -> Pose {
    Pose { p, q: [0., (t * 0.5).sin(), 0., (t * 0.5).cos()] }
}
/// A skater at `at`, moving a little every sample (so its streams count as changing).
fn movement(at: [f32; 3], now: u64) -> (Packed, Packed) {
    let t = now as f32 / 1000.;
    let root = pose([at[0] + (t * 0.7).sin(), at[1], at[2] + (t * 0.7).cos()], t * 0.2);
    let bodies = (0..33)
        .map(|i| {
            let phase = t * 3. + i as f32;
            Body {
                pose: pose([root.p[0] + phase.sin() * 0.2, root.p[1] + i as f32 * 0.03, root.p[2] + phase.cos() * 0.1], phase * 0.1),
                velocity: [3. + phase.cos(), 0., phase.sin()],
                angular: [0., phase.cos(), 0.],
            }
        })
        .collect();
    // 30 anchors and the camera row, as multiplayer::send_pose sends.
    let mut bones: Vec<Bone> = (0..30)
        .map(|i| Bone { index: i, pose: pose([0., i as f32 * 0.03, (t * 3. + i as f32).sin() * 0.1], t + i as f32) })
        .collect();
    bones.push(Bone { index: packed::CAMERA, pose: pose([root.p[0], root.p[1] + 2., root.p[2] - 4.], t) });
    (
        Packed::body(&BodyState { root, enabled: (1 << 33) - 1, bodies }).unwrap(),
        Packed::pose(&PoseState { root, bones }).unwrap(),
    )
}

struct Peer {
    s: Session,
    at: [f32; 3],
    last_body: u64,
    last_pose: u64,
    up: u64,
    down: u64,
    /// When its uplink is free again (ms), under `Net::uplink`.
    busy: f64,
    /// Publishes no movement (still loading).
    silent: bool,
}
struct Flight {
    at: u64,
    from: u64,
    to: usize,
    data: Vec<u8>,
}
pub struct Net {
    peers: Vec<Option<Peer>>,
    now: u64,
    flights: Vec<Flight>,
    rng: u64,
    /// Per datagram, in percent.
    pub loss: u64,
    pub dup: u64,
    /// One-way latency range in ms (jitter reorders).
    pub latency: (u64, u64),
    /// Frame step in ms.
    pub step: u64,
    /// Bytes sent by endpoint 1 per datagram kind.
    pub host_kinds: [u64; 32],
    /// Every datagram sent, while set (a corpus for the bad-peer test).
    pub capture: Option<Vec<Vec<u8>>>,
    /// Each player's upload in bytes/s (0: unlimited); datagrams wait their turn, and past `queue_ms` of backlog
    /// they are dropped (Steam's send buffer).
    pub uplink: u64,
    pub queue_ms: f64,
    pub queue_drops: u64,
    /// (from endpoint, to endpoint) one-way links that carry nothing.
    pub blocked: Vec<(u64, u64)>,
    /// BODY datagrams sent, by (from endpoint, to endpoint).
    pub bodies: std::collections::BTreeMap<(u64, u64), u64>,
}
impl Net {
    pub fn new() -> Self {
        Net { peers: vec![], now: 0, flights: vec![], rng: 0x9E37_79B9_7F4A_7C15, loss: 0, dup: 0, latency: (20, 20), step: 16, host_kinds: [0; 32], capture: None, uplink: 0, queue_ms: 500., queue_drops: 0, blocked: vec![], bodies: Default::default() }
    }
    fn rand(&mut self, n: u64) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng % n.max(1)
    }
    /// A player with actor id 1000 + index; the first is the host. Returns its index.
    pub fn join(&mut self, at: [f32; 3], host_endpoint: Option<u64>) -> usize {
        let index = self.peers.len();
        self.peers.push(Some(self.player(index, 1000 + index as u64, at, host_endpoint)));
        index
    }
    fn player(&self, index: usize, actor: u64, at: [f32; 3], host_endpoint: Option<u64>) -> Peer {
        let mut s = Session::new(7, info(actor), host_endpoint);
        s.publish_application(roster::PLAYER_KEY, Player { name: format!("Skater {index}"), steam: index as u64 + 1 }.encode(), self.now);
        Peer { s, at, last_body: 0, last_pose: 0, up: 0, down: 0, busy: 0., silent: false }
    }
    /// Player `i`'s game restarts and joins again at once from the same endpoint: a new session, a new actor id.
    pub fn rejoin(&mut self, i: usize, actor: u64) {
        let at = self.peers[i].as_ref().map_or([0.; 3], |p| p.at);
        self.peers[i] = Some(self.player(i, actor, at, Some(1)));
    }
    pub fn silence(&mut self, i: usize) {
        self.peers[i].as_mut().unwrap().silent = true;
    }
    pub fn s(&self, i: usize) -> &Session {
        &self.peers[i].as_ref().unwrap().s
    }
    pub fn s_mut(&mut self, i: usize) -> &mut Session {
        &mut self.peers[i].as_mut().unwrap().s
    }
    pub fn live(&self) -> Vec<usize> {
        (0..self.peers.len()).filter(|&i| self.peers[i].is_some()).collect()
    }
    /// Quits without a word (crash, cable pulled).
    pub fn vanish(&mut self, i: usize) {
        self.peers[i] = None;
    }
    pub fn quit(&mut self, i: usize) {
        let out = self.s(i).goodbye();
        let from = i as u64 + 1;
        for p in out {
            self.send(from, p.peer, p.data);
        }
        self.peers[i] = None;
    }
    fn send(&mut self, from: u64, to: u64, data: Vec<u8>) {
        let mut depart = self.now as f64;
        if let Some(Some(p)) = self.peers.get_mut(from as usize - 1) {
            p.up += data.len() as u64;
            if self.uplink != 0 {
                depart = p.busy.max(depart) + data.len() as f64 * 1000. / self.uplink as f64;
                if depart - self.now as f64 > self.queue_ms {
                    self.queue_drops += 1;
                    return;
                }
                p.busy = depart;
            }
        }
        if let Some(c) = &mut self.capture {
            c.push(data.clone());
        }
        if from == 1 && data.len() > 24 {
            self.host_kinds[data[24] as usize % 32] += data.len() as u64;
        }
        if data.len() > 24 && data[24] == packed::BODY {
            *self.bodies.entry((from, to)).or_default() += 1;
        }
        if self.blocked.contains(&(from, to)) || self.rand(100) < self.loss {
            return;
        }
        let Some(to) = (to as usize).checked_sub(1) else { return };
        let copies = if self.rand(100) < self.dup { 2 } else { 1 };
        for _ in 0..copies {
            let at = depart.ceil() as u64 + self.latency.0 + self.rand(self.latency.1 - self.latency.0 + 1);
            self.flights.push(Flight { at, from, to, data: data.clone() });
        }
    }
    /// Raw datagram from `from` (a bad peer), delivered now.
    pub fn inject(&mut self, from: u64, to: usize, data: &[u8]) {
        let now = self.now;
        if let Some(Some(p)) = self.peers.get_mut(to) {
            p.s.receive(from, data, now);
        }
    }
    pub fn frame(&mut self) {
        self.now += self.step;
        let now = self.now;
        // Deliver what is due, in arrival order.
        let mut due: Vec<Flight> = vec![];
        let mut i = 0;
        while i < self.flights.len() {
            if self.flights[i].at <= now {
                due.push(self.flights.swap_remove(i));
            } else {
                i += 1;
            }
        }
        due.sort_by_key(|f| f.at);
        for f in due {
            if let Some(Some(p)) = self.peers.get_mut(f.to) {
                p.down += f.data.len() as u64;
                p.s.receive(f.from, &f.data, now);
            }
        }
        for i in 0..self.peers.len() {
            let Some(p) = self.peers[i].as_mut() else { continue };
            if !p.silent && now - p.last_body >= 50 {
                let (body, _) = movement(p.at, now);
                p.s.publish(packed::BODY, body, now);
                p.last_body = now;
            }
            // perf 10-18: POSE every 50 ms, as multiplayer::send_pose now publishes (SKATE_NET_POSE_MS).
            if !p.silent && now - p.last_pose >= 50 {
                let (_, pose) = movement(p.at, now);
                p.s.publish(packed::POSE, pose, now);
                p.last_pose = now;
            }
            let out = p.s.service(now);
            for o in out {
                self.send(i as u64 + 1, o.peer, o.data);
            }
        }
    }
    pub fn run(&mut self, ms: u64) {
        let end = self.now + ms;
        while self.now < end {
            self.frame();
        }
    }
    pub fn now(&self) -> u64 {
        self.now
    }
    /// (up, down) bytes so far, per index.
    pub fn bytes(&self, i: usize) -> (u64, u64) {
        self.peers[i].as_ref().map_or((0, 0), |p| (p.up, p.down))
    }
    /// Every live peer holds exactly the live actors, each with its `@player` name.
    pub fn converged(&self) -> bool {
        let live: Vec<u64> = self.live().iter().map(|&i| self.s(i).local).collect();
        self.live().iter().all(|&i| {
            let s = self.s(i);
            s.actors.keys().copied().collect::<Vec<_>>() == live && roster::members(s).len() == live.len()
        })
    }
    pub fn report(&self) -> String {
        self.live().iter().map(|&i| format!("{i}:{}/{}", self.s(i).actors.len(), roster::members(self.s(i)).len())).collect::<Vec<_>>().join(" ")
    }
    /// Live peers whose movement from some other live peer is older than `ms` (or missing).
    pub fn stale(&self, ms: u64) -> usize {
        let live = self.live();
        live.iter()
            .map(|&i| {
                let s = self.s(i);
                live.iter()
                    .filter(|&&j| j != i)
                    .filter(|&&j| {
                        let id = self.s(j).local;
                        let fresh = s.actors.get(&id).and_then(|a| {
                            let b = a.body.latest().map(|r| r.received);
                            let p = a.pose.latest().map(|r| r.received);
                            b.max(p)
                        });
                        fresh.is_none_or(|t| self.now - t > ms)
                    })
                    .count()
            })
            .sum()
    }
}

/// Runs until every peer converged: the ms it took, None past `limit`.
fn settle(net: &mut Net, limit: u64) -> Option<u64> {
    let start = net.now();
    while net.now() < start + limit {
        net.run(100);
        if net.converged() {
            return Some(net.now() - start);
        }
    }
    None
}
fn cluster(i: usize) -> [f32; 3] {
    // 50 players inside a 28 m square: everyone within 30 m of everyone (worst case: the spawn).
    [(i % 7) as f32 * 4., 0., (i / 7) as f32 * 4.]
}

/// Item 1: 29 in, then a join storm of 20 within one second over a 5% lossy, 40-120 ms link; the 51st is refused with
/// the "Lobby full" message; every peer converges on the same 50 names.
#[test]
fn a_join_storm_fills_fifty_and_the_fifty_first_is_refused() {
    let mut net = Net::new();
    net.loss = 5;
    net.dup = 2;
    net.latency = (40, 120);
    net.join(cluster(0), None);
    for i in 1..30 {
        net.join(cluster(i), Some(1));
    }
    let first = settle(&mut net, 20_000).unwrap_or_else(|| panic!("30 players converge: {}", net.report()));
    eprintln!("30 at once: converged in {first} ms");
    net.run(2_000);
    let before = net.bytes(0).0;
    for i in 30..50 {
        net.join(cluster(i), Some(1));
        net.run(50);
    }
    let at = settle(&mut net, 20_000).unwrap_or_else(|| panic!("50 converge after the storm: {}", net.report()));
    let storm = net.bytes(0).0 - before;
    eprintln!("host by kind (kB): {:?}", net.host_kinds.iter().map(|b| b / 1000).collect::<Vec<_>>());
    eprintln!("join storm: every peer holds all 50 names {at} ms after the 20th joined; host sent {} kB meanwhile", storm / 1000);
    assert!(at < 3_000, "{at} ms");
    let fifty_first = net.join(cluster(50), Some(1));
    net.run(3_000);
    let s = net.s(fifty_first);
    assert_eq!((s.rejected, s.notice.as_str()), (Some(3), "Lobby full"));
    assert_eq!(net.s(0).actors.len(), MAX_PLAYERS, "never admitted");
    net.vanish(fifty_first);
    net.run(2_000);
    assert!(net.converged());
    assert_eq!(net.stale(1_500), 0, "movement flows between all 50");
}

/// Bandwidth bug B1: the host re-sent the whole 50-player ROSTER (3 pages) to all 49 members every 2 s (~88 kB/s of
/// host upload for nothing) and once per admitted player in a join storm. Now members acknowledge the roster they hold
/// (in HELLO) and the host sends it only to those who lack it.
#[test]
fn a_settled_lobby_sends_no_roster_traffic() {
    let mut net = Net::new();
    net.join(cluster(0), None);
    for i in 1..50 {
        net.join(cluster(i), Some(1));
    }
    settle(&mut net, 20_000).unwrap_or_else(|| panic!("{}", net.report()));
    net.run(1_000);
    let before = net.host_kinds[skate_net::lobby::ROSTER as usize];
    net.run(10_000);
    let roster = net.host_kinds[skate_net::lobby::ROSTER as usize] - before;
    eprintln!("settled 50: host ROSTER upload {} B/s", roster / 10);
    assert!(roster < 2_000, "{roster} bytes of ROSTER in 10 s");
}

/// Item 3: the host quits a full lobby. Steam elects player 1 and tells each member over ~0.5 s (Event::Owner ->
/// Session::migrate, as multiplayer::receive does). Nobody is dropped or duplicated, the new host's `@session`
/// (emulating online_roster: it republishes the copy it last saw) reaches everyone, and the other skaters keep moving:
/// movement is peer to peer, so a migration must not stop it (B2: migrate dropped every remote actor and mesh link, so
/// all 48 other skaters vanished until the new host's ROSTER came back).
#[test]
fn the_host_leaving_a_full_lobby_keeps_everyone_moving() {
    use skate_net::roster::{SESSION_KEY, World};
    let mut net = Net::new();
    net.loss = 2;
    net.latency = (30, 90);
    net.join(cluster(0), None);
    for i in 1..50 {
        net.join(cluster(i), Some(1));
    }
    let state = SessionState { world: Some(World { weather: 2, time: 1, minute: 600 }), cohosts: vec![1005], difficulty: Some(2), ..Default::default() };
    let now = net.now();
    net.s_mut(0).publish_application(SESSION_KEY, state.encode(), now);
    settle(&mut net, 20_000).unwrap_or_else(|| panic!("{}", net.report()));
    net.run(3_000);
    assert!((1..50).all(|i| roster::session_state(net.s(i)).as_ref() == Some(&state)));
    net.quit(0);
    let left = net.now();
    // Steam's owner change reaches each member at its own time.
    let mut owner: Vec<(u64, usize)> = (1..50).map(|i| (left + 100 + (i as u64 * 37) % 500, i)).collect();
    owner.sort();
    let mut worst = 0;
    let mut vanished = 0;
    let mut settled = None;
    while net.now() < left + 10_000 {
        net.frame();
        let now = net.now();
        while owner.first().is_some_and(|(at, _)| *at <= now) {
            let (_, i) = owner.remove(0);
            net.s_mut(i).migrate(if i == 1 { None } else { Some(2) }, now);
            if i == 1 {
                // online_roster: the promoted host republishes the `@session` it last saw.
                net.s_mut(1).publish_application(SESSION_KEY, state.encode(), now);
            }
        }
        // How long since each member last heard each other surviving member move.
        let live = net.live();
        for &i in &live {
            for &j in &live {
                if i != j {
                    let id = net.s(j).local;
                    let t = net.s(i).actors.get(&id).and_then(|a| a.body.latest().map(|r| r.received).max(a.pose.latest().map(|r| r.received)));
                    worst = worst.max(t.map_or(now - left, |t| now - t.max(left)));
                    // The game draws a remote skater only while its actor is in the session (multiplayer::receive).
                    vanished += usize::from(!net.s(i).actors.contains_key(&id));
                }
            }
        }
        if settled.is_none() && owner.is_empty() && net.converged() && (1..50).all(|i| net.s(i).host_actor() == Some(1001)) {
            settled = Some(now - left);
        }
    }
    let settled = settled.unwrap_or_else(|| panic!("never settled: {}", net.report()));
    eprintln!("host left: membership settled {settled} ms after; longest movement gap {worst} ms; skater-frames missing {vanished}");
    assert_eq!(vanished, 0, "a skater vanished during the hand-over");
    assert!((1..50).all(|i| net.s(i).actors.len() == 49), "nobody dropped or duplicated");
    assert!((1..50).all(|i| roster::session_state(net.s(i)).as_ref() == Some(&state)), "weather, time, co-hosts, difficulty survive");
    // Without a migration: a 333 ms crowd pose (lobby B3) or 250 ms tier past 30 m, plus a lost datagram or two.
    assert!(worst < 1_200, "movement paused {worst} ms");
}

/// Item 4: bandwidth and network CPU at 50, everyone at the spawn (all within 30 m: the worst case) and spread over a
/// map. Budget: a home upload of ~1 MB/s; a weak one of 250 kB/s must still carry a client.
#[test]
fn fifty_players_fit_a_home_upload() {
    for (label, spread) in [("clustered", 0.), ("spread 150 m", 150.)] {
        let mut net = Net::new();
        net.loss = 2;
        net.latency = (30, 90);
        let at = |i: usize| if spread == 0. { cluster(i) } else { [(i % 7) as f32 * spread, 0., (i / 7) as f32 * spread] };
        net.join(at(0), None);
        for i in 1..50 {
            net.join(at(i), Some(1));
        }
        settle(&mut net, 20_000).unwrap_or_else(|| panic!("{}", net.report()));
        net.run(3_000);
        let before: Vec<_> = (0..50).map(|i| net.bytes(i)).collect();
        let kinds = net.host_kinds;
        let started = std::time::Instant::now();
        net.run(10_000);
        let cpu = started.elapsed().as_secs_f64() * 1000. / (10_000. / net.step as f64) / 50.;
        let rate = |i: usize| ((net.bytes(i).0 - before[i].0) / 10_000, (net.bytes(i).1 - before[i].1) / 10_000);
        let host = rate(0);
        let clients: Vec<_> = (1..50).map(rate).collect();
        let max_up = clients.iter().map(|r| r.0).max().unwrap();
        let max_down = clients.iter().map(|r| r.1).max().unwrap();
        eprintln!(
            "{label}: host up {} / down {} kB/s; worst client up {max_up} / down {max_down} kB/s; network CPU {cpu:.3} ms per player per frame (debug build); stale pairs {}",
            host.0, host.1, net.stale(1_500)
        );
        eprintln!("  host up by kind (B/s): {:?}", net.host_kinds.iter().zip(kinds).map(|(a, b)| (a - b) / 10).enumerate().filter(|(_, b)| *b > 0).collect::<Vec<_>>());
        assert!(host.0 < 1_000 && max_up < 250, "{label}: over budget");
        assert_eq!(net.stale(1_500), 0, "{label}: movement flows");
    }
}

/// Item 6: players that vanish (crash, cable) are dropped everywhere within the host's 5 s timeout, without a GOODBYE.
#[test]
fn dead_players_are_dropped_everywhere() {
    let mut net = Net::new();
    net.loss = 5;
    net.latency = (30, 90);
    net.join(cluster(0), None);
    for i in 1..50 {
        net.join(cluster(i), Some(1));
    }
    settle(&mut net, 20_000).unwrap_or_else(|| panic!("{}", net.report()));
    for i in (5..50).step_by(5) {
        net.vanish(i);
    }
    let at = settle(&mut net, 15_000).unwrap_or_else(|| panic!("{}", net.report()));
    eprintln!("9 dead players dropped by all 41 others in {at} ms");
    assert!(at < 7_000, "{at} ms");
    // And a polite quit is immediate.
    net.quit(7);
    let at = settle(&mut net, 5_000).unwrap();
    assert!(at < 1_000, "{at} ms");
}

/// Item 2: churn in a full lobby, five rounds: 5 quit, 3 vanish and 8 new players knock within the same second. Until
/// the host times the vanished out the lobby is full, so 3 knockers are refused ("Lobby full"), keep knocking, and get
/// in once slots free. Every round every peer converges on the same 50 names, and nobody is left refused.
#[test]
fn a_full_lobby_survives_repeated_join_and_leave_churn() {
    let mut net = Net::new();
    net.loss = 5;
    net.latency = (30, 90);
    net.join(cluster(0), None);
    for i in 1..50 {
        net.join(cluster(i), Some(1));
    }
    settle(&mut net, 20_000).unwrap_or_else(|| panic!("{}", net.report()));
    let mut worst = 0;
    for round in 0..5 {
        let leaving: Vec<usize> = net.live().into_iter().skip(1 + round * 3).take(8).collect();
        for (n, i) in leaving.into_iter().enumerate() {
            if n < 5 { net.quit(i) } else { net.vanish(i) }
        }
        for k in 0..8 {
            net.join(cluster(k), Some(1));
            net.run(100);
        }
        let at = settle(&mut net, 15_000).unwrap_or_else(|| panic!("round {round}: {}", net.report()));
        assert!(net.live().iter().all(|&i| net.s(i).rejected.is_none() && net.s(i).notice.is_empty()), "round {round}: someone left refused");
        worst = worst.max(at);
    }
    eprintln!("churn: 5 rounds of 8 out / 8 in at 50; slowest round converged {worst} ms after the last knock");
    assert_eq!(net.s(0).actors.len(), MAX_PLAYERS);
    assert!(worst < 8_000, "{worst} ms");
    // The last roster just made the newest mesh links; movement then flows on every one.
    net.run(2_000);
    assert_eq!(net.stale(1_500), 0, "a pair never got movement");
}

/// Item 5: chat at 50: everyone says a line at once over a lossy link; every line reaches every other member once.
#[test]
fn fifty_players_chatting_at_once_each_line_arrives_once() {
    let mut net = Net::new();
    net.loss = 10;
    net.dup = 5;
    net.latency = (30, 150);
    net.join(cluster(0), None);
    for i in 1..50 {
        net.join(cluster(i), Some(1));
    }
    settle(&mut net, 20_000).unwrap_or_else(|| panic!("{}", net.report()));
    let now = net.now();
    for i in 0..50 {
        let long = format!("{i} {}", "é".repeat(110));
        assert!(net.s_mut(i).say(&long, now).is_some());
    }
    let mut got: Vec<Vec<(u64, String)>> = vec![vec![]; 50];
    for _ in 0..200 {
        net.frame();
        for (i, g) in got.iter_mut().enumerate() {
            g.extend(net.s_mut(i).chat.drain(..));
        }
    }
    for (i, g) in got.iter().enumerate() {
        let mut from: Vec<u64> = g.iter().map(|l| l.0).collect();
        from.sort();
        assert_eq!(from, (0..50).filter(|&j| j != i).map(|j| 1000 + j as u64).collect::<Vec<_>>(), "peer {i}");
    }
}

/// Item 6: one bad member sends mutated, truncated, oversized and spoofed copies of real traffic to everyone. Nobody
/// panics, and the lobby stays the same 50 with movement flowing.
#[test]
fn a_bad_peer_cannot_crash_or_desync_the_lobby() {
    let mut net = Net::new();
    net.join(cluster(0), None);
    for i in 1..50 {
        net.join(cluster(i), Some(1));
    }
    settle(&mut net, 20_000).unwrap_or_else(|| panic!("{}", net.report()));
    net.capture = Some(vec![]);
    net.run(1_000);
    let corpus = net.capture.take().unwrap();
    let mut rng = 0x1234_5678_9ABC_DEFu64;
    let mut rand = move |n: usize| {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng % n.max(1) as u64) as usize
    };
    let bad = 10u64 + 1; // endpoint of member 10
    for round in 0..20_000 {
        let mut d = corpus[rand(corpus.len())].clone();
        match round % 6 {
            0 => {
                for _ in 0..1 + rand(4) {
                    let i = rand(d.len());
                    d[i] ^= 1 << rand(8);
                }
            }
            1 => d.truncate(rand(d.len() + 1)),
            2 => d.extend((0..rand(1400)).map(|_| rand(256) as u8)),
            3 => d[24] = rand(32) as u8,
            4 => d[16..24].copy_from_slice(&(1000 + rand(50) as u64).to_le_bytes()),
            _ => {
                let i = 29 + rand(d.len().saturating_sub(29).max(1));
                if i < d.len() {
                    d[i] = rand(256) as u8;
                }
            }
        }
        let to = rand(50);
        let from = if round % 3 == 0 { 9_999 } else { bad };
        if to != 10 {
            net.inject(from, to, &d);
        }
        if round % 200 == 0 {
            net.frame();
        }
    }
    net.run(6_000);
    assert!(net.converged(), "{}", net.report());
    assert_eq!(net.stale(1_500), 0);
}

/// Item 5: 50 players with home uploads (1 MB/s each) each bring their own outfit (8 kB, a typical customised
/// profile; 25 kB is a full one) into a full lobby at once. Every player gets all 49 looks, and nobody's movement
/// stalls meanwhile.
#[test]
fn fifty_outfits_reach_everyone_without_stalling_movement() {
    let mut net = Net::new();
    net.loss = 2;
    net.latency = (30, 90);
    net.uplink = 1_000_000;
    net.join(cluster(0), None);
    for i in 1..50 {
        net.join(cluster(i), Some(1));
    }
    settle(&mut net, 20_000).unwrap_or_else(|| panic!("{}", net.report()));
    net.run(2_000);
    let outfit = |i: usize| (0..8_000).map(|n| (n * 31 + i * 7) as u8).collect::<Vec<u8>>();
    for i in 0..50 {
        let local = net.s(i).local;
        assert!(net.s_mut(i).blobs.publish(local, outfit(i)));
    }
    let start = net.now();
    let (mut done, mut worst_stale) = (None, 0);
    while net.now() < start + 60_000 {
        net.run(250);
        worst_stale = worst_stale.max(net.stale(1_500));
        let all = (0..50).all(|i| (0..50).filter(|&j| j != i).all(|j| net.s(i).blobs.ready(1000 + j as u64).is_some_and(|(_, b)| b == outfit(j))));
        if all {
            done = Some(net.now() - start);
            break;
        }
    }
    eprintln!("50 outfits: everyone has all 49 after {done:?} ms; stale movement pairs at worst {worst_stale}; queue drops {}; host up {} kB", net.queue_drops, net.bytes(0).0 / 1000);
    let done = done.expect("every look arrives");
    assert!(done < 10_000, "{done} ms");
    assert_eq!(worst_stale, 0, "movement stalled behind the looks");
}

/// Mirrors online_roster: the event log diffs the roster only while connected to a host, and pauses otherwise.
fn log(tracker: &mut roster::Tracker, s: &Session, now: u64, lines: &mut Vec<roster::RosterEvent>) {
    if s.connected() {
        lines.extend(tracker.update(&roster::members(s), roster::session_state(s).as_ref(), now));
    } else {
        tracker.pause(now);
    }
}

/// I1 (end review 10-12): Make Host mid-session (Steam's owner change reaches each player over 0.5 s, the old host
/// too): the old host stays a member on every peer, with its name and its movement, and the log says "is now the
/// host". Then the new host really quits; where Steam's owner change outruns its GOODBYE (the next host included) the
/// GOODBYE still drops it at once, and the log says the host left, then who the new one is. A migration used to drop
/// the old host's actor everywhere until the new ROSTER: a blank name, which every match mode takes as a leave.
#[test]
fn make_host_keeps_the_old_host_and_a_real_quit_drops_it_at_once() {
    use roster::RosterEvent::*;
    let mut net = Net::new();
    net.loss = 2;
    net.latency = (30, 90);
    net.join(cluster(0), None);
    for i in 1..10 {
        net.join(cluster(i), Some(1));
    }
    settle(&mut net, 10_000).unwrap_or_else(|| panic!("{}", net.report()));
    let (mut tracker, mut lines) = (roster::Tracker::default(), vec![]);
    tracker.reset(net.now());
    for _ in 0..200 {
        net.frame();
        log(&mut tracker, net.s(5), net.now(), &mut lines);
    }
    assert_eq!(lines, vec![]);
    // Make Host: player 1 (endpoint 2).
    let at = net.now();
    let mut owner: Vec<(u64, usize)> = (0..10).map(|i| (at + 100 + (i as u64 * 53) % 500, i)).collect();
    owner.sort();
    let mut worst = 0;
    while net.now() < at + 4_000 {
        net.frame();
        let now = net.now();
        while owner.first().is_some_and(|(t, _)| *t <= now) {
            let (_, i) = owner.remove(0);
            net.s_mut(i).migrate(if i == 1 { None } else { Some(2) }, now);
        }
        for i in 1..10 {
            let s = net.s(i);
            assert!(
                roster::members(s).iter().any(|m| m.actor == 1000 && m.name == "Skater 0"),
                "player {i} lost the old host {} ms into the Make Host",
                now - at
            );
            let a = &s.actors[&1000];
            let heard = a.body.latest().map(|r| r.received).max(a.pose.latest().map(|r| r.received));
            worst = worst.max(heard.map_or(now - at, |t| now - t));
        }
        log(&mut tracker, net.s(5), now, &mut lines);
    }
    eprintln!("Make Host: the old host's longest movement gap {worst} ms");
    assert!(worst < 1_200, "{worst} ms");
    assert!((0..10).all(|i| net.s(i).host_actor() == Some(1001)));
    assert_eq!(lines, vec![AdminTransfer("Skater 1".into())]);
    lines.clear();
    // The new host quits. Half the players (player 2, the next host, among them) hear Steam's owner change first.
    // Lossless here: a lost GOODBYE falls back to the host's 5 s timeout (dead_players_are_dropped_everywhere).
    net.loss = 0;
    let at = net.now();
    for i in (2..10).step_by(2) {
        net.s_mut(i).migrate(if i == 2 { None } else { Some(3) }, at);
    }
    net.quit(1);
    let mut owner: Vec<(u64, usize)> = (3..10).step_by(2).map(|i| (at + 100 + (i as u64 * 53) % 500, i)).collect();
    owner.push((at + 300, 0));
    owner.sort();
    while net.now() < at + 6_000 {
        net.frame();
        let now = net.now();
        while owner.first().is_some_and(|(t, _)| *t <= now) {
            let (_, i) = owner.remove(0);
            net.s_mut(i).migrate(Some(3), now);
        }
        if now >= at + 200 {
            for i in net.live() {
                assert!(!net.s(i).actors.contains_key(&1001), "player {i} still holds the quitter {} ms after", now - at);
            }
        }
        log(&mut tracker, net.s(5), now, &mut lines);
    }
    assert!(net.converged(), "{}", net.report());
    assert!(net.live().iter().all(|&i| net.s(i).host_actor() == Some(1002)));
    assert_eq!(lines, vec![HostLeft, NewHost("Skater 2".into())]);
}

/// M2 (end review 10-12): Make Host and straight back, and one player's Steam poll never saw the owner change: it
/// still holds the old host's roster count. The host restarted its count at 0, so that player dropped every roster
/// until the count caught up (a newcomer never appeared for it).
#[test]
fn a_player_that_missed_a_host_round_trip_still_takes_the_rosters() {
    let mut net = Net::new();
    net.latency = (20, 40);
    net.join(cluster(0), None);
    for i in 1..8 {
        net.join(cluster(i), Some(1));
        net.run(400);
    }
    settle(&mut net, 5_000).unwrap_or_else(|| panic!("{}", net.report()));
    let missed = 5;
    for host in [1, 0] {
        let now = net.now();
        for i in (0..8).filter(|&i| i != missed) {
            net.s_mut(i).migrate(if i == host { None } else { Some(host as u64 + 1) }, now);
        }
        net.run(1_000);
    }
    net.join(cluster(8), Some(1));
    settle(&mut net, 5_000).unwrap_or_else(|| panic!("the newcomer never reached player {missed}: {}", net.report()));
}

/// M4 (end review 10-12): the host's datagrams stop reaching one player for 6 s (one-way), so it times the host out
/// and drops the host's actor. When they flow again, the host's records (its name) come back: the host still counted
/// them as delivered and never sent them again.
#[test]
fn a_player_that_timed_the_host_out_gets_its_records_back() {
    let mut net = Net::new();
    net.latency = (20, 40);
    net.join(cluster(0), None);
    for i in 1..5 {
        net.join(cluster(i), Some(1));
    }
    settle(&mut net, 5_000).unwrap_or_else(|| panic!("{}", net.report()));
    net.run(1_000);
    net.blocked = vec![(1, 3)];
    net.run(6_000);
    assert!(!net.s(2).actors.contains_key(&1000), "player 2 timed the host out");
    net.blocked.clear();
    settle(&mut net, 5_000).unwrap_or_else(|| panic!("the host's name never came back: {}", net.report()));
}

/// Online audit 10-24: a player's traffic stops reaching the others for 6 s (a Wi-Fi blip, a stalled upload; a game
/// hitch is the same to them), so the host times it out and everyone drops its actor, name and look. Its next HELLO
/// came straight back in, but it still counted its name and look as delivered and never sent them again: a nameless
/// "Skater" in the default look, missing from the Player List (no kick or mute), for the rest of the session. The mirror
/// (M4, the player times the host out) brought the host's name back but not its look.
#[test]
fn a_player_back_from_a_6_s_dropout_keeps_its_name_and_look() {
    let mut net = Net::new();
    net.latency = (20, 40);
    net.join(cluster(0), None);
    for i in 1..5 {
        net.join(cluster(i), Some(1));
    }
    let look = |seed: u32| (0..200_000u32).map(|i| (i * seed) as u8).collect::<Vec<u8>>();
    assert!(net.s_mut(0).blobs.publish(1000, look(7)));
    assert!(net.s_mut(2).blobs.publish(1002, look(11)));
    settle(&mut net, 5_000).unwrap_or_else(|| panic!("{}", net.report()));
    net.run(5_000);
    let looks = |net: &Net, actor: u64| net.live().iter().filter(|&&i| net.s(i).local != actor && net.s(i).blobs.ready(actor).is_some()).count();
    assert_eq!((looks(&net, 1000), looks(&net, 1002)), (4, 4));
    // Player 2's upload stops.
    net.blocked = (1..=5).filter(|&e| e != 3).map(|e| (3, e)).collect();
    net.run(6_000);
    assert!(!net.s(0).actors.contains_key(&1002), "the host timed player 2 out");
    net.blocked.clear();
    settle(&mut net, 5_000).unwrap_or_else(|| panic!("player 2's name never came back: {}", net.report()));
    net.run(5_000);
    assert_eq!(looks(&net, 1002), 4, "player 2's look came back");
    // M4's way round: the host's datagrams stop reaching player 2.
    net.blocked = vec![(1, 3)];
    net.run(6_000);
    assert!(!net.s(2).actors.contains_key(&1000), "player 2 timed the host out");
    net.blocked.clear();
    settle(&mut net, 5_000).unwrap_or_else(|| panic!("the host's name never came back: {}", net.report()));
    net.run(5_000);
    assert!(net.s(2).blobs.ready(1000).is_some(), "the host's look came back");
}

/// M5 (end review 10-12): members whose position is unknown (still loading: no movement yet) ranked at distance 0
/// and took full-rate BODY slots from skaters that really are near.
#[test]
fn the_nearest_eight_are_the_skaters_really_near() {
    let mut net = Net::new();
    net.latency = (20, 40);
    net.join([0., 0., 0.], None);
    for i in 1..=8 {
        net.join([i as f32 * 2., 0., 0.], Some(1));
    }
    for i in 0..3 {
        net.join([200. + i as f32 * 20., 0., 0.], Some(1));
    }
    for _ in 0..3 {
        let k = net.join([0.; 3], Some(1));
        net.silence(k);
    }
    settle(&mut net, 5_000).unwrap_or_else(|| panic!("{}", net.report()));
    net.run(1_000);
    let before = net.bodies.clone();
    net.run(2_000);
    for e in 2..=9u64 {
        let n = net.bodies.get(&(1, e)).copied().unwrap_or(0) - before.get(&(1, e)).copied().unwrap_or(0);
        assert!(n >= 30, "the host sent near player {} only {n} BODY in 2 s", e - 1);
    }
}

/// M7 (end review 10-12): a player's game restarts and joins again at once from the same endpoint. Everyone's send
/// state for that endpoint said "delivered" (the link was replaced inside one receive), so nobody sent it their look.
#[test]
fn a_quick_rejoin_from_the_same_endpoint_gets_every_look() {
    let mut net = Net::new();
    net.latency = (20, 40);
    net.join(cluster(0), None);
    for i in 1..5 {
        net.join(cluster(i), Some(1));
    }
    settle(&mut net, 5_000).unwrap_or_else(|| panic!("{}", net.report()));
    let outfit = |actor: u64| (0..3_000u64).map(|n| (n * 31 + actor) as u8).collect::<Vec<u8>>();
    let looks = |net: &Net| {
        let live = net.live();
        live.iter().all(|&i| {
            live.iter().filter(|&&j| j != i).all(|&j| {
                let id = net.s(j).local;
                net.s(i).blobs.ready(id).is_some_and(|(_, b)| b == outfit(id))
            })
        })
    };
    let wait = |net: &mut Net| {
        let start = net.now();
        while !looks(net) && net.now() < start + 10_000 {
            net.run(100);
        }
        looks(net)
    };
    for i in 0..5 {
        let id = net.s(i).local;
        assert!(net.s_mut(i).blobs.publish(id, outfit(id)));
    }
    assert!(wait(&mut net), "every look arrives");
    net.rejoin(3, 2003);
    assert!(net.s_mut(3).blobs.publish(2003, outfit(2003)));
    assert!(wait(&mut net), "the rejoined player has every look and everyone has its new one");
}
