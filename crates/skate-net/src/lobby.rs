//! Transport-neutral lobby of up to MAX_PLAYERS. The host admits players and relays the roster and
//! application records; movement (BODY/POSE) and looks (blobs) are peer-to-peer: each actor sends its own
//! streams straight to every member the roster lists. No platform SDK or wall clock.
use crate::packed::{self, BODY, HEADER, POSE, Packed, Reader};
use std::collections::{BTreeMap, VecDeque};
/// Protocol ceiling. A Steam lobby's own size (directory::Tags::capacity) sets each mode's limit:
/// freeskate fills this, challenges keep retail's 6.
pub const MAX_PLAYERS: usize = 50;
/// Roster entries per ROSTER packet: [total][start] + 48 bytes (info, endpoint) each stays under packed::MTU.
const ROSTER_PAGE: usize = 24;
/// Members that get our physics (BODY) at full rate: the nearest ones, who can touch us. The rest get it
/// once a second and draw us from POSE.
const BODY_NEIGHBOURS: usize = 8;
/// Audit 10-28 #8: a member of our nearest set ranks at this share of its distance, so a newcomer must be clearly closer
/// to take its place. A dozen members at one spot swapped in and out every service, their BODY rate jumping from 25 a
/// second to 1.
const NEAR_STICK: f32 = 0.75;
/// B3 (10-12 audit): members that get our animation (POSE) at its full rate; the rest of a crowd within 30 m get it
/// about 3 times a second. 50 players at one spot (a join, a session move) took ~360 kB/s of every player's upload,
/// past a weak 250 kB/s line; a lobby of up to 9 is unchanged.
const POSE_NEIGHBOURS: usize = BODY_NEIGHBOURS;
const POSE_CROWD_MS: u64 = 333;
/// ACKs to members outside our nearest BODY_NEIGHBOURS (their BODY reaches us once a second).
const FAR_ACK_MS: u64 = 250;
/// Members within this many metres of us get our skater audio records ([`crate::audio`]), the nearest
/// [`BODY_NEIGHBOURS`] of them. UNTRACED knob: retail's player sync plays a record only within 30 m of the listener
/// (`sub_824F8EF8`, 30.0 at 0x820D4924); the other 20 m cover a camera away from its skater.
pub const AUDIO_RANGE: f32 = 50.;
/// A membership change waits this long after the last ROSTER, so a join storm is one roster, not one per joiner.
const ROSTER_COALESCE: u64 = 250;
/// A member whose HELLO has not acknowledged the current ROSTER gets it again after this many ms.
const ROSTER_RESEND: u64 = 500;
/// Audit 10-28 #12: members a new ROSTER goes to in one service, and the ms between those batches (joiners first). A
/// change in a full lobby sent ~170 kB at once, past a home upload's share for movement.
const ROSTER_FANOUT: usize = 8;
const ROSTER_SPACING: u64 = 50;
/// Audit 10-28 #12: records' share of our upload in bytes/s, and how much of it may bunch up. A new host sends every
/// member's records again to everyone (up to ~1 MB in a full lobby), which took the whole session budget for a second.
/// End review: room for a busy match's events too (a 6-player match relays ~6 kB an event: ~60 a second here).
const RECORD_RATE: f64 = 400_000.;
const RECORD_BANK: f64 = 16_000.;
/// A chat line not acknowledged is sent again after this many ms.
const CHAT_RESEND: u64 = 250;
/// Audio frames kept per actor until the game takes them.
const AUDIO_QUEUE: usize = 32;
pub const HELLO: u8 = 3;
pub const ROSTER: u8 = 4;
pub const ACK: u8 = 5;
pub const PING: u8 = 6;
pub const PONG: u8 = 7;
pub const REJECT: u8 = 8;
pub const GOODBYE: u8 = 9;
pub const APPLICATION: u8 = 10;
pub const APPLICATION_ACK: u8 = 11;
pub const MAX_APP_KEYS: usize = 256;
pub const MAX_APP_VALUE: usize = 1024;
/// REJECT code sent to a peer the host kicked.
pub const REJECT_KICKED: u8 = 4;
/// Host to a HELLO without its cookie: `[the host's token for that endpoint u64]`, header token the HELLO's own. Smaller
/// than the HELLO it answers, and stateless (online safety Q1).
pub const COOKIE: u8 = 18;
/// Probes (PINGs to links that never answered) per second, all links together (online safety P1).
const PROBE_RATE: f64 = 20.;
/// COOKIE replies per second: a forged-HELLO flood reflects at most this many 45-byte datagrams.
const COOKIE_RATE: f64 = 100.;
/// Datagrams per second one link gets decoded; the rest are dropped unread (online safety 3.7). A look downloads at
/// up to ~4 MB/s from one sender.
const LINK_RX_RATE: f64 = 4000.;
/// A link nobody has heard from this long is dropped (the host's member, a member's host) or, member to member, gets
/// probes only until it answers. Retail kicks a silent player at its mRcvTimeOut, the online config's "timeoutMS"
/// (default 5000 ms, Player::Update 0x82DD04E0; .superpowers/sdd/2026-10-26-voice/retail-timeouts.md).
pub const LINK_TIMEOUT_MS: u64 = 5000;
/// Direct sessions (not on one PC): members per IP address (C6, 3.13: one PC cannot fill the lobby through ports).
const MAX_PER_IP: usize = 4;
#[derive(Clone, Debug)]
pub struct Application {
    pub seq: u32,
    pub value: Vec<u8>,
    pub received: u64,
}

pub const DEFAULT_LINK_BUDGET: f64 = 180_000.;
pub const DEFAULT_HOST_BUDGET: f64 = 1_000_000.;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info {
    pub id: u64,
    pub map: u64,
    pub rig: u64,
    pub physics: u64,
    pub appearance: u64,
}
#[derive(Clone, Debug)]
pub struct Revision {
    pub seq: u32,
    pub state: Packed,
    pub received: u64,
    interest: Vec<[f32; 3]>,
}
#[derive(Default)]
pub struct Stream {
    pub history: VecDeque<Revision>,
    pub changed: u64,
}
impl Stream {
    pub fn latest(&self) -> Option<&Revision> {
        self.history.back()
    }
    fn at(&self, seq: u32) -> Option<&Packed> {
        self.history.iter().find(|r| r.seq == seq).map(|r| &r.state)
    }
    fn push(&mut self, seq: u32, state: Packed, now: u64) {
        if self.latest().is_none_or(|r| !r.state.same_state(&state)) {
            self.changed = now;
        }
        let mut interest = vec![state.position()];
        if state.rows.len() == 33 {
            // perf 10-18: 7 positions, not a whole BODY decoded again (apply checked it).
            interest.extend(state.body_points(7));
        } else {
            // A POSE: its camera second ([`Actor::camera`]).
            interest.extend(state.camera());
        }
        self.history.push_back(Revision {
            interest,
            seq,
            state,
            received: now,
        });
        while self.history.len() > 64 {
            self.history.pop_front();
        }
    }
}
pub struct Actor {
    pub info: Info,
    pub body: Stream,
    pub pose: Stream,
    pub application: BTreeMap<String, Application>,
    /// Its audio frames (tick, record), oldest first, not yet taken by the game.
    pub audio: VecDeque<(u32, crate::audio::Record)>,
    audio_tick: Option<u32>,
    /// Spectating: the camera of the player its `@watch` asks for ([`Session::refresh_eyes`]).
    eye: Option<[f32; 3]>,
}
impl Actor {
    fn new(info: Info) -> Self {
        Self {
            info,
            body: Stream::default(),
            pose: Stream::default(),
            application: BTreeMap::new(),
            audio: VecDeque::new(),
            audio_tick: None,
            eye: None,
        }
    }
    pub fn stream(&self, kind: u8) -> &Stream {
        if kind == BODY { &self.body } else { &self.pose }
    }
    /// Where it watches from: a spectator's eye, by the player it watches however far off its own skater stands (user
    /// 10-24, build 10), else the camera its newest POSE carries.
    fn camera(&self) -> Option<[f32; 3]> {
        self.eye.or_else(|| self.own_camera())
    }
    /// The camera its newest POSE carries: its own skater's gameplay camera (an older build's spectator: the one it shows).
    fn own_camera(&self) -> Option<[f32; 3]> {
        self.pose.latest()?.interest.get(1).copied()
    }
    /// The player its `@watch` asks to show ([`crate::roster::WATCH_KEY`]'s first 8 bytes).
    fn asked(&self) -> Option<u64> {
        let value = &self.application.get(crate::roster::WATCH_KEY)?.value;
        Some(u64::from_le_bytes(value.get(..8)?.try_into().ok()?))
    }
    fn stream_mut(&mut self, kind: u8) -> &mut Stream {
        if kind == BODY {
            &mut self.body
        } else {
            &mut self.pose
        }
    }
}
struct Link {
    actor: u64,
    seen: u64,
    acks: BTreeMap<(u64, u8), u32>,
    sent: BTreeMap<(u64, u8), (u64, u32)>,
    keys: BTreeMap<(u64, u8), u64>,
    credits: f64,
    /// perf 10-18: by actor, then key, so a lookup clones no key.
    app_sent: BTreeMap<u64, BTreeMap<String, (u64, u32)>>,
    app_acks: BTreeMap<u64, BTreeMap<String, u32>>,
    app_round: usize,
    last_ack: u64,
    /// perf 10-18: the [BODY, POSE] sequences of this member's streams we last acknowledged to it.
    acked: [u32; 2],
    last_ping: u64,
    rtt: u64,
    /// Our chat lines (sequence, text) this member has not acknowledged, oldest first ([`crate::chat`]).
    chat_out: VecDeque<(u32, String)>,
    /// When `chat_out` was last sent (0: at once).
    chat_sent: u64,
    /// The last line taken in order from this link's actor.
    chat_in: u32,
    chat_limit: crate::chat::Limiter,
    /// Host only: the ROSTER this member's latest HELLO says it holds, and (seq, when) we last sent it one.
    roster_ack: u32,
    roster_sent: (u32, u64),
    /// The token this peer gave us (HELLO, COOKIE, PING, PONG); every datagram we send it carries it (3.4).
    theirs: u64,
    /// It sent us a datagram carrying our token, so it really receives at this endpoint. Until then it gets probes
    /// (PING) only, never movement, looks, audio or chat (P1, P9).
    verified: bool,
    /// When it was created or went silent: probes slow down after 10 s.
    probing: u64,
    /// Receive token bucket ([`LINK_RX_RATE`]) and when it was last filled.
    rx: (f64, u64),
    /// When `chat_in` last moved, and how often `chat_out` went out unacknowledged (resends back off: C10).
    chat_at: u64,
    chat_tries: u32,
}
impl Link {
    fn new(actor: u64, now: u64) -> Self {
        Self {
            theirs: 0,
            verified: false,
            probing: now,
            rx: (LINK_RX_RATE, now),
            chat_at: now,
            chat_tries: 0,
            actor,
            seen: now,
            acks: BTreeMap::new(),
            sent: BTreeMap::new(),
            keys: BTreeMap::new(),
            credits: 2400.,
            app_sent: BTreeMap::new(),
            app_acks: BTreeMap::new(),
            app_round: 0,
            last_ack: 0,
            acked: [0; 2],
            last_ping: 0,
            rtt: 0,
            chat_out: VecDeque::new(),
            chat_sent: 0,
            chat_in: 0,
            chat_limit: Default::default(),
            roster_ack: 0,
            roster_sent: (0, 0),
        }
    }
}
#[derive(Default, Clone, Debug)]
pub struct Stats {
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub decoded: u64,
    pub late: u64,
    pub baseline_miss: u64,
    pub invalid: u64,
    pub budget_skips: u64,
    pub send_errors: u64,
    pub rtt_ms: u64,
}
pub struct Outgoing {
    pub peer: u64,
    pub data: Vec<u8>,
}
pub struct Session {
    pub local: u64,
    pub session: u64,
    pub actors: BTreeMap<u64, Actor>,
    pub stats: Stats,
    pub notice: String,
    /// True on a client after the host sent REJECT_KICKED.
    pub kicked: bool,
    /// A client's other REJECT code (3: lobby full); the session will never admit us.
    pub rejected: Option<u8>,
    /// A connected client lost its host (GOODBYE, or 5 s of silence); cleared by the next
    /// ROSTER or `migrate`. Steam re-elects a host; a Direct session is over.
    pub host_lost: bool,
    /// The SK8NET version of the first datagram from another build ([`packed::other_version`]); `notice` said so.
    pub mismatch: Option<[u8; 2]>,
    /// Endpoints the host kicked ([`Self::ban_key`]); survives `migrate`. Direct sessions ban the IP address (3.13: a
    /// new port no longer gets back in; a household behind one IP goes with it). Steam endpoints are SteamIDs. A new
    /// host seeds it with `ban` from the last `@session` it saw (roster::seed_bans). Builds older than REJECT_KICKED
    /// speak an older SK8NET version, which `packed::envelope` refuses, so they are never admitted.
    banned: std::collections::BTreeSet<u64>,
    /// Endpoints are Direct `ip << 16 | port` (multiplayer/transport.rs), not SteamIDs.
    direct: bool,
    /// Keys our link tokens (`token`): random per session, never sent.
    secret: [u8; 32],
    probe_credits: f64,
    cookie_credits: f64,
    /// Actors that said GOODBYE: a ROSTER or HELLO still in flight never brings them back (actor ids are per session).
    // ponytail: grows by one per leave for the session's life; a few kB after thousands of leaves.
    departed: std::collections::BTreeSet<u64>,
    host: Option<u64>,
    links: BTreeMap<u64, Link>,
    pending: Vec<Outgoing>,
    last_hello: u64,
    last_roster: u64,
    /// Host only: when the last ROSTER_FANOUT batch went (0: none yet).
    roster_batch: u64,
    /// Our nearest BODY_NEIGHBOURS members at the last service ([`nearest`]).
    near: Vec<u64>,
    /// Bytes records may still send now ([`RECORD_RATE`]).
    record_credits: f64,
    /// Host only: membership changed since the last ROSTER.
    roster_dirty: bool,
    roster_seq: u32,
    received_roster: u32,
    /// A ROSTER being assembled from its pages: (seq, infos so far).
    roster_pages: (u32, Vec<Option<(Info, u64)>>),
    last_service: u64,
    credits: f64,
    budget: f64,
    link_budget: f64,
    /// Steam is queueing our sends (the relay's STATS, [`Self::set_congested`]). Looks don't read it (a stale sample
    /// must not hold them back: tests/appearance_bandwidth.rs); voice does.
    pub(crate) congested: bool,
    round: usize,
    loopback: bool,
    pub blobs: crate::blob::Blobs,
    /// Chat lines from other members (sender actor, text), oldest first, until the game takes them.
    pub chat: VecDeque<(u64, String)>,
    /// The last chat line's sequence: one counter for the whole session, so every link's lines rise.
    chat_seq: u32,
    chat_limit: crate::chat::Limiter,
    /// perf 10-18: a record changed or was acknowledged since the last record pass, and when that pass ran.
    app_dirty: bool,
    last_app: u64,
}
impl Session {
    pub fn new(session: u64, info: Info, host: Option<u64>) -> Self {
        let mut actors = BTreeMap::new();
        actors.insert(info.id, Actor::new(info));
        let mut links = BTreeMap::new();
        if let Some(peer) = host {
            links.insert(peer, Link::new(0, 0));
        }
        Self {
            direct: false,
            secret: secret(),
            probe_credits: PROBE_RATE,
            cookie_credits: COOKIE_RATE,
            local: info.id,
            session,
            actors,
            host,
            links,
            stats: Stats::default(),
            notice: String::new(),
            kicked: false,
            rejected: None,
            host_lost: false,
            mismatch: None,
            banned: Default::default(),
            departed: Default::default(),
            pending: vec![],
            last_hello: 0,
            last_roster: 0,
            roster_batch: 0,
            near: vec![],
            record_credits: RECORD_BANK,
            roster_dirty: false,
            roster_seq: 0,
            received_roster: 0,
            roster_pages: (0, vec![]),
            last_service: 0,
            credits: 12_000.,
            // Everyone sends its own movement to every member, so every session gets the full budget.
            budget: DEFAULT_HOST_BUDGET,
            link_budget: DEFAULT_LINK_BUDGET,
            congested: false,
            round: 0,
            loopback: false,
            blobs: Default::default(),
            chat: VecDeque::new(),
            chat_seq: 0,
            chat_limit: Default::default(),
            app_dirty: false,
            last_app: 0,
        }
    }
    /// An external membership authority chooses the new endpoint. Preserve our
    /// actor/capture history; rebuild membership and recipient delta baselines.
    ///
    /// Not retail (user 10-12): B2 + I1 (10-12 audit/end review): movement is peer to peer, so every member and mesh
    /// link we hold stays through the hand-over, the old host's too (after Make Host it is just a member); a member
    /// that really left goes by its GOODBYE or the timeouts, like any leave.
    pub fn migrate(&mut self, host: Option<u64>, now: u64) {
        self.host = host;
        if let Some(peer) = host {
            let link = self.links.entry(peer).or_insert_with(|| Link::new(0, now));
            link.seen = now;
            // Records go to the host only: it gets ours all again.
            link.app_sent.clear();
            link.app_acks.clear();
        }
        for link in self.links.values_mut() {
            link.seen = link.seen.max(now);
            link.roster_ack = 0;
            link.roster_sent = (0, 0);
        }
        self.pending.clear();
        // M2 (end review 10-12): the count keeps rising across hosts, so a player whose Steam poll missed an owner
        // change still takes the next host's rosters.
        self.roster_seq = self.roster_seq.max(self.received_roster);
        self.received_roster = 0;
        self.host_lost = false;
        self.last_hello = 0;
        self.last_roster = 0;
        self.roster_pages = (0, vec![]);
        // A new host tells everyone who is in at once.
        self.roster_dirty = host.is_none();
        self.last_service = now;
        self.budget = DEFAULT_HOST_BUDGET;
        self.credits = 12_000.;
        self.notice = if host.is_none() {
            String::new()
        } else {
            "Host changed; reconnecting players...".into()
        };
    }
    /// Host only: drop `actor`, ban its endpoint for this session, tell it why. Returns the
    /// banned endpoint (a SteamID in Steam sessions), which is the only trustworthy identity.
    pub fn kick(&mut self, actor: u64) -> Option<u64> {
        if !self.is_host() || actor == self.local {
            return None;
        }
        let peer = self.endpoint(actor)?;
        let theirs = self.links.remove(&peer).map_or(0, |l| l.theirs);
        self.actors.remove(&actor);
        self.banned.insert(self.ban_key(peer));
        let mut b = packed::header(self.session, self.local, REJECT, 0);
        b.push(REJECT_KICKED);
        packed::stamp(&mut b, theirs);
        self.queue(peer, b);
        self.roster_dirty = true;
        Some(peer)
    }
    /// What a ban holds: a Direct endpoint's IP address (any port), else the endpoint (a SteamID).
    fn ban_key(&self, peer: u64) -> u64 {
        if self.direct { peer >> 16 } else { peer }
    }
    /// Tests and tools: `data` carries the token we gave `peer`, as a datagram from its real link would.
    pub fn stamp_from(&self, peer: u64, data: &mut [u8]) {
        packed::stamp(data, self.token(peer));
    }
    /// Endpoints are Direct `ip << 16 | port`: bans and the members-per-IP cap go by the IP.
    pub fn set_direct(&mut self, direct: bool) {
        self.direct = direct;
    }
    /// Our token for `peer`'s link: the datagrams it sends us carry it. Keyed by our secret, so nobody who has not
    /// received it from us (at that endpoint) can make one; never 0.
    fn token(&self, peer: u64) -> u64 {
        let h = blake3::keyed_hash(&self.secret, &peer.to_le_bytes());
        u64::from_le_bytes(h.as_bytes()[..8].try_into().unwrap()).max(1)
    }
    /// The endpoint (a SteamID in Steam sessions) of the link behind `actor`. Hosts have a link
    /// per member; clients only to the host. Unlike `@player`, it cannot be spoofed.
    pub fn endpoint(&self, actor: u64) -> Option<u64> {
        self.links
            .iter()
            .find(|(_, l)| l.actor == actor)
            .map(|(&p, _)| p)
    }
    /// Voice chat (the game's voice thread): where our voice goes. Verified links, less `exclude` (an endpoint: the
    /// game's blocked players; Steam endpoints are SteamIDs), each with the token it gave us.
    pub fn voice_route(&self, exclude: impl Fn(u64) -> bool) -> crate::voice::VoiceRoute {
        let peers = self.links.iter().filter(|&(&p, l)| l.verified && !exclude(p)).map(|(&p, l)| (p, l.theirs)).collect();
        crate::voice::VoiceRoute { session: self.session, local: self.local, congested: self.congested, peers }
    }
    /// Voice chat: whose voice we take. Verified links, with their actor and our token for them.
    pub fn voice_listen(&self) -> crate::voice::VoiceListen {
        let peers = self.links.iter().filter(|(_, l)| l.verified).map(|(&p, l)| (p, (l.actor, self.token(p)))).collect();
        crate::voice::VoiceListen { session: self.session, peers }
    }
    /// Ban an endpoint without a live link (a new host re-applying earlier kicks).
    pub fn ban(&mut self, peer: u64) {
        self.banned.insert(self.ban_key(peer));
    }
    pub fn is_banned(&self, peer: u64) -> bool {
        self.banned.contains(&self.ban_key(peer))
    }
    /// The host's actor id: ours when hosting, else the actor behind the host link (once its ROSTER arrived).
    /// Audit 10-28 #7: our link to `actor` answers (or it is us). A member whose link went silent stays on the host's
    /// roster but sends us nothing, so the game hides its skater until the link answers again.
    pub fn linked(&self, actor: u64) -> bool {
        actor == self.local || self.links.values().any(|l| l.actor == actor && l.verified)
    }
    pub fn host_actor(&self) -> Option<u64> {
        match self.host {
            None => Some(self.local),
            Some(peer) => self.links.get(&peer).map(|l| l.actor).filter(|&a| a != 0),
        }
    }
    pub fn set_loopback(&mut self, enabled: bool) {
        self.loopback = enabled;
    }
    pub fn is_host(&self) -> bool {
        self.host.is_none()
    }
    pub fn connected(&self) -> bool {
        self.is_host() || self.received_roster != 0
    }
    pub fn set_congested(&mut self, congested: bool) {
        self.congested = congested;
        self.link_budget = if congested {
            DEFAULT_LINK_BUDGET * 0.5
        } else {
            DEFAULT_LINK_BUDGET
        };
    }
    pub fn publish(&mut self, kind: u8, mut state: Packed, now: u64) {
        state.captured = now;
        let stream = self.actors.get_mut(&self.local).unwrap().stream_mut(kind);
        let seq = stream.latest().map_or(1, |r| {
            r.seq.checked_add(1).expect("Session sequence exhausted")
        });
        stream.push(seq, state, now);
    }
    /// Latest owner-authenticated state. Empty values are persistent tombstones.
    /// Repeated full records recover loss and initialize late joiners without replaying Lua.
    pub fn publish_application(&mut self, key: &str, value: Vec<u8>, now: u64) -> bool {
        if key.is_empty() || key.len() > 128 || value.len() > MAX_APP_VALUE {
            return false;
        }
        let records = &mut self.actors.get_mut(&self.local).unwrap().application;
        if records.len() >= MAX_APP_KEYS && !records.contains_key(key) {
            return false;
        }
        if records.get(key).is_some_and(|r| r.value == value) {
            return true;
        }
        let seq = records.get(key).map_or(1, |r| r.seq.saturating_add(1));
        records.insert(
            key.into(),
            Application {
                seq,
                value,
                received: now,
            },
        );
        self.app_dirty = true;
        true
    }
    /// Spectators' eyes (audit 10-30): a member whose `@watch` asks for another watches from that one's camera, not by
    /// its own parked skater, so the players beside the one it watches come to it at their near rates and are heard
    /// ([`apart`]). Its own camera rows stay its own skater's, what a member watching it sees: retail's spectate camera
    /// follows the skater picked.
    fn refresh_eyes(&mut self) {
        let cameras: BTreeMap<u64, [f32; 3]> = self.actors.iter().filter_map(|(&id, a)| Some((id, a.own_camera()?))).collect();
        for (&id, actor) in &mut self.actors {
            actor.eye = actor.asked().filter(|&watched| watched != id).and_then(|watched| cameras.get(&watched).copied());
        }
    }
    /// Queues our latest audio frames (oldest first) to the nearest [`BODY_NEIGHBOURS`] members within
    /// [`AUDIO_RANGE`], and to the nearest [`POSE_NEIGHBOURS`] members watching us wherever they are; sent with the next
    /// [`Self::service`]. Other members whose position we do not know get none.
    pub fn send_audio(&mut self, frames: &[(u32, crate::audio::Record)]) {
        self.refresh_eyes();
        let me = &self.actors[&self.local];
        let mut near: Vec<(f32, u64, bool)> = self
            .links
            .iter()
            .filter(|(_, l)| l.verified)
            .filter_map(|(&p, l)| {
                let them = self.actors.get(&l.actor).filter(|_| l.actor != self.local)?;
                let (d, watcher) = (apart(me, them).unwrap_or(f32::MAX), watches(them, self.local));
                (watcher || d < AUDIO_RANGE).then_some((d, p, watcher))
            })
            .collect();
        if near.is_empty() {
            return;
        }
        near.sort_by(|a, b| a.0.total_cmp(&b.0));
        // Up to POSE_NEIGHBOURS members watching us, wherever they stand (their camera is by us, user 10-25), and the
        // BODY_NEIGHBOURS nearest of the rest.
        let peers: Vec<u64> = near.iter().filter(|n| n.2).take(POSE_NEIGHBOURS).chain(near.iter().filter(|n| !n.2).take(BODY_NEIGHBOURS)).map(|n| n.1).collect();
        let payload = crate::audio::pack(frames, packed::MTU - HEADER);
        let Some(&(tick, _)) = frames.last().filter(|_| !payload.is_empty()) else {
            return;
        };
        let mut b = packed::header(self.session, self.local, crate::audio::AUDIO, tick);
        b.extend(payload);
        for peer in peers {
            let bytes = b.len() as f64;
            let link = self.links.get_mut(&peer).unwrap();
            if link.credits < bytes || self.credits < bytes {
                self.stats.budget_skips += 1;
                continue;
            }
            link.credits -= bytes;
            self.credits -= bytes;
            self.queue(peer, b.clone());
        }
    }
    /// Not retail (user 10-11): queues a chat line for every member; the cleaned line (to show as ours), or None when
    /// it is empty or too long once cleaned, or past the rate ([`crate::chat`]).
    pub fn say(&mut self, text: &str, now: u64) -> Option<String> {
        let text = crate::chat::clean(text)?;
        if !self.chat_limit.allow(now) {
            return None;
        }
        self.chat_seq = self.chat_seq.wrapping_add(1);
        for link in self.links.values_mut() {
            link.chat_out.push_back((self.chat_seq, text.clone()));
            // A member that never acknowledges (gone, an older build) holds at most a window's worth.
            while link.chat_out.len() > 2 * crate::chat::RATE {
                link.chat_out.pop_front();
            }
            link.chat_sent = 0;
            link.chat_tries = 0;
        }
        Some(text)
    }
    pub fn record_send(&mut self, len: usize, success: bool) {
        if success {
            self.stats.tx_bytes += len as u64;
        } else {
            self.stats.send_errors += 1;
        }
    }
    fn queue(&mut self, peer: u64, data: Vec<u8>) {
        if self.pending.len() < 4 * MAX_PLAYERS && data.len() <= packed::MTU {
            self.pending.push(Outgoing { peer, data });
        }
    }
    fn local_info(&self) -> Info {
        self.actors[&self.local].info
    }
    pub fn receive(&mut self, peer: u64, data: &[u8], now: u64) {
        self.stats.rx_bytes += data.len() as u64;
        let Some((session, actor, kind, seq)) = packed::envelope(data) else {
            self.stats.invalid += 1;
            // M7: say it once; neither side can read the other, so the session would just never connect.
            if self.mismatch.is_none()
                && let Some(v) = packed::other_version(data)
            {
                self.mismatch = Some(v);
                let (theirs, ours) = (String::from_utf8_lossy(&v), String::from_utf8_lossy(&packed::MAGIC[6..]));
                self.notice = format!("Version mismatch: a player runs network version {theirs}, this build {ours}. Both need the same build.");
            }
            return;
        };
        if session != self.session || actor == 0 {
            return;
        }
        // 3.7: a link's datagrams past LINK_RX_RATE are dropped before anything is decoded.
        if let Some(link) = self.links.get_mut(&peer) {
            let (credits, at) = link.rx;
            let credits = (credits + now.saturating_sub(at).min(1000) as f64 / 1000. * LINK_RX_RATE).min(LINK_RX_RATE);
            if credits < 1. {
                link.rx = (credits, now);
                self.stats.invalid += 1;
                return;
            }
            link.rx = (credits - 1., now);
        }
        // 3.4: only a datagram carrying the token we gave this endpoint is from whoever we gave it to.
        let authed = packed::token(data) == self.token(peer);
        let mut r = Reader(&data[HEADER..]);
        if kind == HELLO && self.is_host() {
            let (Some(info), Some(roster_ack), Some(theirs)) = (read_info(&mut r), r.u32(), r.u64()) else {
                return;
            };
            if !r.0.is_empty() || info.id != actor || actor == self.local || theirs == 0 || self.departed.contains(&actor) {
                return;
            }
            let cookie = |s: &mut Self| {
                if s.cookie_credits >= 1. {
                    s.cookie_credits -= 1.;
                    let mut b = packed::header(s.session, s.local, COOKIE, 0);
                    b.extend(s.token(peer).to_le_bytes());
                    packed::stamp(&mut b, theirs);
                    s.queue(peer, b);
                }
            };
            // Q1: nothing is admitted, and no roster sent, until the endpoint shows it receives (our cookie).
            // Online audit 10-24: no link, yet it holds our roster: we timed it out (a hitch, a stalled upload) and it
            // still counts its records and look as delivered. The cookie has it send them all again and HELLO afresh.
            if !authed || (roster_ack != 0 && !self.links.contains_key(&peer)) {
                cookie(self);
                return;
            }
            let reject = |s: &mut Self, code: u8| {
                let mut b = packed::header(s.session, s.local, REJECT, 0);
                b.push(code);
                packed::stamp(&mut b, theirs);
                s.queue(peer, b);
            };
            if self.is_banned(peer) {
                reject(self, REJECT_KICKED);
                return;
            }
            // Map, rig and physics fingerprints describe peers; they do not gate admission.
            let new = !self.links.contains_key(&peer);
            let same_ip = self.links.keys().filter(|&&p| p >> 16 == peer >> 16).count();
            if new && (self.links.len() >= MAX_PLAYERS - 1 || (self.direct && !self.loopback && same_ip >= MAX_PER_IP)) {
                reject(self, 3);
                return;
            }
            if self
                .links
                .iter()
                .any(|(&endpoint, l)| endpoint != peer && l.actor == actor)
            {
                return;
            }
            let old = self.links.get(&peer).map(|l| l.actor);
            if old != Some(actor) {
                if let Some(old) = old {
                    self.actors.remove(&old);
                }
                self.links.insert(peer, Link::new(actor, now));
                self.actors.insert(actor, Actor::new(info));
                self.roster_dirty = true;
            }
            let link = self.links.get_mut(&peer).unwrap();
            link.seen = now;
            link.theirs = theirs;
            link.verified = true;
            // M4 (end review 10-12): it timed us out (dropped our actor and its records): send them all again.
            let forgot = roster_ack == 0 && link.roster_ack != 0;
            if forgot {
                link.app_sent.clear();
                link.app_acks.clear();
            }
            link.roster_ack = roster_ack;
            if forgot {
                self.blobs.forget(Some(peer)); // and our look (audit 10-24)
            }
            self.notice.clear();
            return;
        }
        if self.host.is_some_and(|h| h != peer)
            && !matches!(
                kind,
                BODY | POSE
                    | ACK
                    | PING
                    | PONG
                    | crate::audio::AUDIO
                    | crate::chat::CHAT
                    | crate::chat::CHAT_ACK
                    | crate::blob::META
                    | crate::blob::DATA
                    | crate::blob::ACK
                    | GOODBYE
            )
        {
            return;
        }
        if !self.links.contains_key(&peer) {
            return;
        }
        let token = self.token(peer);
        let link = self.links.get_mut(&peer).unwrap();
        // The link handshake: each side's PING or PONG hands the other its token; one carrying ours back verifies.
        if kind == PING || kind == PONG {
            let (Some(stamp), Some(theirs)) = (r.u64(), r.u64()) else {
                return;
            };
            if !r.0.is_empty() || theirs == 0 {
                return;
            }
            // Only an authenticated one may replace a token the link already relies on (a forged one would cut it).
            if authed || !link.verified {
                link.theirs = theirs;
            }
            if authed {
                link.verified = true;
                link.seen = now;
            }
            if kind == PING {
                // As big as the PING: answering never amplifies.
                let mut b = packed::header(self.session, self.local, PONG, 0);
                b.extend(stamp.to_le_bytes());
                b.extend(token.to_le_bytes());
                self.queue(peer, b);
            } else if authed && stamp <= now && now - stamp <= 5000 {
                link.rtt = now - stamp;
            }
            return;
        }
        if kind == COOKIE {
            if let (true, Some(theirs)) = (authed && self.host == Some(peer), r.u64().filter(|&t| t != 0 && r.0.is_empty())) {
                link.theirs = theirs;
                link.verified = true;
                link.seen = now;
                // Audit 10-24: when already in, the host timed us out and has nothing of ours (nor have the others):
                // our records and look go again, and the HELLO comes afresh, without a roster.
                link.app_sent.clear();
                link.app_acks.clear();
                self.received_roster = 0;
                self.blobs.forget(None);
                // HELLO again at once, now with the cookie.
                self.last_hello = 0;
            }
            return;
        }
        // ponytail: a REJECT before the host's COOKIE arrived cannot carry our token; it is taken unauthenticated then,
        // so an off-path forger racing a join can still fake "Lobby full" or "Kicked" for that join.
        if kind == REJECT && !authed && (link.verified || self.host != Some(peer)) {
            return;
        }
        if kind != REJECT && !authed {
            self.stats.invalid += 1;
            return;
        }
        link.verified |= authed;
        if kind == ROSTER && self.host == Some(peer) {
            if seq <= self.received_roster {
                return;
            }
            let (Some(total), Some(start)) = (r.byte(), r.byte()) else {
                return;
            };
            let (total, start) = (total as usize, start as usize);
            if total == 0 || total > MAX_PLAYERS || start % ROSTER_PAGE != 0 || start >= total {
                return;
            }
            let mut page = vec![];
            while !r.0.is_empty() {
                let (Some(info), Some(endpoint)) = (read_info(&mut r), r.u64()) else {
                    return;
                };
                page.push((info, endpoint));
            }
            if page.len() != ROSTER_PAGE.min(total - start) {
                return;
            }
            // An older roster's late page never resets a newer one being assembled.
            if seq < self.roster_pages.0 {
                return;
            }
            if self.roster_pages.0 != seq || self.roster_pages.1.len() != total {
                self.roster_pages = (seq, vec![None; total]);
            }
            for (i, info) in page.into_iter().enumerate() {
                self.roster_pages.1[start + i] = Some(info);
            }
            let Some(entries) = self.roster_pages.1.iter().copied().collect::<Option<Vec<(Info, u64)>>>() else {
                return;
            };
            let unique = entries
                .iter()
                .enumerate()
                .all(|(n, (i, e))| i.id != 0 && entries[..n].iter().all(|(j, f)| j.id != i.id && (*e == 0 || e != f)));
            // P1: two members at one endpoint is a host aiming our traffic; a whole roster like that is refused.
            // Audit 10-24: so is one whose host is us (a modified host speaking as us).
            if !unique || actor == self.local || entries[0].0.id != actor || !entries.iter().any(|(i, _)| i.id == self.local) {
                return;
            }
            let entries: Vec<(Info, u64)> = entries.into_iter().filter(|(i, _)| i.id == actor || !self.departed.contains(&i.id)).collect();
            // A link to every other member (endpoint as the host sees it); the host link stays.
            self.links.retain(|&p, l| p == peer || entries.iter().any(|(i, e)| i.id == l.actor && *e == p));
            for &(info, endpoint) in &entries[1..] {
                if info.id != self.local && endpoint != 0 && endpoint != peer {
                    self.links.entry(endpoint).or_insert_with(|| Link::new(info.id, now));
                }
            }
            let infos: Vec<Info> = entries.into_iter().map(|(i, _)| i).collect();
            self.received_roster = seq;
            self.host_lost = false;
            self.rejected = None;
            self.links.get_mut(&peer).unwrap().actor = actor;
            self.links.get_mut(&peer).unwrap().seen = now;
            self.actors
                .retain(|id, _| infos.iter().any(|i| i.id == *id));
            for info in infos {
                self.actors
                    .entry(info.id)
                    .or_insert_with(|| Actor::new(info));
            }
            self.notice.clear();
            return;
        }
        if kind == REJECT && self.host == Some(peer) {
            let code = r.byte();
            self.kicked |= code == Some(REJECT_KICKED);
            if code != Some(REJECT_KICKED) {
                self.rejected = Some(code.unwrap_or(0));
            }
            self.notice = match code {
                Some(1) => "Map mismatch",
                Some(2) => "Physics definition mismatch",
                Some(3) => "Lobby full",
                Some(REJECT_KICKED) => "Kicked by the host",
                _ => "Lobby rejected connection",
            }
            .into();
            return;
        }
        if matches!(
            kind,
            crate::blob::META | crate::blob::DATA | crate::blob::ACK
        ) {
            // B4 (10-12 audit): a look comes from its owner's own link only, like movement (peer to peer).
            let valid = self.links[&peer].actor == actor && (kind == crate::blob::ACK || (actor != self.local && self.actors.contains_key(&actor)));
            if valid {
                self.blobs.receive(peer, actor, kind, seq, &data[HEADER..], now);
                self.links.get_mut(&peer).unwrap().seen = now;
            }
            return;
        }
        if self.links[&peer].actor != actor && !matches!(kind, BODY | POSE | APPLICATION) {
            return;
        }
        if kind == APPLICATION_ACK {
            let Some(origin) = r.u64() else {
                return;
            };
            let Ok(key) = std::str::from_utf8(r.0) else {
                return;
            };
            if self
                .actors
                .get(&origin)
                .and_then(|a| a.application.get(key))
                .is_some_and(|record| seq <= record.seq)
            {
                let link = self.links.get_mut(&peer).unwrap();
                let ack = link.app_acks.entry(origin).or_default().entry(key.into()).or_default();
                *ack = (*ack).max(seq);
                link.seen = now;
                self.app_dirty = true;
            }
            return;
        }
        if kind == APPLICATION {
            if actor == self.local || (self.is_host() && self.links[&peer].actor != actor) {
                return;
            }
            let Some(a) = self.actors.get_mut(&actor) else {
                return;
            };
            let Some(length) = r.byte() else {
                return;
            };
            let length = length as usize;
            if length == 0
                || length > 128
                || r.0.len() < length
                || r.0.len() - length > MAX_APP_VALUE
            {
                return;
            }
            let Ok(key) = std::str::from_utf8(&r.0[..length]) else {
                return;
            };
            if a.application.len() >= MAX_APP_KEYS && !a.application.contains_key(key) {
                return;
            }
            if a.application.get(key).is_none_or(|old| seq > old.seq) {
                a.application.insert(
                    key.into(),
                    Application {
                        seq,
                        value: r.0[length..].to_vec(),
                        received: now,
                    },
                );
            }
            let latest = a.application.get(key).map_or(seq, |r| r.seq);
            self.app_dirty = true;
            let mut ack = packed::header(self.session, self.local, APPLICATION_ACK, latest);
            ack.extend(actor.to_le_bytes());
            ack.extend(key.as_bytes());
            self.queue(peer, ack);
            self.links.get_mut(&peer).unwrap().seen = now;
            return;
        }
        if kind == crate::audio::AUDIO {
            // Its owner's own link only (checked above), like movement.
            let Some(a) = self.actors.get_mut(&actor).filter(|_| actor != self.local) else {
                return;
            };
            let Some(frames) = crate::audio::decode(r.0) else {
                self.stats.invalid += 1;
                return;
            };
            for (tick, record) in frames {
                // Repeats (the frames a datagram resends) are dropped; a tick far behind is a sender that restarted.
                if a.audio_tick.is_none_or(|t| tick > t || t - tick > 600) {
                    a.audio.push_back((tick, record));
                    a.audio_tick = Some(tick);
                }
            }
            while a.audio.len() > AUDIO_QUEUE {
                a.audio.pop_front();
            }
            self.links.get_mut(&peer).unwrap().seen = now;
            return;
        }
        // Item 32. Its sender's own link only (checked above): the sender is who the link is, never the text.
        if kind == crate::chat::CHAT {
            let Some(base) = r.u32() else {
                return;
            };
            let link = self.links.get_mut(&peer).unwrap();
            // C2: past its first line a sender's lines rise at most at chat::RATE; one far ahead would mute the real
            // ones that follow (they would look like repeats).
            let room = 4 * crate::chat::RATE as u64 + now.saturating_sub(link.chat_at) * crate::chat::RATE as u64 / crate::chat::WINDOW_MS;
            if base > seq || (link.chat_in != 0 && u64::from(seq.saturating_sub(link.chat_in)) > room) {
                self.stats.invalid += 1;
                return;
            }
            link.seen = now;
            // The lines before `base` were taken by this member's earlier link (or never meant for it).
            link.chat_in = link.chat_in.max(base.saturating_sub(1));
            // In order: anything else is a repeat (acknowledged again) or ahead of a lost line (resent).
            if seq == link.chat_in.wrapping_add(1) {
                link.chat_in = seq;
                link.chat_at = now;
                let text = std::str::from_utf8(r.0).ok().and_then(crate::chat::valid);
                if let Some(text) = text.filter(|_| link.chat_limit.allow_up_to(2 * crate::chat::RATE, now)) {
                    self.chat.push_back((actor, text.to_owned()));
                    while self.chat.len() > 4 * crate::chat::RATE {
                        self.chat.pop_front();
                    }
                }
            }
            let ack = packed::header(self.session, self.local, crate::chat::CHAT_ACK, link.chat_in);
            self.queue(peer, ack);
            return;
        }
        if kind == crate::chat::CHAT_ACK {
            // C2: never above the last line we sent (it would drop lines it never got).
            if seq > self.chat_seq {
                return;
            }
            let link = self.links.get_mut(&peer).unwrap();
            link.seen = now;
            let before = link.chat_out.len();
            link.chat_out.retain(|&(s, _)| s > seq);
            if link.chat_out.len() < before {
                link.chat_tries = 0;
            }
            return;
        }
        if kind == GOODBYE {
            // Not retail (user 10-12), I1: a leaver says GOODBYE on every link, so each peer drops it at once (also
            // when Steam's owner change outran it); the members stay while Steam elects a new host (B2), and a
            // Direct session ends (online_roster::end_lines).
            // Audit 10-24: never our own skater (a modified host speaking as us crashed the next service).
            if actor == self.local {
                return;
            }
            self.links.remove(&peer);
            self.actors.remove(&actor);
            self.departed.insert(actor);
            if self.is_host() {
                self.roster_dirty = true;
            } else if self.host == Some(peer) {
                self.received_roster = 0;
                self.host_lost = true;
                self.notice = "Host left. Join another lobby.".into();
            }
            return;
        }
        if kind == ACK {
            let Some(count) = r.byte() else {
                return;
            };
            if count as usize > MAX_PLAYERS {
                return;
            }
            let mut ack = vec![];
            for _ in 0..count {
                let (Some(id), Some(body), Some(pose)) = (r.u64(), r.u32(), r.u32()) else {
                    return;
                };
                ack.push((id, body, pose));
            }
            if !r.0.is_empty() {
                return;
            }
            let link = self.links.get_mut(&peer).unwrap();
            link.seen = now;
            for (id, body, pose) in ack {
                if self.actors.contains_key(&id) {
                    link.acks.insert((id, BODY), body);
                    link.acks.insert((id, POSE), pose);
                }
            }
            return;
        }
        if !matches!(kind, BODY | POSE) || actor == self.local || !self.actors.contains_key(&actor)
        {
            return;
        }
        // Movement comes only from its owner's own link.
        if self.links[&peer].actor != actor {
            return;
        }
        let stream = self.actors.get_mut(&actor).unwrap().stream_mut(kind);
        if stream.latest().is_some_and(|s| seq <= s.seq) {
            self.stats.late += 1;
            return;
        }
        // P2: a sender publishes at most ~20 a second; a sequence far past that would make its real updates "late".
        if stream.latest().is_some_and(|s| u64::from(seq - s.seq) > 64 + now.saturating_sub(s.received) / 25) {
            self.stats.invalid += 1;
            return;
        }
        // perf 10-18 (SK8NET15): the ACK it carries, what this member holds of our streams (the baselines we delta against).
        if let Some([body, pose]) = packed::ack(data) {
            let acks = &mut self.links.get_mut(&peer).unwrap().acks;
            acks.insert((self.local, BODY), body);
            acks.insert((self.local, POSE), pose);
        }
        let stream = self.actors.get_mut(&actor).unwrap().stream_mut(kind);
        let Some(base) = packed::baseline(data) else {
            return;
        };
        let baseline = if base == 0 { None } else { stream.at(base) };
        if base != 0 && baseline.is_none() {
            self.stats.baseline_miss += 1;
            return;
        }
        let Some(state) = packed::apply(data, baseline) else {
            self.stats.invalid += 1;
            return;
        };
        stream.push(seq, state, now);
        self.stats.decoded += 1;
        self.links.get_mut(&peer).unwrap().seen = now;
    }
    pub fn goodbye(&self) -> Vec<Outgoing> {
        self.links
            .iter()
            .filter(|(_, l)| l.verified)
            .map(|(&peer, l)| {
                let mut data = packed::header(self.session, self.local, GOODBYE, 0);
                packed::stamp(&mut data, l.theirs);
                Outgoing { peer, data }
            })
            .collect()
    }
    pub fn service(&mut self, now: u64) -> Vec<Outgoing> {
        let mut output = self.service_unstamped(now);
        // Every datagram carries its receiver's token; a link that never answered gets the handshake only (P1).
        output.retain_mut(|o| match (self.links.get(&o.peer), o.data[24]) {
            // Answers to a HELLO carry the HELLO's token already.
            (_, COOKIE | REJECT) => true,
            (Some(l), kind) if l.verified || matches!(kind, HELLO | PING | PONG) => {
                packed::stamp(&mut o.data, l.theirs);
                true
            }
            _ => false,
        });
        output
    }
    fn service_unstamped(&mut self, now: u64) -> Vec<Outgoing> {
        self.refresh_eyes();
        let dt = now.saturating_sub(self.last_service).min(1000) as f64 / 1000.;
        self.last_service = now;
        self.credits = (self.credits + dt * self.budget).min(32_000.);
        self.record_credits = (self.record_credits + dt * RECORD_RATE).min(RECORD_BANK);
        self.probe_credits = (self.probe_credits + dt * PROBE_RATE).min(PROBE_RATE);
        self.cookie_credits = (self.cookie_credits + dt * COOKIE_RATE).min(COOKIE_RATE);
        let host = self.host;
        for (&p, l) in self.links.iter_mut() {
            l.credits = (l.credits + dt * self.link_budget).min(3600.);
            // A member that stopped answering (left without GOODBYE, or a roster naming someone else's address) gets
            // probes only until it answers again.
            if host.is_some_and(|h| h != p) && l.verified && now.saturating_sub(l.seen) > LINK_TIMEOUT_MS {
                l.verified = false;
                l.probing = now;
            }
        }
        if self.is_host() {
            let expired: Vec<_> = self
                .links
                .iter()
                .filter(|(_, l)| now.saturating_sub(l.seen) > LINK_TIMEOUT_MS)
                .map(|(&p, l)| (p, l.actor))
                .collect();
            for (peer, id) in expired {
                self.links.remove(&peer);
                self.actors.remove(&id);
                self.roster_dirty = true;
            }
        // Refused (lobby full) or kicked: the host's answer stays the notice (it is not "unavailable").
        } else if self.rejected.is_none()
            && !self.kicked
            && self.host.and_then(|h| self.links.get(&h)).is_some_and(|l| now.saturating_sub(l.seen) > LINK_TIMEOUT_MS)
        {
            // B2: only the host's actor goes; a host that never answered may still be starting, so its link stays.
            if let Some(id) = self.host.and_then(|h| self.links.get(&h)).map(|l| l.actor).filter(|&a| a != 0 && a != self.local) {
                self.actors.remove(&id);
            }
            // Audit 10-24: it most likely timed us out too (a dropped connection): our records and look go again.
            if self.received_roster != 0 {
                if let Some(link) = self.host.and_then(|h| self.links.get_mut(&h)) {
                    link.app_sent.clear();
                    link.app_acks.clear();
                }
                self.blobs.forget(None);
            }
            // Only a lost connection: a host that never answered may still be starting.
            self.host_lost |= self.received_roster != 0;
            self.received_roster = 0;
            self.notice = "Host unavailable; waiting to reconnect".into();
        }
        let mut roster = Vec::new();
        if let Some(peer) = self.host {
            if now.saturating_sub(self.last_hello) >= 500 || self.last_hello == 0 {
                let mut b = packed::header(self.session, self.local, HELLO, 0);
                write_info(&mut b, self.local_info());
                b.extend(self.received_roster.to_le_bytes());
                b.extend(self.token(peer).to_le_bytes());
                self.queue(peer, b);
                self.last_hello = now.max(1);
            }
        // Bandwidth B1 (10-12 audit): a membership change is one new ROSTER (coalesced over ROSTER_COALESCE, so a
        // join storm is not one per joiner); it goes to each member until its HELLO acknowledges it, every
        // ROSTER_RESEND. A settled lobby sends none (a full roster to 49 members is ~120 kB).
        } else if self.is_host() {
            if self.roster_dirty && (self.last_roster == 0 || now.saturating_sub(self.last_roster) >= ROSTER_COALESCE) {
                self.roster_dirty = false;
                self.roster_seq = self.roster_seq.wrapping_add(1).max(1);
                self.last_roster = now.max(1);
            }
            let seq = self.roster_seq;
            // Joiners (never sent one) at once and first; the rest ROSTER_FANOUT at a time, longest waiting first.
            let spaced = self.roster_batch == 0 || now.saturating_sub(self.roster_batch) >= ROSTER_SPACING;
            let mut due: Vec<(u64, u64)> = self
                .links
                .iter()
                .filter(|(_, l)| {
                    seq != 0 && l.roster_ack != seq && (l.roster_sent.0 != seq || now.saturating_sub(l.roster_sent.1) >= ROSTER_RESEND)
                })
                .map(|(&p, l)| (l.roster_sent.1, p))
                .filter(|&(sent, _)| spaced || sent == 0)
                .collect();
            due.sort();
            due.truncate(ROSTER_FANOUT);
            let due: Vec<u64> = due.into_iter().map(|(_, p)| p).collect();
            for p in &due {
                self.links.get_mut(p).unwrap().roster_sent = (seq, now.max(1));
            }
            if !due.is_empty() {
                self.roster_batch = now.max(1);
                // Each member with the endpoint the others reach it at (0: the host, already linked).
                let entries: Vec<(Info, u64)> = std::iter::once((self.local_info(), 0))
                    .chain(self.actors.iter().filter(|(id, _)| **id != self.local).map(|(&id, a)| (a.info, self.endpoint(id).unwrap_or(0))))
                    .collect();
                for (n, chunk) in entries.chunks(ROSTER_PAGE).enumerate() {
                    let mut b = packed::header(self.session, self.local, ROSTER, seq);
                    b.push(entries.len() as u8);
                    b.push((n * ROSTER_PAGE) as u8);
                    for &(info, endpoint) in chunk {
                        write_info(&mut b, info);
                        b.extend(endpoint.to_le_bytes());
                    }
                    // Straight out, never through the bounded `pending` queue (acks could crowd it out).
                    roster.extend(due.iter().map(|&peer| Outgoing { peer, data: b.clone() }));
                }
            }
        }
        // Chat lines not yet acknowledged, resent every CHAT_RESEND ms, the wait doubling every 4 unanswered sends up to 16x
        // (C10); a member that acknowledges nothing for ~30 s loses them. Sent at once when new.
        // ponytail: outside the byte budget: at most 2 * chat::RATE short lines per member.
        let mut chat = Vec::new();
        for (&peer, link) in &mut self.links {
            let wait = CHAT_RESEND << (link.chat_tries / 4).min(4);
            if !link.verified || link.chat_out.is_empty() || (link.chat_sent != 0 && now.saturating_sub(link.chat_sent) < wait) {
                continue;
            }
            if link.chat_tries >= 20 {
                link.chat_out.clear();
                continue;
            }
            link.chat_tries += 1;
            link.chat_sent = now.max(1);
            let base = link.chat_out.front().map_or(0, |l| l.0);
            for (seq, text) in &link.chat_out {
                let mut b = packed::header(self.session, self.local, crate::chat::CHAT, *seq);
                b.extend(base.to_le_bytes());
                b.extend(text.as_bytes());
                chat.push(Outgoing { peer, data: b });
            }
        }
        let mut output = std::mem::take(&mut self.pending);
        output.extend(roster);
        output.extend(chat);
        let mut ids: Vec<_> = self.actors.keys().copied().collect();
        if !ids.is_empty() {
            let offset = self.round % ids.len();
            ids.rotate_left(offset);
        }
        self.round = self.round.wrapping_add(1);
        let mut peers: Vec<_> = self.links.keys().copied().collect();
        if !peers.is_empty() {
            let offset = self.round % peers.len();
            peers.rotate_left(offset);
        }
        // ponytail: ranked by distance ([`apart`]), every service; a spatial index if MAX_PLAYERS grows past ~100.
        let me = &self.actors[&self.local];
        let mut ranked: Vec<(f32, u64, bool)> = self
            .links
            .iter()
            .filter(|(_, l)| l.verified)
            .map(|(&p, l)| {
                let them = self.actors.get(&l.actor);
                let d = them.and_then(|t| apart(me, t)).unwrap_or(f32::MAX);
                (d, p, them.is_some_and(|t| watches(t, self.local)))
            })
            .collect();
        nearest(&mut ranked, &self.near);
        let neighbours: Vec<u64> = ranked.iter().take(BODY_NEIGHBOURS).map(|r| r.1).collect();
        self.near.clone_from(&neighbours);
        let pose_near: Vec<u64> = ranked.iter().take(POSE_NEIGHBOURS).map(|r| r.1).collect();
        // Spectating (user 10-25): up to POSE_NEIGHBOURS members watching us, nearest first, get our POSE (root, pose and
        // camera: all their view plays) at the nearest rate past every cap; BODY stays by distance.
        let watchers: Vec<u64> = ranked.iter().filter(|r| r.2).take(POSE_NEIGHBOURS).map(|r| r.1).collect();
        for peer in peers {
            let token = self.token(peer);
            let link = self.links.get_mut(&peer).unwrap();
            if !link.verified {
                // P1, P9: a probe (PING) only, 1 a second for 10 s then 1 per 10 s, all links within PROBE_RATE.
                let every = if now.saturating_sub(link.probing) < 10_000 { 1000 } else { 10_000 };
                if (link.last_ping == 0 || now.saturating_sub(link.last_ping) >= every) && self.probe_credits >= 1. {
                    self.probe_credits -= 1.;
                    let mut b = packed::header(self.session, self.local, PING, 0);
                    b.extend(now.to_le_bytes());
                    b.extend(token.to_le_bytes());
                    output.push(Outgoing { peer, data: b });
                    link.last_ping = now.max(1);
                }
                continue;
            }
            // What we hold of this peer's own streams, the baselines it deltas against: on every BODY / POSE we send it
            // (SK8NET15), else in an ACK below.
            let held = self.actors.get(&link.actor).filter(|_| link.actor != self.local);
            let holds = held.is_some();
            let held_seq = held.map_or([0; 2], |a| [a.body.latest().map_or(0, |r| r.seq), a.pose.latest().map_or(0, |r| r.seq)]);
            // One we watch gets our ACKs at the near rate, so its deltas stay small.
            let watcher = watchers.contains(&peer);
            let watched = watches(&self.actors[&self.local], link.actor);
            if now.saturating_sub(link.last_ping) >= 1000 {
                let mut b = packed::header(self.session, self.local, PING, 0);
                b.extend(now.to_le_bytes());
                b.extend(token.to_le_bytes());
                output.push(Outgoing { peer, data: b });
                link.last_ping = now;
            }
            // Essential physics first; optional pose detail consumes only remaining budget.
            for kind in [BODY, POSE] {
                for &id in &ids {
                    if id != self.local {
                        continue;
                    }
                    let actor = &self.actors[&id];
                    let stream = actor.stream(kind);
                    let Some(latest) = stream.latest() else {
                        continue;
                    };
                    let distance = self.actors.get(&link.actor).and_then(|a| apart(a, actor)).unwrap_or(0.);
                    let mut interval = match (kind, if kind == POSE && watcher { 0. } else { distance }) {
                        // perf 10-18: BODY is published every 3 fixed steps (multiplayer::send), ~50 ms apart give or
                        // take a frame; 40 here never holds one back a frame.
                        (BODY, d) if d < 30. => if crate::knob!("SKATE_NET_SIM_STAMP", 1) != 0 { 40 } else { 50 },
                        (BODY, d) if d < 100. => 100,
                        (BODY, _) => 500,
                        (POSE, d) if d < 30. => {
                            if self.loopback {
                                50
                            } else {
                                // perf 10-18: 20 Hz to the nearest POSE_NEIGHBOURS (was 10 Hz), what lets a remote
                                // skater play ~100 ms behind instead of 150-250 (interpolation::Clock).
                                crate::knob!("SKATE_NET_POSE_MS", 50) * 4 / 5
                            }
                        }
                        (POSE, d) if d < 100. => 250,
                        _ => 1000,
                    };
                    if kind == BODY && !neighbours.contains(&peer) {
                        interval = interval.max(1000);
                    }
                    if kind == POSE && !pose_near.contains(&peer) && !watcher {
                        interval = interval.max(POSE_CROWD_MS);
                    }
                    if !self.loopback
                        && now.saturating_sub(stream.changed) > 250
                        && (kind == POSE || distance >= 8.)
                    {
                        interval = interval.max(200);
                    }
                    if let Some(&(sent, seq)) = link.sent.get(&(id, kind)) {
                        if seq == latest.seq || now.saturating_sub(sent) < interval {
                            continue;
                        }
                    }
                    let keyframe = link
                        .keys
                        .get(&(id, kind))
                        .is_none_or(|at| now.saturating_sub(*at) >= 2000);
                    let baseline = link
                        .acks
                        .get(&(id, kind))
                        .and_then(|&seq| stream.at(seq).map(|s| (seq, s)))
                        .filter(|_| !keyframe);
                    let mut data =
                        packed::delta(self.session, id, kind, latest.seq, &latest.state, baseline);
                    packed::set_ack(&mut data, held_seq);
                    if data.len() > packed::MTU {
                        self.stats.invalid += 1;
                        continue;
                    }
                    let bytes = data.len() as f64;
                    if link.credits < bytes || self.credits < bytes {
                        self.stats.budget_skips += 1;
                        continue;
                    }
                    link.credits -= bytes;
                    self.credits -= bytes;
                    link.sent.insert((id, kind), (now, latest.seq));
                    if baseline.is_none() {
                        link.keys.insert((id, kind), now);
                    }
                    output.push(Outgoing { peer, data });
                    link.acked = held_seq;
                    link.last_ack = now;
                }
            }
            // perf 10-18: an ACK of its own only when what we hold changed and no BODY / POSE took it, or once a second
            // (SKATE_NET_ACK_LAZY=0: every 100 / 250 ms as before).
            let lazy = crate::knob!("SKATE_NET_ACK_LAZY", 1) != 0;
            let since = now.saturating_sub(link.last_ack);
            if since >= if neighbours.contains(&peer) || watched { 100 } else { FAR_ACK_MS } && (!lazy || held_seq != link.acked || since >= 1000) {
                let mut b = packed::header(self.session, self.local, ACK, 0);
                b.push(u8::from(holds));
                if holds {
                    b.extend(link.actor.to_le_bytes());
                    b.extend(held_seq[0].to_le_bytes());
                    b.extend(held_seq[1].to_le_bytes());
                }
                output.push(Outgoing { peer, data: b });
                link.acked = held_seq;
                link.last_ack = now;
            }
        }
        // Rotate every record, not just actors, so a busy mod cannot starve later keys.
        let mut app_peers: Vec<_> = self.links.keys().copied().collect();
        if !app_peers.is_empty() {
            let offset = self.round % app_peers.len();
            app_peers.rotate_left(offset);
        }
        // perf 10-18: the pass (every record for every link) runs when a record changed or was acknowledged, else
        // every SKATE_NET_APP_EVERY_MS (0: every service, twice a frame, as before); resends wait 50+ ms anyway.
        let every = crate::knob!("SKATE_NET_APP_EVERY_MS", 10);
        if every != 0 && !self.app_dirty && now.saturating_sub(self.last_app) < every {
            app_peers.clear();
        } else {
            self.app_dirty = false;
            self.last_app = now;
        }
        for peer in app_peers {
            // Records go through the host, which relays them; mesh links carry movement only.
            if self.host.is_some_and(|h| h != peer) || !self.links[&peer].verified {
                continue;
            }
            let link = self.links.get_mut(&peer).unwrap();
            link.app_sent.retain(|id, _| self.actors.contains_key(id));
            link.app_acks.retain(|id, _| self.actors.contains_key(id));
            let records: Vec<_> = self
                .actors
                .iter()
                .filter(|(id, _)| **id != link.actor && (self.host.is_none() || **id == self.local))
                .flat_map(|(&id, a)| {
                    a.application
                        .iter()
                        .map(move |(key, record)| (id, key, record))
                })
                .collect();
            let count = records.len();
            for offset in 0..count {
                let index = (link.app_round + offset) % count;
                let (id, key, record) = records[index];
                if link
                    .app_acks
                    .get(&id)
                    .and_then(|keys| keys.get(key))
                    .is_some_and(|&seq| seq == record.seq)
                {
                    continue;
                }
                let previous = link.app_sent.get(&id).and_then(|keys| keys.get(key));
                if previous.is_some_and(|&(at, seq)| {
                    now.saturating_sub(at)
                        < if seq == record.seq {
                            110 + (self.round as u64).wrapping_add(id).wrapping_mul(17) % 130
                        } else {
                            50
                        }
                }) {
                    continue;
                }
                let mut data = packed::header(self.session, id, APPLICATION, record.seq);
                data.push(key.len() as u8);
                data.extend(key.as_bytes());
                data.extend(&record.value);
                let bytes = data.len() as f64;
                if bytes > link.credits || bytes > self.credits || bytes > self.record_credits {
                    link.app_round = index;
                    break;
                }
                link.credits -= bytes;
                self.credits -= bytes;
                self.record_credits -= bytes;
                let sent = link.app_sent.entry(id).or_default();
                match sent.get_mut(key) {
                    Some(at) => *at = (now, record.seq),
                    None => {
                        sent.insert(key.clone(), (now, record.seq));
                    }
                }
                output.push(Outgoing { peer, data });
            }
        }
        // B4 (10-12 audit): each player sends its own look straight to every member (peer to peer, like movement); the
        // host relaying all 49 looks to 48 others was ~20 MB through one home upline in a full lobby.
        let members = self.actors.keys().copied().collect();
        let peers: Vec<_> = self.links.iter().filter(|(_, l)| l.verified).map(|(&peer, link)| (peer, link.actor, link.rtt)).collect();
        output.extend(self.blobs.service(self.session, self.local, &members, &peers, now));
        self.stats.rtt_ms = self.links.values().map(|l| l.rtt).max().unwrap_or(0);
        output
    }
}
/// 32 random bytes for the link tokens.
/// ponytail: std's RandomState keys (seeded from the OS random source once per thread) rather than a rand crate.
fn secret() -> [u8; 32] {
    use std::hash::{BuildHasher, Hasher};
    let mut out = [0; 32];
    for (i, part) in out.chunks_mut(8).enumerate() {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_usize(i);
        part.copy_from_slice(&h.finish().to_le_bytes());
    }
    out
}
/// Sorts `ranked` (distance, endpoint, watching us) nearest first, the members of `near` (our last nearest set) at
/// [`NEAR_STICK`] of their distance.
fn nearest(ranked: &mut [(f32, u64, bool)], near: &[u64]) {
    for r in ranked.iter_mut().filter(|r| near.contains(&r.1)) {
        r.0 *= NEAR_STICK;
    }
    ranked.sort_by(|a, b| a.0.total_cmp(&b.0));
}
fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(a, b)| (a - b) * (a - b))
        .sum::<f32>()
        .sqrt()
}
/// How near two members are, for rates and audio: the least distance from either one's root to the other's points,
/// its bodies (a detached board near another player needs frequent updates too) and where it watches from
/// ([`Actor::camera`]: a spectator's eye is the camera of the player it watches, user 10-24 and audit 10-30). None until
/// both have sent a BODY.
fn apart(a: &Actor, b: &Actor) -> Option<f32> {
    fn reach(x: &Actor, own: &Revision, root: [f32; 3]) -> f32 {
        own.interest.iter().copied().chain(x.camera()).map(|p| distance(p, root)).fold(f32::INFINITY, f32::min)
    }
    let (ra, rb) = (a.body.latest()?, b.body.latest()?);
    Some(reach(a, ra, rb.interest[0]).min(reach(b, rb, ra.interest[0])))
}
/// Whether `a`'s `@watch` ([`crate::roster::WATCH_KEY`]) names `actor`: it asked to show that player's camera, or still
/// shows it while it waits for the asked one's, so that player sends it everything at the nearest rate, however
/// crowded (user 10-25). Online safety 10-26: only those two ever count; a modified one listed everyone, and all of
/// them sent it everything.
fn watches(a: &Actor, actor: u64) -> bool {
    let watch = a.application.get(crate::roster::WATCH_KEY);
    watch.is_some_and(|r| r.value.chunks_exact(8).take(2).any(|id| id == actor.to_le_bytes()))
}
fn write_info(b: &mut Vec<u8>, i: Info) {
    for v in [i.id, i.map, i.rig, i.physics, i.appearance] {
        b.extend(v.to_le_bytes());
    }
}
fn read_info(r: &mut Reader) -> Option<Info> {
    Some(Info {
        id: r.u64()?,
        map: r.u64()?,
        rig: r.u64()?,
        physics: r.u64()?,
        appearance: r.u64()?,
    })
}
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) fn info(id: u64) -> Info {
        Info {
            id,
            map: 1,
            rig: 2,
            physics: 3,
            appearance: 4,
        }
    }
    /// Delivers packets between host (endpoint 1) and client (endpoint 2) for `rounds` service ticks.
    fn pump(host: &mut Session, client: &mut Session, now: &mut u64, rounds: usize) {
        for _ in 0..rounds {
            *now += 50;
            for p in host.service(*now) {
                if p.peer == 2 {
                    client.receive(1, &p.data, *now);
                }
            }
            for p in client.service(*now) {
                if p.peer == 1 {
                    host.receive(2, &p.data, *now);
                }
            }
        }
    }
    fn body_at(x: f32) -> Packed {
        let pose = crate::Pose { p: [x, 0., 0.], q: [0., 0., 0., 1.] };
        let body = crate::Body { pose, velocity: [0.; 3], angular: [0.; 3] };
        Packed::body(&packed::BodyState { root: pose, enabled: 0, bodies: vec![body; 33] }).unwrap()
    }
    /// Item 30b: the skater audio records reach the members near the sender, from its own link only, each frame
    /// once although datagrams repeat frames; a member beyond AUDIO_RANGE gets none.
    #[test]
    fn audio_frames_reach_near_members_once() {
        let (mut now, mut host, mut near) = (0, Session::new(7, info(10), None), Session::new(7, info(20), Some(1)));
        let mut far = Session::new(7, info(30), Some(1));
        let round = |host: &mut Session, near: &mut Session, far: &mut Session, now: &mut u64| {
            *now += 50;
            for p in host.service(*now) {
                match p.peer {
                    2 => near.receive(1, &p.data, *now),
                    3 => far.receive(1, &p.data, *now),
                    _ => {}
                }
            }
            for (from, s) in [(2, near), (3, far)] {
                for p in s.service(*now) {
                    if p.peer == 1 {
                        host.receive(from, &p.data, *now);
                    }
                }
            }
        };
        for _ in 0..10 {
            round(&mut host, &mut near, &mut far, &mut now);
        }
        host.publish(BODY, body_at(0.), now);
        near.publish(BODY, body_at(20.), now);
        far.publish(BODY, body_at(80.), now);
        for _ in 0..10 {
            round(&mut host, &mut near, &mut far, &mut now);
        }
        let record = |n: u32| {
            let mut r = [0u32; crate::audio::RECORD_WORDS];
            r[0] = n;
            r[40] = 0xABCD;
            r
        };
        let frames: Vec<_> = (1..=6).map(|n| (n, record(n))).collect();
        host.send_audio(&frames[..3]);
        host.send_audio(&frames[..]);
        round(&mut host, &mut near, &mut far, &mut now);
        assert_eq!(near.actors[&10].audio.iter().cloned().collect::<Vec<_>>(), frames, "every frame once, in order");
        assert!(far.actors[&10].audio.is_empty(), "80 m away: nothing sent");
        // Only the owner's link carries its records: the host's frames relayed under another actor are dropped.
        let mut forged = packed::header(7, 30, crate::audio::AUDIO, 9);
        forged.extend(crate::audio::encode(&[(9, record(9))]));
        near.stamp_from(1, &mut forged); // the host's real link: only the actor is forged
        near.receive(1, &forged, now);
        assert!(near.actors.get(&30).is_none_or(|a| a.audio.is_empty()));
    }
    /// User 10-24 (build 10): a member spectating us watches through a camera by our skater while its own stands 200 m
    /// off. It gets our BODY at the near rate and our audio, as one standing there would; watching from far, neither.
    #[test]
    fn a_member_watching_through_its_camera_is_near() {
        let camera = |x: f32| {
            let at = |p| crate::Pose { p, q: [0., 0., 0., 1.] };
            Packed::pose(&packed::PoseState { root: at([200., 0., 0.]), bones: vec![crate::Bone { index: packed::CAMERA, pose: at([x, 1.5, 0.]) }] }).unwrap()
        };
        for (watching, near) in [(3., true), (200., false)] {
            let (mut now, mut host, mut far) = (0, Session::new(7, info(10), None), Session::new(7, info(20), Some(1)));
            pump(&mut host, &mut far, &mut now, 10);
            host.publish(BODY, body_at(0.), now);
            far.publish(BODY, body_at(200.), now);
            far.publish(POSE, camera(watching), now);
            pump(&mut host, &mut far, &mut now, 10);
            let mut bodies = 0;
            for i in 0..20 {
                now += 50;
                host.publish(BODY, body_at(0.01 * i as f32), now);
                for p in host.service(now) {
                    bodies += usize::from(p.peer == 2 && p.data[24] == BODY);
                    far.receive(1, &p.data, now);
                }
                for p in far.service(now) {
                    host.receive(2, &p.data, now);
                }
            }
            assert_eq!(bodies >= 15, near, "watching from x = {watching}: {bodies} BODY datagrams in a second");
            let mut record = [0u32; crate::audio::RECORD_WORDS];
            record[0] = 1;
            host.send_audio(&[(1, record)]);
            pump(&mut host, &mut far, &mut now, 1);
            assert_eq!(!far.actors[&10].audio.is_empty(), near, "watching from x = {watching}: our audio");
        }
    }
    /// Audit 10-30: a spectator watches from the camera of the player its `@watch` asks for, wherever its own skater and
    /// camera stand (its camera rows stay its own skater's: what a member watching it sees). A third player beside the
    /// one it watches sends it BODY at the near rate and its audio, as to one standing there; before it watches, neither.
    #[test]
    fn a_member_watching_by_its_record_is_near_the_players_by_the_one_it_watches() {
        let pose = |x: f32, camera: f32| {
            let at = |p| crate::Pose { p, q: [0., 0., 0., 1.] };
            Packed::pose(&packed::PoseState { root: at([x, 0., 0.]), bones: vec![crate::Bone { index: packed::CAMERA, pose: at([camera, 1.5, 0.]) }] }).unwrap()
        };
        // 0: the host, a third player at x = 0; 1: the watched player at x = 3, its camera at x = 1; 2: the spectator,
        // its skater and camera 200 m off.
        let mut s: Vec<Session> = (0..3u64).map(|i| Session::new(7, info(10 + i), (i > 0).then_some(1))).collect();
        let mut now = 0;
        mesh(&mut s, &mut now, 20, &mut |_, _, _| false);
        for (session, x) in s.iter_mut().zip([0., 3., 200.]) {
            session.publish(BODY, body_at(x), now);
        }
        s[1].publish(POSE, pose(3., 1.), now);
        s[2].publish(POSE, pose(200., 200.), now);
        mesh(&mut s, &mut now, 10, &mut |_, _, _| false);
        // BODY datagrams in a second from the third player to the spectator (endpoint 3).
        let second = |s: &mut Vec<Session>, now: &mut u64| {
            let mut bodies = 0;
            for _ in 0..20 {
                s[0].publish(BODY, body_at(0.002 * (*now / 50) as f32), *now);
                mesh(s, now, 1, &mut |from, to, d| {
                    bodies += usize::from(from == 1 && to == 3 && d[24] == BODY);
                    false
                });
            }
            bodies
        };
        let heard = |s: &mut Vec<Session>, now: &mut u64, tick: u32| {
            let mut record = [0u32; crate::audio::RECORD_WORDS];
            record[0] = 1;
            s[2].actors.get_mut(&10).unwrap().audio.clear();
            s[0].send_audio(&[(tick, record)]);
            mesh(s, now, 1, &mut |_, _, _| false);
            !s[2].actors[&10].audio.is_empty()
        };
        let bodies = second(&mut s, &mut now);
        assert!(bodies <= 3, "200 m off: {bodies} BODY in a second");
        assert!(!heard(&mut s, &mut now, 1), "200 m off: no audio");
        assert!(s[2].publish_application(crate::roster::WATCH_KEY, 11u64.to_le_bytes().to_vec(), now));
        mesh(&mut s, &mut now, 4, &mut |_, _, _| false);
        let bodies = second(&mut s, &mut now);
        assert!(bodies >= 15, "watching the player beside it: {bodies} BODY in a second");
        assert!(heard(&mut s, &mut now, 2), "watching the player beside it: its audio");
    }
    /// Delivers every datagram between `sessions` (endpoint = index + 1), member to member too, for `rounds` ticks;
    /// `lose(from, to, datagram)` drops one.
    pub(crate) fn mesh(sessions: &mut [Session], now: &mut u64, rounds: usize, lose: &mut dyn FnMut(u64, u64, &[u8]) -> bool) {
        for _ in 0..rounds {
            *now += 50;
            for from in 0..sessions.len() {
                for p in sessions[from].service(*now) {
                    let Some(to) = (p.peer as usize).checked_sub(1).filter(|&to| to < sessions.len()) else { continue };
                    if !lose(from as u64 + 1, p.peer, &p.data) {
                        sessions[to].receive(from as u64 + 1, &p.data, *now);
                    }
                }
            }
        }
    }
    fn chat_line(to: &Session, from: u64, actor: u64, seq: u32, base: u32, text: &[u8]) -> Vec<u8> {
        let mut b = packed::header(7, actor, crate::chat::CHAT, seq);
        b.extend(base.to_le_bytes());
        b.extend(text);
        to.stamp_from(from, &mut b);
        b
    }
    fn lines(s: &Session) -> Vec<(u64, &str)> {
        s.chat.iter().map(|(a, t)| (*a, t.as_str())).collect()
    }
    /// User 10-25: the player you watch is always smooth. Its spectator stands 200 m off (skater and camera) and a crowd
    /// of 10 shares its spot, so the spectator is outside its nearest 8 and gets it once a second; once the spectator's
    /// `@watch` names it (relayed by the host; alone, or second of two while the camera waits), POSE comes at the
    /// nearest rate and its audio past AUDIO_RANGE, while BODY stays by distance and the 8th nearest of the crowd keeps
    /// its BODY rate; the tombstone ends that.
    #[test]
    fn a_member_watching_us_by_its_record_gets_the_nearest_rate_in_a_crowd() {
        let pose_at = |x: f32| {
            let at = |p| crate::Pose { p, q: [0., 0., 0., 1.] };
            Packed::pose(&packed::PoseState { root: at([x, 0., 0.]), bones: vec![crate::Bone { index: 1, pose: at([x, 1., 0.]) }] }).unwrap()
        };
        // 0: the host, in the crowd; 1: the watched player at x = 0; 2..=10 the rest of the crowd; 11: the spectator.
        let mut s: Vec<Session> = (0..12u64).map(|i| Session::new(7, info(10 + i), (i > 0).then_some(1))).collect();
        let mut now = 0;
        mesh(&mut s, &mut now, 20, &mut |_, _, _| false);
        assert_eq!(s[11].endpoint(11), Some(2), "the spectator links to the watched player");
        for (i, session) in s.iter_mut().enumerate() {
            let x = match i { 1 => 0., 11 => 200., _ => 0.5 + 0.1 * i as f32 };
            session.publish(BODY, body_at(x), now);
        }
        mesh(&mut s, &mut now, 10, &mut |_, _, _| false);
        // (BODY, POSE) datagrams in a second from the watched player to the spectator (endpoint 12) and to the 8th
        // nearest of the crowd (endpoint 9, at 1.3 m).
        let second = |s: &mut Vec<Session>, now: &mut u64| {
            let mut got = [[0; 2]; 2];
            for _ in 0..20 {
                // 2 mm a tick: moving, but never past the crowd's left edge, so its order round the watched one holds.
                let x = 0.002 * (*now / 50) as f32;
                s[1].publish(BODY, body_at(x), *now);
                s[1].publish(POSE, pose_at(x), *now);
                mesh(s, now, 1, &mut |from, to, d| {
                    if from == 2 && (to == 12 || to == 9) && (d[24] == BODY || d[24] == POSE) {
                        got[usize::from(to == 9)][usize::from(d[24] == POSE)] += 1;
                    }
                    false
                });
            }
            got
        };
        let [[bodies, poses], [eighth, _]] = second(&mut s, &mut now);
        assert!(bodies <= 2 && poses <= 2, "outside the nearest 8: {bodies} BODY, {poses} POSE in a second");
        assert!(eighth >= 15, "the 8th nearest: {eighth} BODY in a second");
        for (tick, watch, why) in [(1, 11u64.to_le_bytes().to_vec(), "watching"), (2, [99u64, 11].map(u64::to_le_bytes).concat(), "still shown")] {
            assert!(s[11].publish_application(crate::roster::WATCH_KEY, watch, now));
            mesh(&mut s, &mut now, 4, &mut |_, _, _| false);
            let [[bodies, poses], [eighth, _]] = second(&mut s, &mut now);
            assert!(poses >= 15 && bodies <= 2, "{why}: {bodies} BODY, {poses} POSE in a second (BODY by distance)");
            assert!(eighth >= 15, "{why}: the 8th nearest keeps its rate ({eighth} BODY in a second)");
            let mut record = [0u32; crate::audio::RECORD_WORDS];
            record[0] = 1;
            s[11].actors.get_mut(&11).unwrap().audio.clear();
            s[1].send_audio(&[(tick, record)]);
            mesh(&mut s, &mut now, 1, &mut |_, _, _| false);
            assert!(!s[11].actors[&11].audio.is_empty(), "{why}, 200 m off: our audio");
        }
        // Online safety 10-26: a third name gets nothing extra (a list of everyone made them all send at full rate).
        assert!(s[11].publish_application(crate::roster::WATCH_KEY, [99u64, 98, 11].map(u64::to_le_bytes).concat(), now));
        mesh(&mut s, &mut now, 4, &mut |_, _, _| false);
        let [[bodies, poses], _] = second(&mut s, &mut now);
        assert!(bodies <= 2 && poses <= 2, "a third name: {bodies} BODY, {poses} POSE in a second");
        assert!(s[11].publish_application(crate::roster::WATCH_KEY, Vec::new(), now));
        mesh(&mut s, &mut now, 4, &mut |_, _, _| false);
        let [[bodies, poses], _] = second(&mut s, &mut now);
        assert!(bodies <= 2 && poses <= 2, "stopped watching: {bodies} BODY, {poses} POSE in a second");
    }
    /// Audit 10-28 #7: when only our link to one member died, the host still listed it, so its skater stood frozen on
    /// our screen for the rest of the session (and ours on its). It is not `linked` while that link is silent, and is
    /// again once it answers.
    #[test]
    fn a_member_whose_link_went_silent_is_not_linked_until_it_answers() {
        let mut s: Vec<Session> = (0..3u64).map(|i| Session::new(7, info(10 + i), (i > 0).then_some(1))).collect();
        let mut now = 0;
        mesh(&mut s, &mut now, 20, &mut |_, _, _| false);
        assert!(s[1].linked(12) && s[2].linked(11) && s[1].linked(10) && s[1].linked(11), "all linked");
        // Endpoints 2 and 3 stop reaching each other; both still reach the host.
        mesh(&mut s, &mut now, (LINK_TIMEOUT_MS / 50 + 4) as usize, &mut |from, to, _| matches!((from, to), (2, 3) | (3, 2)));
        assert!(s[1].actors.contains_key(&12), "the host still lists it");
        assert!(!s[1].linked(12) && !s[2].linked(11), "but its link is silent");
        assert!(s[1].linked(10), "the host's link answers");
        mesh(&mut s, &mut now, 40, &mut |_, _, _| false);
        assert!(s[1].linked(12) && s[2].linked(11), "it answers again");
    }
    /// Audit 10-28 #8: with a dozen members at one spot, ones a few centimetres apart swapped in and out of our nearest
    /// 8 every service. A member keeps its place until a newcomer is clearly (a quarter) closer.
    #[test]
    fn the_nearest_eight_keep_a_member_until_a_newcomer_is_clearly_closer() {
        // Members 1..=8 at 10.1 .. 10.8 m were the nearest; member 9 comes closer.
        let near: Vec<u64> = (1..=8).collect();
        let nearest_with = |nine: f32| {
            let mut r: Vec<(f32, u64, bool)> = (1..=8).map(|p| (10. + 0.1 * p as f32, p, false)).chain([(nine, 9, false)]).collect();
            nearest(&mut r, &near);
            r.iter().take(BODY_NEIGHBOURS).map(|r| r.1).collect::<Vec<_>>()
        };
        assert!(!nearest_with(9.9).contains(&9), "a little closer: nobody swaps");
        assert!(nearest_with(7.).contains(&9) && !nearest_with(7.).contains(&8), "clearly closer: the farthest goes");
    }
    /// Audit 10-28 #12: a membership change sent the ROSTER to every member in one service (a full lobby: ~170 kB at
    /// once). It goes ROSTER_FANOUT members at a time.
    #[test]
    fn a_new_roster_goes_out_a_few_members_at_a_time() {
        let mut s: Vec<Session> = (0..21u64).map(|i| Session::new(7, info(10 + i), (i > 0).then_some(1))).collect();
        let mut now = 0;
        mesh(&mut s, &mut now, 60, &mut |_, _, _| false);
        assert!(s.iter().all(|x| x.actors.len() == 21), "settled");
        s.push(Session::new(7, info(31), Some(1)));
        let mut most = 0;
        for _ in 0..40 {
            let mut rosters = 0;
            mesh(&mut s, &mut now, 1, &mut |from, _, d| {
                rosters += usize::from(from == 1 && d[24] == ROSTER);
                false
            });
            most = most.max(rosters);
        }
        assert!(s.iter().all(|x| x.actors.len() == 22), "everyone has the joiner");
        assert!(most <= ROSTER_FANOUT, "{most} ROSTER datagrams in one service");
    }
    /// Audit 10-28 #12: records went out as fast as the session budget allowed (1 MB/s), so a new host sending every
    /// member's records again took the upload movement needs. Records get RECORD_RATE: here the host relays ten 1 kB
    /// records to the nine others.
    #[test]
    fn records_take_only_their_share_of_the_upload() {
        let mut s: Vec<Session> = (0..11u64).map(|i| Session::new(7, info(10 + i), (i > 0).then_some(1))).collect();
        let mut now = 0;
        mesh(&mut s, &mut now, 20, &mut |_, _, _| false);
        for (i, x) in s.iter_mut().enumerate().skip(1) {
            assert!(x.publish_application("big", vec![i as u8; MAX_APP_VALUE], now));
        }
        let mut most = 0;
        for _ in 0..40 {
            let mut bytes = 0;
            mesh(&mut s, &mut now, 2, &mut |from, _, d| {
                if from == 1 && d[24] == APPLICATION {
                    bytes += d.len();
                }
                false
            });
            most = most.max(bytes);
        }
        assert!(s[1..].iter().all(|x| (11..21).all(|id| x.actors[&id].application.contains_key("big"))), "every record arrives");
        assert!(most as f64 <= RECORD_RATE / 10. + RECORD_BANK + 2_000., "{most} bytes of records in 100 ms");
    }
    /// Item 32: a chat line reaches every other member once and in order, resent through loss, under the actor of the
    /// link it came from; a line under another member's actor, or with bad text, shows nowhere.
    #[test]
    fn chat_reaches_every_member_once_from_its_sender() {
        use crate::chat::CHAT;
        let mut s = vec![Session::new(7, info(10), None), Session::new(7, info(20), Some(1)), Session::new(7, info(30), Some(1))];
        let mut now = 0;
        mesh(&mut s, &mut now, 20, &mut |_, _, _| false);
        assert_eq!(s[2].endpoint(20), Some(2), "the members link to each other");
        // Every line's first datagram to each member is lost.
        let mut tried = std::collections::BTreeSet::new();
        let mut lossy = |from: u64, to: u64, d: &[u8]| d[24] == CHAT && tried.insert((from, to, d[25..29].to_vec()));
        assert_eq!(s[1].say("  hello all ", now).as_deref(), Some("hello all"));
        assert_eq!(s[1].say("second", now).as_deref(), Some("second"));
        mesh(&mut s, &mut now, 20, &mut lossy);
        assert_eq!(lines(&s[0]), [(20, "hello all"), (20, "second")]);
        assert_eq!(lines(&s[2]), [(20, "hello all"), (20, "second")], "straight from the sender, not through the host");
        assert!(s[1].chat.is_empty(), "our own lines are the game's to show");
        assert!(s[1].service(now + 1000).iter().all(|p| p.data[24] != CHAT), "acknowledged lines are not resent");
        // Actor 30's link carrying a line under actor 20: refused.
        { let l = chat_line(&s[0], 3, 20, 50, 50, b"I am 20"); s[0].receive(3, &l, now); }
        // Actor 30's own lines: control characters and broken UTF-8 are dropped (and acknowledged), then a good one.
        { let l = chat_line(&s[0], 3, 30, 1, 1, b"bell\x07"); s[0].receive(3, &l, now); }
        { let l = chat_line(&s[0], 3, 30, 2, 1, b"\xff\xfe"); s[0].receive(3, &l, now); }
        { let l = chat_line(&s[0], 3, 30, 2, 1, b"again"); s[0].receive(3, &l, now); }
        { let l = chat_line(&s[0], 3, 30, 3, 1, b" fine "); s[0].receive(3, &l, now); }
        { let l = chat_line(&s[0], 3, 30, 3, 1, b" fine "); s[0].receive(3, &l, now); }
        assert_eq!(lines(&s[0])[2..], [(30, "fine")], "a repeat is taken once");
        // A member whose link is new skips to the oldest line its sender still holds.
        { let l = chat_line(&s[0], 3, 30, 9, 8, b"skipped"); s[0].receive(3, &l, now); }
        { let l = chat_line(&s[0], 3, 30, 8, 8, b"late"); s[0].receive(3, &l, now); }
        { let l = chat_line(&s[0], 3, 30, 9, 8, b"later"); s[0].receive(3, &l, now); }
        assert_eq!(lines(&s[0])[3..], [(30, "late"), (30, "later")]);
    }
    /// Item 32: at most chat::RATE lines per chat::WINDOW_MS from a sender, on its side and on ours.
    #[test]
    fn chat_is_rate_limited_on_send_and_receive() {
        let (mut now, mut host, mut client) = (0, Session::new(7, info(10), None), Session::new(7, info(20), Some(1)));
        pump(&mut host, &mut client, &mut now, 10);
        assert!((0..5).all(|n| client.say(&format!("line {n}"), now).is_some()));
        assert_eq!(client.say("one more", now), None, "past the rate");
        assert_eq!(client.say("\n\t ", now + 10_000), None, "nothing left once cleaned");
        assert!(client.say("x".repeat(crate::chat::MAX_CHARS + 1).as_str(), now + 10_000).is_none(), "too long");
        // A sender past twice the rate (an older or altered build): the extra lines are dropped here.
        let (mut now, mut host, mut client) = (0, Session::new(7, info(10), None), Session::new(7, info(20), Some(1)));
        pump(&mut host, &mut client, &mut now, 10);
        for seq in 1..=11 {
            { let l = chat_line(&host, 2, 20, seq, 1, format!("spam {seq}").as_bytes()); host.receive(2, &l, now); }
        }
        assert_eq!(host.chat.len(), 10);
        { let l = chat_line(&host, 2, 20, 12, 1, b"later"); host.receive(2, &l, now + 5_000); }
        assert_eq!(host.chat.back().map(|l| l.1.as_str()), Some("later"));
    }
    /// Audit 10-24: lines said within the rate arrive bunched up (jitter, a resend). The receiver's own limit dropped the
    /// last ones for good (acknowledged, so never resent) while the sender showed them as sent.
    #[test]
    fn lines_said_within_the_rate_all_arrive_though_they_bunch_up() {
        let (mut host, mut client, mut now) = joined();
        let deliver = |host: &mut Session, client: &mut Session, sent: u64, arrive: u64| {
            for p in client.service(sent) {
                if p.peer == 1 {
                    host.receive(2, &p.data, arrive);
                }
            }
            for p in host.service(arrive) {
                if p.peer == 2 {
                    client.receive(1, &p.data, arrive);
                }
            }
        };
        assert!((0..5).all(|n| client.say(&format!("first {n}"), now).is_some()));
        deliver(&mut host, &mut client, now, now + 400); // late
        now += 5_000;
        assert!((0..5).all(|n| client.say(&format!("second {n}"), now).is_some()));
        deliver(&mut host, &mut client, now, now + 10); // on time
        assert_eq!(host.chat.len(), 10, "{:?}", host.chat);
    }
    #[test]
    fn kicked_peer_is_rejected_and_stays_banned() {
        let (mut now, mut host, mut client) = (
            0,
            Session::new(7, info(10), None),
            Session::new(7, info(20), Some(1)),
        );
        pump(&mut host, &mut client, &mut now, 10);
        assert!(host.actors.contains_key(&20), "client admitted");
        assert_eq!(client.host_actor(), Some(10));
        assert_eq!(host.host_actor(), Some(10));
        assert_eq!(host.kick(20), Some(2));
        assert!(!host.actors.contains_key(&20));
        pump(&mut host, &mut client, &mut now, 10);
        assert!(client.kicked, "client saw REJECT_KICKED");
        assert!(
            !host.actors.contains_key(&20),
            "HELLO from the banned endpoint is not admitted again"
        );
        assert!(client.kick(10).is_none(), "clients cannot kick");
        assert!(host.kick(10).is_none(), "host cannot kick itself");
    }
    #[test]
    fn full_lobby_roster_spans_pages_and_the_next_player_is_refused() {
        let mut now = 0;
        let mut host = Session::new(7, info(1), None);
        // Endpoint e (2..) is the client with actor 100 + e.
        let mut clients: Vec<Session> =
            (0..MAX_PLAYERS as u64).map(|n| Session::new(7, info(102 + n), Some(1))).collect();
        for _ in 0..20 {
            now += 50;
            for p in host.service(now) {
                if let Some(c) = clients.get_mut(p.peer as usize - 2) {
                    c.receive(1, &p.data, now);
                }
            }
            for (n, c) in clients.iter_mut().enumerate() {
                for p in c.service(now) {
                    host.receive(n as u64 + 2, &p.data, now);
                }
            }
        }
        assert_eq!(host.actors.len(), MAX_PLAYERS, "host plus MAX_PLAYERS - 1 admitted");
        let last = clients.pop().unwrap();
        assert_eq!(last.rejected, Some(3), "one more is refused");
        for c in &clients {
            assert_eq!(c.actors.len(), MAX_PLAYERS, "every client assembled the paged roster");
        }
    }
    /// Audit 10-24: real actor ids are full 64-bit hashes. The resend spacing multiplied the id, which wraps only in
    /// release builds: a debug or dev build panicked at its first record still waiting for an acknowledgement.
    #[test]
    fn a_full_64_bit_actor_id_resends_its_records() {
        let (mut now, mut host, mut client) = (0, Session::new(7, info(10), None), Session::new(7, info(u64::MAX - 5), Some(1)));
        pump(&mut host, &mut client, &mut now, 10);
        assert!(client.connected() && client.publish_application("x", vec![1], now));
        for _ in 0..10 {
            now += 50;
            client.service(now); // the acknowledgements never arrive
        }
    }
    /// A host and one client joined through the cookie (endpoints 1 and 2).
    fn joined() -> (Session, Session, u64) {
        let (mut now, mut host, mut client) = (0, Session::new(7, info(10), None), Session::new(7, info(20), Some(1)));
        pump(&mut host, &mut client, &mut now, 10);
        assert!(host.actors.contains_key(&20) && client.connected());
        (host, client, now)
    }
    /// Ledger #54: a member nobody hears from goes at retail's receive timeout (Player::Update 0x82DD04E0 kicks at the
    /// "timeoutMS" key's 5000 ms: .superpowers/sdd/2026-10-26-voice/retail-timeouts.md), not before; a member drops a
    /// silent host the same way.
    #[test]
    fn a_silent_member_lasts_until_the_retail_timeout() {
        let (mut host, mut client, _) = joined();
        let (heard, host_heard) = (host.links[&2].seen, client.links[&1].seen);
        host.service(heard + LINK_TIMEOUT_MS);
        client.service(host_heard + LINK_TIMEOUT_MS);
        assert!(host.actors.contains_key(&20) && client.actors.contains_key(&10), "still there at the timeout");
        host.service(heard + LINK_TIMEOUT_MS + 1);
        client.service(host_heard + LINK_TIMEOUT_MS + 1);
        assert!(!host.actors.contains_key(&20), "the host drops the silent member");
        assert!(!client.actors.contains_key(&10), "the member drops the silent host");
    }
    /// Ledger #54: a host hand-over keeps a member that went silent just before it until retail's timeout after its
    /// last datagram (the new host keeps that link's `seen`), then everyone drops it.
    #[test]
    fn a_migration_keeps_a_silent_member_until_the_timeout() {
        let mut s: Vec<Session> = (0..3u64).map(|i| Session::new(7, info(10 + i), (i > 0).then_some(1))).collect();
        let mut now = 0;
        mesh(&mut s, &mut now, 20, &mut |_, _, _| false);
        assert!(s.iter().all(|x| x.actors.len() == 3), "a full mesh first");
        // Member 3 (endpoint 3, actor 12) goes silent; a second later endpoint 2 takes over as host.
        let mut quiet = |from: u64, _: u64, _: &[u8]| from == 3;
        mesh(&mut s, &mut now, 20, &mut quiet);
        s[0].migrate(Some(2), now);
        s[1].migrate(None, now);
        s[2].migrate(Some(2), now);
        let last = s[1].links[&3].seen;
        while now + 50 <= last + LINK_TIMEOUT_MS {
            mesh(&mut s, &mut now, 1, &mut quiet);
            assert!(s[..2].iter().all(|x| x.actors.contains_key(&12)), "kept after {} ms of silence", now - last);
        }
        mesh(&mut s, &mut now, 4, &mut quiet);
        assert!(s[..2].iter().all(|x| !x.actors.contains_key(&12)), "gone past the timeout");
    }
    /// Ledger #54: a fast link at 15 FPS that loses a whole step's datagrams every 31 steps (a whole flight of the look,
    /// or its acknowledgement) still moves a 32 MB look in under 15 s (skate-game's appearance_transfer delivery test
    /// pins the same): a probe finds each loss within a few steps, not the 400 ms retry timeout (blob.rs, TLP + RACK).
    #[test]
    fn a_lossy_fast_link_moves_a_big_look_quickly() {
        let (mut host, mut client, mut now) = joined();
        let look: Vec<u8> = (0..32_067_793u32).map(|i| i.wrapping_mul(17) as u8).collect();
        assert!(host.blobs.publish(10, look));
        let start = now;
        for step in 1..=300u64 {
            now += 67;
            let (up, down) = (host.service(now), client.service(now));
            if step % 31 != 0 {
                for p in up.into_iter().filter(|p| p.peer == 2) {
                    client.receive(1, &p.data, now);
                }
                for p in down.into_iter().filter(|p| p.peer == 1) {
                    host.receive(2, &p.data, now);
                }
            }
            if client.blobs.ready(10).is_some() {
                assert!(now - start < 15_000, "32 MB took {} ms", now - start);
                return;
            }
        }
        panic!("32 MB not delivered in 20 s");
    }
    /// Ledger #54 (user 10-26): a custom model's look (up to 64 MiB) went out ~64 kB a peer per service into one Steam
    /// connection (256 KB/s, 512 KB buffer): movement queued behind it for seconds, or was refused. Movement now keeps
    /// flowing while the look still arrives.
    #[test]
    fn a_look_transfer_leaves_movement_flowing() {
        let (mut host, mut client, mut now) = joined();
        client.publish(BODY, body_at(1.), now);
        pump(&mut host, &mut client, &mut now, 4);
        let look: Vec<u8> = (0..2u32 << 20).map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8).collect();
        assert!(host.blobs.publish(10, look));
        // Host to client is one Steam connection: 256 KB/s (12_800 bytes a 50 ms tick), first in first out.
        let mut queue: VecDeque<(u64, Vec<u8>)> = VecDeque::new();
        let (mut worst, mut ready) = (0, None);
        for tick in 0..600u64 {
            now += 50;
            host.publish(BODY, body_at(tick as f32 * 0.01), now);
            queue.extend(host.service(now).into_iter().filter(|p| p.peer == 2).map(|p| (now, p.data)));
            let mut room = 12_800;
            while queue.front().is_some_and(|(_, d)| d.len() <= room) {
                let (at, data) = queue.pop_front().unwrap();
                room -= data.len();
                if data[24] == BODY {
                    worst = worst.max(now - at);
                }
                client.receive(1, &data, now);
            }
            for p in client.service(now) {
                host.receive(2, &p.data, now);
            }
            if ready.is_none() && client.blobs.ready(10).is_some() {
                ready = Some(tick);
            }
        }
        assert!(worst <= 250, "movement waited {worst} ms behind the look");
        assert!(ready.is_some(), "the 2 MiB look arrives within 30 s");
    }
    /// Q1: a HELLO from an address that never received anything from us (a spoofed source) admits nobody and is
    /// answered with one COOKIE no bigger than it; a flood of them gets at most COOKIE_RATE answers.
    #[test]
    fn a_hello_is_admitted_only_once_its_sender_returned_the_cookie() {
        let mut host = Session::new(7, info(10), None);
        let mut client = Session::new(7, info(20), Some(1));
        let hello = client.service(0).into_iter().find(|p| p.data[24] == HELLO).unwrap().data;
        // Replayed from the victim's address 99, a thousand times.
        for _ in 0..1000 {
            host.receive(99, &hello, 5);
        }
        assert!(!host.actors.contains_key(&20) && host.links.is_empty(), "no member, no state");
        let out = host.service(5);
        assert!(out.len() <= COOKIE_RATE as usize && !out.is_empty(), "{} answers", out.len());
        assert!(out.iter().all(|p| p.peer == 99 && p.data[24] == COOKIE && p.data.len() <= hello.len()));
        // The real client returns the cookie and is admitted.
        let mut now = 5;
        pump(&mut host, &mut client, &mut now, 14); // its next HELLO is due 500 ms after the first
        assert!(host.actors.contains_key(&20));
    }
    /// 3.4 / C2: once linked, a datagram without the link's token (an off-path forger spoofing the member's address)
    /// is refused: GOODBYE, chat.
    #[test]
    fn forged_datagrams_without_the_link_token_are_ignored() {
        let (mut host, client, now) = joined();
        let mut bye = packed::header(7, 20, GOODBYE, 0);
        host.receive(2, &bye, now);
        packed::stamp(&mut bye, 12345);
        host.receive(2, &bye, now);
        let mut line = packed::header(7, 20, crate::chat::CHAT, 1);
        line.extend(1u32.to_le_bytes());
        line.extend(b"forged");
        host.receive(2, &line, now);
        assert!(host.actors.contains_key(&20) && host.chat.is_empty());
        // The real one, which the client stamps, still works.
        for p in client.goodbye() {
            host.receive(2, &p.data, now);
        }
        assert!(!host.actors.contains_key(&20));
    }
    /// Online audit 10-24: a modified host (the code is public) names the joiner as the host in a ROSTER, then says
    /// GOODBYE as the joiner: that removed the joiner's own actor, and its next service panicked (a lobby that crashes
    /// everyone who joins).
    #[test]
    fn a_host_speaking_as_us_cannot_remove_our_own_skater() {
        let (_, mut client, now) = joined();
        let mut roster = packed::header(7, 20, ROSTER, 1_000);
        roster.extend([2, 0]);
        for (i, endpoint) in [(info(20), 0u64), (info(77), 5)] {
            write_info(&mut roster, i);
            roster.extend(endpoint.to_le_bytes());
        }
        client.stamp_from(1, &mut roster);
        client.receive(1, &roster, now);
        let mut bye = packed::header(7, 20, GOODBYE, 0);
        client.stamp_from(1, &mut bye);
        client.receive(1, &bye, now);
        assert!(client.actors.contains_key(&20), "our own skater stays");
        client.service(now + 600);
    }
    /// C2 / C10: a chat sequence far ahead, a base past its sequence, or an acknowledgement of lines never sent is
    /// refused; unanswered lines back off and are given up.
    #[test]
    fn chat_sequences_cannot_jump_and_resends_back_off() {
        let (mut host, mut client, mut now) = joined();
        { let l = chat_line(&host, 2, 20, 1, 1, b"one"); host.receive(2, &l, now); }
        { let l = chat_line(&host, 2, 20, 4_000_000_000, 1, b"far"); host.receive(2, &l, now); }
        { let l = chat_line(&host, 2, 20, 5, 9, b"base past seq"); host.receive(2, &l, now); }
        { let l = chat_line(&host, 2, 20, 2, 1, b"two"); host.receive(2, &l, now); }
        assert_eq!(lines(&host), [(20, "one"), (20, "two")], "the real next line still shows");
        // An ack above anything sent drops nothing.
        client.say("hi", now);
        let mut ack = packed::header(7, 10, crate::chat::CHAT_ACK, u32::MAX);
        client.stamp_from(1, &mut ack);
        client.receive(1, &ack, now);
        assert_eq!(client.links[&1].chat_out.len(), 1);
        // Never acknowledged: the resends slow down and stop.
        let mut sent = vec![];
        for _ in 0..2400 {
            now += 50;
            if client.service(now).iter().any(|p| p.data[24] == crate::chat::CHAT) {
                sent.push(now);
            }
        }
        assert!(sent.len() <= 21, "{} resends", sent.len());
        assert!(sent.windows(2).last().is_some_and(|w| w[1] - w[0] >= 16 * CHAT_RESEND), "{sent:?}");
        assert!(client.links[&1].chat_out.is_empty(), "given up");
    }
    /// P2: one movement update with a huge sequence would make every real update after it "late".
    #[test]
    fn a_movement_sequence_far_ahead_is_refused() {
        let (mut host, mut client, mut now) = joined();
        client.publish(BODY, body_at(1.), now);
        let real: Vec<_> = client.service(now + 50).into_iter().filter(|p| p.peer == 1 && p.data[24] == BODY).collect();
        assert!(!real.is_empty());
        for p in &real {
            host.receive(2, &p.data, now + 50);
        }
        // The member's own link (its token), only the sequence altered.
        let mut far = real[0].data.clone();
        far[25..29].copy_from_slice(&u32::MAX.to_le_bytes());
        host.receive(2, &far, now + 60);
        assert_ne!(host.actors[&20].body.latest().unwrap().seq, u32::MAX);
        now += 100;
        client.publish(BODY, body_at(2.), now);
        let decoded = host.stats.decoded;
        for p in client.service(now + 50).into_iter().filter(|p| p.peer == 1 && p.data[24] == BODY) {
            host.receive(2, &p.data, now + 50);
        }
        assert!(host.stats.decoded > decoded, "the next real update is taken");
    }
    /// perf 10-18 (SK8NET15): while movement streams, the ACKs ride in it: no ACK datagram but the 1 s keepalive, and
    /// every update after a keyframe still goes as a delta.
    #[test]
    fn acks_ride_in_movement_and_deltas_keep_flowing() {
        let (mut host, mut client, mut now) = joined();
        let (mut acks, mut deltas, mut keys) = (0, 0, 0);
        for i in 0..60 {
            now += 50;
            host.publish(BODY, body_at(i as f32 * 0.01), now);
            client.publish(BODY, body_at(5. + i as f32 * 0.01), now);
            for p in host.service(now).into_iter().filter(|p| p.peer == 2) {
                client.receive(1, &p.data, now);
            }
            for p in client.service(now).into_iter().filter(|p| p.peer == 1) {
                if i >= 20 {
                    match p.data[24] {
                        ACK => acks += 1,
                        BODY if packed::baseline(&p.data) != Some(0) => deltas += 1,
                        BODY => keys += 1,
                        _ => {}
                    }
                }
                host.receive(2, &p.data, now);
            }
        }
        assert!(acks <= 2, "{acks} ACK datagrams in 2 s");
        assert!(deltas >= 30 && keys <= 2, "{deltas} deltas, {keys} keyframes");
        assert_eq!(host.stats.baseline_miss, 0);
    }
    /// 3.7: a link past LINK_RX_RATE datagrams a second is dropped before decoding.
    #[test]
    fn a_flooding_link_is_cut_off_before_decode() {
        let (mut host, _client, now) = joined();
        let mut ping = packed::header(7, 20, PING, 0);
        ping.extend(now.to_le_bytes());
        ping.extend(1u64.to_le_bytes());
        host.stamp_from(2, &mut ping);
        let before = host.stats.invalid;
        for _ in 0..10_000 {
            host.receive(2, &ping, now);
        }
        assert!(host.stats.invalid - before >= 10_000 - LINK_RX_RATE as u64, "{}", host.stats.invalid - before);
    }
    /// 3.13: a Direct ban is the player's IP address (a new port is the same player) and at most MAX_PER_IP members
    /// share one address.
    #[test]
    fn direct_bans_and_the_member_cap_go_by_ip() {
        let at = |ip: u32, port: u64| (ip as u64) << 16 | port;
        let server = at(0x0a00_0001, 31030);
        let mut host = Session::new(7, info(10), None);
        host.set_direct(true);
        let round = |host: &mut Session, clients: &mut [(u64, Session)], now: &mut u64| {
            *now += 50;
            for p in host.service(*now) {
                if let Some((_, c)) = clients.iter_mut().find(|(e, _)| *e == p.peer) {
                    c.receive(server, &p.data, *now);
                }
            }
            for (e, c) in clients.iter_mut() {
                for p in c.service(*now) {
                    host.receive(*e, &p.data, *now);
                }
            }
        };
        let mut clients: Vec<(u64, Session)> =
            (0..6).map(|n| (at(0x0a00_0002, 5000 + n), Session::new(7, info(20 + n), Some(server)))).collect();
        let mut now = 0;
        for _ in 0..10 {
            round(&mut host, &mut clients, &mut now);
        }
        assert_eq!(host.actors.len(), 1 + MAX_PER_IP, "the host and MAX_PER_IP members from one address");
        assert_eq!(clients.iter().filter(|(_, c)| c.rejected == Some(3)).count(), 6 - MAX_PER_IP);
        // Kick one; it comes back on a new port of the same address.
        assert!(host.kick(20).is_some());
        let mut back = vec![(at(0x0a00_0002, 6000), Session::new(7, info(20), Some(server)))];
        for _ in 0..10 {
            round(&mut host, &mut back, &mut now);
        }
        assert!(back[0].1.kicked && !host.actors.contains_key(&20), "banned by address, not address and port");
    }
    /// P1: a host's roster naming someone else's address (or one address for several members) gets that address a
    /// probe now and then, never movement or chat.
    #[test]
    fn a_roster_aiming_members_at_a_victim_gets_it_probes_only() {
        let (_host, mut client, mut now) = joined();
        let roster = |client: &Session, seq: u32, members: &[(u64, u64)]| {
            let mut b = packed::header(7, 10, ROSTER, seq);
            b.push(2 + members.len() as u8);
            b.push(0);
            write_info(&mut b, info(10));
            b.extend(0u64.to_le_bytes());
            write_info(&mut b, info(20));
            b.extend(2u64.to_le_bytes());
            for &(id, e) in members {
                write_info(&mut b, info(id));
                b.extend(e.to_le_bytes());
            }
            client.stamp_from(1, &mut b);
            b
        };
        // Two members at one endpoint: refused whole.
        let r = roster(&client, 1000, &[(30, 77), (31, 77)]);
        client.receive(1, &r, now);
        assert!(!client.actors.contains_key(&30) && !client.links.contains_key(&77));
        // Twenty members at twenty ports of a victim.
        let fakes: Vec<(u64, u64)> = (0..20).map(|n| (30 + n, 0x0102_0304_0000 + n)).collect();
        let r = roster(&client, 1001, &fakes);
        client.receive(1, &r, now);
        assert_eq!(client.actors.len(), 22);
        client.say("hello", now);
        client.publish(BODY, body_at(0.), now);
        let mut to_victim = vec![];
        for _ in 0..400 {
            now += 50;
            to_victim.extend(client.service(now).into_iter().filter(|p| p.peer >> 16 == 0x0102_0304));
        }
        assert!(to_victim.iter().all(|p| p.data[24] == PING && p.data.len() == HEADER + 16), "probes only");
        assert!(to_victim.len() <= 20 * 12, "{} probes in 20 s", to_victim.len());
    }
    /// Q12: a datagram that looks like another version but whose "version" is not two digits names nothing.
    #[test]
    fn only_a_real_version_number_reaches_the_notice() {
        let mut host = Session::new(7, info(10), None);
        let mut b = packed::header(7, 20, HELLO, 0);
        b[..8].copy_from_slice(b"SK8NET\x1b[");
        host.receive(2, &b, 0);
        assert_eq!((host.mismatch, host.notice.as_str()), (None, ""));
    }
    /// 3.7: no datagram, whatever its bytes, panics a session; with a valid header (any kind, the link's token) too.
    #[test]
    fn random_datagrams_never_panic_a_session() {
        let (mut host, mut client, now) = joined();
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for i in 0..200_000u64 {
            let len = (next() % 1300) as usize;
            let mut d: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            if i % 2 == 0 && len >= HEADER {
                d[..HEADER].copy_from_slice(&packed::header(7, [10, 20, 30, next()][i as usize / 2 % 4], (next() % 24) as u8, next() as u32));
                (if i % 4 == 0 { &host } else { &client }).stamp_from(if i % 4 == 0 { 2 } else { 1 }, &mut d);
            }
            if i % 4 == 0 {
                host.receive(2, &d, now + i / 100);
            } else {
                client.receive(1, &d, now + i / 100);
            }
            let _ = crate::audio::decode(&d);
            let _ = packed::envelope(&d);
            let _ = packed::apply(&d, None);
            let _ = crate::roster::SessionState::decode(&d);
            let _ = crate::roster::Player::decode(&d);
            if i % 1000 == 0 {
                host.service(now + i / 100);
                client.service(now + i / 100);
            }
        }
    }
}
