//! Transport-neutral free-skate of up to skate_net::lobby::MAX_PLAYERS; each player owns their simulation.
pub(crate) mod appearance;
mod appearance_transfer;
mod render;
mod transport;
use crate::{
    app::SimulationSet,
    physics::{GamePhysics, SkaterRuntime, network},
};
use bevy::prelude::*;
use skate_net::{
    directory::{Command as LobbyCommand, Event, Row, Tags},
    lobby::{Info, Session},
    packed::{self, BodyState, Packed},
    roster::{
        ACCEPT_KEY, Accept, COHOST_KEY, CREATOR_KEYS, CoHost, EVENTS_KEY, Events, NONCE_KEY, PLAYER_KEY, PROPOSE_KEY, Player, Propose,
        READY_KEY, Ready, SCORE_KEY, SESSION_KEY, SWITCH_KEY, Score, SessionState, Switch, VOICE_KEY, WATCH_KEY,
    },
};
/// The engine's own records, never a mod's.
fn reserved(key: &str) -> bool {
    key.starts_with("@look/")
        || [PLAYER_KEY, SESSION_KEY, PROPOSE_KEY, SCORE_KEY, EVENTS_KEY, COHOST_KEY, READY_KEY, ACCEPT_KEY, SWITCH_KEY, WATCH_KEY, NONCE_KEY, VOICE_KEY]
            .contains(&key)
        || CREATOR_KEYS.contains(&key)
}
use std::{
    collections::{BTreeMap, VecDeque},
    net::SocketAddr,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
pub(crate) fn unique() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    skate_net::hash(
        &[
            nanos.to_le_bytes().as_slice(),
            std::process::id().to_le_bytes().as_slice(),
        ]
        .concat(),
    )
}
/// This game's build as the Steam helpers tag lobbies with (`directory::BUILD_KEY`): its lobbies and its players-online
/// count are this build's alone.
// ponytail: SKATE_BUILD_ID only changes when build.rs reruns, so two dirty dev builds of one revision can share it;
// releases (a clean revision each) never do. Hash the exe if dev builds must split.
pub(crate) fn build_tag() -> String {
    blake3::hash(env!("SKATE_BUILD_ID").as_bytes()).to_hex()[..16].to_string()
}
#[derive(Default)]
pub(crate) struct Options {
    pub direct: Option<(SocketAddr, SocketAddr)>,
    pub host: Option<SocketAddr>,
    pub session: u64,
    pub spawn_offset: f32,
    pub appearance: Option<String>,
    pub title: Option<String>,
    pub controller: Option<u32>,
}
struct VisualRoot {
    captured: f64,
    received: f64,
    pose: skate_net::Pose,
}
struct VisualPose {
    captured: f64,
    received: f64,
    bones: Vec<skate_net::Bone>,
    /// The sender's gameplay camera (packed::CAMERA), for spectating them.
    camera: Option<skate_net::Pose>,
}

/// Each remote player's own camera at this frame's playout time (render::present), by actor: what a spectator watching
/// them sees (match_core::observer_camera).
#[derive(Resource, Default)]
pub(crate) struct RemoteCameras(pub BTreeMap<u64, Transform>);

/// The pose stream's camera row: the engine's own camera transform, world space.
fn camera_row(camera: &GlobalTransform) -> skate_net::Bone {
    let (_, rotation, translation) = camera.to_scale_rotation_translation();
    skate_net::Bone {
        index: skate_net::packed::CAMERA,
        pose: skate_net::Pose { p: translation.to_array(), q: rotation.normalize().to_array() },
    }
}

/// Splits the camera row off a received pose (old builds and vehicles send none).
fn take_camera(bones: &mut Vec<skate_net::Bone>) -> Option<skate_net::Pose> {
    let i = bones.iter().position(|b| b.index == skate_net::packed::CAMERA)?;
    Some(bones.remove(i).pose)
}
/// Audit 10-28 #13: m/s a skater may cover between two samples on top of 10 m before the jump counts as a teleport (no
/// blend). A player past 100 m sends once a second, so a fast one hopped at every update.
const FAST_SKATER: f32 = 30.;
/// Both streams carry the root: keep each newer sample, so a player we get BODY from only once a
/// second (not one of their nearest, skate_net::lobby) still moves at POSE rate.
fn push_root(remote: &mut Remote, captured: u64, received: u64, pose: skate_net::Pose) {
    if remote.roots.back().is_some_and(|r| r.captured >= captured as f64 / 1000.) {
        return;
    }
    if remote.roots.back().is_some_and(|p| {
        let gap = (captured as f64 / 1000. - p.captured) as f32;
        Vec3::from_array(p.pose.p).distance(Vec3::from_array(pose.p)) > 10. + FAST_SKATER * gap
    }) {
        remote.roots.clear();
        remote.poses.clear();
        remote.epoch += 1;
        remote.visual_since = captured;
    }
    remote.roots.push_back(VisualRoot { captured: captured as f64 / 1000., received: received as f64 / 1000., pose });
    while remote.roots.len() > 64 {
        remote.roots.pop_front();
    }
}
struct Remote {
    roots: VecDeque<VisualRoot>,
    poses: VecDeque<VisualPose>,
    epoch: u64,
    visual_since: u64,
    body: BodyState,
    body_at: Instant,
    body_seq: u32,
    pose_seq: u32,
}
/// The remote skaters follow the session's actors: made on an actor's first BODY, fed every new BODY / POSE sample.
fn sync_remotes(remotes: &mut BTreeMap<u64, Remote>, lobby: &Session, rig: u64, anchors: &[usize], now: u64) {
    // Audit 10-28 #7: one whose link to us went silent (the host still lists it) is hidden until the link answers.
    remotes.retain(|id, _| lobby.actors.contains_key(id) && lobby.linked(*id));
    for (&id, actor) in &lobby.actors {
        if id == lobby.local || !lobby.linked(id) {
            continue;
        }
        let Some(body) = actor.body.latest() else {
            continue;
        };
        // Ledger #54: no timeout of our own (it was 3.5 s, a blink): a silent player stays, frozen on its last pose,
        // until its link goes silent at LINK_TIMEOUT_MS (above).
        if let std::collections::btree_map::Entry::Vacant(entry) = remotes.entry(id) {
            let Some(initial) = body.state.unpack_body() else {
                continue;
            };
            let fallback = actor.info.rig != rig;
            info!("MULTIPLAYER_CONNECTED peer={id} fallback={fallback}");
            entry.insert(Remote {
                roots: VecDeque::new(),
                poses: VecDeque::new(),
                epoch: 0,
                visual_since: 0,
                body: initial,
                body_at: Instant::now(),
                body_seq: 0,
                pose_seq: 0,
            });
        }
        let remote = remotes.get_mut(&id).unwrap();
        // Consume every newly decoded source sample, including several delivered in one frame.
        for revision in &actor.body.history {
            if revision.seq <= remote.body_seq {
                continue;
            }
            let Some(state) = revision.state.unpack_body() else {
                continue;
            };
            push_root(remote, revision.state.captured, revision.received, state.root);
            remote.body = state;
            remote.body_at =
                Instant::now() - Duration::from_millis(now.saturating_sub(revision.received));
            remote.body_seq = revision.seq;
        }
        for revision in &actor.pose.history {
            if revision.seq <= remote.pose_seq {
                continue;
            }
            remote.pose_seq = revision.seq;
            if revision.state.captured < remote.visual_since {
                continue;
            }
            let Some(mut pose) = revision.state.unpack_pose() else {
                continue;
            };
            push_root(remote, revision.state.captured, revision.received, pose.root);
            if revision.state.captured < remote.visual_since {
                continue;
            }
            let camera = take_camera(&mut pose.bones);
            if actor.info.rig != rig
                || pose
                    .bones
                    .iter()
                    .map(|b| b.index as usize)
                    .ne(anchors.iter().copied())
            {
                pose.bones.clear();
            }
            remote.poses.push_back(VisualPose {
                captured: revision.state.captured as f64 / 1000.,
                received: revision.received as f64 / 1000.,
                bones: pose.bones,
                camera,
            });
            while remote.poses.len() > 64 {
                remote.poses.pop_front();
            }
        }
    }
}
#[derive(Resource)]
pub(crate) struct Multiplayer {
    transport: Option<Box<dyn transport::Transport>>,
    lobby: Option<Session>,
    info: Info,
    schema: network::Schema,
    anchors: Vec<usize>,
    remotes: BTreeMap<u64, Remote>,
    pub status: String,
    pub join_code: String,
    pub host_code: String,
    last_body: Instant,
    last_pose: Instant,
    /// perf 10-18: fixed steps since the last BODY ([`send`]).
    body_steps: u32,
    started: Instant,
    last_metrics: Instant,
    counts: (u64, u64),
    rates: String,
    provider_metrics: String,
    visual_status: String,
    loopback: bool,
    room: Option<(u64, u64)>,
    pub(crate) map_name: String,
    pub browser_rows: Vec<Row>,
    pub browser_page: usize,
    pub browser_total: usize,
    pub browser_status: String,
    /// Bumped by each lobby list or Steam error the relay answers (the server browser reads every page as it lands),
    /// and whether that last answer was the error.
    pub(crate) browser_answers: u64,
    pub(crate) browser_error: bool,
    /// A Steam lobby join is in flight (no session yet).
    joining: bool,
    /// That join failed (lobby gone or full, Steam unavailable); online_roster logs it and clears this.
    pub(crate) join_failed: bool,
    /// Steam's reason for that failure (a different version, lobby full, Steam not running), shown under the retail
    /// line; empty when the join just timed out.
    pub(crate) join_error: String,
    /// Audit 10-24: the Steam relay stopped mid-session and we left it (online_roster says so in the event log).
    pub(crate) steam_lost: bool,
    /// Not retail (user 10-25): the server browser listing Host Actions set, for every Steam lobby we own this game run
    /// ([`Multiplayer::publish_details`]).
    pub(crate) details: skate_net::directory::Details,
    /// What went to which lobby, and when.
    details_sent: Option<(u64, skate_net::directory::Details, Instant)>,
}
impl Multiplayer {
    /// Tests only: in session `lobby`, with no transport and no skater rig.
    #[cfg(test)]
    pub(crate) fn for_test(lobby: Session) -> Self {
        let now = Instant::now();
        Self {
            transport: None,
            info: Info { id: lobby.local, map: 0, rig: 0, physics: 0, appearance: 0 },
            lobby: Some(lobby),
            schema: network::Schema::empty(),
            anchors: vec![],
            remotes: BTreeMap::new(),
            status: String::new(),
            join_code: String::new(),
            host_code: String::new(),
            last_body: now,
            last_pose: now,
            body_steps: 0,
            started: now,
            last_metrics: now,
            counts: (0, 0),
            rates: String::new(),
            provider_metrics: String::new(),
            visual_status: String::new(),
            loopback: false,
            room: None,
            map_name: String::new(),
            browser_rows: vec![],
            browser_page: 0,
            browser_total: 0,
            browser_status: String::new(),
            browser_answers: 0,
            browser_error: false,
            joining: false,
            join_failed: false,
            join_error: String::new(),
            steam_lost: false,
            details: Default::default(),
            details_sent: None,
        }
    }
    pub(crate) fn diagnostic_summary(&self) -> String {
        let provider = if self.room.is_some() {
            "platform_relay"
        } else if self.transport.is_some() && self.lobby.is_some() {
            "direct_local"
        } else {
            "inactive"
        };
        let rtt = self.lobby.as_ref().map(|lobby| lobby.stats.rtt_ms);
        format!(
            "provider:{provider} active:{} remote_count:{} rtt_ms:{rtt:?}",
            self.active(),
            self.remotes.len()
        )
    }
    pub(crate) fn mod_identity(&self) -> (bool, u64, bool) {
        self.lobby
            .as_ref()
            .map_or((false, 0, true), |l| (true, l.local, l.is_host()))
    }
    pub(crate) fn collision_players(&self) -> Vec<(u64, skate_net::packed::BodyState)> {
        self.remotes
            .iter()
            .filter(|(_, r)| r.body_at.elapsed() < Duration::from_millis(500))
            .map(|(&id, r)| (id, r.body.clone()))
            .collect()
    }
    pub(crate) fn mod_records(&self) -> Vec<(u64, String, u32, Vec<u8>)> {
        self.lobby.as_ref().map_or_else(Vec::new, |l| {
            l.actors
                .iter()
                .filter(|(id, _)| **id != l.local)
                .flat_map(|(&id, actor)| {
                    actor
                        .application
                        .iter()
                        .filter(|(key, _)| !reserved(key))
                        .map(move |(key, r)| (id, key.clone(), r.seq, r.value.clone()))
                })
                .collect()
        })
    }
    pub(crate) fn publish_mod(&mut self, key: &str, value: Vec<u8>) -> bool {
        if reserved(key) {
            return false;
        }
        let now = self.now_ms();
        self.lobby
            .as_mut()
            .is_some_and(|l| l.publish_application(key, value, now))
    }
    pub(crate) fn session_mut(&mut self) -> Option<&mut Session> {
        self.lobby.as_mut()
    }
    pub(crate) fn session(&self) -> Option<&Session> {
        self.lobby.as_ref()
    }
    /// Voice chat (user 10-26): where ours goes (verified links, less the `exclude`d endpoints) and whose we take.
    pub(crate) fn voice_route(&self, exclude: impl Fn(u64) -> bool) -> Option<skate_net::voice::VoiceRoute> {
        self.lobby.as_ref().map(|l| l.voice_route(exclude))
    }
    pub(crate) fn voice_listen(&self) -> Option<skate_net::voice::VoiceListen> {
        self.lobby.as_ref().map(Session::voice_listen)
    }
    /// The transport's voice I/O for this session: a new voice channel each call (None offline).
    pub(crate) fn voice_io(&mut self) -> Option<transport::VoiceIo> {
        self.lobby.as_ref()?;
        self.transport.as_mut()?.voice_io()
    }
    /// The session clock (ms) every `Session` call takes.
    pub(crate) fn now_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }
    pub(crate) fn publish_player(&mut self, player: &Player) -> bool {
        let now = self.now_ms();
        self.lobby
            .as_mut()
            .is_some_and(|l| l.publish_application(PLAYER_KEY, player.encode(), now))
    }
    /// A guest's proposal for the host (online_moves reads it).
    pub(crate) fn publish_proposal(&mut self, proposal: &Propose) -> bool {
        let now = self.now_ms();
        self.lobby
            .as_mut()
            .is_some_and(|l| l.publish_application(PROPOSE_KEY, proposal.encode(), now))
    }
    /// Our accept of the open proposal `token` (online_moves counts it on the host).
    pub(crate) fn publish_accept(&mut self, token: u64) -> bool {
        let now = self.now_ms();
        self.lobby.as_mut().is_some_and(|l| l.publish_application(ACCEPT_KEY, Accept { token }.encode(), now))
    }
    /// A guest's Switch Team proposal for the host (online_moves reads it).
    pub(crate) fn publish_switch(&mut self) -> bool {
        let now = self.now_ms();
        self.lobby.as_mut().is_some_and(|l| l.publish_application(SWITCH_KEY, Switch { nonce: unique() }.encode(), now))
    }
    /// A co-host's request for the host (online_moves reads and checks it).
    pub(crate) fn publish_cohost(&mut self, ask: skate_net::roster::CoHostAsk) -> bool {
        let now = self.now_ms();
        let request = CoHost { nonce: unique(), ask };
        self.lobby.as_mut().is_some_and(|l| l.publish_application(COHOST_KEY, request.encode(), now))
    }
    /// We are a co-host of the session we're in (never while hosting).
    pub(crate) fn cohost(&self, state: &SessionState) -> bool {
        self.lobby.as_ref().is_some_and(|l| !l.is_host() && state.is_cohost(l.local))
    }
    /// Our result in the running match (match_core); idempotent.
    pub(crate) fn publish_score(&mut self, score: &Score) -> bool {
        let now = self.now_ms();
        self.lobby
            .as_mut()
            .is_some_and(|l| l.publish_application(SCORE_KEY, score.encode(), now))
    }
    /// Our newest match events (match_core); idempotent.
    pub(crate) fn publish_events(&mut self, events: &Events) -> bool {
        let now = self.now_ms();
        self.lobby
            .as_mut()
            .is_some_and(|l| l.publish_application(EVENTS_KEY, events.encode(), now))
    }
    /// Our `@watch` (match_core::ask_watch): the skater our camera is asked to show, then the one it still shows while it
    /// waits for the asked one's samples; empty for our own; idempotent.
    pub(crate) fn publish_watch(&mut self, asked: Option<u64>, shown: Option<u64>) -> bool {
        let now = self.now_ms();
        let value: Vec<u8> = asked.into_iter().chain(shown.filter(|&s| Some(s) != asked)).flat_map(u64::to_le_bytes).collect();
        self.lobby.as_mut().is_some_and(|l| l.publish_application(WATCH_KEY, value, now))
    }
    /// Our `@voice`: voice chat on with a microphone open (voice::run); idempotent.
    pub(crate) fn publish_voice(&mut self, on: bool) -> bool {
        let now = self.now_ms();
        self.lobby.as_mut().is_some_and(|l| l.publish_application(VOICE_KEY, vec![u8::from(on)], now))
    }
    /// The actors whose `@voice` says their voice chat is on.
    pub(crate) fn voiced(&self) -> std::collections::BTreeSet<u64> {
        let Some(session) = self.lobby.as_ref() else { return Default::default() };
        (session.actors.iter()).filter(|(_, a)| a.application.get(VOICE_KEY).is_some_and(|r| r.value == [1])).map(|(&id, _)| id).collect()
    }
    /// Whether we can play the lobby's row (match_core); idempotent.
    pub(crate) fn publish_ready(&mut self, ready: &Ready) -> bool {
        let now = self.now_ms();
        self.lobby
            .as_mut()
            .is_some_and(|l| l.publish_application(READY_KEY, ready.encode(), now))
    }
    /// Host only; idempotent (publish_application ignores an equal value).
    pub(crate) fn publish_session(&mut self, state: &SessionState) -> bool {
        let now = self.now_ms();
        self.lobby.as_mut().is_some_and(|l| {
            l.is_host() && l.publish_application(SESSION_KEY, state.encode(), now)
        })
    }
    /// Host only: kick `actor`, recording the banned endpoint in `state` (republished by
    /// online_roster). Returns the kicked display name.
    pub(crate) fn kick(&mut self, state: &mut SessionState, actor: u64) -> Option<String> {
        let steam = self.is_steam();
        skate_net::roster::kick(self.lobby.as_mut()?, state, actor, steam)
    }
    /// Re-ban the Steam endpoints in the host's `@session` (so a promoted host already has them).
    pub(crate) fn seed_bans(&mut self, state: &SessionState) {
        if let Some(l) = &mut self.lobby {
            skate_net::roster::seed_bans(l, state);
        }
    }
    pub(crate) fn is_steam(&self) -> bool {
        self.room.is_some()
    }
    pub(crate) fn room_id(&self) -> Option<u64> {
        self.room.map(|(id, _)| id)
    }
    /// The Steam lobby's owner as Steam reports it (online_session checks a session move's new lobby by it).
    pub(crate) fn room_owner(&self) -> Option<u64> {
        self.room.map(|(_, owner)| owner)
    }
    /// Host only, Steam only: hand the lobby to `actor`'s real endpoint SteamID (never the
    /// self-reported `@player` value); Steam's owner change then migrates every client.
    pub(crate) fn make_host(&mut self, actor: u64) -> bool {
        let Some(member) = self
            .lobby
            .as_ref()
            .filter(|l| self.room.is_some() && l.is_host() && actor != l.local)
            .and_then(|l| l.endpoint(actor))
        else {
            return false;
        };
        self.lobby_command(LobbyCommand::Owner { member })
    }
    pub(crate) fn set_private(&mut self, private: bool) -> bool {
        if !self.is_steam() {
            return false;
        }
        self.lobby_command(LobbyCommand::Privacy { private })
    }
    /// Join a Steam lobby by id (friend join / invite), as the join code path does.
    pub(crate) fn join_lobby(&mut self, lobby: u64) {
        self.leave();
        self.steam_join(lobby);
    }
    fn steam_join(&mut self, lobby: u64) {
        let sent = self.lobby_command(LobbyCommand::Join {
            lobby,
            map: self.info.map,
            physics: self.info.physics,
        });
        self.joining = sent;
        self.join_failed = !sent;
        // Audit 10-24: and why it could not be sent (Steam offline, the relay files missing).
        if !sent {
            self.join_error = self.browser_status.clone();
        }
    }
    pub fn active(&self) -> bool {
        self.lobby.is_some()
    }
    /// User 10-25: the host's server browser listing, to the Steam lobby we own when it or the lobby changes and every
    /// few seconds (the relay doesn't answer it); a migrated lobby's new host sends its own.
    fn publish_details(&mut self) {
        const AGAIN: Duration = Duration::from_secs(5);
        let Some((room, _)) = self.room.filter(|_| self.lobby.as_ref().is_some_and(|l| l.is_host())) else { return };
        let due = (self.details_sent.as_ref()).is_none_or(|(lobby, sent, at)| *lobby != room || *sent != self.details || at.elapsed() >= AGAIN);
        if due && self.transport.as_mut().is_some_and(|t| t.details(&self.details)) {
            self.details_sent = Some((room, self.details.clone(), Instant::now()));
        }
    }
    /// In a session and hearing from at least one other player: a join is complete.
    pub(crate) fn synced(&self) -> bool {
        self.lobby.is_some() && !self.remotes.is_empty()
    }
    /// In a session, or a Steam lobby join is under way.
    pub(crate) fn engaged(&self) -> bool {
        self.lobby.is_some() || self.joining
    }
    pub fn leave(&mut self) {
        if let (Some(lobby), Some(t)) = (&self.lobby, &mut self.transport) {
            for p in lobby.goodbye() {
                let _ = t.send(p.peer, &p.data);
            }
        }
        // perf 10-18: Steam's relay stays up for the next session (transport::Steam::leave).
        if !self.transport.as_mut().is_some_and(|t| t.leave()) {
            self.transport = None;
        }
        self.lobby = None;
        self.remotes.clear();
        self.host_code.clear();
        self.room = None;
        self.joining = false;
        self.status = "Offline. Steam is only needed for Steam multiplayer.".into();
    }
    /// Leave, and stop the Steam relay too (3.3: signed out, nothing of ours keeps Steam showing us in a game; audit
    /// 10-24: the relay kept for the next session showed "Playing Spacewar" until the game closed).
    pub(crate) fn leave_steam(&mut self) {
        self.leave();
        self.transport = None;
    }
    /// Retail Entity.DoesSkaterExist for a player in the session: its skater is spawned on this console's map. Ours
    /// always is; a remote one once its movement arrives (`remotes`, which render::spawn draws, dropped again after
    /// 3.5 s of silence) and it runs the same map. A match's load / start barrier waits for every slot's actor.
    pub(crate) fn skater_exists(&self, actor: u64) -> bool {
        self.lobby.as_ref().is_some_and(|l| exists(l, self.info.map, actor, spawned(l, &self.remotes, actor)))
    }
    fn start(&mut self, transport: Box<dyn transport::Transport>, session: u64, host: Option<u64>) {
        self.leave();
        self.info.id = unique();
        self.loopback = transport.loopback();
        let mut lobby = Session::new(session, self.info, host);
        lobby.set_loopback(self.loopback);
        lobby.set_direct(transport.ip_endpoints());
        self.lobby = Some(lobby);
        self.transport = Some(transport);
        self.started = Instant::now();
        self.last_metrics = Instant::now();
        self.counts = (0, 0);
        self.rates.clear();
        self.provider_metrics.clear();
        self.visual_status.clear();
        self.status = "Waiting for players...".into();
    }
    pub fn local(&mut self, host: bool) {
        let bind = if host {
            "127.0.0.1:31030"
        } else {
            "127.0.0.1:0"
        };
        match transport::Direct::new(bind.parse().unwrap(), (!host).then(|| "127.0.0.1:31030".parse().unwrap())) {
            Ok(t) => self.start(
                Box::new(t),
                480_31030,
                if host {
                    None
                } else {
                    Some(transport::endpoint("127.0.0.1:31030".parse().unwrap()).unwrap())
                },
            ),
            Err(e) => self.status = format!("Could not open local session: {e}"),
        }
    }
    /// False if the command could not be sent (no Steam relay, a request already in flight).
    fn lobby_command(&mut self, command: LobbyCommand) -> bool {
        // Retry discovery after Steam was opened following an initialization failure.
        if !self.active()
            && self.transport.as_ref().is_some_and(|t| relay_down(&t.status()))
        {
            self.transport = None;
        }
        if self.transport.is_none() {
            match transport::Steam::new(0, 0) {
                Ok(t) => self.transport = Some(Box::new(t)),
                Err(e) => {
                    self.status = e.clone();
                    self.browser_status = e;
                    return false;
                }
            }
        }
        let result = self.transport.as_mut().unwrap().command(command);
        let sent = result.is_ok();
        self.browser_status = match result {
            Ok(()) => "Contacting Steam...".into(),
            Err(e) => e,
        };
        if !self.active() {
            self.status = self.browser_status.clone();
        }
        sent
    }
    /// Every map's lobbies on this build, a page of them (false: the browse could not be sent).
    pub fn browse(&mut self, page: usize) -> bool {
        self.lobby_command(LobbyCommand::Browse {
            page,
            map: self.info.map,
            physics: self.info.physics,
            label: String::new(),
            tags: Tags::default(),
        })
    }
    /// Online Challenge Map: the first page of lobbies on this map whose tags match `tags`.
    pub fn browse_tagged(&mut self, tags: Tags) {
        self.lobby_command(LobbyCommand::Browse {
            page: 0,
            map: self.info.map,
            physics: self.info.physics,
            label: self.map_name.clone(),
            tags,
        });
    }
    pub fn join_row(&mut self, index: usize) {
        let Some(row) = self.browser_rows.get(index) else {
            return;
        };
        if row.players >= row.capacity {
            self.browser_status = "Lobby is full. Refresh to check for a free slot.".into();
            return;
        }
        let lobby = row.id;
        self.leave();
        self.steam_join(lobby);
    }
    /// Host a public Steam lobby on this map carrying Online Challenge Map tags.
    pub fn steam_host_tagged(&mut self, tags: Tags) {
        self.leave();
        self.lobby_command(LobbyCommand::Host {
            map: self.info.map,
            physics: self.info.physics,
            label: self.map_name.clone(),
            tags,
        });
    }
    pub fn steam(&mut self, host: bool) {
        if host {
            self.steam_host_tagged(Tags::default());
            return;
        }
        if let Ok(lobby) = self.join_code.trim().parse::<u64>() {
            if lobby != 0 {
                self.join_lobby(lobby);
                return;
            }
        }
        let (peer, session) = {
            let Some((id, key)) = self.join_code.trim().split_once('-') else {
                self.status = "Enter a lobby code, or select a lobby in the browser.".into();
                return;
            };
            match (id.parse::<u64>(), u64::from_str_radix(key, 16)) {
                (Ok(id), Ok(key)) if id != 0 && key != 0 => (id, key),
                _ => {
                    self.status = "Invalid join code. Expected SteamID-session.".into();
                    return;
                }
            }
        };
        match transport::Steam::new(peer, session) {
            Ok(t) => self.start(Box::new(t), session, (peer != 0).then_some(peer)),
            Err(e) => self.status = e,
        }
    }
}
/// [`Multiplayer::skater_exists`] on `session`: `spawned` is whether `remotes` holds the actor.
/// A remote skater is spawned here: drawn (`remotes`), or hidden while its link is silent once we had its movement
/// (end review 10-28 on #7: a match barrier passes for it as it did for the frozen skater).
fn spawned(lobby: &Session, remotes: &BTreeMap<u64, Remote>, actor: u64) -> bool {
    remotes.contains_key(&actor) || !lobby.linked(actor) && lobby.actors.get(&actor).is_some_and(|a| a.body.latest().is_some())
}
fn exists(session: &Session, map: u64, actor: u64, spawned: bool) -> bool {
    actor == session.local || spawned && session.actors.get(&actor).is_some_and(|a| a.info.map == map)
}
/// In a session or joining one, copied from `Multiplayer::engaged` every frame: what single-player-only features
/// (the Skate 1 and 2 tutorials) check, and what tests set without a network.
#[derive(Resource, Default, Clone, Copy, PartialEq)]
pub(crate) struct Online(pub bool);

pub(crate) fn engaged(world: &World) -> bool {
    world.get_resource::<Online>().is_some_and(|o| o.0)
}

/// The Steam relay's status once it can no longer serve (transport::Steam).
fn relay_down(status: &str) -> bool {
    status.starts_with("ERROR") || status.contains("stopped") || status.contains("did not respond")
}

/// Audit 10-24: a session move counts too, so what online play switches off stays off between its two sessions; user
/// 10-25: so does a fresh pick from the pick on (`OnlineSession::going_online`), its map load included.
fn publish_online(
    net: Res<Multiplayer>,
    session: Option<Res<crate::online_session::OnlineSession>>,
    mut online: ResMut<Online>,
) {
    let going = session.is_some_and(|s| s.going_online());
    online.set_if_neq(Online(net.engaged() || going));
}

/// Audit 10-28 #6: quitting the game leaves the session first (GOODBYE to every player, then the relay's QUIT), as the
/// Online menu's leave does: Quit went straight out, so the others saw our skater frozen for 5 s.
fn leave_on_exit(mut exit: MessageReader<AppExit>, mut net: ResMut<Multiplayer>) {
    if exit.read().next().is_some() && net.active() {
        net.leave_steam();
    }
}
pub(crate) struct MultiplayerPlugin;
impl Plugin for MultiplayerPlugin {
    fn build(&self, app: &mut App) {
        let config = app.world().resource::<crate::config::Config>();
        let skater = app.world().resource::<SkaterRuntime>();
        let physics = app.world().resource::<GamePhysics>();
        let rig = skate_net::hash(
            format!(
                "{:?}{:?}",
                skater.animation.evaluator.frames.bone_names,
                skater.animation.evaluator.frames.parents
            )
            .as_bytes(),
        );
        let schema =
            network::Schema::new(physics, skater).expect("Default multiplayer collision schema");
        let mut net = Multiplayer {
            transport: None,
            lobby: None,
            info: Info {
                id: unique(),
                map: config.map_fingerprint,
                rig,
                physics: schema.fingerprint,
                appearance: skate_net::hash(
                    config
                        .multiplayer
                        .appearance
                        .as_deref()
                        .unwrap_or(skate_net::DEFAULT_APPEARANCE)
                        .as_bytes(),
                ),
            },
            schema,
            anchors: network::anchors(skater),
            remotes: BTreeMap::new(),
            status: "Offline. Steam is only needed for Steam multiplayer.".into(),
            join_code: String::new(),
            host_code: String::new(),
            last_body: Instant::now(),
            last_pose: Instant::now(),
            body_steps: 0,
            started: Instant::now(),
            last_metrics: Instant::now(),
            counts: (0, 0),
            rates: String::new(),
            provider_metrics: String::new(),
            visual_status: String::new(),
            loopback: false,
            room: None,
            map_name: config
                .map_path
                .as_ref()
                .and_then(|p| p.file_stem())
                .map(|n| skate_net::directory::label(&n.to_string_lossy()))
                .unwrap_or_else(|| "Test world".into()),
            browser_rows: vec![],
            browser_page: 0,
            browser_total: 0,
            browser_status: String::new(),
            browser_answers: 0,
            browser_error: false,
            joining: false,
            join_failed: false,
            join_error: String::new(),
            steam_lost: false,
            details: Default::default(),
            details_sent: None,
        };
        if let Some(bind) = config
            .multiplayer
            .host
            .or(config.multiplayer.direct.map(|(b, _)| b))
        {
            let peer = config.multiplayer.direct.map(|(_, p)| p);
            // An IPv6 --net-direct peer was a panic at startup; now Direct::new names it in the status.
            match transport::Direct::new(bind, peer) {
                Ok(t) => {
                    let target = peer.and_then(|p| transport::endpoint(p).ok());
                    net.start(Box::new(t), config.multiplayer.session, target)
                }
                Err(e) => net.status = format!("Local multiplayer could not start: {e}"),
            }
        }
        app.insert_resource(net)
            .init_resource::<Online>()
            .init_resource::<RemoteCameras>()
            .init_resource::<crate::skate_audio::player::remote::RemoteAudio>()
            .add_systems(
                PreUpdate,
                (world_changed, receive, publish_online, crate::skate_audio::player::remote::exchange)
                    .chain()
                    .after(crate::map_transition::MapTransitionSet),
            )
            .add_systems(Startup, setup_hud)
            .add_systems(Update, hud)
            .add_systems(Update, (send_pose, flush).chain().after(crate::modding::vehicles::present))
            .add_systems(
                FixedUpdate,
                prepare
                    .before(SimulationSet::Physics)
                    .after(SimulationSet::Controls),
            )
            .add_systems(FixedUpdate, send.after(SimulationSet::Physics))
            .init_resource::<appearance::Appearances>()
            .add_systems(Last, (appearance::cleanup, leave_on_exit))
            .add_systems(Update, appearance::sync.before(render::spawn))
            .add_plugins(render::RemoteRenderPlugin);
    }
}
// A world swap replaces the local physics assembly. Never retain lobby identity
// or remote collision state from the previous map, including pending Steam joins.
fn world_changed(
    mut changed: MessageReader<crate::map_transition::WorldChanged>,
    config: Res<crate::config::Config>,
    physics: Res<GamePhysics>,
    skater: Res<SkaterRuntime>,
    mut net: ResMut<Multiplayer>,
) {
    if changed.read().count() == 0 {
        return;
    }
    net.leave();
    net.info.map = config.map_fingerprint;
    net.map_name = config
        .map_path
        .as_ref()
        .and_then(|p| p.file_stem())
        .map(|n| skate_net::directory::label(&n.to_string_lossy()))
        .unwrap_or_else(|| "Test world".into());
    if let Ok(schema) = network::Schema::new(&physics, &skater) {
        net.info.physics = schema.fingerprint;
        net.schema = schema;
    }
    net.anchors = network::anchors(&skater);
    net.browser_rows.clear();
    net.browser_page = 0;
    net.browser_total = 0;
    net.browser_status.clear();
}
fn receive(mut net: ResMut<Multiplayer>) {
    let now = net.started.elapsed().as_millis() as u64;
    let net = &mut *net;
    net.publish_details();
    let Some(t) = &mut net.transport else {
        return;
    };
    let packets = match t.receive() {
        Ok(p) => p,
        Err(e) => {
            net.status = format!("Connection error: {e}");
            return;
        }
    };
    let provider = t.status();
    for response in t.events() {
        match response.event {
            Event::Rows { page, total, rows } => {
                net.browser_rows = rows;
                net.browser_page = page;
                net.browser_total = total;
                (net.browser_answers, net.browser_error) = (net.browser_answers + 1, false);
                net.browser_status = if total == 0 {
                    "No public lobbies found. Host one or refresh.".into()
                } else {
                    format!(
                        "{} lobbies | Page {} | select a row to join",
                        total,
                        page + 1
                    )
                };
            }
            Event::Error(e) => {
                net.browser_status = e.clone();
                (net.browser_answers, net.browser_error) = (net.browser_answers + 1, true);
                if net.lobby.is_none() {
                    net.status = e;
                }
                // The retail UI has no net status readout: the event log says it.
                if std::mem::take(&mut net.joining) {
                    net.join_failed = true;
                    net.join_error = net.browser_status.clone();
                }
            }
            Event::Owner {
                lobby: room,
                owner,
                own,
            } => {
                net.joining = false;
                if net.room != Some((room, owner)) {
                    let host = (owner != own).then_some(owner);
                    if net.room.is_some_and(|(id, _)| id == room) {
                        if let Some(session) = &mut net.lobby {
                            session.migrate(host, now);
                        }
                        info!(
                            "MULTIPLAYER_HOST_MIGRATED lobby={room} owner={owner} local_host={}",
                            host.is_none()
                        );
                    } else {
                        net.info.id = unique();
                        net.lobby = Some(Session::new(room, net.info, host));
                        net.counts = (0, 0);
                        net.last_metrics = Instant::now();
                        net.remotes.clear();
                    }
                    // Not retail (user 10-12): B2 + I1 (10-12 audit): a migration keeps the remote skaters, the old
                    // host's too (their movement is peer to peer and keeps coming); `remotes` follows the session's
                    // actors below.
                    net.loopback = false;
                    net.room = Some((room, owner));
                    net.host_code = room.to_string();
                    net.browser_status = if host.is_none() {
                        "Hosting public lobby. Other players can join from the browser.".into()
                    } else {
                        "Lobby joined; connecting to host...".into()
                    };
                }
            }
        }
    }
    // Audit 10-24: Steam closed or the lobby lost mid-session; the session kept its players frozen until a quit.
    if relay_down(&provider) && net.room.is_some() {
        warn!("MULTIPLAYER_STEAM_LOST {provider}");
        net.leave();
        // Steam closed: online_menu says so once the presence helper notices (one popup, not two).
        net.steam_lost = provider != skate_net::directory::STEAM_CLOSED;
        return;
    }
    let Some(lobby) = &mut net.lobby else {
        if relay_down(&provider) {
            net.browser_status = provider.clone();
            net.status = provider;
        }
        return;
    };
    if net.room.is_none() && lobby.is_host() {
        if let Some(id) = provider.strip_prefix("READY ") {
            net.host_code = format!("{}-{:016x}", id, lobby.session);
        }
    }
    lobby.set_congested(t.congested());
    net.provider_metrics = t.metrics();
    for (peer, p) in packets {
        let said = lobby.mismatch.is_some();
        lobby.receive(peer, &p, now);
        if !said && lobby.mismatch.is_some() {
            warn!("{}", lobby.notice); // no peer: a Steam id or IP address must not reach a crash report
        }
    }
    service(lobby, t.as_mut(), now);
    sync_remotes(&mut net.remotes, lobby, net.info.rig, &net.anchors, now);
    net.status = if !lobby.notice.is_empty() {
        lobby.notice.clone()
    } else if lobby.actors.len() > 1 {
        format!(
            "Connected: {}/{} players | collisions on | synced characters",
            lobby.actors.len(),
            skate_net::lobby::MAX_PLAYERS
        )
    } else {
        format!("{} | waiting for players (1/{})", provider, skate_net::lobby::MAX_PLAYERS)
    };
    if net.last_metrics.elapsed() >= Duration::from_secs(1) {
        let dt = net.last_metrics.elapsed().as_secs_f64();
        let s = &lobby.stats;
        net.rates = format!(
            "App up {:.1} / down {:.1} kB/s | RTT {} ms | stale {} | delta misses {} | send errors {}",
            (s.tx_bytes - net.counts.0) as f64 / dt / 1000.,
            (s.rx_bytes - net.counts.1) as f64 / dt / 1000.,
            s.rtt_ms,
            s.late,
            s.baseline_miss,
            s.send_errors
        );
        info!(
            "MULTIPLAYER_STATS players={} {} budget_skips={} {}",
            lobby.actors.len(),
            net.rates,
            s.budget_skips,
            net.provider_metrics
        );
        net.counts = (s.tx_bytes, s.rx_bytes);
        net.last_metrics = Instant::now();
    }
}
fn service(lobby: &mut Session, t: &mut dyn transport::Transport, now: u64) {
    for p in lobby.service(now) {
        let success = t.send(p.peer, &p.data).is_ok();
        lobby.record_send(p.data.len(), success);
    }
}
/// Send this frame's BODY/POSE now: waiting for the next PreUpdate `receive` added up to a frame of latency.
fn flush(mut net: ResMut<Multiplayer>) {
    let now = net.now_ms();
    let net = &mut *net;
    if let (Some(lobby), Some(t)) = (&mut net.lobby, &mut net.transport) {
        service(lobby, t.as_mut(), now);
    }
}
pub(crate) fn prepare(
    net: Res<Multiplayer>,
    mut physics: ResMut<GamePhysics>,
    skater: Res<SkaterRuntime>,
    vehicles: Res<crate::modding::vehicles::Vehicles>,
    roster: Option<Res<crate::online_roster::OnlineRoster>>,
) {
    physics.network_active = net.active();
    physics.network_contacts = 0;
    let mut proxies = std::mem::take(&mut physics.network_proxies);
    proxies.bodies.clear();
    proxies.volumes.clear();
    proxies.cars.clear();
    // Not retail (user 10-25): players only collide with each other when the host turns it on (off by default).
    let collide = roster.is_some_and(|r| r.state.collisions);
    // Audit 10-24: a blocked or hidden player (not drawn) doesn't collide either: invisible, they could still ram us.
    for (_, remote) in net.remotes.iter().filter(|(id, _)| collide && !crate::player_list::ghost(**id)) {
        if remote.body_at.elapsed() < Duration::from_millis(500) {
            proxies.append(
                &remote.body,
                &net.schema,
                &physics,
                &skater,
                remote.body_at.elapsed().as_secs_f32().min(0.05),
            );
        }
    }
    for shape in crate::modding::vehicles::network::collision_shapes(&vehicles, |actor| collide && !crate::player_list::ghost(actor)) {
        proxies.append_vehicle(&shape, &physics, &skater);
    }
    physics.network_proxies = proxies;
}
#[derive(Component)]
struct NetworkHud;
fn setup_hud(mut commands: Commands) {
    commands.spawn((
        NetworkHud,
        Text::new(""),
        TextFont {
            font_size: 15.,
            ..default()
        },
        TextColor(Color::WHITE),
        GlobalZIndex(5),
        Node {
            position_type: PositionType::Absolute,
            top: px(8),
            left: px(8),
            padding: UiRect::all(px(6)),
            ..default()
        },
        BackgroundColor(Color::srgba(0., 0., 0., 0.6)),
    ));
}
fn hud(
    net: Res<Multiplayer>,
    physics: Res<GamePhysics>,
    label: Single<(&mut Text, &mut Visibility), With<NetworkHud>>,
    mods: Res<crate::modding::network::ModSync>,
    appearances: Res<appearance::Appearances>,
    mut stats: Local<Option<bool>>,
) {
    let (mut label, mut visibility) = label.into_inner();
    // The connection stats are a developer readout (retail has none, and it covers the online event log),
    // so they only show with SKATE_NET_STATS=1.
    let stats = *stats.get_or_insert_with(|| std::env::var("SKATE_NET_STATS").is_ok_and(|v| v == "1"));
    let text = if stats && net.active() {
        format!(
            "{}\n{}\n{}\n{}\n{}\n{} {}\nPlayer contacts: {} | Esc > Multiplayer",
            net.status,
            net.rates,
            net.provider_metrics,
            net.visual_status,
            mods.status,
            appearances.progress,
            appearances.status,
            physics.network_contacts
        )
    } else {
        String::new()
    };
    // Writing marks the text changed (a relayout); only write a new string.
    if label.0 != text {
        label.0 = text;
    }
    visibility.set_if_neq(if label.0.is_empty() { Visibility::Hidden } else { Visibility::Inherited });
}
/// perf 10-18: the session clock (ms) of the fixed step whose state goes out: this frame's start less the time the fixed
/// clock has not stepped yet. BODY (FixedUpdate) and POSE (Update) both carry fixed-step state, and wall time stamped a
/// burst of steps in one frame alike. SKATE_NET_SIM_STAMP=0: wall time, as before.
fn sim_stamp(net: &Multiplayer, real: &Time<Real>, fixed: &Time<Fixed>) -> u64 {
    if skate_net::knob!("SKATE_NET_SIM_STAMP", 1) == 0 {
        return net.now_ms();
    }
    let frame = real.last_update().map_or(net.now_ms(), |at| at.saturating_duration_since(net.started).as_millis() as u64);
    frame.saturating_sub(fixed.overstep().min(Duration::from_millis(100)).as_millis() as u64)
}
fn send(
    mut net: ResMut<Multiplayer>,
    physics: Res<GamePhysics>,
    skater: Res<SkaterRuntime>,
    vehicles: Res<crate::modding::vehicles::Vehicles>,
    (real, fixed): (Res<Time<Real>>, Res<Time<Fixed>>),
) {
    if !net.active() || skater.pose_generation == 0 {
        return;
    }
    let now = sim_stamp(&net, &real, &fixed);
    // perf 10-18: every 3 fixed steps at 60 Hz (50 ms of simulation), not a 49 ms wall gate that a frame's burst of steps
    // missed or doubled.
    let due = if skate_net::knob!("SKATE_NET_SIM_STAMP", 1) != 0 {
        net.body_steps += 1;
        let every = (0.05 / fixed.timestep().as_secs_f64().max(1e-3)).round().max(1.) as u32;
        net.body_steps >= every
    } else {
        net.last_body.elapsed() >= Duration::from_millis(49)
    };
    if due {
        net.body_steps = 0;
        let mut state = network::capture_body(&physics, &skater);
        if let Some(pose) = &vehicles.network_pose {
            state.root = pose.root;
        }
        if vehicles.occupied() {
            state.enabled = 1u64 << 62;
        }
        if let Some(p) = Packed::body(&state) {
            net.lobby.as_mut().unwrap().publish(packed::BODY, p, now);
        }
        net.last_body = Instant::now();
    }
}
/// The camera a pose goes with (user 10-29, spectators saw odd angles and our editors' cameras): the gameplay camera of
/// the same fixed tick as the bones (presentation's newest snapshot), not the one drawn last frame, a blend a tick
/// behind them that Edit Skater and the Replay Editor take over; a vehicle's camera is only drawn (`rendered`).
/// Spectating, still ours: what a member watching us sees; the lobby takes where we watch from from our `@watch` (audit
/// 10-30, skate_net::lobby `Session::refresh_eyes`).
fn sent_camera(tick: Option<Transform>, driving: bool, rendered: Option<&GlobalTransform>) -> Option<GlobalTransform> {
    match (tick, driving) {
        (Some(tick), false) => Some(GlobalTransform::from(tick)),
        _ => rendered.copied(),
    }
}
fn send_pose(
    mut net: ResMut<Multiplayer>,
    skater: Res<SkaterRuntime>,
    vehicles: Res<crate::modding::vehicles::Vehicles>,
    camera: Query<&GlobalTransform, With<crate::camera::GameplayCamera>>,
    presentation: Res<crate::presentation::Presentation>,
    (real, fixed): (Res<Time<Real>>, Res<Time<Fixed>>),
) {
    if !net.active() || skater.pose_generation == 0 {
        return;
    }
    let now = sim_stamp(&net, &real, &fixed);
    // perf 10-18: 20 Hz (was 10; the lobby sends it at that rate only to the nearest).
    let every = if net.loopback { 50 } else { skate_net::knob!("SKATE_NET_POSE_MS", 50) };
    if net.last_pose.elapsed() >= Duration::from_millis(every.saturating_sub(1)) {
        let mut pose = vehicles
            .network_pose
            .clone()
            .unwrap_or_else(|| network::capture_pose(&skater, &net.anchors));
        // Packed::pose takes 32 rows; the rig's anchors leave room (a full set goes without the camera).
        let tick = presentation.pair().map(|(_, current)| current.camera);
        if let (Some(camera), true) = (sent_camera(tick, vehicles.camera().is_some(), camera.single().ok()), pose.bones.len() < 32) {
            pose.bones.push(camera_row(&camera));
        }
        if let Some(p) = Packed::pose(&pose) {
            net.lobby.as_mut().unwrap().publish(packed::POSE, p, now);
        }
        net.last_pose = Instant::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// User 10-29: a 4-player session showed nobody's mic, voice on or not: a player had voice only once heard. Now a
    /// guest's `@voice` reaches the host, and off takes it back.
    #[test]
    fn a_players_voice_on_reaches_the_others() {
        let info = |id| Info { id, map: 1, rig: 2, physics: 3, appearance: 4 };
        let (mut host, mut guest) = (Multiplayer::for_test(Session::new(7, info(10), None)), Multiplayer::for_test(Session::new(7, info(20), Some(1))));
        let mut now = 0;
        let mut pump = |host: &mut Multiplayer, guest: &mut Multiplayer| {
            for _ in 0..20 {
                now += 50;
                let (h, g) = (host.session_mut().unwrap(), guest.session_mut().unwrap());
                for p in h.service(now) {
                    g.receive(1, &p.data, now);
                }
                for p in g.service(now) {
                    h.receive(2, &p.data, now);
                }
            }
        };
        pump(&mut host, &mut guest);
        assert!(host.voiced().is_empty(), "nothing said yet");
        guest.publish_voice(true);
        pump(&mut host, &mut guest);
        assert_eq!(host.voiced(), [20].into());
        guest.publish_voice(false);
        pump(&mut host, &mut guest);
        assert!(host.voiced().is_empty(), "voice turned off");
    }

    /// User 10-09: spectating followed the watched skater's body, not their camera. The camera now rides the pose
    /// stream as its own row, survives the wire (far from the origin too) and never reaches the skeleton.
    #[test]
    fn the_senders_camera_rides_the_pose_stream_and_is_split_off_the_bones() {
        let at = Transform::from_xyz(812.5, 40.25, -1210.)
            .with_rotation(Quat::from_rotation_y(1.1) * Quat::from_rotation_x(-0.3));
        let bone = |index| skate_net::Bone { index, pose: skate_net::Pose { p: [1., 2., 3.], q: [0., 0., 0., 1.] } };
        let mut bones: Vec<_> = (0..31).map(bone).collect();
        bones.push(camera_row(&GlobalTransform::from(at)));
        let state = skate_net::packed::PoseState { root: bone(0).pose, bones };
        let wire = skate_net::packed::delta(1, 2, packed::POSE, 1, &Packed::pose(&state).unwrap(), None);
        let mut back = packed::apply(&wire, None).unwrap().unpack_pose().unwrap().bones;
        let camera = take_camera(&mut back).unwrap();
        assert_eq!(back.len(), 31, "the skeleton keeps only its anchors");
        assert!(Vec3::from_array(camera.p).distance(at.translation) < 1e-3);
        assert!(Quat::from_array(camera.q).angle_between(at.rotation) < 0.01);
        assert!(take_camera(&mut back).is_none(), "an older sender's pose has none");
    }

    /// User 10-29: the camera sent is the bones' own tick's gameplay camera, whatever was drawn (a tick behind, or an
    /// editor's); a vehicle's is the one drawn.
    #[test]
    fn the_camera_sent_is_the_bones_ticks() {
        let (tick, drawn) = (Transform::from_xyz(1., 2., 3.), GlobalTransform::from(Transform::from_xyz(9., 9., 9.)));
        assert_eq!(sent_camera(Some(tick), false, Some(&drawn)), Some(GlobalTransform::from(tick)));
        assert_eq!(sent_camera(Some(tick), true, Some(&drawn)), Some(drawn), "driving");
        assert_eq!(sent_camera(None, false, Some(&drawn)), Some(drawn), "no tick yet");
        assert_eq!(sent_camera(None, false, None), None);
    }

    /// Audit 10-28 #13: a skater past 100 m sends once a second, so one moving fast covered more than the 10 m a sample
    /// may jump and hopped (a teleport) at every update. The jump allowed grows with the time between samples.
    #[test]
    fn a_far_fast_skater_is_followed_not_teleported() {
        let at = |x: f32| skate_net::Pose { p: [x, 0., 0.], q: [0., 0., 0., 1.] };
        let body = BodyState { root: at(0.), enabled: 0, bodies: vec![] };
        let now = Instant::now();
        let mut r = Remote { roots: VecDeque::new(), poses: VecDeque::new(), epoch: 0, visual_since: 0, body, body_at: now, body_seq: 0, pose_seq: 0 };
        push_root(&mut r, 1_000, 1_000, at(0.));
        push_root(&mut r, 2_000, 2_000, at(15.));
        assert_eq!(r.epoch, 0, "15 m in a second: skating");
        push_root(&mut r, 2_050, 2_050, at(30.));
        assert_eq!(r.epoch, 1, "15 m in 50 ms: a teleport");
        push_root(&mut r, 3_050, 3_050, at(330.));
        assert_eq!(r.epoch, 2, "300 m in a second: a teleport");
    }

    /// Audit 10-28 #7: when only our link to one member died, the host still listed it, and its skater stood frozen on
    /// our screen for the rest of the session. It goes while that link is silent and comes back once it answers.
    #[test]
    fn a_skater_whose_link_went_silent_is_hidden_until_it_answers() {
        let info = |id| Info { id, map: 1, rig: 2, physics: 3, appearance: 4 };
        let mut s = [Session::new(7, info(10), None), Session::new(7, info(11), Some(1)), Session::new(7, info(12), Some(1))];
        let root = skate_net::Pose { p: [0.; 3], q: [0., 0., 0., 1.] };
        let body = skate_net::Body { pose: root, velocity: [0.; 3], angular: [0.; 3] };
        let state = Packed::body(&packed::BodyState { root, enabled: 0, bodies: vec![body; 33] }).unwrap();
        // Endpoint i + 1 is s[i]; `cut` drops what goes between the two guests (endpoints 2 and 3).
        let run = |s: &mut [Session; 3], now: &mut u64, rounds: u64, cut: bool| {
            for _ in 0..rounds {
                *now += 50;
                s[2].publish(packed::BODY, state.clone(), *now);
                for from in 0..3 {
                    for p in s[from].service(*now) {
                        let Some(to) = (p.peer as usize).checked_sub(1).filter(|&t| t < 3) else { continue };
                        if !(cut && matches!((from, to), (1, 2) | (2, 1))) {
                            s[to].receive(from as u64 + 1, &p.data, *now);
                        }
                    }
                }
            }
        };
        let mut now = 0;
        run(&mut s, &mut now, 20, false);
        let mut remotes = BTreeMap::new();
        sync_remotes(&mut remotes, &s[1], 2, &[], now);
        assert!(remotes.contains_key(&12), "it moves on our screen");
        run(&mut s, &mut now, skate_net::lobby::LINK_TIMEOUT_MS / 50 + 4, true);
        sync_remotes(&mut remotes, &s[1], 2, &[], now);
        assert!(s[1].actors.contains_key(&12), "the host still lists it");
        assert!(!remotes.contains_key(&12), "but it is hidden");
        assert!(spawned(&s[1], &remotes, 12), "a match barrier still counts it, as the frozen skater was");
        assert!(!spawned(&s[1], &remotes, 99), "one never seen is not");
        run(&mut s, &mut now, 40, false);
        sync_remotes(&mut remotes, &s[1], 2, &[], now);
        assert!(remotes.contains_key(&12), "its link answers: back");
    }

    /// Audit 10-28 #6: Quit went straight out, so the others saw our skater frozen for 5 s. Quitting the game leaves the
    /// session first.
    #[test]
    fn quitting_the_game_leaves_the_session() {
        let info = Info { id: 10, map: 1, rig: 2, physics: 3, appearance: 4 };
        let mut app = App::new();
        app.add_message::<AppExit>()
            .insert_resource(Multiplayer::for_test(Session::new(7, info, None)))
            .add_systems(Last, leave_on_exit);
        app.update();
        assert!(app.world().resource::<Multiplayer>().active(), "playing");
        app.world_mut().write_message(AppExit::Success);
        app.update();
        assert!(!app.world().resource::<Multiplayer>().active(), "left");
    }

    /// 10-12 audit (modes handoff): Entity.DoesSkaterExist for a match's load barrier. Ours always exists; another
    /// player's once its skater is spawned here (movement arrived) and it is on our map; nobody else's.
    #[test]
    fn a_skater_exists_once_spawned_on_our_map() {
        let info = |id, map| Info { id, map, rig: 2, physics: 3, appearance: 4 };
        let mut host = Session::new(7, info(1, 5), None);
        for (endpoint, id, map) in [(2, 20, 5), (3, 30, 99)] {
            let mut guest = Session::new(7, info(id, map), Some(1));
            for now in [10, 20, 30] {
                for p in guest.service(now) {
                    host.receive(endpoint, &p.data, now);
                }
                // The host's cookie, then admission (SK8NET14).
                for p in host.service(now).into_iter().filter(|p| p.peer == endpoint) {
                    guest.receive(1, &p.data, now);
                }
            }
        }
        assert!(host.actors.contains_key(&20) && host.actors.contains_key(&30));
        assert!(exists(&host, 5, 1, false), "our own skater");
        assert!(!exists(&host, 5, 20, false), "in the session, not spawned yet");
        assert!(exists(&host, 5, 20, true));
        assert!(!exists(&host, 5, 30, true), "spawned, but on another map");
        assert!(!exists(&host, 5, 40, true), "not in the session");
    }

    /// Online audit M7: a peer on another SK8NET version was dropped silently (both sides just never connected). The
    /// host's lobby now names both versions in its notice, which `net.status` shows, and only once.
    #[test]
    fn another_network_version_is_named_not_dropped_silently() {
        let info = |id| Info { id, map: 1, rig: 2, physics: 3, appearance: 4 };
        let mut host = Session::new(7, info(1), None);
        let mut hello = packed::header(7, 2, skate_net::lobby::HELLO, 0);
        hello.resize(packed::HEADER + 40, 0);
        hello[..8].copy_from_slice(b"SK8NET08");
        host.receive(99, &hello, 1);
        assert_eq!(host.stats.invalid, 1, "still refused, never misread");
        assert_eq!(host.mismatch, Some(*b"08"));
        assert!(host.notice.contains("version 08") && host.notice.contains("this build 18"), "{}", host.notice);
        host.notice.clear();
        host.receive(99, &hello, 2);
        assert!(host.notice.is_empty(), "said once");
        // 10-12 end review I1: an SK8NET12 build (online1) lacks the match relay's EndMark / Voted / sequenced events.
        let mut host = Session::new(7, info(1), None);
        hello[..8].copy_from_slice(b"SK8NET12");
        host.receive(99, &hello, 1);
        assert_eq!((host.stats.invalid, host.mismatch), (1, Some(*b"12")), "an SK8NET12 build is refused");
        // A build 11 (SK8NET17) lacks the 150-character chat line, the spectate eye and the Skate 1 trick state's id /
        // switch (its relay drops every event that carries them).
        let mut host = Session::new(7, info(1), None);
        hello[..8].copy_from_slice(b"SK8NET17");
        host.receive(99, &hello, 1);
        assert_eq!((host.stats.invalid, host.mismatch), (1, Some(*b"17")), "a build 11 is refused");
        // Not another game's or a garbled datagram.
        let mut quiet = Session::new(7, info(1), None);
        hello[..8].copy_from_slice(b"SK8NET\x1b\n"); // Q12: a stranger's bytes never reach the screen
        for junk in [&b"XXXXXXXX"[..], b"SK8NE", b"SK8NET18", &hello] { // 18: ours, truncated
            quiet.receive(99, junk, 1);
        }
        assert_eq!((quiet.mismatch, quiet.notice.as_str()), (None, ""));
    }

    /// Ledger #54 (user 10-26, "randomly disconnecting for a second"): a player whose movement stopped arriving for
    /// 3.5 s was deleted and spawned again when it came back (its look applied again: a blink). It now holds its last
    /// pose until the lobby drops its link at LINK_TIMEOUT_MS.
    #[test]
    fn a_silent_player_stays_frozen_until_the_link_drops() {
        let info = |id| Info { id, map: 5, rig: 2, physics: 3, appearance: 4 };
        let (mut host, mut guest) = (Session::new(7, info(1), None), Session::new(7, info(20), Some(1)));
        let (mut remotes, mut now) = (BTreeMap::new(), 0);
        // A 50 ms tick both ways; `movement` false drops the host's BODY (its movement stalls while the link lives),
        // `link` false drops everything.
        let tick = |host: &mut Session, guest: &mut Session, now: &mut u64, link: bool, movement: bool| {
            *now += 50;
            for p in guest.service(*now) {
                if link {
                    host.receive(2, &p.data, *now);
                }
            }
            for p in host.service(*now).into_iter().filter(|p| p.peer == 2) {
                if link && (movement || p.data[24] != packed::BODY) {
                    guest.receive(1, &p.data, *now);
                }
            }
        };
        for _ in 0..10 {
            tick(&mut host, &mut guest, &mut now, true, true);
        }
        let pose = skate_net::Pose { p: [1., 0., 2.], q: [0., 0., 0., 1.] };
        let body = skate_net::Body { pose, velocity: [0.; 3], angular: [0.; 3] };
        host.publish(packed::BODY, Packed::body(&packed::BodyState { root: pose, enabled: 0, bodies: vec![body; 33] }).unwrap(), now);
        for _ in 0..30 {
            tick(&mut host, &mut guest, &mut now, true, true);
        }
        sync_remotes(&mut remotes, &guest, 2, &[], now);
        let seq = remotes.get(&1).map(|r| r.body_seq).expect("the host's skater arrives");
        for _ in 0..200 {
            tick(&mut host, &mut guest, &mut now, true, false);
            sync_remotes(&mut remotes, &guest, 2, &[], now);
        }
        assert_eq!(remotes.get(&1).map(|r| r.body_seq), Some(seq), "10 s without movement: kept, frozen on its last pose");
        let silent = now;
        while now <= silent + skate_net::lobby::LINK_TIMEOUT_MS + 1_000 {
            tick(&mut host, &mut guest, &mut now, false, false);
            sync_remotes(&mut remotes, &guest, 2, &[], now);
        }
        assert!(!remotes.contains_key(&1), "gone once the lobby drops the link");
    }
}
