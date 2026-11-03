//! Headless fake players for Direct sessions: bots FakeSkater1..N join (or host) a session and
//! orbit the first real player, with scripted leave/rejoin/privacy/kick events.
//! `fake_players [--count N=4] [--addr 127.0.0.1:31030] [--session 48031030] [--bot-host] [--seconds 120]`
//! `[--first K=0] [--ring N=count]`: run bots K+1..K+N in this process (split big tests across processes so
//! one thread is not the bottleneck), spaced on a ring of N. `[--names SEED]`: random gamer tags (pass every
//! process the same seed). `[--propose TAB:ID]`: bot 1 proposes that online-map row (TAB = the online map's tab
//! index, ID = a challenge id or location name) to the host 5 s after it sees another player, then every 45 s.
use skate_net::{
    Body, Bone, Pose,
    lobby::{Info, Session},
    packed::{self, BodyState, Packed, PoseState},
    roster::{self, PLAYER_KEY, PROPOSE_KEY, Player, Propose, SESSION_KEY, SessionState, Target, members},
};
use std::{
    collections::BTreeSet,
    net::{SocketAddr, SocketAddrV4, UdpSocket},
    time::{Duration, Instant},
};

const ORBIT_RADIUS: f32 = 6.;
const TAU: f32 = std::f32::consts::TAU;

/// Same encoding as the game's Direct transport (`multiplayer/transport.rs`).
fn endpoint(a: SocketAddrV4) -> u64 {
    ((u32::from(*a.ip()) as u64) << 16) | a.port() as u64
}
fn address(peer: u64) -> SocketAddrV4 {
    SocketAddrV4::new(std::net::Ipv4Addr::from((peer >> 16) as u32), peer as u16)
}

enum What {
    Join,
    Leave,
    Privacy(bool),
    Kick(usize),
}
struct Event {
    at: f32,
    bot: usize,
    what: What,
}
fn script(count: usize, bot_host: bool) -> Vec<Event> {
    let mut s: Vec<Event> = (0..count)
        .map(|bot| Event { at: 2. * bot as f32, bot, what: What::Join })
        .collect();
    if count > 1 {
        s.push(Event { at: 20., bot: 1, what: What::Leave });
        s.push(Event { at: 30., bot: 1, what: What::Join });
    }
    if bot_host {
        s.push(Event { at: 15., bot: 0, what: What::Privacy(true) });
        s.push(Event { at: 35., bot: 0, what: What::Privacy(false) });
        if count > 3 {
            s.push(Event { at: 25., bot: 0, what: What::Kick(3) });
        }
    }
    s.sort_by(|a, b| a.at.total_cmp(&b.at));
    s
}
fn orbit(center: [f32; 3], bot: usize, count: usize, t: f32) -> [f32; 3] {
    let a = t * 0.3 + bot as f32 * TAU / count as f32;
    [center[0] + a.cos() * ORBIT_RADIUS, center[1], center[2] + a.sin() * ORBIT_RADIUS]
}

// Synthetic state, as in lobby_audit.rs, re-rooted at `p`.
fn pose(p: [f32; 3], t: f32) -> Pose {
    Pose { p, q: [0., (t * 0.5).sin(), 0., (t * 0.5).cos()] }
}
fn state(p: [f32; 3], t: f32) -> (Packed, Packed) {
    let root = pose(p, t * 0.2);
    let bodies = (0..33)
        .map(|i| {
            let phase = t * 3. + i as f32;
            Body {
                pose: pose(
                    [root.p[0] + phase.sin() * 0.2, root.p[1] + i as f32 * 0.03, root.p[2] + phase.cos() * 0.1],
                    phase * 0.1,
                ),
                velocity: [3. + phase.cos(), 0., phase.sin()],
                angular: [0., phase.cos(), 0.],
            }
        })
        .collect();
    let bones = (0..30)
        .map(|i| Bone {
            index: i,
            pose: pose([0., i as f32 * 0.03, (t * 3. + i as f32).sin() * 0.1], t + i as f32),
        })
        .collect();
    (
        Packed::body(&BodyState { root, enabled: (1 << 33) - 1, bodies }).unwrap(),
        Packed::pose(&PoseState { root, bones }).unwrap(),
    )
}

struct Bot {
    name: String,
    live: Option<(UdpSocket, Session)>,
    joins: u64,
    done: bool,
}
struct Opts {
    count: usize,
    addr: SocketAddrV4,
    session: u64,
    bot_host: bool,
    first: usize,
    ring: usize,
    seconds: f32,
    names: Option<u64>,
    propose: Option<(usize, String)>,
}
fn opts() -> Opts {
    let mut o = Opts {
        count: 4,
        addr: "127.0.0.1:31030".parse().unwrap(),
        session: 48031030,
        bot_host: false,
        first: 0,
        ring: 0,
        seconds: 120.,
        names: None,
        propose: None,
    };
    let mut a = std::env::args().skip(1);
    while let Some(k) = a.next() {
        match k.as_str() {
            "--count" => o.count = a.next().and_then(|v| v.parse().ok()).expect("--count N"),
            "--addr" => o.addr = a.next().and_then(|v| v.parse().ok()).expect("--addr ipv4:port"),
            "--session" => o.session = a.next().and_then(|v| v.parse().ok()).expect("--session N"),
            "--seconds" => o.seconds = a.next().and_then(|v| v.parse().ok()).expect("--seconds N"),
            "--bot-host" => o.bot_host = true,
            "--first" => o.first = a.next().and_then(|v| v.parse().ok()).expect("--first K"),
            "--ring" => o.ring = a.next().and_then(|v| v.parse().ok()).expect("--ring N"),
            "--names" => o.names = Some(a.next().and_then(|v| v.parse().ok()).expect("--names SEED")),
            "--propose" => {
                let v = a.next().expect("--propose TAB:ID");
                let (tab, id) = v.split_once(':').expect("--propose TAB:ID");
                o.propose = Some((tab.parse().expect("--propose TAB:ID"), id.to_owned()));
            }
            _ => panic!("unknown argument {k}"),
        }
    }
    assert!(o.count >= 1, "--count must be at least 1");
    o
}

/// `--names SEED`: a gamer tag for bot `index` (1-based) instead of FakeSkaterN, the same in every process given
/// the same seed. Mixed lengths, so the roster's cut names show too.
fn tag(seed: u64, index: usize) -> String {
    const FIRST: [&str; 24] = [
        "Sk8", "Shred", "Kick", "Grind", "Rail", "Ollie", "Nollie", "Heel", "Flip", "Deck", "Truck", "Street", "Vert",
        "Bowl", "Ramp", "Curb", "Gap", "Manny", "Slappy", "Bomb", "Hill", "Pool", "Ledge", "Coping",
    ];
    const SECOND: [&str; 20] = [
        "Lord", "King", "Rat", "Wizard", "Demon", "Kid", "Master", "God", "Junkie", "Head", "Bandit", "Ninja", "Ghost",
        "Tank", "Goblin", "Queen", "Boss", "Dog", "Legend", "Wolf",
    ];
    const SHORT: [&str; 8] = ["Jo", "Max", "Sam", "Riz", "Kai", "Bo", "Lu", "Tex"];
    // splitmix64 over (seed, index).
    let mut x = seed ^ (index as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    let mut pick = |n: usize| {
        x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        ((z ^ (z >> 31)) % n as u64) as usize
    };
    if pick(8) == 0 {
        return SHORT[pick(SHORT.len())].to_owned();
    }
    let name = format!("{}{}", FIRST[pick(FIRST.len())], SECOND[pick(SECOND.len())]);
    match pick(6) {
        0 => format!("xX_{name}_Xx"),
        1 => format!("{name}{}", pick(1000)),
        2 => name.to_lowercase(),
        3 => format!("TheReal{name}"),
        _ => name,
    }
}

/// The first remote real player's root, or the origin. `bots`: the --names tags of every process's bots.
fn target(s: &Session, bots: &BTreeSet<String>) -> [f32; 3] {
    s.actors
        .iter()
        .filter(|(id, _)| **id != s.local)
        .filter(|(_, a)| {
            a.application
                .get(PLAYER_KEY)
                .and_then(|r| Player::decode(&r.value))
                .is_some_and(|p| !p.name.starts_with("FakeSkater") && !bots.contains(&p.name))
        })
        // POSE: every member gets it often; BODY only reaches a player's nearest few at full rate.
        .find_map(|(_, a)| a.pose.latest()?.state.unpack_pose().map(|p| p.root.p))
        .unwrap_or([0.; 3])
}

fn main() {
    let o = opts();
    let host_ep = endpoint(o.addr);
    let events = script(o.count, o.bot_host);
    let mut next = 0;
    let name = |i: usize| o.names.map_or_else(|| format!("FakeSkater{i}"), |seed| tag(seed, i));
    // ponytail: bot indices past 1000 would count as real players.
    let tags: BTreeSet<String> = o.names.map(|seed| (1..=1000).map(|i| tag(seed, i)).collect()).unwrap_or_default();
    let mut bots: Vec<Bot> =
        (0..o.count).map(|i| Bot { name: name(o.first + i + 1), live: None, joins: 0, done: false }).collect();
    let mut host_state = SessionState::default();
    let (mut last_body, mut last_pose) = (0u64, 0u64);
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let start = Instant::now();
    let log = |t: f32, msg: String| println!("t={t:.1} {msg}");
    // Every 5 s: how long a loop over all bots takes, and how often bot 1 sees the real player move
    // (each bot orbits that position, so a slow target makes every bot jump).
    let (mut loops, mut loop_max, mut loop_sum, mut target_moves, mut last_target) = (0u32, 0f32, 0f32, 0u32, [0f32; 3]);
    // Bot 1's upload at the last report.
    let mut sent = 0u64;
    let mut report = 5.;
    let mut propose_at: Option<f32> = None;
    loop {
        let tick = Instant::now();
        let t = start.elapsed().as_secs_f32();
        if t >= report {
            let (mut pose, mut body, mut up, mut remotes) = (0u64, 0u64, 0u64, 0u64);
            if let Some((_, s)) = &bots[0].live {
                // Updates that arrived in the last 2 s (history holds 64, enough at 20/s).
                let recent = |h: &std::collections::VecDeque<skate_net::lobby::Revision>| {
                    h.iter().filter(|r| r.received + 2000 > (t * 1000.) as u64).count() as u64
                };
                for (_, a) in s.actors.iter().filter(|(id, _)| **id != s.local) {
                    pose += recent(&a.pose.history);
                    body += recent(&a.body.history);
                    remotes += 1;
                }
                up = s.stats.tx_bytes;
            }
            let per = |n: u64| n as f32 / 2. / remotes.max(1) as f32;
            log(t, format!("loop avg {:.1} ms max {:.1} ms | target updates {:.1}/s | bot 1 sees {} players: pose {:.1}/s body {:.1}/s each, uploads {:.0} kB/s",
                loop_sum / loops.max(1) as f32, loop_max, target_moves as f32 / 5., remotes,
                per(pose), per(body), up.saturating_sub(sent) as f32 / 5000.));
            sent = up;
            (loops, loop_max, loop_sum, target_moves, report) = (0, 0., 0., 0, report + 5.);
        }
        let now = start.elapsed().as_millis() as u64;
        if t >= o.seconds {
            break;
        }
        while next < events.len() && events[next].at <= t {
            let e = &events[next];
            next += 1;
            let i = e.bot;
            if bots[i].done {
                continue;
            }
            match e.what {
                What::Join => {
                    let hosting = o.bot_host && i == 0;
                    let bind: SocketAddr = if hosting { o.addr.into() } else { "127.0.0.1:0".parse().unwrap() };
                    let socket = match UdpSocket::bind(bind) {
                        Ok(s) => s,
                        Err(err) => {
                            eprintln!("{}: bind {bind}: {err}", bots[i].name);
                            continue;
                        }
                    };
                    skate_net::socket::configure(&socket).expect("configure socket");
                    let id = 0xFA4E_0000_0000 + ((o.first + i) as u64) * 100 + bots[i].joins;
                    let info = Info { id, map: 0, rig: 0, physics: 0, appearance: 0 };
                    let mut s = Session::new(o.session, info, (!hosting).then_some(host_ep));
                    s.publish_application(
                        PLAYER_KEY,
                        Player { name: bots[i].name.clone(), steam: 0 }.encode(),
                        now,
                    );
                    if hosting {
                        s.publish_application(SESSION_KEY, host_state.encode(), now);
                    }
                    let word = if bots[i].joins == 0 { "joined" } else { "rejoined" };
                    bots[i].joins += 1;
                    bots[i].live = Some((socket, s));
                    log(t, format!("{} {word}", bots[i].name));
                }
                What::Leave => {
                    if let Some((socket, s)) = bots[i].live.take() {
                        for p in s.goodbye() {
                            let _ = socket.send_to(&p.data, address(p.peer));
                        }
                        log(t, format!("{} left", bots[i].name));
                    }
                }
                What::Privacy(private) => {
                    if let Some((_, s)) = bots[i].live.as_mut() {
                        host_state.private = private;
                        s.publish_application(SESSION_KEY, host_state.encode(), now);
                        log(t, format!("{} private={private}", bots[i].name));
                    }
                }
                What::Kick(k) => {
                    let victim = bots[k].live.as_ref().map(|(_, s)| s.local);
                    if let (Some((_, s)), Some(actor)) = (bots[i].live.as_mut(), victim) {
                        if roster::kick(s, &mut host_state, actor, false).is_some() {
                            s.publish_application(SESSION_KEY, host_state.encode(), now);
                            log(t, format!("{} kick {}", bots[i].name, bots[k].name));
                        }
                    }
                }
            }
        }
        let (do_body, do_pose) = (now >= last_body + 50, now >= last_pose + 100);
        if do_body {
            last_body = now;
        }
        if do_pose {
            last_pose = now;
        }
        for i in 0..bots.len() {
            let Some((socket, s)) = bots[i].live.as_mut() else { continue };
            let mut buf = [0u8; 1500];
            while let Ok((n, from)) = socket.recv_from(&mut buf) {
                if let SocketAddr::V4(a) = from {
                    s.receive(endpoint(a), &buf[..n], now);
                }
            }
            if s.kicked {
                log(t, format!("{} kicked", bots[i].name));
                bots[i].live = None;
                bots[i].done = true;
                continue;
            }
            if do_body || do_pose {
                let (body, pose) = state(orbit(target(s, &tags), o.first + i, o.ring.max(o.count), t), t);
                if do_body {
                    s.publish(packed::BODY, body, now);
                }
                if do_pose {
                    s.publish(packed::POSE, pose, now);
                }
            }
            for p in s.service(now) {
                let sent = socket.send_to(&p.data, address(p.peer)).is_ok();
                s.record_send(p.data.len(), sent);
            }
            if i == 0 {
                let p = target(s, &tags);
                if p != last_target {
                    target_moves += 1;
                    last_target = p;
                }
                let names: BTreeSet<String> = members(s).into_iter().map(|m| m.name).collect();
                // Before the logs below: they borrow `bots`, so `s` must be done with by then.
                if let Some((tab, id)) = &o.propose {
                    if propose_at.is_none() && names.len() >= 2 {
                        propose_at = Some(t + 5.);  // after the host has seen us: it ignores records present at first sight
                    }
                    if propose_at.is_some_and(|at| t >= at) {
                        let nonce = 0xB0B0_0000 + (t * 10.) as u64;
                        s.publish_application(PROPOSE_KEY, Propose { nonce, target: Target::new(*tab, id, "", "") }.encode(), now);
                        log(t, format!("bot 1 proposes {tab}:{id}"));
                        propose_at = Some(t + 45.);
                    }
                }
                for n in names.difference(&seen) {
                    log(t, format!("{} sees {n} joined", bots[0].name));
                }
                for n in seen.difference(&names) {
                    log(t, format!("{} sees {n} left", bots[0].name));
                }
                seen = names;
            }
        }
        let ms = tick.elapsed().as_secs_f32() * 1000.;
        (loops, loop_max, loop_sum) = (loops + 1, loop_max.max(ms), loop_sum + ms);
        std::thread::sleep(Duration::from_millis(16));
    }
    for b in &bots {
        if let Some((socket, s)) = &b.live {
            for p in s.goodbye() {
                let _ = socket.send_to(&p.data, address(p.peer));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn endpoints_match_the_game_direct_transport() {
        assert_eq!(endpoint("127.0.0.1:31030".parse().unwrap()), (0x7f00_0001u64 << 16) | 31030);
        assert_eq!(address(endpoint("10.1.2.3:99".parse().unwrap())), "10.1.2.3:99".parse::<SocketAddrV4>().unwrap());
    }
    #[test]
    fn script_staggers_joins_and_scripts_leave_rejoin() {
        let s = script(4, false);
        // The first four Joins are the staggered starts (bot 1 also rejoins at 30 s).
        let starts: Vec<f32> = s.iter().filter(|e| matches!(e.what, What::Join) && e.at < 10.).map(|e| e.at).collect();
        assert_eq!(starts, [0., 2., 4., 6.]);
        assert!(s.iter().any(|e| e.bot == 1 && matches!(e.what, What::Leave) && e.at == 20.));
        assert!(s.iter().any(|e| e.bot == 1 && matches!(e.what, What::Join) && e.at == 30.));
        let h = script(4, true);
        assert!(h.iter().any(|e| e.bot == 0 && matches!(e.what, What::Privacy(true)) && e.at == 15.));
        assert!(h.iter().any(|e| e.bot == 0 && matches!(e.what, What::Kick(3)) && e.at == 25.));
    }
    #[test]
    fn orbit_stays_around_the_target() {
        let p = orbit([10., 2., -5.], 1, 4, 3.);
        let d = ((p[0] - 10.).powi(2) + (p[2] + 5.).powi(2)).sqrt();
        assert!((d - ORBIT_RADIUS).abs() < 1e-3 && p[1] == 2.);
    }
}
