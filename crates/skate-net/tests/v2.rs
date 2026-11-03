use skate_net::{
    Body, Bone, Pose,
    lobby::{Info, Session},
    packed::{self, BodyState, Packed, PoseState},
};
fn pose(p: [f32; 3], t: f32) -> Pose {
    Pose {
        p,
        q: [0., (t / 2.).sin(), 0., (t / 2.).cos()],
    }
}
fn body() -> BodyState {
    BodyState {
        root: pose([330., 20., 100.], 0.),
        enabled: ((1 << 33) - 1) | (1 << 63),
        bodies: (0..33)
            .map(|i| Body {
                pose: pose(
                    [330. + i as f32 * 0.04567, 20.1234, 100.2345],
                    i as f32 * 0.23,
                ),
                velocity: [12.345, 0., -31.123],
                angular: [0.123, 24.789, 0.],
            })
            .collect(),
    }
}
fn info(id: u64) -> Info {
    Info {
        id,
        map: 1,
        rig: 2,
        physics: 3,
        appearance: 4,
    }
}
/// Twice: the HELLO, the host's cookie; the HELLO with it, admission and roster (SK8NET14).
fn exchange(host: &mut Session, guest: &mut Session, endpoint: u64, time: u64) {
    exchange_once(host, guest, endpoint, time);
    exchange_once(host, guest, endpoint, time);
}
fn exchange_once(host: &mut Session, guest: &mut Session, endpoint: u64, time: u64) {
    for p in guest.service(time) {
        host.receive(endpoint, &p.data, time);
    }
    for p in host.service(time) {
        if p.peer == endpoint {
            guest.receive(1, &p.data, time);
        }
    }
}
#[test]
fn compact_roundtrip_and_delta_survive_a_lost_update() {
    let original = body();
    let a = Packed::body(&original).unwrap();
    let wire = packed::delta(1, 2, packed::BODY, 1, &a, None);
    assert_eq!(wire.len(), 849); // perf 10-18: +8, the ACK a BODY carries (SK8NET15)
    let decoded = packed::apply(&wire, None).unwrap();
    for (a, b) in original
        .bodies
        .iter()
        .zip(decoded.unpack_body().unwrap().bodies)
    {
        for i in 0..3 {
            assert!((a.pose.p[i] - b.pose.p[i]).abs() < 0.00055);
            assert!((a.velocity[i] - b.velocity[i]).abs() < 0.02);
        }
        let dot = a
            .pose
            .q
            .iter()
            .zip(b.pose.q)
            .map(|(a, b)| a * b)
            .sum::<f32>()
            .abs()
            .min(1.);
        assert!(2. * dot.acos() < 0.006);
    }
    let mut moved = original.clone();
    moved.bodies[0].pose.p[0] += 0.02;
    let b = Packed::body(&moved).unwrap();
    let delta = packed::delta(1, 2, packed::BODY, 3, &b, Some((1, &a)));
    // perf 10-18 (SK8NET15, was <= 110): +8 ACK, +16 compact and mixed masks, the row 12 bytes not 23.
    assert!(delta.len() <= 123, "{}", delta.len());
    assert_eq!(packed::apply(&delta, Some(&decoded)), Some(b));
    assert!(packed::apply(&delta, None).is_none());
    for n in 0..wire.len() {
        assert!(packed::apply(&wire[..n], None).is_none());
    }
}
#[test]
fn maximum_offsets_and_pose_fit_one_datagram() {
    let mut b = body();
    for body in &mut b.bodies {
        body.pose.p[0] += 1000.;
    }
    let p = Packed::body(&b).unwrap();
    let wire = packed::delta(1, 2, packed::BODY, 1, &p, None);
    assert_eq!(wire.len(), 1047); // perf 10-18: +8, the ACK
    assert!(packed::apply(&wire, None).is_some());
    let state = PoseState {
        root: b.root,
        bones: (0..32)
            .map(|i| Bone {
                index: i,
                pose: pose([1000.; 3], i as f32),
            })
            .collect(),
    };
    let p = Packed::pose(&state).unwrap();
    let wire = packed::delta(1, 2, packed::POSE, 1, &p, None);
    assert!(wire.len() <= packed::MTU);
    assert!(packed::apply(&wire, None).is_some());
    b.bodies[0].velocity[0] = f32::NAN;
    assert!(Packed::body(&b).is_none());
}
#[test]
fn admission_full_lobby_mismatch_departure_and_rejoin() {
    let m = skate_net::lobby::MAX_PLAYERS as u64;
    let mut host = Session::new(1, info(1), None);
    for id in 2..=m {
        let mut guest = Session::new(1, info(id), Some(1));
        exchange(&mut host, &mut guest, id, id * 100);
        assert!(guest.connected());
    }
    assert_eq!(host.actors.len() as u64, m);
    let mut extra = Session::new(1, info(m + 1), Some(1));
    exchange(&mut host, &mut extra, m + 1, 1100);
    assert!(extra.notice.contains("full"));
    assert!(!extra.connected());
    let mut bad = info(m + 2);
    bad.physics = 99;
    let mut guest = Session::new(1, bad, Some(1));
    exchange(&mut host, &mut guest, m + 2, 1300);
    assert!(guest.notice.contains("full"));
    let mut bye = packed::header(1, 2, skate_net::lobby::GOODBYE, 0);
    host.stamp_from(2, &mut bye); // guest 2's own link token
    host.receive(2, &bye, 1400);
    assert_eq!(host.actors.len() as u64, m - 1);
    exchange(&mut host, &mut extra, m + 1, 1700);
    assert!(extra.connected());
    assert_eq!(host.actors.len() as u64, m);
    let mut restarted = Session::new(1, info(m + 12), Some(1));
    exchange(&mut host, &mut restarted, 3, 1800);
    assert!(!host.actors.contains_key(&3));
    assert!(host.actors.contains_key(&(m + 12)));
}
#[test]
fn movement_goes_peer_to_peer_and_the_host_forwards_none() {
    let mut host = Session::new(1, info(1), None);
    // Guest actor n sits at endpoint n; the host is endpoint 1.
    let mut guests: Vec<Session> = (2..=4).map(|id| Session::new(1, info(id), Some(1))).collect();
    let state = Packed::body(&body()).unwrap();
    let mut forwarded = 0;
    for step in 1..=30u64 {
        let now = step * 60;
        let mut mail: Vec<(u64, u64, Vec<u8>)> = host.service(now).into_iter().map(|o| (1, o.peer, o.data)).collect();
        for g in guests.iter_mut() {
            g.publish(packed::BODY, state.clone(), now);
            let from = g.local;
            mail.extend(g.service(now).into_iter().map(|o| (from, o.peer, o.data)));
        }
        for (from, to, data) in mail {
            let (_, actor, kind, _) = packed::envelope(&data).unwrap();
            forwarded += usize::from(from == 1 && kind == packed::BODY && actor != 1);
            match to {
                1 => host.receive(from, &data, now),
                n => guests[n as usize - 2].receive(from, &data, now),
            }
        }
    }
    for g in &guests {
        for id in (2..=4).filter(|&id| id != g.local) {
            assert!(g.actors[&id].body.latest().is_some(), "{} has {id}'s movement", g.local);
            assert_eq!(g.endpoint(id), Some(id), "{} links {id} directly", g.local);
        }
    }
    assert_eq!(forwarded, 0, "the host sends only its own movement");
}
#[test]
fn guests_cannot_spoof_other_actors_and_timeouts_remove_proxies() {
    let mut host = Session::new(1, info(1), None);
    let mut guest = Session::new(1, info(2), Some(1));
    exchange(&mut host, &mut guest, 2, 100);
    let p = Packed::body(&body()).unwrap();
    let mut forged = packed::delta(1, 1, packed::BODY, 1, &p, None);
    host.stamp_from(2, &mut forged); // guest 2's real link, another actor
    host.receive(2, &forged, 110);
    assert!(host.actors[&1].body.latest().is_none());
    host.receive(999, &packed::delta(1, 2, packed::BODY, 1, &p, None), 120);
    assert!(host.actors[&2].body.latest().is_none());
    host.service(6000);
    assert_eq!(host.actors.len(), 1);
    guest.service(6000);
    assert_eq!(guest.actors.len(), 1);
    assert!(!guest.connected());
}
#[test]
fn ten_sessions_exchange_production_packets_over_real_loopback_udp() {
    use std::{net::UdpSocket, time::Duration};
    let sockets: Vec<_> = (0..10)
        .map(|_| {
            let s = UdpSocket::bind("127.0.0.1:0").unwrap();
            s.set_nonblocking(true).unwrap();
            s
        })
        .collect();
    let addresses: Vec<_> = sockets.iter().map(|s| s.local_addr().unwrap()).collect();
    let mut sessions: Vec<_> = (0..10)
        .map(|i| Session::new(9, info(i + 1), (i != 0).then_some(1)))
        .collect();
    let packed = Packed::body(&body()).unwrap();
    let mut buf = [0; 1500];
    for time in (0..2500).step_by(10) {
        for (i, s) in sessions.iter_mut().enumerate() {
            while let Ok((n, from)) = sockets[i].recv_from(&mut buf) {
                let peer = addresses.iter().position(|&a| a == from).unwrap() as u64 + 1;
                s.receive(peer, &buf[..n], time);
            }
            if time % 50 == 0 {
                s.publish(packed::BODY, packed.clone(), time);
            }
            for p in s.service(time) {
                sockets[i]
                    .send_to(&p.data, addresses[p.peer as usize - 1])
                    .unwrap();
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    for s in &sessions {
        assert_eq!(s.actors.len(), 10);
        assert!(s.actors.values().all(|a| a.body.latest().is_some()));
        assert_eq!(s.stats.invalid + s.stats.baseline_miss, 0);
    }
}

#[test]
fn repeated_host_migration_preserves_local_state_and_rebuilds_ten_player_lobby() {
    let mut sessions: Vec<_> = (1..=10)
        .map(|id| Session::new(77, info(id), if id == 1 { None } else { Some(1) }))
        .collect();
    let state = Packed::body(&body()).unwrap();
    for s in &mut sessions {
        s.publish(packed::BODY, state.clone(), 50);
    }
    fn pump(sessions: &mut [Session], first: usize, begin: u64) {
        for now in (begin..begin + 2000).step_by(10) {
            let mut packets = Vec::new();
            for (i, s) in sessions.iter_mut().enumerate().skip(first) {
                for p in s.service(now) {
                    packets.push((i as u64 + 1, p));
                }
            }
            for (from, p) in packets {
                if p.peer as usize > first {
                    sessions[p.peer as usize - 1].receive(from, &p.data, now);
                }
            }
        }
    }
    pump(&mut sessions, 0, 100);
    assert!(sessions.iter().all(|s| s.actors.len() == 10));
    for first in 1..=2 {
        let now = 2200 * first as u64;
        // I1 (end review 10-12): the old host quits (its GOODBYE); a migration alone keeps it (Make Host).
        for p in sessions[first - 1].goodbye() {
            if p.peer as usize > first {
                sessions[p.peer as usize - 1].receive(first as u64, &p.data, now);
            }
        }
        for (i, s) in sessions.iter_mut().enumerate().skip(first) {
            let before = s.actors[&s.local].body.latest().unwrap().clone();
            s.migrate(
                if i == first {
                    None
                } else {
                    Some(first as u64 + 1)
                },
                now,
            );
            let after = s.actors[&s.local].body.latest().unwrap();
            assert_eq!(before.seq, after.seq);
            assert_eq!(before.state, after.state);
            // B2 (10-12 audit): a migration keeps every member but the departed host (movement is peer to peer);
            // it used to drop them all until the new host's roster.
            assert_eq!(s.actors.len(), 10 - first);
            assert!(!s.actors.contains_key(&(first as u64)));
        }
        // Late packets from the departed host must not restore its membership.
        let stale = packed::delta(77, 1, packed::BODY, 100, &state, None);
        for s in sessions.iter_mut().skip(first) {
            s.receive(1, &stale, now);
        }
        pump(&mut sessions, first, now);
        for s in sessions.iter().skip(first) {
            assert!(s.connected());
            assert_eq!(s.actors.len(), 10 - first);
            assert!(s.actors.values().all(|a| a.body.latest().is_some()));
            assert!(!s.actors.contains_key(&1));
        }
    }
}

#[test]
fn occupied_driver_flag_preserves_root_without_native_collision_parts() {
    let mut frame = body();
    frame.enabled = 1u64 << 62;
    frame.root = pose([10., 4., 20.], 0.4);
    let packed = Packed::body(&frame).unwrap();
    let bytes = packed::delta(1, 2, packed::BODY, 1, &packed, None);
    let decoded = packed::apply(&bytes, None).unwrap().unpack_body().unwrap();
    assert_eq!(decoded.enabled, 1u64 << 62);
    assert_eq!(decoded.root.p, frame.root.p);
    frame.enabled = 1u64 << 61;
    assert!(Packed::body(&frame).is_none());
}

#[test]
fn different_maps_physics_and_rigs_can_join_and_exchange_body_updates() {
    let mut host = Session::new(1, info(1), None);
    let mut other = info(2);
    other.map = 88;
    other.physics = 99;
    other.rig = 77;
    let mut guest = Session::new(1, other, Some(1));
    exchange(&mut host, &mut guest, 2, 100);
    assert!(guest.connected());
    assert_eq!(host.actors[&2].info.map, 88);
    host.publish(packed::BODY, Packed::body(&body()).unwrap(), 150);
    guest.publish(packed::BODY, Packed::body(&body()).unwrap(), 150);
    exchange(&mut host, &mut guest, 2, 200);
    assert!(host.actors[&2].body.latest().is_some());
    assert!(guest.actors[&1].body.latest().is_some());
}

#[test]
fn discovery_accepts_other_game_versions_but_not_unrelated_spacewar_lobbies() {
    assert!(skate_net::directory::is_game_lobby(
        "skate3rust-free-skate-v4"
    ));
    assert!(skate_net::directory::is_game_lobby(
        "skate3rust-free-skate-v5"
    ));
    assert!(skate_net::directory::is_game_lobby(
        "skate3rust-free-skate-v7"
    ));
    assert!(!skate_net::directory::is_game_lobby("another-game"));
}
/// One service round between a host and several guests: the host's packets go out once, each to its own peer.
fn round(host: &mut Session, guests: &mut [(u64, &mut Session)], time: u64) {
    for (endpoint, guest) in guests.iter_mut() {
        for p in guest.service(time) {
            host.receive(*endpoint, &p.data, time);
        }
    }
    for p in host.service(time) {
        if let Some((_, guest)) = guests.iter_mut().find(|(e, _)| *e == p.peer) {
            guest.receive(1, &p.data, time);
        }
    }
}
/// Item 29 (leak audit): a long-lived lobby where players keep joining and leaving keeps only who is there. The host
/// and a guest who stays all along end each round with the same actors (and no link to anyone gone), so 200 visits
/// leave nothing behind for the game's remote-skater entities to hang on to.
#[test]
fn players_joining_and_leaving_for_hours_leave_nothing_behind() {
    let mut host = Session::new(1, info(1), None);
    let mut stayer = Session::new(1, info(2), Some(1));
    exchange(&mut host, &mut stayer, 2, 100);
    assert!(stayer.connected());
    let mut now = 200;
    for visit in 0..200u64 {
        let id = 1000 + visit;
        let mut guest = Session::new(1, info(id), Some(1));
        for _ in 0..4 {
            now += 100;
            round(&mut host, &mut [(id, &mut guest), (2, &mut stayer)], now);
        }
        assert!(guest.connected() && host.actors.contains_key(&id) && stayer.actors.contains_key(&id), "visit {visit} joined");
        for p in guest.goodbye().into_iter().filter(|p| p.peer == 1) {
            host.receive(id, &p.data, now);
        }
        for _ in 0..4 {
            now += 100;
            round(&mut host, &mut [(2, &mut stayer)], now);
        }
        assert_eq!(host.actors.keys().copied().collect::<Vec<_>>(), [1, 2], "visit {visit}: the host forgot the visitor");
        assert_eq!(stayer.actors.keys().copied().collect::<Vec<_>>(), [1, 2], "visit {visit}: the stayer forgot the visitor");
        assert_eq!((host.endpoint(id), stayer.endpoint(id)), (None, None));
    }
    assert!(stayer.connected());
}
