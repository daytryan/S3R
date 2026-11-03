//! What the whole session shares beyond the roster: the host's difficulty, guests' challenge proposals, and session
//! moves (the host picks a challenge or location, or accepts a proposal, and everyone travels there and rejoins the
//! host's new lobby). All of it lives in the host's `@session` record (online_roster); the host's accept popup is
//! online_menu's.
use crate::{
    challenge_map::ChallengeMap,
    config::Config,
    difficulty::Difficulty,
    graphics_menu::Menu,
    multiplayer::Multiplayer,
    online_roster::OnlineRoster,
    online_session::{capacity, Follow, OnlineSession},
    skate2_physics::settings::Physics,
    world_time::{WorldTime, CYCLE},
};
use bevy::prelude::*;
use skate_net::roster::{
    ACCEPT_KEY, Accept, COHOST_KEY, CoHost, CoHostAsk, Move, PROPOSE_KEY, Propose, Proposal, SWITCH_KEY, SessionState, Switch, Target, World,
};
use std::collections::BTreeMap;

/// Seconds between a move's announcement and leaving for it. Guests go first, so the host leaving is not a migration
/// (online_roster's leave grace hides their departures). Knobs: user 10-29, 5 and 6 s held everyone on the old map.
const GUEST_DELAY: f32 = 2.;
const HOST_DELAY: f32 = 3.;
/// Not retail (online safety P6/Q6): the session's world, a guest's proposal and a forced move come at most this
/// often (s), the latest kept, so a host or co-host flipping them can't keep a player loading nor a guest keep the
/// host's popup up; and a guest always has the time between moves to leave. Knobs.
const WORLD_EVERY: f32 = 3.;
/// Reduce Flashing (Q6): a guest takes the host's world at most this often (s). Knob.
const CALM_WORLD_EVERY: f32 = 15.;
fn world_every(reduce_flashing: bool) -> f32 {
    if reduce_flashing { CALM_WORLD_EVERY } else { WORLD_EVERY }
}
const PROPOSE_EVERY: f32 = 30.;
const MOVE_EVERY: f32 = 30.;
fn due(now: f32, last: Option<f32>, every: f32) -> bool {
    last.is_none_or(|l| now - l >= every)
}
/// When a guest follows a move announced at `now`: after its `delay`, and [`MOVE_EVERY`] after the last one followed.
fn follow_at(now: f32, delay: f32, last: Option<f32>) -> f32 {
    (now + delay).max(last.map_or(f32::MIN, |l| l + MOVE_EVERY))
}
/// When this player follows a move announced at `now`: `alone`, a host with nobody to wait for.
fn follow_time(now: f32, host: bool, alone: bool, last: Option<f32>) -> f32 {
    match (host, alone) {
        (true, true) => now,
        (true, false) => follow_at(now, HOST_DELAY, last),
        (false, _) => follow_at(now, GUEST_DELAY, last),
    }
}

/// The co-host a newly made host `local` hands the session to: the first one appointed that is still `here`; none
/// when `local` is a co-host itself.
fn successor(state: &SessionState, local: u64, here: impl Fn(u64) -> bool) -> Option<u64> {
    (!state.is_cohost(local)).then(|| state.cohosts.iter().copied().find(|&a| a != local && here(a))).flatten()
}

pub(crate) struct OnlineMovesPlugin;
impl Plugin for OnlineMovesPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (difficulty, physics, world, intake, votes, teams, cohost, follow, lines).chain().after(crate::online_roster::track));
    }
}

/// Host only: announce a move (this clears any proposal, and ends a match under way).
pub(crate) fn start_move(state: &mut SessionState, target: Target) {
    state.proposal = None;
    state.game = None;
    state.moving = Some(Move { token: crate::multiplayer::unique(), target });
}

/// Retail "All players in Session use this Difficulty": the host's goes out, guests take it unsaved, and their own
/// comes back when the session ends.
fn difficulty(
    net: Res<Multiplayer>,
    mut roster: ResMut<OnlineRoster>,
    mut menu: ResMut<Menu>,
    mut physics: ResMut<crate::physics::GamePhysics>,
    mut config: ResMut<Config>,
    mut own: Local<Option<Difficulty>>,
) {
    let (active, _, host) = net.mod_identity();
    let guest = active && !host;
    if menu.guest != guest {
        menu.guest = guest;
    }
    if !active {
        if let Some(d) = own.take() {
            menu.adopt_difficulty(d, &mut physics, &mut config);
        }
        return;
    }
    if host {
        let d = Some(menu.difficulty() as u8);
        if roster.state.difficulty != d {
            roster.state.difficulty = d;
        }
        return;
    }
    let Some(d) = roster.state.difficulty.and_then(|i| Difficulty::ALL.get(i as usize).copied()) else { return };
    if menu.difficulty() != d {
        own.get_or_insert(menu.difficulty());
        menu.adopt_difficulty(d, &mut physics, &mut config);
    }
}

/// The physics a session record carries: the host's, Skate 3 from a host without the field (older builds had only it).
fn session_physics(state: &SessionState) -> Physics {
    state.physics.and_then(|i| Physics::ALL.get(usize::from(i)).copied()).unwrap_or_default()
}

/// User 10-28 (spec 2026-10-20 P4 / section 7): the host picks the physics for everyone in the session, as
/// Difficulty: the host's goes out, guests take it unsaved, and their own comes back when the session ends;
/// skate2_physics' switch acts on the menu's. Not retail (Skate 2 has one physics). A guest takes a new one at most
/// every WORLD_EVERY (online safety Q6: a host flipping it can't keep handing the skater over).
fn physics(
    net: Res<Multiplayer>,
    mut roster: ResMut<OnlineRoster>,
    mut menu: ResMut<Menu>,
    time: Res<Time<Real>>,
    mut own: Local<Option<Physics>>,
    mut applied: Local<Option<f32>>,
) {
    let (active, _, host) = net.mod_identity();
    if !active {
        if let Some(p) = own.take() {
            menu.adopt_physics(p);
        }
        *applied = None;
        return;
    }
    if host {
        // Only where it acts (New San Vanelona): omitted is Skate 3, and `@session` keeps its room for a team lobby's
        // fields (Skate 3 maps only; roster's worst-case fit test).
        let p = (menu.physics() == Physics::Skate2 && menu.on_nsv).then_some(1);
        if roster.state.physics != p {
            roster.state.physics = p;
        }
        return;
    }
    let (p, now) = (session_physics(&roster.state), time.elapsed_secs());
    if menu.physics() != p && due(now, *applied, WORLD_EVERY) {
        own.get_or_insert(menu.physics());
        menu.adopt_physics(p);
        *applied = Some(now);
    }
}

/// Seconds between the host's clock updates while the day cycles. Knob.
const CLOCK_RESEND: f32 = 10.;
/// Game hours a guest's cycling clock may drift from the host's before it snaps. Knob.
const CLOCK_SLACK: f32 = 0.1;

/// Seconds a co-host's world pick shows before the host's record must have it. Knob.
const WORLD_WAIT: f32 = 3.;

/// Settings > World in a session: it starts Clear and Day (whatever the players' own are), the host's Game Settings
/// change it for everyone (with the clock while the day cycles), a co-host's ask the host to, other guests can't.
/// Everyone's own come back after.
#[allow(clippy::too_many_arguments)]
fn world(
    mut net: ResMut<Multiplayer>,
    mut roster: ResMut<OnlineRoster>,
    mut menu: ResMut<Menu>,
    mut clock: ResMut<WorldTime>,
    time: Res<Time<Real>>,
    mut sent: Local<f32>,
    mut heard: Local<Option<u16>>,
    mut asked: Local<Option<([u32; 2], f32)>>,
    mut applied: Local<Option<f32>>,
) {
    let (active, _, host) = net.mod_identity();
    let cohost = net.cohost(&roster.state);
    if menu.cohost != cohost {
        menu.cohost = cohost;
    }
    let request = menu.world_request.take();
    if !active {
        if menu.session_world().is_some() {
            menu.set_session_world(None);
        }
        *heard = None;
        *asked = None;
        *applied = None;
        return;
    }
    let now = time.elapsed_secs();
    if let Some([weather, time_of_day]) = request.filter(|_| cohost)
        && net.publish_cohost(CoHostAsk::World(weather as u8, time_of_day as u8))
    {
        *asked = Some(([weather, time_of_day], now));
    }
    if host {
        // A moved session keeps the host's (online_roster carries the state).
        let [weather, time_of_day] = match menu.session_world() {
            Some(w) => w,
            None => {
                // Not retail (user 10-19): on a real Halloween or Christmas a new session starts in that mode.
                let season = crate::seasonal::today().map_or(0, |s| s.weather());
                let w = roster.state.world.map_or([season, 0], |w| [w.weather.into(), w.time.into()]);
                menu.set_session_world(Some(w));
                w
            }
        };
        let changed = roster.state.world.is_none_or(|w| [u32::from(w.weather), u32::from(w.time)] != [weather, time_of_day]);
        if changed || (time_of_day == CYCLE && now - *sent >= CLOCK_RESEND) {
            *sent = now;
            let minute = (clock.hour * 60.) as u16 % 1440;
            roster.state.world = Some(World { weather: weather as u8, time: time_of_day as u8, minute });
        }
        return;
    }
    follow_world(roster.state.world, &mut menu, &mut clock, now, &mut asked, &mut applied, &mut heard);
}

/// A guest's side of [`world`], each frame: the host's world (`record`: None until its `@session` lands, Clear and Day
/// meanwhile) onto Settings > World, and its clock while the day cycles. `asked`: a co-host's own pick and when;
/// `applied`: when the host's world last went on; `heard`: the record whose clock was taken.
fn follow_world(
    record: Option<World>,
    menu: &mut Menu,
    clock: &mut WorldTime,
    now: f32,
    asked: &mut Option<([u32; 2], f32)>,
    applied: &mut Option<f32>,
    heard: &mut Option<u16>,
) {
    let w = record.unwrap_or_default();
    let want = Some([u32::from(w.weather), u32::from(w.time)]);
    // A co-host's own pick stays up until the host's record agrees (or it was refused).
    if let Some((pick, at)) = *asked {
        if want != Some(pick) && now - at < WORLD_WAIT {
            return;
        }
        *asked = None;
    }
    if menu.session_world() != want && due(now, *applied, world_every(menu.reduce_flashing())) {
        menu.set_session_world(want);
        // User 10-26: the Clear and Day stand-in before the host's first record is no flip of the host's, so that record
        // goes on the frame it lands (a joiner loads in with it), not WORLD_EVERY later.
        *applied = record.map(|_| now);
    }
    // Snap only on a new record whose world is on (a held one would spend it on the old world's clock): between them
    // the local clock runs on.
    if *heard != Some(w.minute) && menu.session_world() == want.map(Menu::clamp_world) {
        *heard = Some(w.minute);
        let host_hour = f32::from(w.minute) / 60.;
        let gap = (clock.hour - host_hour).rem_euclid(24.);
        if u32::from(w.time) == CYCLE && gap.min(24. - gap) > CLOCK_SLACK {
            clock.hour = host_hour;
        }
    }
}

/// Host only: a guest's new `@propose` becomes the session's proposal (online_menu asks the host) when proposals are
/// allowed, none is open, the session fits the target and the host has the row; the target is rebuilt from the
/// host's own row. A co-host's is a move at once, allowed or not. Records already there when this session (or our
/// hosting) began are not new.
#[allow(clippy::type_complexity)]
fn intake(
    net: Res<Multiplayer>,
    mut roster: ResMut<OnlineRoster>,
    menu: Res<Menu>,
    challenge: Option<Res<ChallengeMap>>,
    time: Res<Time<Real>>,
    mut seen: Local<(Option<(u64, u64, bool)>, BTreeMap<u64, u64>)>,
    mut proposed: Local<BTreeMap<u64, f32>>,
) {
    let Some(session) = net.session() else {
        seen.0 = None;
        proposed.clear();
        return;
    };
    let now = time.elapsed_secs();
    let key = (session.session, session.local, session.is_host());
    let records: Vec<(u64, Propose)> = (session.actors.iter())
        .filter(|(id, _)| **id != session.local)
        .filter_map(|(&id, a)| Some((id, Propose::decode(&a.application.get(PROPOSE_KEY)?.value)?)))
        .collect();
    if seen.0 != Some(key) {
        *seen = (Some(key), records.iter().map(|(a, p)| (*a, p.nonce)).collect());
        return;
    }
    if !key.2 {
        return;
    }
    for (actor, p) in records {
        if seen.1.insert(actor, p.nonce) == Some(p.nonce) {
            continue;
        }
        let cohost = roster.state.is_cohost(actor);
        if ignored(&roster.state, actor, crate::player_list::quiet(actor), proposed.get(&actor).copied(), now) {
            continue;
        }
        let found = challenge.as_ref().and_then(|c| c.find_online(&menu.map_labels(), p.target.tab as usize, &p.target.id));
        let fits = found.filter(|pick| roster.members.len() <= capacity(pick));
        let Some(target) = fits.and_then(|pick| pick.target()) else {
            // The proposer's menu offers only rows that fit: an old or modded client.
            warn!("Refused proposal {:?} from {actor:x}: no such row here, or {} players don't fit it", p.target.id, roster.members.len());
            continue;
        };
        if cohost {
            start_move(&mut roster.state, target);
            continue;
        }
        let by = crate::player_list::shown_name(&roster.members, actor).unwrap_or_default();
        roster.state.proposal = Some(Proposal { by, actor, target, token: p.nonce });
        proposed.insert(actor, now);
    }
}
/// Host: a new proposal not taken up: a move is on, or the proposer is muted or blocked (player_list); a guest's also
/// while proposals are off, one is open, or within [`PROPOSE_EVERY`] of their last.
fn ignored(state: &SessionState, actor: u64, quiet: bool, last: Option<f32>, now: f32) -> bool {
    state.moving.is_some() || quiet || !state.is_cohost(actor) && (state.no_proposals || state.proposal.is_some() || !due(now, last, PROPOSE_EVERY))
}

/// Retail's proposal lifetime: HostSetup+340, the "proposaltimeout" attribute's default (TU3 setter 0x82dcfdc8,
/// initialised at 0x8273e1cc); the deadline (now + timeout, in seconds) is set at 0x82dc54b0 and checked at
/// 0x82dc58a8. The Switch Team proposal runs on the same timeout (0x82dcb8b0).
pub(crate) const PROPOSAL_TIMEOUT: f32 = 15.;

/// Who has accepted `p`: its proposer (accepted on proposing, TU3 0x82dc54b0) and every player still here whose
/// `@accept` names it.
pub(crate) fn acceptors(p: &Proposal, members: &[u64], accepts: &BTreeMap<u64, Accept>) -> Vec<u64> {
    members.iter().copied().filter(|a| *a == p.actor || accepts.get(a).is_some_and(|x| x.token == p.token)).collect()
}
/// Retail's pass test (TU3 0x82dc57a0): accepted * 2 > players. UNRESOLVED: retail also passes a tie when an
/// acceptor's Blaze id reads -1 (0x82dc57a0's second branch, meaning not traced); here a tie does not pass.
pub(crate) fn passes(accepted: usize, players: usize) -> bool {
    accepted * 2 > players
}

fn records<T>(net: &Multiplayer, key: &str, decode: fn(&[u8]) -> Option<T>) -> BTreeMap<u64, T> {
    net.session().map_or_else(BTreeMap::new, |s| {
        (s.actors.iter()).filter_map(|(&a, actor)| Some((a, decode(&actor.application.get(key)?.value)?))).collect()
    })
}

/// The open proposal: everyone announces each accept (ID_ONLINE_EVENT_PROPOSAL_ACCEPTED); the host passes it once a
/// majority has accepted (a session move to its target: ID_EVENTLOG_PROPOSAL_PASSED) or lets it expire after
/// PROPOSAL_TIMEOUT (declined: ID_EVENTLOG_PROPOSAL_FAILURE_1 / _2). The host's own pick overrides it (start_move).
fn votes(
    net: Res<Multiplayer>,
    mut roster: ResMut<OnlineRoster>,
    time: Res<Time<Real>>,
    mut opened: Local<Option<(u64, f32)>>,
    mut announced: Local<(u64, Vec<u64>)>,
) {
    let (active, _, host) = net.mod_identity();
    let Some(p) = roster.state.proposal.clone().filter(|_| active) else {
        *opened = None;
        return;
    };
    let now = time.elapsed_secs();
    let members: Vec<u64> = roster.members.iter().map(|m| m.actor).collect();
    let accepted = acceptors(&p, &members, &records(&net, ACCEPT_KEY, Accept::decode));
    if announced.0 != p.token {
        *announced = (p.token, Vec::new());
    }
    for a in accepted.iter().filter(|a| **a != p.actor) {
        if !announced.1.contains(a) {
            announced.1.push(*a);
            let name = roster.members.iter().find(|m| m.actor == *a).map_or_else(String::new, |m| m.name.clone());
            roster.lines.push(("ID_ONLINE_EVENT_PROPOSAL_ACCEPTED", name));
        }
    }
    if !host {
        return;
    }
    let at = match *opened {
        Some((token, at)) if token == p.token => at,
        _ => opened.insert((p.token, now)).1,
    };
    if passes(accepted.len(), members.len()) {
        start_move(&mut roster.state, p.target);
    } else if now - at >= PROPOSAL_TIMEOUT {
        roster.state.proposal = None;
    }
}

/// Host only, team lobbies (decision T1: the host assigns teams): players start split into two teams (each newcomer
/// joins the smaller one, ties to the first), the list keeps to players still here, and a guest's Switch Team proposal
/// (`@switch`) opens for the host's answer (online_menu) while none is open. Outside a team lobby there are no teams.
fn teams(net: Res<Multiplayer>, mut roster: ResMut<OnlineRoster>, mut seen: Local<(Option<(u64, u64, bool)>, BTreeMap<u64, u64>)>) {
    let Some(session) = net.session() else {
        seen.0 = None;
        return;
    };
    let key = (session.session, session.local, session.is_host());
    let members: Vec<u64> = roster.members.iter().map(|m| m.actor).collect();
    let switches: Vec<(u64, u64)> = (session.actors.iter())
        .filter(|(id, _)| **id != session.local)
        .filter_map(|(&id, a)| Some((id, Switch::decode(&a.application.get(SWITCH_KEY)?.value)?.nonce)))
        .collect();
    if seen.0 != Some(key) {
        *seen = (Some(key), switches.into_iter().collect());
        return;
    }
    if !key.2 {
        return;
    }
    let state = &mut roster.state;
    if !state.challenge.as_ref().is_some_and(|c| c.team) {
        if !state.team_a.is_empty() || !state.team_b.is_empty() || state.switching.is_some() {
            (state.team_a, state.team_b, state.switching) = (Vec::new(), Vec::new(), None);
        }
        return;
    }
    keep_teams(state, &members);
    for (actor, nonce) in switches {
        if seen.1.insert(actor, nonce) != Some(nonce) && state.switching.is_none() && members.contains(&actor) {
            state.switching = Some(actor);
        }
    }
}
/// The teams and an open switch keep to players still here; anyone here on no team yet (a newcomer, or everyone in
/// a moved session's new lobby) is assigned, in roster order.
fn keep_teams(state: &mut SessionState, members: &[u64]) {
    if state.team_a.iter().chain(&state.team_b).any(|a| !members.contains(a)) {
        state.team_a.retain(|a| members.contains(a));
        state.team_b.retain(|a| members.contains(a));
    }
    if state.switching.is_some_and(|a| !members.contains(&a)) {
        state.switching = None;
    }
    for &a in members {
        assign(state, a);
    }
}
/// Retail's team size: each team's NIS slots pad to 3 (`for i = TeamSize+1, 3 do SubmitNISPlayerIndex(-1)`: 1up.lua
/// 1597, contest_shared 2508, domination 427, hall_of_meat 841); 2 x 3 = CHALLENGE_MAX.
pub(crate) const TEAM_MAX: usize = 3;
/// A newcomer joins the smaller team (the first on a tie) while it has room; a 7th player waits on no team (Start
/// Challenge refuses above CHALLENGE_MAX anyway) and takes a leaver's place. UNTRACED: retail's placement of a 7th
/// player in a team lobby is not recovered; this is ours.
fn assign(state: &mut SessionState, actor: u64) {
    if state.team_a.contains(&actor) || state.team_b.contains(&actor) {
        return;
    }
    let side = if state.team_b.len() < state.team_a.len() { &mut state.team_b } else { &mut state.team_a };
    if side.len() < TEAM_MAX {
        side.push(actor);
    }
}
/// Host: `actor` changes team (the host's answer to a Switch Team proposal, its own Switch Team, or its Player List
/// pick).
pub(crate) fn switch_team(state: &mut SessionState, actor: u64) {
    if state.switching == Some(actor) {
        state.switching = None;
    }
    // Not retail (user 10-12): a switch onto a side already holding TEAM_MAX is refused (the proposal closes, nobody
    // moves); retail's own answer is UNRECOVERED.
    let to_a = state.team_b.contains(&actor);
    if (if to_a { &state.team_a } else { &state.team_b }).len() >= TEAM_MAX {
        return;
    }
    if let Some(i) = state.team_b.iter().position(|a| *a == actor) {
        state.team_b.remove(i);
        state.team_a.push(actor);
    } else {
        state.team_a.retain(|a| *a != actor);
        state.team_b.push(actor);
    }
}
/// Teams.GetName for host-assigned teams. ponytail: retail reads the EA Online Team's name (TU3 Teams.GetName
/// 0x828383b8: team record + 2008), which no longer exists; retail's own unranked-team names stand in.
pub(crate) const TEAM_NAMES: [&str; 2] = ["ID_UNRANKEDNAME_TEAM1", "ID_UNRANKEDNAME_TEAM2"];

/// Host only, and the only place a co-host's power is used: their `@cohost` requests, each checked against the
/// host's own state (sender a co-host now; a kick never reaches the host or another co-host; world indices
/// clamped). Also keeps the co-host list to players still here, and makes a moved session's co-hosts co-hosts again
/// by their real endpoints.
fn cohost(
    mut net: ResMut<Multiplayer>,
    mut roster: ResMut<OnlineRoster>,
    mut menu: ResMut<Menu>,
    time: Res<Time<Real>>,
    mut seen: Local<(Option<(u64, u64, bool)>, BTreeMap<u64, u64>)>,
    mut world: Local<(Option<[u32; 2]>, Option<f32>)>,
) {
    let Some(session) = net.session() else {
        seen.0 = None;
        *world = default();
        return;
    };
    let now = time.elapsed_secs();
    let key = (session.session, session.local, session.is_host());
    let records: Vec<(u64, CoHost)> = (session.actors.iter())
        .filter(|(id, _)| **id != session.local)
        .filter_map(|(&id, a)| Some((id, CoHost::decode(&a.application.get(COHOST_KEY)?.value)?)))
        .collect();
    if seen.0 != Some(key) {
        // User 10-29: a co-host is the host's first successor. Steam hands a left host's lobby to any member: one who
        // isn't a co-host, made host by the migration (the same session, us a guest before), passes it on to the
        // first co-host still here. Without co-hosts, Steam's pick stays (another hand-over is another migration).
        let promoted = seen.0 == Some((key.0, key.1, false)) && key.2;
        if let Some(next) = promoted.then(|| successor(&roster.state, session.local, |a| session.actors.contains_key(&a))).flatten() {
            info!("MULTIPLAYER_HOST_TO_COHOST {next:x}");
            net.make_host(next);
        }
        *seen = (Some(key), records.iter().map(|(a, r)| (*a, r.nonce)).collect());
        return;
    }
    if !key.2 {
        return;
    }
    let local = session.local;
    let gone: Vec<u64> = roster.state.cohosts.iter().copied().filter(|a| *a == local || !session.actors.contains_key(a)).collect();
    let back: Vec<(u64, u64)> = (session.actors.keys())
        .filter(|_| !roster.state.cohost_endpoints.is_empty())
        .filter_map(|&a| Some((a, session.endpoint(a)?)))
        .filter(|(a, e)| roster.state.cohost_endpoints.contains(e) && !roster.state.is_cohost(*a))
        .collect();
    if !gone.is_empty() {
        roster.state.cohosts.retain(|a| !gone.contains(a));
    }
    for (actor, endpoint) in back {
        roster.state.cohost_endpoints.retain(|e| *e != endpoint);
        if roster.state.cohosts.len() < skate_net::roster::COHOSTS_MAX {
            roster.state.cohosts.push(actor);
        }
    }
    for (actor, request) in records {
        if seen.1.insert(actor, request.nonce) == Some(request.nonce) {
            continue;
        }
        if !allowed(&roster.state, local, actor, request.ask) {
            warn!("Refused {:?} from {actor:x}: not a co-host, or aimed at the host or a co-host", request.ask);
            continue;
        }
        match request.ask {
            CoHostAsk::Kick(target) => {
                if net.kick(&mut roster.state, target).is_some() {
                    net.publish_session(&roster.state);
                }
            }
            CoHostAsk::World(weather, time) => world.0 = Some([weather.into(), time.into()]),
        }
    }
    // The latest ask, at most every WORLD_EVERY s. set_session_world clamps the indices.
    if let Some(pick) = world.0.filter(|_| due(now, world.1, WORLD_EVERY)) {
        menu.set_session_world(Some(pick));
        *world = (None, Some(now));
    }
}
/// The host's check of a co-host request: the sender is a co-host now, and a kick is never at the host or a co-host.
fn allowed(state: &SessionState, host: u64, sender: u64, ask: CoHostAsk) -> bool {
    state.is_cohost(sender) && !matches!(ask, CoHostAsk::Kick(target) if target == host || state.is_cohost(target))
}

/// Everyone (the host too): a new move is followed after its delay. The row is found on this player's own map, and
/// the Start menu performs it as a session move (core_menu perform_online, online_session follow_intent).
#[allow(clippy::too_many_arguments)]
fn follow(
    net: Res<Multiplayer>,
    mut roster: ResMut<OnlineRoster>,
    menu: Res<Menu>,
    mut online: ResMut<OnlineSession>,
    mut challenge: Option<ResMut<ChallengeMap>>,
    mut ui: ResMut<crate::online_menu::OnlineUi>,
    (time, covered): (Res<Time<Real>>, Res<crate::tip_loader::Covered>),
    mut state: Local<(u64, Option<(u64, f32, bool, Target)>, Option<f32>, Option<u64>)>,
) {
    let now = time.elapsed_secs();
    let (active, _, host) = net.mod_identity();
    let (done, pending, last, behind) = &mut *state;
    // Audit 10-28 #10: a move this player stayed behind from (its row is not on this map) goes once it hosts the old
    // lobby: one still "moving" started no challenge and pulled every joiner into the dead move, alone.
    if host && roster.state.moving.as_ref().is_some_and(|m| Some(m.token) == *behind) {
        roster.state.moving = None;
    }
    if let Some(m) = roster.state.moving.as_ref().filter(|m| active && m.token != *done) {
        *done = m.token;
        // A newer move replaces a waiting one. Audit 10-28 #9: it is followed as the host this player was when it saw
        // it: a guest promoted while it waits (the host left first) still follows the host's new lobby; it opened its
        // own and split the session. End review (I1): and to the target it saw: the record may go meanwhile (#10's
        // left-behind host clears it), which dropped the wait and split the session too.
        *pending = Some((m.token, follow_time(now, host, roster.members.len() <= 1, *last), host, m.target.clone()));
    }
    let Some((token, at, host, target)) = pending.clone() else { return };
    if !active {
        *pending = None;
        return;
    }
    if now < at {
        return;
    }
    let pick = challenge.as_ref().and_then(|c| c.find_online(&menu.map_labels(), target.tab as usize, &target.id));
    let (Some(pick), Some(challenge)) = (pick, challenge.as_mut()) else {
        *pending = None;
        *behind = Some(token);
        roster.lines.push(("ID_ONLINE_EVENT_CANNOT_CHANGE_LOCATION", String::new()));
        return;
    };
    if online.follow.as_ref().is_none_or(|f| f.token != token) {
        info!("Session move {token:x} to {target:?}");
        *last = Some(now);
        // Whatever screen is up gives way, as a Steam join does.
        challenge.dismiss();
        ui.dismiss = true;
        let mut carry = if host { roster.state.clone() } else { SessionState::default() };
        // Actor ids are new in the next lobby; the co-hosts' endpoints find them there (cohost).
        if let Some(session) = net.session().filter(|_| host) {
            carry.cohost_endpoints = carry.cohosts.iter().filter_map(|&a| session.endpoint(a)).collect();
        }
        online.follow = Some(Follow { token, host, carry });
        // User 10-26: retail moves a session behind its loading screen; this lobby is left once it is up.
        online.cover(now);
    }
    if covered.0 {
        *pending = None;
        challenge.picked_online = Some(pick);
    }
}

/// The event log's lines for changes to the host's `@session` (retail ids). Item 30: only lines TU3 has; a joiner's
/// first difficulty (SESSION_DIFFICULTY_SET_*) and a new proposal (ID_ONLINE_EVENT_CHALLENGE_PROPOSED; its poster is
/// the notice) are not in the image, so they log nothing.
pub(crate) fn changes(old: &SessionState, new: &SessionState) -> Vec<(&'static str, String)> {
    const CHANGED: [&str; 3] = [
        "ID_ONLINE_EVENT_SESSION_DIFFICULTY_CHANGED_EASY",
        "ID_ONLINE_EVENT_SESSION_DIFFICULTY_CHANGED_NORMAL",
        "ID_ONLINE_EVENT_SESSION_DIFFICULTY_CHANGED_HARD",
    ];
    let line = |id: &'static str| (id, String::new());
    let mut out = Vec::new();
    match (old.difficulty, new.difficulty) {
        (Some(a), Some(b)) if a != b => out.extend(CHANGED.get(b as usize).map(|&id| line(id))),
        _ => {}
    }
    if old.no_proposals != new.no_proposals {
        out.push(line(if new.no_proposals {
            "ID_ONLINE_EVENT_HOST_DISABLES_PROPOSALS"
        } else {
            "ID_ONLINE_EVENT_HOST_ALLOWS_PROPOSALS"
        }));
    }
    match (&old.proposal, &new.proposal, &new.moving) {
        (Some(p), None, Some(m)) if m.target == p.target => out.push(line("ID_EVENTLOG_PROPOSAL_PASSED")),
        (Some(_), None, Some(_)) => out.push(line("ID_EVENTLOG_PROPOSAL_OVERRIDDEN")),
        (Some(_), None, None) => out.extend([line("ID_EVENTLOG_PROPOSAL_FAILURE_1"), line("ID_EVENTLOG_PROPOSAL_FAILURE_2")]),
        _ => {}
    }
    if let (None, Some(m)) = (&old.moving, &new.moving) {
        let place = crate::challenge_map::place_name(&m.target.map)
            .map_or_else(|| m.target.map.replace('_', " "), |p| p.trim_start_matches('#').to_string());
        out.push(("ID_ONLINE_EVENT_LOCATION_CHANGE", format!("{place}\u{1f}{}", m.target.name)));
    }
    out
}

/// The event log's Switch Team lines: every move between the teams, with the team's name (SWITCHED_TEAMS, 0x825187f8).
/// A proposal logs nothing (ID_ONLINE_EVENT_SWITCH_TEAM_PROPOSED_PLAYER is not a TU3 log line; the host's popup asks).
fn team_lines(old: &SessionState, new: &SessionState, name: impl Fn(&u64) -> Option<String>) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    for (to, from, team) in [(&new.team_a, &old.team_b, TEAM_NAMES[0]), (&new.team_b, &old.team_a, TEAM_NAMES[1])] {
        for a in to.iter().filter(|a| from.contains(a)) {
            out.extend(name(a).map(|n| ("ID_ONLINE_EVENT_SWITCHED_TEAMS", format!("{n}\u{1f}{team}"))));
        }
    }
    out
}

fn lines(net: Res<Multiplayer>, mut roster: ResMut<OnlineRoster>, mut last: Local<Option<((u64, u64), SessionState)>>) {
    let Some(key) = net.session().map(|s| (s.session, s.local)) else {
        *last = None;
        return;
    };
    let (old, known) = match last.take() {
        Some((k, s)) if k == key => (s, true),
        _ => (SessionState::default(), false),
    };
    if old != roster.state {
        let mut new = changes(&old, &roster.state);
        // Co-hosts made or unmade (not ones already there when we joined, nor ones who left).
        let name = |actor: &u64| roster.members.iter().find(|m| m.actor == *actor).map(|m| m.name.clone());
        if known {
            let made = roster.state.cohosts.iter().filter(|a| !old.is_cohost(**a)).filter_map(name);
            new.extend(made.map(|n| ("{0:s} is now a co-host.", n)));
            let unmade = old.cohosts.iter().filter(|a| !roster.state.is_cohost(**a)).filter_map(name);
            new.extend(unmade.map(|n| ("{0:s} is no longer a co-host.", n)));
            new.extend(team_lines(&old, &roster.state, name));
        }
        roster.lines.extend(new);
    }
    *last = Some((key, roster.state.clone()));
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Audit 10-28 #10: a guest without the move's map stays behind; hosting the old lobby once the host left for the
    /// new one, its `@session` still said "moving": no challenge could start and every joiner was pulled into the dead
    /// move, alone. The move goes once that player hosts.
    #[test]
    fn a_player_left_behind_by_a_move_drops_it_when_it_hosts() {
        let info = skate_net::lobby::Info { id: 11, map: 1, rig: 2, physics: 3, appearance: 4 };
        let mut app = App::new();
        app.init_resource::<Time<Real>>()
            .init_resource::<OnlineSession>()
            .init_resource::<crate::online_menu::OnlineUi>()
            .insert_resource(crate::tip_loader::Covered(false))
            .insert_resource(crate::graphics_menu::test_menu())
            .insert_resource(Multiplayer::for_test(skate_net::lobby::Session::new(7, info, Some(1))))
            .add_systems(Update, follow);
        let mut roster = OnlineRoster::default();
        let target = Target { tab: 0, id: "a_dlc_row".into(), ..Default::default() };
        roster.state.moving = Some(Move { token: 9, target });
        app.insert_resource(roster);
        app.update();
        app.world_mut().resource_mut::<Time<Real>>().advance_by(std::time::Duration::from_secs(60));
        // Its turn: the row is not on its map (no challenge map here), so it stays.
        app.update();
        assert!(app.world().resource::<OnlineRoster>().lines.iter().any(|(id, _)| *id == "ID_ONLINE_EVENT_CANNOT_CHANGE_LOCATION"));
        app.world_mut().resource_mut::<Multiplayer>().session_mut().unwrap().migrate(None, 0);
        app.update();
        assert!(app.world().resource::<OnlineRoster>().state.moving.is_none(), "hosting: the dead move goes");
    }
    /// User 10-29: a host who leaves goes to the first co-host still here; a co-host made host keeps it.
    #[test]
    fn a_left_hosts_first_successor_is_its_first_cohost() {
        let state = SessionState { cohosts: vec![5, 6], ..Default::default() };
        assert_eq!(successor(&state, 9, |_| true), Some(5));
        assert_eq!(successor(&state, 9, |a| a != 5), Some(6), "the first one still here");
        assert_eq!(successor(&state, 6, |_| true), None, "a co-host made host keeps it");
        assert_eq!(successor(&SessionState::default(), 9, |_| true), None, "no co-hosts: Steam's pick");
    }
    /// End review 10-28 (I1): a guest still waiting out its delay lost the move when the record went (a left-behind
    /// player hosting the old lobby clears it, #10) and stayed behind: the session split. A move once seen is followed.
    #[test]
    fn a_seen_move_is_followed_though_its_record_goes() {
        let info = skate_net::lobby::Info { id: 12, map: 1, rig: 2, physics: 3, appearance: 4 };
        let mut app = App::new();
        app.init_resource::<Time<Real>>()
            .init_resource::<OnlineSession>()
            .init_resource::<crate::online_menu::OnlineUi>()
            .insert_resource(crate::tip_loader::Covered(false))
            .insert_resource(crate::graphics_menu::test_menu())
            .insert_resource(Multiplayer::for_test(skate_net::lobby::Session::new(7, info, Some(1))))
            .add_systems(Update, follow);
        let mut roster = OnlineRoster::default();
        roster.state.moving = Some(Move { token: 9, target: Target { tab: 0, id: "a_row".into(), ..Default::default() } });
        app.insert_resource(roster);
        app.update();
        app.world_mut().resource_mut::<OnlineRoster>().state.moving = None;
        app.world_mut().resource_mut::<Time<Real>>().advance_by(std::time::Duration::from_secs(60));
        app.update();
        // It still went to follow (no challenge map here, so the row is not on its map: the line says so).
        assert!(app.world().resource::<OnlineRoster>().lines.iter().any(|(id, _)| *id == "ID_ONLINE_EVENT_CANNOT_CHANGE_LOCATION"));
    }
    /// User 10-28: the session's physics is the host's; a host without the field (an older build) or a made-up value
    /// is Skate 3.
    #[test]
    fn a_sessions_physics_is_the_hosts() {
        assert_eq!(session_physics(&SessionState::default()), Physics::Skate3);
        assert_eq!(session_physics(&SessionState { physics: Some(1), ..Default::default() }), Physics::Skate2);
        assert_eq!(session_physics(&SessionState { physics: Some(0), ..Default::default() }), Physics::Skate3);
        assert_eq!(session_physics(&SessionState { physics: Some(200), ..Default::default() }), Physics::Skate3);
    }
    #[test]
    fn moves_world_flips_and_proposals_are_rate_limited() {
        assert!(due(0., None, WORLD_EVERY) && !due(1., Some(0.), WORLD_EVERY) && due(3., Some(0.), WORLD_EVERY));
        // Q6: Reduce Flashing holds a guest's world 15 s between the host's flips.
        assert!(due(3., Some(0.), world_every(false)) && !due(10., Some(0.), world_every(true)) && due(15., Some(0.), world_every(true)));
        assert_eq!(follow_at(100., GUEST_DELAY, None), 100. + GUEST_DELAY, "the first move after its delay");
        assert_eq!(follow_at(100., GUEST_DELAY, Some(90.)), 120., "a second within MOVE_EVERY waits it out");
        // Audit 10-24: the host left 6 s after a second move while its guests waited out MOVE_EVERY in the old lobby;
        // one of them became its host and hosted the move too, splitting the session. The host waits it out as well,
        // still after its guests (its last move was a second after theirs); alone it goes at once.
        assert_eq!(follow_time(100., true, false, None), 100. + HOST_DELAY);
        assert_eq!((follow_time(100., false, false, Some(90.)), follow_time(100., true, false, Some(91.))), (120., 121.));
        assert_eq!(follow_time(100., true, true, Some(91.)), 100.);
        let (guest, cohost) = (4, 2);
        let state = SessionState { cohosts: vec![cohost], ..default() };
        assert!(!ignored(&state, guest, false, None, 0.));
        assert!(ignored(&state, guest, true, None, 0.), "muted or blocked");
        assert!(ignored(&state, cohost, true, None, 0.));
        assert!(ignored(&state, guest, false, Some(0.), 10.) && !ignored(&state, guest, false, Some(0.), 31.), "one every 30 s");
        assert!(!ignored(&state, cohost, false, Some(0.), 10.), "a co-host's move is no popup");
        let open = SessionState { proposal: Some(Proposal { by: "Bo".into(), actor: 9, target: Target::new(3, "a", "A", "ID"), token: 1 }), ..state };
        assert!(ignored(&open, guest, false, None, 0.), "one open at a time");
    }
    /// User 10-26: a joiner loaded in on Clear and Day and took the host's weather and time up to WORLD_EVERY later
    /// (CALM_WORLD_EVERY with Reduce Flashing): the stand-in before the host's first record started the hold on flips.
    /// The host's world goes on the frame its record lands, a cycling host's hour with it.
    #[test]
    fn a_joiner_takes_the_hosts_world_the_frame_it_lands() {
        let snow_night = World { weather: 4, time: 1, minute: 0 };
        let rain_cycle = World { weather: 2, time: CYCLE as u8, minute: 18 * 60 };
        for (calm, record) in [(false, snow_night), (true, snow_night), (false, rain_cycle)] {
            let mut menu = crate::graphics_menu::test_menu();
            if calm {
                menu.choose(crate::graphics_menu::Setting::ReduceFlashing, 1);
            }
            let (mut clock, mut asked, mut applied, mut heard) = (WorldTime::default(), None, None, None);
            // In the lobby, the host not heard from yet: Clear and Day stand in.
            follow_world(None, &mut menu, &mut clock, 0., &mut asked, &mut applied, &mut heard);
            assert_eq!(menu.session_world(), Some([0, 0]));
            follow_world(Some(record), &mut menu, &mut clock, 0.5, &mut asked, &mut applied, &mut heard);
            assert_eq!(menu.session_world(), Some([u32::from(record.weather), u32::from(record.time)]), "{record:?}, Reduce Flashing {calm}");
            if u32::from(record.time) == CYCLE {
                assert_eq!(clock.hour, 18., "the host's hour");
            }
        }
    }
    /// A switch to the cycle held by WORLD_EVERY took the host's clock while still on Day, whose noon (world_time) then
    /// overwrote it: the cycle ran from noon until the host's next clock, 10 s on. The clock goes on with the cycle.
    #[test]
    fn a_held_switch_to_the_cycle_takes_the_hosts_hour_with_it() {
        let mut menu = crate::graphics_menu::test_menu();
        let (mut clock, mut asked, mut applied, mut heard) = (WorldTime::default(), None, None, None);
        let day = World { weather: 0, time: 0, minute: 0 };
        follow_world(Some(day), &mut menu, &mut clock, 0., &mut asked, &mut applied, &mut heard);
        let cycle = World { time: CYCLE as u8, minute: 18 * 60, ..day };
        let mut now = 1.;
        while now < WORLD_EVERY + 0.5 {
            follow_world(Some(cycle), &mut menu, &mut clock, now, &mut asked, &mut applied, &mut heard);
            if now < WORLD_EVERY {
                assert_eq!(menu.session_world(), Some([0, 0]), "held at {now} s");
            }
            // world_time::tick, a frame.
            clock.hour = crate::world_time::next_hour(menu.time_of_day(), clock.hour, 0.25);
            now += 0.25;
        }
        assert_eq!(menu.session_world(), Some([0, CYCLE]));
        assert!((clock.hour - 18.).abs() < 0.2, "the host's hour, not noon's: {}", clock.hour);
    }
    #[test]
    fn the_host_refuses_what_a_cohost_may_not_do() {
        let (host, cohost, other_cohost, guest) = (1, 2, 3, 4);
        let state = SessionState { cohosts: vec![cohost, other_cohost], ..default() };
        assert!(allowed(&state, host, cohost, CoHostAsk::Kick(guest)));
        assert!(allowed(&state, host, cohost, CoHostAsk::World(2, 1)));
        assert!(!allowed(&state, host, cohost, CoHostAsk::Kick(host)), "never the host");
        assert!(!allowed(&state, host, cohost, CoHostAsk::Kick(other_cohost)), "never a co-host");
        assert!(!allowed(&state, host, guest, CoHostAsk::Kick(cohost)), "a guest's request is ignored");
        assert!(!allowed(&state, host, guest, CoHostAsk::World(0, 0)));
        // The record round-trips; anything else is dropped.
        let request = CoHost { nonce: 9, ask: CoHostAsk::World(2, 1) };
        assert_eq!(CoHost::decode(&request.encode()), Some(request));
        assert_eq!(CoHost::decode(br#"{"k":1,"r":{"x":5}}"#), None);
        let crowded = SessionState { cohosts: (0..=skate_net::roster::COHOSTS_MAX as u64).collect(), ..default() };
        assert_eq!(SessionState::decode(&serde_json::to_vec(&crowded).unwrap()), None, "too many co-hosts");
    }
    #[test]
    fn session_changes_become_retail_lines() {
        let target = Target::new(3, "hom_dwtn_01", "DownTown", "ID_SPOT");
        let ids = |old: &SessionState, new: &SessionState| changes(old, new).into_iter().map(|(id, _)| id).collect::<Vec<_>>();
        let start = SessionState::default();
        let easy = SessionState { difficulty: Some(0), ..start.clone() };
        // Item 30: SESSION_DIFFICULTY_SET_* is not in TU3.
        assert!(ids(&start, &easy).is_empty(), "a joiner's first difficulty logs nothing");
        let hard = SessionState { difficulty: Some(2), ..start.clone() };
        assert_eq!(ids(&easy, &hard), ["ID_ONLINE_EVENT_SESSION_DIFFICULTY_CHANGED_HARD"]);
        let off = SessionState { no_proposals: true, ..easy.clone() };
        assert_eq!(ids(&easy, &off), ["ID_ONLINE_EVENT_HOST_DISABLES_PROPOSALS"]);
        let proposed = SessionState { proposal: Some(Proposal { by: "Ann".into(), actor: 2, target: target.clone(), token: 1 }), ..easy.clone() };
        assert!(changes(&easy, &proposed).is_empty(), "item 30: no TU3 line; the poster is the notice");
        assert_eq!(ids(&proposed, &easy), ["ID_EVENTLOG_PROPOSAL_FAILURE_1", "ID_EVENTLOG_PROPOSAL_FAILURE_2"], "declined");
        let mut accepted = proposed.clone();
        start_move(&mut accepted, target.clone());
        assert_eq!(accepted.proposal, None);
        assert_eq!(changes(&proposed, &accepted), [
            ("ID_EVENTLOG_PROPOSAL_PASSED", String::new()),
            ("ID_ONLINE_EVENT_LOCATION_CHANGE", "ID_DISTRICT_DOWNTOWN\u{1f}ID_SPOT".to_string()),
        ]);
        let mut overridden = proposed.clone();
        start_move(&mut overridden, Target::new(2, "ID_LOC", "University", "ID_LOC"));
        assert_eq!(ids(&proposed, &overridden), ["ID_EVENTLOG_PROPOSAL_OVERRIDDEN", "ID_ONLINE_EVENT_LOCATION_CHANGE"]);
        // Bug 10-09 #11: the place name the Locations list shows, never the map's file stem.
        let line = |map: &str| {
            let mut moved = easy.clone();
            start_move(&mut moved, Target::new(2, "r", map, "ID_SPOT"));
            changes(&easy, &moved).pop().unwrap().1
        };
        assert_eq!(line("Art_Gallery"), "Art Gallery\u{1f}ID_SPOT", "a literal place name loses its '#'");
        assert_eq!(line("DownTownSkatePark"), "ID_WORLD_SKATEPARK_DOWNTOWN\u{1f}ID_SPOT", "a skate park");
        assert_eq!(line("Some_Mod_Map"), "Some Mod Map\u{1f}ID_SPOT", "an unlisted map reads as words");
        assert!(changes(&accepted, &accepted).is_empty());
    }

    /// U4: the proposer counts as accepted; an accept counts only for the open proposal and only from a player still
    /// here; a strict majority passes (retail accepted * 2 > players).
    #[test]
    fn a_majority_of_accepts_passes_the_proposal() {
        let p = Proposal { by: "Ann".into(), actor: 2, target: Target::new(3, "r", "m", "n"), token: 7 };
        let accepts: BTreeMap<u64, Accept> = [(3, Accept { token: 7 }), (4, Accept { token: 6 }), (9, Accept { token: 7 })].into();
        let members = [1, 2, 3, 4];
        let yes = acceptors(&p, &members, &accepts);
        assert_eq!(yes, [2, 3], "the proposer and the current accept; a stale token and a player who left don't count");
        assert!(!passes(yes.len(), members.len()), "2 of 4 is a tie: no pass");
        assert!(passes(3, 4) && passes(2, 3) && passes(1, 1) && !passes(1, 2));
        assert_eq!(PROPOSAL_TIMEOUT, 15.);
    }

    /// T1: newcomers fill the smaller team (ties to the first), a player who left drops out, Switch Team moves a
    /// player across and closes its open proposal; each move is an event line with the team's name.
    #[test]
    fn the_host_splits_players_into_two_teams_and_switches_them() {
        let mut s = SessionState::default();
        keep_teams(&mut s, &[10, 11, 12, 13, 14]);
        assert_eq!((s.team_a.as_slice(), s.team_b.as_slice()), (&[10, 12, 14][..], &[11, 13][..]));
        keep_teams(&mut s, &[10, 12, 13, 14, 15]);
        assert_eq!((s.team_a.as_slice(), s.team_b.as_slice()), (&[10, 12, 14][..], &[13, 15][..]), "11 left; 15 fills team 2");
        let name = |a: &u64| Some(format!("P{a}"));
        let before = s.clone();
        switch_team(&mut s, 10);
        assert_eq!(team_lines(&before, &s, name), [("ID_ONLINE_EVENT_SWITCHED_TEAMS", "P10\u{1f}ID_UNRANKEDNAME_TEAM2".to_string())]);
        let old = s.clone();
        s.switching = Some(13);
        assert!(team_lines(&old, &s, name).is_empty(), "item 30: a proposal is no TU3 line");
        let asked = s.clone();
        switch_team(&mut s, 13);
        assert_eq!((s.switching, s.team(13), s.team(15)), (None, 1, 2));
        assert_eq!(team_lines(&asked, &s, name), [("ID_ONLINE_EVENT_SWITCHED_TEAMS", "P13\u{1f}ID_UNRANKEDNAME_TEAM1".to_string())]);
        // A newcomer's first team is no switch.
        let before = s.clone();
        keep_teams(&mut s, &[10, 12, 13, 14, 15, 16]);
        assert!(s.team_a.contains(&16) || s.team_b.contains(&16));
        assert!(team_lines(&before, &s, name).is_empty());
    }

    /// Matrix G4 (10-12): a team holds retail's 3 (TEAM_MAX). A 7th player waits on no team, a leaver's place goes to
    /// them (never 4v2), and Switch Team onto a full side is refused (never 4v2 / 6v0).
    #[test]
    fn teams_hold_three_a_side() {
        let mut s = SessionState::default();
        keep_teams(&mut s, &[1, 2, 3, 4, 5, 6, 7]);
        assert_eq!((s.team_a.as_slice(), s.team_b.as_slice()), (&[1, 3, 5][..], &[2, 4, 6][..]), "7 waits on no team");
        keep_teams(&mut s, &[1, 2, 3, 5, 6, 7]);
        assert_eq!((s.team_a.as_slice(), s.team_b.as_slice()), (&[1, 3, 5][..], &[2, 6, 7][..]), "4 left: 3v3, not 4v2");
        s.switching = Some(2);
        let before = s.clone();
        switch_team(&mut s, 2);
        assert_eq!((s.team_a.len(), s.team_b.len(), s.switching), (3, 3, None), "team 1 is full: the proposal closes, nobody moves");
        assert!(team_lines(&before, &s, |a: &u64| Some(format!("P{a}"))).is_empty());
        keep_teams(&mut s, &[1, 2, 3, 5, 6]);
        switch_team(&mut s, 1);
        assert_eq!((s.team_a.as_slice(), s.team_b.as_slice()), (&[3, 5][..], &[2, 6, 1][..]), "team 2 had room");
        assert_eq!(TEAM_MAX * 2, crate::match_core::CHALLENGE_MAX);
    }
}
