//! After an Online Challenge Map spawn: the client's own lobby slot, the session marker auto-set and the
//! retail online-area boundary (data/boundary/<key>.xml, cMsgBoundaryTriggered): leaving the polygon shows
//! ID_ONLINE_BOUNDARY_WARN and starts the out-of-bounds countdown, coming back cancels it, running out puts the
//! skater back on its spawn slot. Skate 1 rows instead: walls on the polygon and retail's edge static, no warning
//! and no countdown.
// ponytail: the challenge-boundary trigger volumes (ChallengeBoundary / RoundBoundary in the mission Sim
// streams) are not loaded; only the online-area polygon is enforced.
use crate::online_map::Spawn;
use bevy::prelude::*;

/// OutOfBoundsTime's framework default (challenge_local_data_framework).
const OOB_SECONDS: f32 = 5.;
/// freeskate_ol: SessionMarker.SetSessionMarker() on a 1 s timer after the spawn reposition.
const MARKER_DELAY: f32 = 1.;
/// Boundary checks pause this long after a respawn, until the skater has moved.
const GRACE: f32 = 1.;

/// The online row the session was started from.
#[derive(Clone, Debug, Default)]
pub(crate) struct Area {
    pub spawn: Spawn,
    slot: usize,
    /// The slot is settled for the current lobby.
    placed: bool,
    countdown: Countdown,
    marker_in: Option<f32>,
    grace: f32,
    /// CurrentMap.generation of the map the pick loaded: set by a same-map pick, else taken on the first
    /// settled frame.
    pub generation: Option<u64>,
}

impl Area {
    pub fn new(spawn: Spawn) -> Self {
        Self { spawn, grace: GRACE, ..default() }
    }

    /// Settled in a lobby: leaving that lobby ends the area (online_session::run, which runs in menus too).
    pub fn placed(&self) -> bool {
        self.placed
    }

    /// `rejects`: the ground below a point is one the respawn refuses (physics::unrideable_below).
    fn spawn_matrix(&self, rejects: impl Fn([f32; 4]) -> bool) -> Option<[[f32; 4]; 4]> {
        let start = self.spawn.start.as_ref()?;
        let m = start.slot(self.slot);
        // Not retail (user 10-12): retail lobbies stayed within a row's 6-9 children; at 50 the resolver's 5 m line
        // leaves most free-skate areas (INDU_NORTH's from slot 7) and the expiry put the skater back outside for good.
        // Past the children and outside the area, or (user 10-30) over Unrideable ground, which now teleports: the
        // children in turn.
        // ponytail: wrapped clients share a child (skaters pass through each other); a ring around it if that looks bad.
        let n = start.slots.len();
        let outside = self.spawn.boundary.as_deref().is_some_and(|p| p.len() >= 3 && !inside(p, [m[3][0], m[3][2]]));
        Some(match n {
            _ if self.slot < n || !(outside || rejects(m[3])) => m,
            0 => start.matrix,
            _ => start.slots[self.slot % n],
        })
    }

    /// Where the lobby placement sends this skater: its slot's spawn; none for slot 0 (the host), where the pick put
    /// everyone already. M1 (end review 10-12): by slot, not by matrix: a client wrapped onto child 0 still travels.
    fn arrival(&self, rejects: impl Fn([f32; 4]) -> bool) -> Option<[[f32; 4]; 4]> {
        (self.slot != 0).then(|| self.spawn_matrix(rejects)).flatten()
    }

    /// False once the lobby it was placed in is left or another map is loaded: the row no longer applies.
    fn keep(&mut self, in_lobby: bool, generation: u64) -> bool {
        *self.generation.get_or_insert(generation) == generation && (in_lobby || !self.placed)
    }
}

// ponytail: remote bodies that have not streamed in yet count as absent; a host-assigned slot published with
// the lobby application is the full fix if shared spawns show up in testing.
/// A client's spawn slot: its own `rank` (its place among the lobby's clients, the same on every peer) while that child
/// is free or past the children, else the first free child, else its rank anyway.
/// Not retail (user 10-12): retail's playerSlot (lobby slot) is not on our wire; a rank by actor id stands in. A lobby
/// moved together places before anyone's new position arrives, so the free-child search alone put them all on child 1,
/// and every client without a free child on the same `members - 1` slot.
fn free_slot(start: Option<&crate::online_map::Start>, remotes: &[[f32; 3]], rank: usize) -> usize {
    let free = |m: &[[f32; 4]; 4]| {
        remotes.iter().all(|p| (0..3).map(|i| (p[i] - m[3][i]).powi(2)).sum::<f32>() > 4.)
    };
    let Some(s) = start else { return rank };
    if s.slots.get(rank).is_none_or(|m| free(m)) {
        return rank;
    }
    (1..s.slots.len()).find(|&i| free(&s.slots[i])).unwrap_or(rank)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Event {
    Warn,
    Cancel,
    Expire,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Countdown {
    left: Option<f32>,
}

impl Countdown {
    /// The out-of-bounds static: freeskate_ol's Effects.StartEffect(OOB, OutOfBoundsTime) ramps it 0 to 1 over the
    /// countdown; coming back (ResetEffect) or running out cuts it.
    pub fn amount(&self, seconds: f32) -> f32 {
        self.left.map_or(0., |t| (1. - t / seconds).clamp(0., 1.))
    }
    pub fn step(&mut self, inside: bool, dt: f32, seconds: f32) -> Option<Event> {
        match (inside, self.left) {
            (true, Some(_)) => {
                self.left = None;
                Some(Event::Cancel)
            }
            (true, None) => None,
            (false, None) => {
                self.left = Some(seconds);
                Some(Event::Warn)
            }
            (false, Some(t)) if t - dt <= 0. => {
                self.left = None;
                Some(Event::Expire)
            }
            (false, Some(t)) => {
                self.left = Some(t - dt);
                None
            }
        }
    }
}

/// The polygon's edges and each one's X/Z distance from `p` (0x823f5f68's segment test).
fn edges(polygon: &[[f32; 2]], p: Vec2) -> impl Iterator<Item = (Vec2, Vec2, f32)> + '_ {
    (0..polygon.len()).map(move |i| {
        let (a, b) = (Vec2::from(polygon[i]), Vec2::from(polygon[(i + 1) % polygon.len()]));
        let t = ((p - a).dot(b - a) / (b - a).length_squared().max(f32::EPSILON)).clamp(0., 1.);
        (a, b, p.distance(a + (b - a) * t))
    })
}

/// Skate 1's online-area static (0x823f7320, every frame) on the distance to the nearest edge (0x823f5f68): 0.5
/// within 5 m, then 0.5 - 0.4 (d - 0.5) / 8, none from 13 m. Skate 1 posts no warning and runs no countdown online:
/// its walls (skate1_walls) keep the skater in.
// ponytail: the ramp going negative past 10.5 m reads as none (the consumer's clamp isn't traced).
pub(crate) fn skate1_static(polygon: &[[f32; 2]], [x, z]: [f32; 2]) -> f32 {
    let d = edges(polygon, Vec2::new(x, z)).map(|e| e.2).fold(f32::MAX, f32::min);
    if d <= 5. { 0.5 } else if d < 13. { (0.5 - 0.4 * (d - 0.5) / 8.).max(0.) } else { 0. }
}

/// Skate 1's online-area walls (0x823f6b70): each polygon edge as two triangles from 500 below to 500 above, static
/// world to the solve with the static world's material (physics settings' floor_material: the moving volume's own
/// wins). Only the edges within 20 m of `at` (X/Z) are built.
// ponytail: retail builds every edge once with flags 0x300 (edge cosines, vertex 0 off); these are two-sided with no
// cosines and rebuilt each step near the skater, which a flat wall face doesn't tell apart.
pub(crate) fn skate1_walls(polygon: &[[f32; 2]], at: [f32; 2]) -> Vec<skate_core::physics::board_world::BoardWorldVolume> {
    use skate_core::{math::Vector3, physics::world_contact::{ContactPrimitive, triangle_from_volume}};
    let material =
        skate_core::physics::contact::RetailContactMaterial { static_friction: 0., dynamic_friction: 0., restitution: 1. };
    let v = |c: Vec2, y: f32| Vector3::new(c.x, y, c.y);
    edges(polygon, Vec2::from(at))
        .filter(|&(a, b, d)| d < 20. && a != b)
        .flat_map(|(a, b, _)| [[v(a, -500.), v(b, -500.), v(b, 500.)], [v(a, -500.), v(b, 500.), v(a, 500.)]])
        .map(|t| skate_core::physics::board_world::BoardWorldVolume {
            body: skate_core::physics::board_step::CollisionBody::StaticWorld,
            primitive: ContactPrimitive::Triangle(triangle_from_volume(t, 0., [-1.; 3], 0)),
            linear_velocity: Vector3::ZERO,
            material,
        })
        .collect()
}

/// Puts the walls in this physics step's network proxies (multiplayer::prepare has just refilled them) while a
/// Skate 1 row's area holds.
pub(crate) fn add_skate1_walls(
    session: Res<crate::online_session::OnlineSession>,
    map: Res<crate::map_transition::CurrentMap>,
    net: Res<crate::multiplayer::Multiplayer>,
    skater: Res<crate::physics::SkaterRuntime>,
    mut physics: ResMut<crate::physics::GamePhysics>,
) {
    let Some(polygon) = session.area.as_ref().filter(|a| a.placed).and_then(|a| a.spawn.boundary.as_deref()) else {
        return;
    };
    if polygon.len() < 3 || !net.mod_identity().0 || !crate::skate1_hud::skate1_map(&map) {
        return;
    }
    let at = skater.animated_skeleton.roots.animation_to_world[3];
    physics.network_proxies.volumes.extend(skate1_walls(polygon, [at[0], at[2]]));
}

/// Even-odd point-in-polygon on world X/Z.
pub(crate) fn inside(polygon: &[[f32; 2]], [x, z]: [f32; 2]) -> bool {
    let mut inside = false;
    let mut j = polygon.len().wrapping_sub(1);
    for (i, &[xi, zi]) in polygon.iter().enumerate() {
        let [xj, zj] = polygon[j];
        if (zi > z) != (zj > z) && x < (xj - xi) * (z - zi) / (zj - zi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

pub(crate) fn update(
    mut session: ResMut<crate::online_session::OnlineSession>,
    net: Res<crate::multiplayer::Multiplayer>,
    mut skater: ResMut<crate::physics::SkaterRuntime>,
    mut marker: Option<ResMut<crate::session_marker::SessionMarker>>,
    mut roster: ResMut<crate::online_roster::OnlineRoster>,
    (transition, map, travel, time, matched): (
        Res<crate::map_transition::MapTransition>,
        Res<crate::map_transition::CurrentMap>,
        Res<crate::teleport_menu::Travel>,
        Res<Time>,
        Option<Res<crate::match_core::Match>>,
    ),
    (challenge_map, menu, physics): (
        Option<Res<crate::challenge_map::ChallengeMap>>,
        Res<crate::graphics_menu::Menu>,
        Res<crate::physics::GamePhysics>,
    ),
    mut looked: Local<Option<u64>>,
) {
    // Still travelling: the spawn teleport has not run yet (a same-map pick's waits for the loading screen).
    let travelling = session.awaiting_world
        || session.arrival.is_some()
        || transition.busy()
        || travel.pending.is_some()
        || travel.pending_matrix.is_some()
        || skater.player_input.pending_teleport().is_some();
    // The static follows the countdown below; any early return has none.
    if let Some(marker) = marker.as_mut().filter(|m| m.oob != 0.) {
        marker.oob = 0.;
    }
    // User 10-29: a guest that joined by invite, Join Game or the server browser has no pick of its own: it takes
    // the row the host's `@session` names, so its own slot there, not the map's single-player spawn.
    let (in_lobby, _, host) = net.mod_identity();
    if session.area.is_none() && in_lobby && !host && !travelling {
        let state = &roster.state;
        let row = state.challenge.as_ref().and_then(|c| c.row.as_ref()).map(|r| (usize::from(r.tab), r.id.as_str()));
        // Once a lobby, once its `@session` names a row (the map is cloned from its cache).
        if (state.location.is_some() || row.is_some()) && *looked != net.session().map(|s| s.session) {
            *looked = net.session().map(|s| s.session);
            let online = challenge_map.as_ref().and_then(|c| c.online_map(&menu.map_labels()).ok());
            if let Some(spawn) = online.and_then(|o| o.session_spawn(state.location.as_deref(), row, travel.destinations())) {
                let mut area = Area::new(spawn);
                area.generation = Some(map.generation);
                session.area = Some(area);
            }
        }
    }
    let Some(area) = session.area.as_mut() else { return };
    if travelling {
        area.countdown = Countdown::default();
        return;
    }
    let dt = time.delta_secs();
    if !area.keep(in_lobby, map.generation) {
        session.area = None;
        return;
    }
    if in_lobby && !area.placed {
        // Retail repositions every player onto its own slot, then sets the session marker 1 s later.
        let members = net.session().map_or(1, |s| s.actors.len());
        if host || members > 1 {
            area.placed = true;
            area.marker_in = Some(MARKER_DELAY);
            area.slot = if host {
                0
            } else {
                let remotes: Vec<_> = net.collision_players().into_iter().map(|(_, b)| b.root.p).collect();
                let rank = net.session().map_or(1, |s| {
                    1 + s.actors.keys().filter(|&&id| Some(id) != s.host_actor()).position(|&id| id == s.local).unwrap_or(0)
                });
                free_slot(area.spawn.start.as_ref(), &remotes, rank)
            };
            if let Some(m) = area.arrival(|p| crate::physics::unrideable_below(&physics, p)) {
                match skater.travel_to(m) {
                    Ok(()) => area.grace = GRACE,
                    Err(e) => warn!("Online spawn slot {}: {e}", area.slot),
                }
                return;
            }
        }
    }
    // Armed only by the placement above, so the auto-set is online only and once per placement.
    if let Some(left) = area.marker_in.as_mut() {
        *left -= dt;
        if *left <= 0. {
            area.marker_in = None;
            if let Some(marker) = marker.as_mut() {
                marker.request_set = true;
            }
        }
    }
    // A match's mode keeps its own area (its ChallengeBoundary / out-of-area rules); the lobby's countdown would double
    // its warning and send-back.
    if matched.is_some_and(|m| m.mode.is_some() && m.phase != crate::match_core::Phase::Idle) {
        area.countdown = Countdown::default();
        return;
    }
    // Without a start an expiry has nowhere to put the skater.
    let respawns = area.spawn.start.is_some();
    let Some(polygon) = area.spawn.boundary.as_deref().filter(|p| in_lobby && respawns && p.len() >= 3) else {
        area.countdown = Countdown::default();
        return;
    };
    if area.grace > 0. {
        area.grace -= dt;
        return;
    }
    let at = skater.animated_skeleton.roots.animation_to_world[3];
    let seconds = area.spawn.oob_time.unwrap_or(OOB_SECONDS);
    let skate1 = crate::skate1_hud::skate1_map(&map);
    // Skate 1 runs no countdown online: its walls (add_skate1_walls) keep the skater in.
    let event = if skate1 { None } else { area.countdown.step(inside(polygon, [at[0], at[2]]), dt, seconds) };
    match event {
        // Skate 3 (0x825DE4E0): the central message while outside, hidden on return (0x826641B8); Skate 2's log line.
        Some(Event::Warn) if crate::skate2_hud::hud_game(Some(&menu), &map) == 3 => {
            roster.central = Some("ID_ONLINE_BOUNDARY_WARN".into());
            roster.central_held = true;
        }
        Some(Event::Warn) => roster.lines.push(("ID_ONLINE_BOUNDARY_WARN", String::new())),
        Some(Event::Expire) => {
            roster.central_held = false;
            if let Some(m) = area.spawn_matrix(|p| crate::physics::unrideable_below(&physics, p)) {
                match skater.travel_to(m) {
                    Ok(()) => area.grace = GRACE,
                    Err(e) => warn!("Online boundary respawn: {e}"),
                }
            }
        }
        Some(Event::Cancel) => roster.central_held = false,
        None => {}
    }
    if let Some(marker) = marker.as_mut() {
        marker.oob = if skate1 { skate1_static(polygon, [at[0], at[2]]) } else { area.countdown.amount(seconds) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_in_polygon_on_x_z() {
        // ml/data/boundary/1up_bbx_01.xml
        let quad = [[-78., -88.], [156., -79.], [140., 163.], [-67., 177.]];
        assert!(inside(&quad, [0., 0.]));
        assert!(inside(&quad, [150., -70.]));
        assert!(!inside(&quad, [160., 0.]), "past the east edge");
        assert!(!inside(&quad, [0., 200.]));
        let concave = [[0., 0.], [10., 0.], [10., 10.], [5., 2.], [0., 10.]];
        assert!(inside(&concave, [2., 1.]));
        assert!(!inside(&concave, [5., 6.]), "in the notch");
        assert!(!inside(&[], [0., 0.]));
    }

    #[test]
    fn countdown_warns_once_cancels_on_return_and_expires() {
        let mut c = Countdown::default();
        assert_eq!(c.step(true, 0.1, 5.), None);
        assert_eq!(c.step(false, 0.1, 5.), Some(Event::Warn));
        assert_eq!(c.step(false, 2., 5.), None, "counting, no repeat warning");
        assert!((c.amount(5.) - 0.4).abs() < 1e-6, "the static ramps with the countdown");
        assert_eq!(c.step(true, 0.1, 5.), Some(Event::Cancel));
        assert_eq!(c.amount(5.), 0., "coming back cuts it");
        assert_eq!(c.step(true, 0.1, 5.), None);
        assert_eq!(c.step(false, 0.1, 3.), Some(Event::Warn), "a new exit restarts the full time");
        assert_eq!(c.step(false, 2.9, 3.), None);
        assert_eq!(c.step(false, 0.2, 3.), Some(Event::Expire));
        assert_eq!(c.step(false, 0.1, 3.), Some(Event::Warn), "still out after the respawn: a new countdown");
    }

    #[test]
    fn skate1_static_ramps_on_the_edge_distance() {
        let square = [[0., 0.], [100., 0.], [100., 100.], [0., 100.]];
        assert_eq!(skate1_static(&square, [50., 50.]), 0., "13 m or more from every edge");
        assert_eq!(skate1_static(&square, [3., 50.]), 0.5);
        assert_eq!(skate1_static(&square, [-2., 50.]), 0.5, "outside, by the edge");
        assert!((skate1_static(&square, [50., 91.5]) - 0.1).abs() < 1e-6);
        assert_eq!(skate1_static(&square, [12., 50.]), 0., "past 10.5 m the ramp is spent");
    }

    #[test]
    fn skate1_walls_stand_on_the_near_edges() {
        use skate_core::physics::world_contact::ContactPrimitive;
        let square = [[0., 0.], [100., 0.], [100., 100.], [0., 100.]];
        assert!(skate1_walls(&square, [50., 50.]).is_empty(), "every edge 50 m off");
        let walls = skate1_walls(&square, [5., 5.]);
        assert_eq!(walls.len(), 4, "two triangles each for the two near edges");
        let ContactPrimitive::Triangle(t) = walls[0].primitive else { panic!("a triangle") };
        assert_eq!(t.vertices.map(|v| [v.x, v.y, v.z]), [[0., -500., 0.], [100., -500., 0.], [100., 500., 0.]]);
        assert!(t.feature.normal.y.abs() < 1e-6, "upright");
    }

    #[test]
    fn area_spawns_on_its_slot() {
        let start = crate::online_map::Start {
            matrix: [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [9., 9., 9., 1.]],
            slots: vec![[[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [1., 0., 0., 1.]]; 6],
        };
        let mut area = Area::new(Spawn { start: Some(start), ..default() });
        assert_eq!(area.marker_in, None, "the marker auto-set waits for the lobby placement");
        assert_eq!(area.grace, GRACE, "no boundary check on the spawn frame");
        // The area spawn is freeskate_ol's AI.RepositionAI(player, Location, playerSlot): the same locator resolver
        // (TU3 0x82910eb0, S2 0x82879950), so slot 7 past the 6 children is child 0 moved 5 m along -Z per slot.
        area.slot = 7;
        assert_eq!(area.spawn_matrix(|_| false).unwrap()[3], [1., 0., -35., 1.]);
        assert_eq!(Area::default().spawn_matrix(|_| false), None);
    }

    #[test]
    fn area_is_dropped_on_leaving_its_lobby_or_its_map() {
        let mut area = Area::default();
        assert!(area.keep(false, 4), "not placed yet: waiting for a lobby");
        area.placed = true;
        assert!(area.keep(true, 4));
        assert!(!area.keep(false, 4), "placed, then the lobby was left");
        let mut area = Area { placed: true, ..default() };
        assert!(area.keep(true, 4));
        assert!(!area.keep(true, 5), "another map was loaded");
    }

    #[test]
    fn client_takes_the_first_free_slot() {
        let at = |x: f32| [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [x, 0., 0., 1.]];
        let start = crate::online_map::Start { matrix: at(0.), slots: (0..6).map(|i| at(i as f32 * 10.)).collect() };
        assert_eq!(free_slot(Some(&start), &[], 1), 1);
        assert_eq!(free_slot(Some(&start), &[[0., 0., 0.], [10.5, 0., 0.], [30., 0., 0.]], 1), 2, "its own slot 1 is taken");
        let all: Vec<_> = (0..6).map(|i| [i as f32 * 10., 0., 0.]).collect();
        assert_eq!(free_slot(Some(&start), &all, 2), 2, "none free: its own");
        assert_eq!(free_slot(None, &[], 1), 1);
    }

    /// 10-12 audit (50 players): a full free-skate lobby moved to a location row with 6 spawn children (most retail
    /// rows). Everyone places at once, before anyone's new position arrives, so every client saw every child free and
    /// took child 1; with the children taken, every client took the same `members - 1` slot, 5 m x 49 = 245 m down the
    /// resolver's line, outside the area, where the out-of-bounds expiry put the skater back forever.
    #[test]
    fn fifty_clients_spawn_inside_the_area_on_their_own_slots() {
        let at = |x: f32, z: f32| [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [x, 0., z, 1.]];
        let start = crate::online_map::Start { matrix: at(0., 0.), slots: (0..6).map(|i| at(i as f32 * 3., 0.)).collect() };
        // 40 m around the children: the resolver's 5 m line leaves it from slot 5.
        let boundary = vec![[-20., -20.], [40., -20.], [40., 20.], [-20., 20.]];
        let taken: Vec<[f32; 3]> = (0..6).map(|i| [i as f32 * 3., 0., 0.]).collect();
        let pick = |remotes: &[[f32; 3]], rank: usize| free_slot(Some(&start), remotes, rank);
        let mut at_once = vec![];
        for rank in 1..50 {
            for remotes in [&[][..], &taken[..]] {
                let mut area = Area::new(Spawn { start: Some(start.clone()), boundary: Some(boundary.clone()), ..default() });
                area.slot = pick(remotes, rank);
                let m = area.spawn_matrix(|_| false).unwrap();
                assert!(inside(&boundary, [m[3][0], m[3][2]]), "rank {rank}: slot {} spawns outside the area", area.slot);
            }
            at_once.push(pick(&[], rank));
        }
        assert_eq!(at_once[..5], [1, 2, 3, 4, 5], "moved together, the first five clients each get their own child");
        assert_eq!(at_once[5..10], [6, 7, 8, 9, 10], "then their own steps down the resolver's line");
    }

    /// M1 (end review 10-12): a client wrapped back onto child 0 (past the children, outside the area) still travels
    /// there; placement skipped any spawn equal to slot 0's, so that client stayed wherever it was.
    #[test]
    fn a_wrapped_client_still_travels_to_its_spawn() {
        let at = |x: f32, z: f32| [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [x, 0., z, 1.]];
        let start = crate::online_map::Start { matrix: at(0., 0.), slots: (0..6).map(|i| at(i as f32 * 3., 0.)).collect() };
        let boundary = vec![[-20., -20.], [40., -20.], [40., 20.], [-20., 20.]];
        let mut area = Area::new(Spawn { start: Some(start.clone()), boundary: Some(boundary), ..default() });
        area.slot = 6;
        assert_eq!(area.spawn_matrix(|_| false), Some(start.slot(0)), "slot 6 wraps onto child 0");
        assert_eq!(area.arrival(|_| false), Some(start.slot(0)), "and travels there");
        area.slot = 0;
        assert_eq!(area.arrival(|_| false), None, "the host stays where the pick put it");
    }

    /// User 10-30: Unrideable now teleports (water.rs), so a slot past the children over ground the respawn rejects
    /// wraps onto the children as one outside the area does (University South's 7-9 stand over a catch plane); an
    /// authored child stays where retail put it.
    #[test]
    fn a_slot_past_the_children_over_unrideable_ground_wraps() {
        let at = |x: f32, z: f32| [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [x, 0., z, 1.]];
        let start = crate::online_map::Start { matrix: at(0., 0.), slots: (0..6).map(|i| at(i as f32 * 3., 0.)).collect() };
        let boundary = vec![[-100., -100.], [100., -100.], [100., 100.], [-100., 100.]];
        let mut area = Area::new(Spawn { start: Some(start.clone()), boundary: Some(boundary), ..default() });
        area.slot = 7;
        assert_eq!(area.spawn_matrix(|_| false), Some(start.slot(7)), "inside the area on rideable ground: its own step");
        let sea = |p: [f32; 4]| p[2] < -32.;
        assert_eq!(area.spawn_matrix(sea), Some(start.slots[1]), "over the sea: child 7 % 6");
        assert_eq!(area.arrival(sea), Some(start.slots[1]));
        area.slot = 2;
        assert_eq!(area.spawn_matrix(|_| true), Some(start.slots[2]), "an authored child is never moved");
    }
}
