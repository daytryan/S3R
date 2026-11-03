//! Voice chat's datagram ([`VOICE`]): sent peer to peer to every verified member like movement, never through the host.
//! The game's voice thread (skate-voice) sends and receives it off the game thread, through these snapshots of the
//! lobby's links ([`crate::lobby::Session::voice_route`], [`crate::lobby::Session::voice_listen`]).
use crate::packed;
use std::collections::{BTreeMap, VecDeque};

/// The first free kind: packed BODY 1, POSE 2; lobby 3-11; blob 12-14; AUDIO 15; CHAT 16-17; COOKIE 18.
pub const VOICE: u8 = 19;
/// A voice payload at most (`[frame_ms]` and one Opus packet).
const MAX_PAYLOAD: usize = 400;
/// Per sender, in any second: datagrams and bytes at most (a talker sends 50 a second, ~5-8 kB).
const RATE: usize = 60;
const BYTES: usize = 12_000;

/// Where our voice goes: verified links (endpoint, the token we stamp for it), less the excluded (blocked) ones.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoiceRoute {
    pub session: u64,
    pub local: u64,
    pub congested: bool,
    pub peers: Vec<(u64, u64)>,
}

/// Whose voice we take: verified links, endpoint → (its actor, our token for it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoiceListen {
    pub session: u64,
    pub peers: BTreeMap<u64, (u64, u64)>,
}

/// Our voice datagram for `payload`, one per peer, each stamped with that peer's token.
pub fn datagrams(route: &VoiceRoute, seq: u32, payload: &[u8]) -> Vec<(u64, Vec<u8>)> {
    route
        .peers
        .iter()
        .map(|&(peer, token)| {
            let mut data = packed::header(route.session, route.local, VOICE, seq);
            packed::stamp(&mut data, token);
            data.extend(payload);
            (peer, data)
        })
        .collect()
}

/// Receive caps per sender (online safety): what each took in the last second, (when, bytes).
#[derive(Default)]
pub struct Limits {
    senders: BTreeMap<u64, VecDeque<(u64, usize)>>,
}

impl Limits {
    /// Forgets the endpoints `listen` no longer has (they left).
    pub fn retain(&mut self, listen: &VoiceListen) {
        self.senders.retain(|endpoint, _| listen.peers.contains_key(endpoint));
    }
    pub fn len(&self) -> usize {
        self.senders.len()
    }
    fn allow(&mut self, from: u64, bytes: usize, now: u64) -> bool {
        let taken = self.senders.entry(from).or_default();
        while taken.front().is_some_and(|&(at, _)| now.saturating_sub(at) >= 1000) {
            taken.pop_front();
        }
        if taken.len() >= RATE || taken.iter().map(|t| t.1).sum::<usize>() + bytes > BYTES {
            return false;
        }
        taken.push_back((now, bytes));
        true
    }
}

/// A datagram from `from` that is voice for us, from that link's actor with our token for it, within its sender's caps:
/// (the actor, seq, payload). Anything else is dropped unread.
pub fn accept(listen: &VoiceListen, limits: &mut Limits, from: u64, data: &[u8], now_ms: u64) -> Option<(u64, u32, Vec<u8>)> {
    let (session, actor, kind, seq) = packed::envelope(data)?;
    let &(link_actor, token) = listen.peers.get(&from)?;
    let payload = &data[packed::HEADER..];
    (session == listen.session
        && kind == VOICE
        && actor == link_actor
        && packed::token(data) == token
        && payload.len() <= MAX_PAYLOAD
        && limits.allow(from, data.len(), now_ms))
    .then(|| (actor, seq, payload.to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        lobby::{
            Session,
            tests::{info, mesh},
        },
        packed,
    };

    /// A host (endpoint 1, actor 10) and two members (2 and 3: actors 11, 12), every link up.
    fn three() -> (Vec<Session>, u64) {
        let mut s: Vec<Session> = (0..3u64).map(|i| Session::new(7, info(10 + i), (i > 0).then_some(1))).collect();
        let mut now = 0;
        mesh(&mut s, &mut now, 20, &mut |_, _, _| false);
        assert!(s.iter().all(|x| x.actors.len() == 3), "a full mesh");
        (s, now)
    }

    /// Endpoint 2's voice datagram (seq 5, `payload`) to endpoint 1.
    fn from_two_to_one(s: &[Session], payload: &[u8]) -> Vec<u8> {
        datagrams(&s[1].voice_route(|_| false), 5, payload).into_iter().find(|(peer, _)| *peer == 1).unwrap().1
    }

    #[test]
    fn voice_reaches_every_verified_member() {
        let (s, now) = three();
        for (i, sender) in s.iter().enumerate() {
            let route = sender.voice_route(|_| false);
            assert_eq!(route.peers.len(), 2, "member {i} reaches both others");
            for (peer, data) in datagrams(&route, 5, b"\x14opus") {
                let receiver = &s[peer as usize - 1];
                let got = accept(&receiver.voice_listen(), &mut Limits::default(), i as u64 + 1, &data, now);
                assert_eq!(got, Some((10 + i as u64, 5, b"\x14opus".to_vec())), "{i} -> {peer}");
            }
        }
    }

    #[test]
    fn a_blocked_endpoint_gets_no_voice() {
        let (s, _) = three();
        let route = s[0].voice_route(|endpoint| endpoint == 2);
        assert_eq!(route.peers.iter().map(|p| p.0).collect::<Vec<_>>(), [3]);
    }

    #[test]
    fn forged_or_foreign_voice_is_dropped() {
        let (s, now) = three();
        let listen = s[0].voice_listen();
        let good = from_two_to_one(&s, b"\x14opus");
        assert!(accept(&listen, &mut Limits::default(), 2, &good, now).is_some());
        let changed = |at: std::ops::Range<usize>, value: u64| {
            let mut d = good.clone();
            d[at].copy_from_slice(&value.to_le_bytes());
            d
        };
        let cases = [
            ("wrong token", changed(29..37, packed::token(&good) ^ 1), 2),
            ("another member's actor on this link", changed(16..24, 12), 2),
            ("another session", changed(8..16, 8), 2),
            ("an endpoint that is no member", good.clone(), 99),
        ];
        for (name, data, from) in cases {
            assert_eq!(accept(&listen, &mut Limits::default(), from, &data, now), None, "{name}");
        }
        let mut chat = good.clone();
        chat[24] = crate::chat::CHAT;
        assert_eq!(accept(&listen, &mut Limits::default(), 2, &chat, now), None, "another kind");
        let big = from_two_to_one(&s, &[20; 401]);
        assert_eq!(accept(&listen, &mut Limits::default(), 2, &big, now), None, "a 401-byte payload");
        assert!(accept(&listen, &mut Limits::default(), 2, &from_two_to_one(&s, &[20; 400]), now).is_some(), "400 is fine");
    }

    #[test]
    fn a_flooding_sender_is_capped() {
        let (s, now) = three();
        let listen = s[0].voice_listen();
        let small = from_two_to_one(&s, b"\x14opus");
        let mut limits = Limits::default();
        let taken = (0..100).filter(|i| accept(&listen, &mut limits, 2, &small, now + 10 * i).is_some()).count();
        assert_eq!(taken, 60, "100 datagrams in a second: 60 taken");
        assert!(accept(&listen, &mut limits, 2, &small, now + 1_000).is_some(), "the next second has room again");
        let full = from_two_to_one(&s, &[20; 400]);
        let mut limits = Limits::default();
        let taken = (0..50).filter(|i| accept(&listen, &mut limits, 2, &full, now + 20 * i).is_some()).count();
        assert!(taken * full.len() <= 12_000 && taken * full.len() > 12_000 - full.len(), "{taken} full datagrams in a second");
    }

    #[test]
    fn limits_forget_endpoints_that_left() {
        let (mut s, mut now) = three();
        let mut limits = Limits::default();
        for (from, data) in [(2, from_two_to_one(&s, b"\x14opus")), (3, datagrams(&s[2].voice_route(|_| false), 5, b"\x14opus").into_iter().find(|p| p.0 == 1).unwrap().1)] {
            assert!(accept(&s[0].voice_listen(), &mut limits, from, &data, now).is_some());
        }
        assert_eq!(limits.len(), 2);
        // Member 3 leaves.
        for p in s[2].goodbye() {
            if p.peer <= 2 {
                s[p.peer as usize - 1].receive(3, &p.data, now);
            }
        }
        mesh(&mut s[..2], &mut now, 2, &mut |_, _, _| false);
        limits.retain(&s[0].voice_listen());
        assert_eq!(limits.len(), 1);
    }
}
