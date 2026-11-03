//! The Skate 2 physics switch (Task 19): which physics has the local skater, and the hand-overs between them. Not
//! tested in play.
//!
//! `update` runs once per frame (Update):
//! - The wanted physics is `--physics` at launch, then the menu's saved setting from the first change on. The sim
//!   (Task 14) starts the first time it is wanted, and again on a switch after its retries stopped. It stays booted
//!   when the switch goes off (spec 3.4).
//! - Skate 2 physics may have the skater on New San Vanelona, out of a vehicle and while nothing forces Skate 3
//!   (`set_force_skate3`: the story mode's missions and challenges). Anywhere else the skater rides Skate 3 physics.
//!   Online (user 10-28, spec P4) the menu holds the session host's choice (online_moves::physics).
//! - Switching on: once the sim is `Ready`, a `Place` at the engine's root, heading and board velocity, ticked with a
//!   neutral pad until the port says it settled, then `drive`. After a restart (Amendment F, `replace_due`) the
//!   placement is at the held skater, at rest.
//! - Switching off (or when the retries stop): the engine's sim takes the skater back through its own teleport
//!   (`travel_to`) at the port's last root and heading, at rest (Step 1: the engine's post-teleport ground pin would
//!   stop a velocity, and releasing it changes code the game runs with the switch off).
//! - While Skate 2 physics has the skater, an engine teleport (`travel_to*`, a session-marker return, a map change)
//!   becomes a `Place` that ends the trick line (spec 3.5); the engine's request stays pending, so the modes see a
//!   teleport in flight, until the port settles, and is then dropped unlanded.
//! - Every placement has a deadline (`PLACE_DEADLINE` ticks); one that does not settle in time is a failed attempt
//!   (`Skate2Physics::fail_placement`), which restarts the sim.
//! - Both hand-overs cut the view (`discontinuity`). The camera options and the gestures follow the menu while the sim
//!   is booted (`SetProfile`, Amendment D; `SetGestures`).

use bevy::prelude::*;
use skate2_physics_protocol::{Boot, EngineMessage, Frame, Pad, Place};
use skate_core::physics::filtered_state::FilteredCategory;
use skate_data::audio::title::SKATE2_MAP;

use super::Skate2Physics;
use super::settings::{self, Physics};
use super::sim::HostState;
use crate::camera::CameraRuntime;
use crate::graphics_menu::Menu;
use crate::physics::{GamePhysics, SkaterRuntime};

/// Port ticks a placement may take to settle: 3 s. A placement settles in about 4 (Task 4).
pub const PLACE_DEADLINE: u32 = 180;

/// The sim's game mode: 3 (Skate 2's free skate) in Free Play, else 1.
pub fn host_mode(free_play: bool) -> u8 {
    if free_play { 3 } else { 1 }
}

impl Skate2Physics {
    /// The story mode's Skate 3 missions and challenges keep Skate 3 physics while this is on; the switch hands the
    /// skater over and back on its next update.
    pub fn set_force_skate3(&mut self, on: bool) {
        self.force_skate3 = on;
    }
}

/// The `Boot` the switch starts the sim with. `ambient` stays off: NSV's city is not part of the switch (Amendment C).
fn boot(menu: &Menu, free_play: bool) -> Boot {
    Boot {
        seed: None,
        exact_delivery: None,
        mode: host_mode(free_play),
        gestures: menu.skate2_gestures(),
        ambient: false,
        profile: settings::profile(menu),
    }
}

/// A placement on its way: the last port tick before it was sent, then the first tick and generation that answer it.
pub(crate) struct Placing {
    after: u32,
    first: Option<u32>,
    generation: Option<u32>,
    /// It lands an engine teleport, whose request is dropped once it settles.
    travel: bool,
}

impl Placing {
    pub(crate) fn new(after: u32, travel: bool) -> Self {
        Self { after, first: None, generation: None, travel }
    }

    /// `Some(true)` once `frame` says this placement settled, `Some(false)` past the deadline, else `None`. The first
    /// frame after the `Place` names its generation (the host answers in order).
    pub(crate) fn follow(&mut self, frame: &Frame) -> Option<bool> {
        if frame.tick <= self.after {
            return None;
        }
        let first = *self.first.get_or_insert(frame.tick);
        if let Some((generation, settled)) = frame.placed {
            if settled && *self.generation.get_or_insert(generation) == generation {
                return Some(true);
            }
            self.generation.get_or_insert(generation);
        }
        (frame.tick.saturating_sub(first) >= PLACE_DEADLINE).then_some(false)
    }
}

/// What the switch keeps between frames.
#[derive(Default)]
pub(crate) struct Switch {
    /// The menu's physics when last seen; `None` before the first update.
    menu: Option<Physics>,
    wanted: bool,
    /// The map generation when last seen.
    map: Option<u64>,
    /// Skate 2 physics had the skater after the last update.
    had: bool,
    /// The notice when last shown.
    notice: Option<String>,
    placing: Option<Placing>,
}

/// `at`'s position and heading (about +y from +z, the port's yaw and the engine's: `forward.x.atan2(forward.z)`).
fn position_heading(at: &[[f32; 4]; 4]) -> ([f32; 3], f32) {
    ([at[3][0], at[3][1], at[3][2]], at[2][0].atan2(at[2][2]))
}

/// A level transform at `position` facing `heading`, as the engine's teleports take one (`vehicles` builds the same).
fn level(position: [f32; 3], heading: f32) -> [[f32; 4]; 4] {
    let (sin, cos) = heading.sin_cos();
    [[cos, 0., -sin, 0.], [0., 1., 0., 0.], [sin, 0., cos, 0.], [position[0], position[1], position[2], 0.]]
}

/// A place the host has collision under: our NSV ground (the host's NSV cells are the disc's, which ours match).
fn grounded(game: Option<&GamePhysics>, at: [[f32; 4]; 4]) -> bool {
    game.is_none_or(|game| crate::skate_world::probe_ground(game.world(), at).1.is_some())
}

/// The cut on both hand-overs: the view snaps instead of blending.
fn cut(view: &mut CameraRuntime) {
    if let Some(frame) = view.frame.as_mut() {
        frame.discontinuity = true;
    }
}

/// The switch, once per frame. With the switch off it only compares the menu with what it saw last.
#[allow(clippy::too_many_arguments)]
pub(crate) fn update(
    mut s2: ResMut<Skate2Physics>,
    menu: Option<ResMut<Menu>>,
    config: Option<Res<crate::config::Config>>,
    map: Option<Res<crate::map_transition::CurrentMap>>,
    vehicles: Option<Res<crate::modding::vehicles::Vehicles>>,
    skater: Option<ResMut<SkaterRuntime>>,
    game: Option<Res<GamePhysics>>,
    view: Option<ResMut<CameraRuntime>>,
    mut roster: Option<ResMut<crate::online_roster::OnlineRoster>>,
    mut local: Local<Switch>,
) {
    let (Some(mut menu), Some(config)) = (menu, config) else {
        return;
    };
    let nsv = map.as_ref().is_some_and(|map| map.name == SKATE2_MAP);
    if menu.on_nsv != nsv {
        menu.on_nsv = nsv;
    }
    let chosen = menu.physics();
    let switched = local.menu != Some(chosen);
    if switched {
        let wanted = if local.menu.is_none() { config.physics_override.unwrap_or(chosen) } else { chosen };
        local.wanted = wanted == Physics::Skate2;
        local.menu = Some(chosen);
    }
    let restart = switched && matches!(s2.state, HostState::Failed(_));
    if local.wanted && (matches!(s2.state, HostState::Off) || restart) {
        let boot = boot(&menu, s2.free_play);
        s2.start(&Skate2Physics::data_dir(&config), &boot);
    }
    if let Some(live) = s2.live_boot().cloned() {
        let profile = settings::profile(&menu);
        if live.profile != profile {
            s2.send(EngineMessage::SetProfile(profile));
        }
        if live.gestures != menu.skate2_gestures() {
            s2.send(EngineMessage::SetGestures(menu.skate2_gestures()));
        }
        let mode = host_mode(s2.free_play);
        if live.mode != mode {
            s2.send(EngineMessage::SetMode(mode));
        }
    }
    s2.update();
    // The player chose Skate 3: no Skate 2 notice (a stopped sim's, or an idle sim's) stays on screen.
    if !local.wanted {
        s2.notice = None;
    }

    let (Some(mut skater), Some(mut view)) = (skater, view) else {
        show(&s2, &mut local, roster.as_deref_mut());
        return;
    };
    let generation = map.as_ref().map(|map| map.generation);
    // A map change replaces the skater: the new one stands at the map's spawn.
    let new_map = local.map.is_some() && local.map != generation;
    local.map = generation;
    let occupied = vehicles.is_some_and(|vehicles| vehicles.occupied());
    let active = local.wanted && nsv && !occupied && !s2.force_skate3;
    if new_map || !matches!(s2.state, HostState::Ready(_) | HostState::Driving(_)) {
        local.placing = None;
    }

    // Off: Skate 3 physics gets the skater back, and the sim waits, booted.
    if !active && s2.drives_local() {
        s2.state = match std::mem::replace(&mut s2.state, HostState::Off) {
            HostState::Driving(link) => HostState::Ready(link),
            other => other,
        };
        s2.replace_due = false;
        local.placing = None;
    }
    if active && matches!(s2.state, HostState::Ready(_)) {
        switch_on(&mut s2, &mut local, &mut skater, &mut view, game.as_deref(), new_map);
    } else if active && matches!(s2.state, HostState::Driving(_)) {
        teleport(&mut s2, &mut local, &mut skater, game.as_deref(), new_map);
    }
    // After the dispatch, whose placements can fail too: a stop at the cap (from here or from `step`) holds the
    // skater (`hand_back_due`) until this runs.
    if s2.hand_back_due || (local.had && !s2.drives_local()) {
        s2.hand_back_due = false;
        s2.replace_due = false;
        let state = skater.player_input.physical.filtered_state_0;
        // `step` wrote the port's category here; the engine's graphs check it against the engine's own last
        // published one (`animation_grind`), so it goes back before the engine's sim ticks.
        let engine = skater.player_state.filtered_output.map_or(0, |output| output.category as u32);
        skater.player_input.physical.filtered_state_0 = engine;
        // The port's line (fed into the engine's scorer, Task 17) does not carry over into Skate 3's.
        skater.scoring.end_line();
        // A new map's skater is at its spawn already, and a vehicle's exit places the skater itself.
        if !new_map && !occupied {
            let (position, heading) = position_heading(&skater.animated_skeleton.roots.animation_to_world);
            let on_board = ![FilteredCategory::Offboard as u32, FilteredCategory::OffboardAir as u32].contains(&state);
            // Not native Skate 2 (user 10-20): Skate 2 has one physics; this hands its skater to Skate 3's, at rest.
            if let Err(error) = skater.travel_to_as(level(position, heading), on_board) {
                warn!("SKATE2 PHYSICS the hand-back teleport waits for the engine's own: {error}");
            }
        }
        cut(&mut view);
        info!("SKATE2 PHYSICS Skate 3 physics has the skater");
    }
    let drives = s2.drives_local();
    let change = status_change(local.had, drives, local.wanted);
    local.had = drives;
    show(&s2, &mut local, roster.as_deref_mut());
    if let (Some(text), Some(roster)) = (change, roster.as_deref_mut()) {
        roster.chyron = Some([TITLE.into(), text.into()]);
    }
}

/// The top box's title for the switch's notices.
const TITLE: &str = "Skate 2 Physics";
/// What the top box says when Skate 2 physics takes the skater (loaded) or the player turns it off.
fn status_change(had: bool, drives: bool, wanted: bool) -> Option<&'static str> {
    match (had, drives) {
        (false, true) => Some("Skate 2 physics enabled"),
        (true, false) if !wanted => Some("Skate 2 physics disabled"),
        _ => None,
    }
}

/// A new notice goes to the HUD's top-middle box (the lost-connection one), which shows it for a few seconds, offline too.
fn show(s2: &Skate2Physics, local: &mut Switch, roster: Option<&mut crate::online_roster::OnlineRoster>) {
    if s2.notice == local.notice {
        return;
    }
    local.notice.clone_from(&s2.notice);
    if let (Some(notice), Some(roster)) = (&s2.notice, roster) {
        roster.chyron = Some([TITLE.into(), notice.clone()]);
    }
}

/// `Ready`: places the skater (once the engine has landed its own teleport), ticks the port with a neutral pad until
/// the placement settles, then drives. Not native Skate 2 (user 10-20): Skate 2 never hands its skater over; the
/// neutral pad keeps the player's input out of the few ticks the port takes to land the skater.
fn switch_on(
    s2: &mut Skate2Physics,
    local: &mut Switch,
    skater: &mut SkaterRuntime,
    view: &mut CameraRuntime,
    game: Option<&GamePhysics>,
    new_map: bool,
) {
    if local.placing.is_none() {
        if let Some(game) = game {
            skater.resolve_teleport_probe(game.world());
        }
        let pending = skater.player_input.pending_teleport();
        let root = skater.animated_skeleton.roots.animation_to_world;
        // After a restart the held skater (or the teleport it held) goes back at rest; a switch-on keeps its speed.
        let (at, travel, velocity) = match (s2.replace_due, pending) {
            (true, Some(target)) => (target, true, [0.0; 3]),
            (true, None) => (root, false, [0.0; 3]),
            // The engine's sim lands its own teleport first, then the switch places the skater where it landed.
            (false, Some(_)) => return,
            (false, None) => {
                let velocity = skater.player_input.physical.skateboard.vector_80.map(f32::from_bits);
                (root, false, [velocity[0], velocity[1], velocity[2]])
            }
        };
        if !s2.replace_due && !new_map && !grounded(game, at) {
            return;
        }
        // The engine's line ends here either way: the port scores from here (Task 17), and a Skate 3 trick left in it
        // would stay on the trick display.
        skater.scoring = skater.scoring.fresh();
        let (position, yaw) = position_heading(&at);
        if !s2.send(EngineMessage::Place(Place { position, yaw, velocity, end_line: travel })) {
            return;
        }
        local.placing = Some(Placing::new(0, travel));
    }
    let Some(frame) = s2.tick(Pad::default()) else {
        local.placing = None;
        return;
    };
    let Some(placing) = local.placing.as_mut() else { return };
    match placing.follow(&frame) {
        Some(true) => {
            if placing.travel {
                skater.take_travel();
            }
            local.placing = None;
            s2.drive();
            // The engine's sim (and its scorer) ran through the placement on the fresh scorer: the port's line starts
            // clean, with nothing the engine scored or opened there (review I2).
            skater.scoring.end_line();
            cut(view);
            // The first port frame cuts too: `step::apply` cuts on the tick that leaves Teleport.
            skater.player_input.physical.filtered_state_0 = FilteredCategory::Teleport as u32;
        }
        Some(false) => {
            local.placing = None;
            s2.fail_placement();
        }
        None => {}
    }
}

/// `Driving`: an engine teleport (or a new map's spawn) becomes a placement that ends the line (spec 3.5), followed
/// through the frames `step` applies.
fn teleport(
    s2: &mut Skate2Physics,
    local: &mut Switch,
    skater: &mut SkaterRuntime,
    game: Option<&GamePhysics>,
    new_map: bool,
) {
    let last = s2.last_good.as_ref().map_or(0, |frame| frame.tick);
    if let Some(placing) = local.placing.as_mut() {
        let Some(frame) = s2.last_good.as_deref() else { return };
        match placing.follow(frame) {
            Some(true) => {
                if placing.travel {
                    skater.take_travel();
                }
                local.placing = None;
            }
            Some(false) => {
                local.placing = None;
                s2.fail_placement();
            }
            None => {}
        }
        return;
    }
    if let Some(game) = game {
        skater.resolve_teleport_probe(game.world());
    }
    let pending = skater.player_input.pending_teleport();
    let at = match (new_map, pending) {
        (true, _) => skater.animated_skeleton.roots.animation_to_world,
        (false, Some(target)) => target,
        (false, None) => return,
    };
    if !new_map && !grounded(game, at) {
        warn!("SKATE2 PHYSICS a teleport with no ground under it is dropped");
        skater.take_travel();
        return;
    }
    // `end_line` ends the port's line; the engine's scorer, which the port's frames feed, ends it too.
    skater.scoring.end_line();
    let (position, yaw) = position_heading(&at);
    if s2.send(EngineMessage::Place(Place { position, yaw, velocity: [0.0; 3], end_line: true })) {
        local.placing = Some(Placing::new(last, pending.is_some() && !new_map));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_top_box_says_when_skate2_physics_comes_on_and_goes_off() {
        assert_eq!(super::status_change(false, true, true), Some("Skate 2 physics enabled"), "loaded");
        assert_eq!(super::status_change(true, false, false), Some("Skate 2 physics disabled"), "turned off");
        assert_eq!(super::status_change(true, false, true), None, "a failure says its own notice");
        assert_eq!(super::status_change(true, true, true), None);
    }
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use skate2_physics_protocol::{Boot, EngineMessage, Frame, HostMessage, Place};
    use skate_core::input::xbox::XboxState;

    use super::*;
    use crate::map_transition::CurrentMap;
    use crate::multiplayer::Online;
    use crate::skate2_physics::sim::Link;
    use crate::skate2_physics::step::tests::{harness, tick};

    /// The host's ends of the links a fake spawner made, oldest first, with the `Boot` each was spawned with.
    type Ends = Arc<Mutex<Vec<(Sender<HostMessage>, Receiver<EngineMessage>, Boot)>>>;

    /// A resource whose spawner makes channel links (no thread) and keeps the host's ends.
    fn faked() -> (Skate2Physics, Ends) {
        let ends: Ends = Arc::default();
        let spawned = Arc::clone(&ends);
        let physics = Skate2Physics::with(
            Box::new(move |_: &Path, boot: &Boot| {
                let (to_host, from_engine) = channel();
                let (to_engine, from_host) = channel();
                spawned.lock().unwrap().push((to_engine, from_engine, boot.clone()));
                Ok(Link::from_channels(to_host, from_host))
            }),
            Box::new(Instant::now),
        );
        (physics, ends)
    }

    fn say(ends: &Ends, i: usize, message: HostMessage) {
        ends.lock().unwrap()[i].0.send(message).unwrap();
    }

    /// What the engine sent link `i` since the last look.
    fn sent(ends: &Ends, i: usize) -> Vec<EngineMessage> {
        ends.lock().unwrap()[i].1.try_iter().collect()
    }

    fn places(messages: &[EngineMessage]) -> Vec<Place> {
        messages.iter().filter_map(|m| if let EngineMessage::Place(p) = m { Some(p.clone()) } else { None }).collect()
    }

    /// A scratch settings root.
    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("sk8-s2switch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(crate::difficulty::Difficulty::path(&root).parent().unwrap()).unwrap();
        root
    }

    fn config(root: &Path) -> crate::config::Config {
        crate::config::Config {
            asset_root: root.to_owned(),
            verification_capture: None,
            map: None,
            map_path: None,
            difficulty: Default::default(),
            check_assets: false,
            start_paused: false,
            multiplayer: Default::default(),
            map_fingerprint: 0,
            teleport: None,
            hero_clip: None,
            skate2_physics_dir: Some("D:/s2".into()),
            physics_override: None,
        }
    }

    fn map(name: &str, generation: u64) -> CurrentMap {
        CurrentMap { path: None, name: name.into(), spawn: [0.0; 3], heading: 0.0, generation }
    }

    fn switch(world: &mut World) {
        world.run_system_cached(update).unwrap();
    }

    fn choose(world: &mut World, physics: Physics) {
        let root = world.resource::<crate::config::Config>().asset_root.clone();
        world.resource_mut::<Menu>().set_physics(physics, &root).unwrap();
    }

    /// The port's root `at`, facing `yaw`, in its layout (rows x, y, z, position).
    fn level16(yaw: f32, at: [f32; 3]) -> [f32; 16] {
        let (s, c) = yaw.sin_cos();
        [c, 0.0, -s, 0.0, 0.0, 1.0, 0.0, 0.0, s, 0.0, c, 0.0, at[0], at[1], at[2], 1.0]
    }

    /// The port's frame `tick` with its skater at `at` facing 0.5 rad, on the ground, with `placed`.
    fn frame(tick: u32, at: [f32; 3], placed: Option<(u32, bool)>) -> HostMessage {
        let mut frame =
            Frame { tick, sim_period_s: 1.0 / 60.0, category: 1, placed, ..crate::skate2_physics::pose::tests::posed() };
        frame.bones[0] = level16(0.5, at);
        HostMessage::Frame(Box::new(frame))
    }

    const AT: [f32; 3] = [10.0, 0.0, 5.0];

    /// The engine's skater, sim and camera on the flat test world with the switch's resources: NSV, offline.
    fn app(name: &str) -> (App, Ends) {
        let mut app = harness();
        let root = scratch(name);
        let (physics, ends) = faked();
        app.insert_resource(physics)
            .insert_resource(config(&root))
            .insert_resource(crate::graphics_menu::test_menu())
            .insert_resource(map(SKATE2_MAP, 0))
            .insert_resource(Online(false));
        (app, ends)
    }

    /// Switch on, the sim boots, the placement settles: `Driving`, and one port tick applied at `AT`. The answers
    /// are queued before each switch update, which ticks the port once while it places.
    fn to_driving(world: &mut World, ends: &Ends) {
        tick(world);
        choose(world, Physics::Skate2);
        switch(world);
        assert!(matches!(world.resource::<Skate2Physics>().state, HostState::Starting(_)));
        say(ends, 0, HostMessage::Ready { boot_ms: 1 });
        say(ends, 0, frame(1, AT, Some((1, false))));
        switch(world);
        assert!(matches!(world.resource::<Skate2Physics>().state, HostState::Ready(_)), "placing");
        say(ends, 0, frame(2, AT, Some((1, true))));
        switch(world);
        assert!(matches!(world.resource::<Skate2Physics>().state, HostState::Driving(_)), "the placement settled");
        say(ends, 0, frame(3, AT, Some((1, true))));
        tick(world);
        sent(ends, 0);
    }

    /// Where presentation draws the skater and the view (its current snapshot's root and camera).
    fn drawn(world: &World) -> (Vec3, Vec3) {
        let current = world.resource::<crate::presentation::Presentation>().pair().expect("captured").1;
        (current.root.translation, current.camera.translation)
    }

    /// After a hand-back: engine ticks until its sim has landed the teleport (it probes the ground first), then
    /// `more` ticks. The engine's sim must never fail (its graphs check the category `step` overwrote: a failure ends
    /// the game). For 40 ticks the skater is never drawn more than 0.5 m from where the port left it (`step` wrote
    /// that root), nor the view more than 8 m from it: not where Skate 2 took the skater over.
    fn land(world: &mut World, more: usize) {
        let port = Vec3::from_array(position_heading(&world.resource::<SkaterRuntime>().animated_skeleton.roots.animation_to_world).0);
        let mut landed = None;
        for n in 0..40 + more {
            tick(world);
            let failed = world.resource::<GamePhysics>().failed;
            assert!(!failed, "tick {n}: the engine's sim failed after the hand-back");
            if n < 40 {
                let (at, view) = drawn(world);
                assert!(at.distance(port) <= 0.5, "tick {n}: drawn at {at}, {} m from the port's root {port}", at.distance(port));
                assert!(view.distance(port) <= 8.0, "tick {n}: the view at {view}, {} m from {port}", view.distance(port));
            }
            if landed.is_none() && pending(world).is_none() {
                landed = Some(n);
            }
        }
        assert!(landed.is_some_and(|n| n < 10), "the engine's sim landed its teleport at tick {landed:?}");
    }

    /// The engine's pending teleport: its position and heading.
    fn pending(world: &World) -> Option<([f32; 3], f32)> {
        world.resource::<SkaterRuntime>().player_input.pending_teleport().map(|at| position_heading(&at))
    }

    #[test]
    fn the_host_starts_at_launch_when_saved() {
        let root = scratch("launch");
        let mut world = World::new();
        let (physics, ends) = faked();
        world.insert_resource(physics);
        world.insert_resource(config(&root));
        let mut menu = crate::graphics_menu::test_menu();
        menu.set_physics(Physics::Skate2, &root).unwrap();
        assert_eq!(settings::Gameplay::load(&root).physics, Physics::Skate2, "saved");
        world.insert_resource(menu);
        world.init_resource::<crate::online_roster::OnlineRoster>();
        switch(&mut world);
        let physics = world.resource::<Skate2Physics>();
        assert!(matches!(physics.state, HostState::Starting(_)));
        assert_eq!(physics.notice.as_deref(), Some("Loading Skate 2 physics..."));
        let chyron = world.resource_mut::<crate::online_roster::OnlineRoster>().chyron.take();
        assert_eq!(chyron.as_ref().map(|[_, body]| body.as_str()), Some("Loading Skate 2 physics..."), "the HUD's top box shows it");
        switch(&mut world);
        assert!(world.resource::<crate::online_roster::OnlineRoster>().chyron.is_none(), "once per new notice");
        let menu = world.resource::<Menu>();
        let want = Boot { mode: 1, gestures: menu.skate2_gestures(), ambient: false, profile: settings::profile(menu), ..Boot::default() };
        assert_eq!(ends.lock().unwrap()[0].2, want);
        assert_eq!(host_mode(true), 3);

        // Amendment D: a camera option changed while the sim is booted reaches it.
        world.resource_mut::<Menu>().choose(crate::graphics_menu::Setting::InvertY, 1);
        switch(&mut world);
        let profile = settings::profile(world.resource::<Menu>());
        assert!(profile.invert_y);
        assert!(sent(&ends, 0).contains(&EngineMessage::SetProfile(profile)));

        // `--physics skate2` starts it with Skate 3 saved; nothing starts with neither.
        for (flag, starts) in [(Some(Physics::Skate2), true), (None, false)] {
            let mut world = World::new();
            world.insert_resource(faked().0);
            world.insert_resource(crate::config::Config { physics_override: flag, ..config(&scratch("flag")) });
            world.insert_resource(crate::graphics_menu::test_menu());
            switch(&mut world);
            assert_eq!(matches!(world.resource::<Skate2Physics>().state, HostState::Starting(_)), starts, "{flag:?}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    #[ignore = "requires SKATE3_ASSET_ROOT (the stock skater, graphs and camera)"]
    fn switching_keeps_position_heading_and_speed() {
        let (mut app, ends) = app("keeps");
        let world = app.world_mut();
        to_driving(world, &ends);
        let (position, heading) = position_heading(&world.resource::<SkaterRuntime>().animated_skeleton.roots.animation_to_world);
        assert!((heading - 0.5).abs() < 1e-4 && position == AT, "driving at {position:?}, {heading}");

        // Off: the engine's teleport at the host's last root and heading (at rest: Step 1).
        choose(world, Physics::Skate3);
        switch(world);
        assert!(matches!(world.resource::<Skate2Physics>().state, HostState::Ready(_)), "the sim stays booted");
        let (at, heading) = pending(world).expect("the engine's teleport");
        assert_eq!(at, AT);
        assert!((heading - 0.5).abs() < 1e-4, "{heading}");
        assert!(world.resource::<CameraRuntime>().frame.is_some_and(|f| f.discontinuity), "the view cuts");
        land(world, 120);

        // On again at 6 m/s: the Place is the engine's state, within one tick's travel.
        let along = [6.0 * 0.5f32.sin(), 0.0, 6.0 * 0.5f32.cos(), 0.0];
        world.resource_mut::<SkaterRuntime>().player_input.physical.skateboard.vector_80 = along.map(f32::to_bits);
        let (engine, engine_heading) =
            position_heading(&world.resource::<SkaterRuntime>().animated_skeleton.roots.animation_to_world);
        choose(world, Physics::Skate2);
        say(&ends, 0, frame(4, engine, Some((2, false))));
        switch(world);
        let place = places(&sent(&ends, 0)).pop().expect("a Place");
        assert!((place.position[0] - AT[0]).abs() < 0.1 && (place.position[2] - AT[2]).abs() < 0.1, "{place:?}");
        assert!((place.yaw - engine_heading).abs() < 1e-4 && (place.yaw - 0.5).abs() < 0.02, "{place:?}");
        assert_eq!(place.position, engine);
        assert_eq!(place.velocity, [along[0], along[1], along[2]]);
        assert!(!place.end_line);
        say(&ends, 0, frame(5, engine, Some((2, true))));
        switch(world);
        assert!(matches!(world.resource::<Skate2Physics>().state, HostState::Driving(_)));
    }

    /// Review I2: the engine's sim (and its scorer) runs through the switch-on placement; nothing it scores there
    /// carries into the port's line.
    #[test]
    #[ignore = "requires SKATE3_ASSET_ROOT (the stock skater, graphs and camera)"]
    fn the_placement_does_not_carry_into_the_ports_line() {
        let (mut app, ends) = app("placement-line");
        let world = app.world_mut();
        tick(world);
        choose(world, Physics::Skate2);
        switch(world);
        say(&ends, 0, HostMessage::Ready { boot_ms: 1 });
        say(&ends, 0, frame(1, AT, Some((1, false))));
        switch(world);
        assert!(matches!(world.resource::<Skate2Physics>().state, HostState::Ready(_)), "placing");
        // A trick the engine's scorer opens during the placement (a mid-manual switch re-enters its collector).
        let manual = skate2_physics_protocol::Tricks { begun: vec![2], sequence_begun: true, multiplier: 1.0, ..Default::default() };
        world.resource_mut::<SkaterRuntime>().scoring.apply_port_frame(&manual);
        assert!(world.resource::<SkaterRuntime>().scoring.sequence_open());
        say(&ends, 0, frame(2, AT, Some((1, true))));
        switch(world);
        assert!(matches!(world.resource::<Skate2Physics>().state, HostState::Driving(_)), "the placement settled");
        say(&ends, 0, frame(3, AT, Some((1, true))));
        tick(world);
        let scoring = &world.resource::<SkaterRuntime>().scoring;
        assert!(!scoring.sequence_open(), "the engine's open sequence did not carry into the port's line");
        // A clean scorer given the same (empty) port frame: what the line shows with nothing carried over.
        let mut clean = scoring.fresh();
        clean.apply_port_frame(&skate2_physics_protocol::Tricks::default());
        assert_eq!(scoring.hud_input().trick_name, clean.hud_input().trick_name, "nor its trick name");
        assert!(scoring.announced.is_empty());
    }

    #[test]
    #[ignore = "requires SKATE3_ASSET_ROOT (the stock skater, graphs and camera)"]
    fn a_teleport_goes_to_the_host_and_ends_the_line() {
        let (mut app, ends) = app("teleport");
        let world = app.world_mut();
        to_driving(world, &ends);
        let target = level([20.0, 0.0, -3.0], 1.0);
        world.resource_mut::<SkaterRuntime>().scoring.sequence_score = 450.0;
        world.resource_mut::<SkaterRuntime>().travel_to(target).unwrap();
        switch(world);
        let place = places(&sent(&ends, 0)).pop().expect("the teleport became a Place");
        assert_eq!((place.velocity, place.end_line), ([0.0; 3], true));
        let probed = [20.0, crate::physics::ground::HEIGHT + 0.15, -3.0];
        assert!(Vec3::from_array(place.position).distance(Vec3::from_array(probed)) < 1e-4, "probed: {place:?}");
        assert!((place.yaw - 1.0).abs() < 1e-5, "{}", place.yaw);
        assert_eq!(world.resource::<SkaterRuntime>().scoring.sequence_score, 0.0, "the line ended");
        assert!(pending(world).is_some(), "the modes see a teleport in flight until the port settles");
        for (tick_no, placed) in [(4, (2, false)), (5, (2, false)), (6, (2, true))] {
            say(&ends, 0, frame(tick_no, [20.0, 0.0, -3.0], Some(placed)));
            tick(world);
            switch(world);
        }
        assert!(pending(world).is_none(), "the engine's own request is cleared");
        assert!(places(&sent(&ends, 0)).is_empty(), "one Place");
        assert!(matches!(world.resource::<Skate2Physics>().state, HostState::Driving(_)));

        // A teleport with no ground under it (off the test world) lands on the walkable face nearest to it
        // (skate_world::travel_ground), placed like any other.
        world.resource_mut::<SkaterRuntime>().travel_to(level([500.0, 0.0, 0.0], 0.0)).unwrap();
        switch(world);
        let place = places(&sent(&ends, 0)).pop().expect("placed on the nearest ground");
        let nearest = [16.666666, crate::physics::ground::HEIGHT + 0.15, -16.666666];
        assert!(Vec3::from_array(place.position).distance(Vec3::from_array(nearest)) < 1e-4, "{place:?}");
        say(&ends, 0, frame(7, nearest, Some((3, true))));
        tick(world);
        switch(world);
        assert!(pending(world).is_none() && places(&sent(&ends, 0)).is_empty(), "settled, one Place");

        // A placement that never settles is a failed attempt at its deadline: the sim restarts, holding the skater.
        world.resource_mut::<SkaterRuntime>().travel_to(target).unwrap();
        switch(world);
        for n in 0..=PLACE_DEADLINE {
            say(&ends, 0, frame(8 + n, AT, Some((4, false))));
            tick(world);
            switch(world);
        }
        let physics = world.resource::<Skate2Physics>();
        assert!(matches!(physics.state, HostState::Restarting(_)), "the placement did not settle");
        assert_eq!(physics.last_failure, Some(crate::skate2_physics::sim::HostError::Failed("the placement did not settle".into())));
        assert!(physics.drives_local() && physics.replace_due);
        assert!(pending(world).is_some(), "the teleport is placed after the restart");
    }

    /// The host side of link `i` ends, as when its thread ends: its sender is dropped.
    fn kill(ends: &Ends, i: usize) {
        drop(std::mem::replace(&mut ends.lock().unwrap()[i].0, channel().0));
    }

    /// From driving on link 0: two failed attempts in a row. The sim dies while driving (in `step`), then the restart's
    /// placement ticks a sim that has died too (in the switch). The next attempt is link 2.
    fn fail_twice(world: &mut World, ends: &Ends) {
        kill(ends, 0);
        tick(world);
        assert!(matches!(world.resource::<Skate2Physics>().state, HostState::Restarting(_)), "failure 1");
        switch(world);
        say(ends, 1, HostMessage::Ready { boot_ms: 1 });
        kill(ends, 1);
        switch(world);
        assert!(matches!(world.resource::<Skate2Physics>().state, HostState::Restarting(_)), "failure 2");
        assert!(world.resource::<Skate2Physics>().drives_local(), "held through the restarts");
        switch(world);
        assert_eq!(ends.lock().unwrap().len(), 3);
    }

    /// Skate 3 has the skater after the cap: the hand-back teleport at the port's last root, the engine's own category
    /// back, the engine's sim landing it without a failure, and the setting untouched (Amendment F).
    fn handed_back(world: &mut World) {
        let physics = world.resource::<Skate2Physics>();
        assert!(matches!(physics.state, HostState::Failed(_)), "the third failure in a row stops the retries");
        assert!(physics.notice.as_deref().is_some_and(|n| n.starts_with("Skate 2 physics keeps failing")));
        assert!(!physics.drives_local(), "Skate 3 has the skater");
        let (at, heading) = pending(world).expect("the hand-back teleport");
        assert!(at == AT && (heading - 0.5).abs() < 1e-4, "{at:?} {heading}");
        let skater = world.resource::<SkaterRuntime>();
        let engine = skater.player_state.filtered_output.map_or(0, |output| output.category as u32);
        assert_eq!(skater.player_input.physical.filtered_state_0, engine, "the engine's own category is back");
        land(world, 120);
        let root = world.resource::<crate::config::Config>().asset_root.clone();
        assert_eq!(settings::Gameplay::load(&root).physics, Physics::Skate2, "the saved setting is still Skate 2");
        assert_eq!(world.resource::<Menu>().physics(), Physics::Skate2);
    }

    #[test]
    #[ignore = "requires SKATE3_ASSET_ROOT (the stock skater, graphs and camera)"]
    fn the_third_failure_hands_the_skater_back() {
        // The third failure in the switch's own placement (Update).
        let (mut first, ends) = app("cap-update");
        let world = first.world_mut();
        to_driving(world, &ends);
        fail_twice(world, &ends);
        say(&ends, 2, HostMessage::Ready { boot_ms: 1 });
        kill(&ends, 2);
        switch(world);
        handed_back(world);

        // The third failure in `step`, with a second fixed tick in the same frame before the switch's update.
        let (mut second, ends) = app("cap-step");
        let world = second.world_mut();
        to_driving(world, &ends);
        fail_twice(world, &ends);
        say(&ends, 2, HostMessage::Ready { boot_ms: 1 });
        say(&ends, 2, frame(1, AT, Some((1, false))));
        switch(world);
        say(&ends, 2, frame(2, AT, Some((1, true))));
        switch(world);
        assert!(matches!(world.resource::<Skate2Physics>().state, HostState::Driving(_)), "the third sim drives");
        kill(&ends, 2);
        let ticks = world.resource::<GamePhysics>().ticks;
        tick(world);
        tick(world);
        assert!(!world.resource::<GamePhysics>().failed, "no engine tick before the hand-back");
        assert_eq!(world.resource::<GamePhysics>().ticks, ticks, "the skater stays held until the switch hands it back");
        switch(world);
        handed_back(world);

        // Choosing Skate 3 clears the stopped sim's notice.
        assert!(world.resource::<Skate2Physics>().notice.is_some(), "the cap's notice shows");
        choose(world, Physics::Skate3);
        switch(world);
        assert_eq!(world.resource::<Skate2Physics>().notice, None, "no Skate 2 notice once Skate 3 is chosen");
    }

    #[test]
    #[ignore = "requires SKATE3_ASSET_ROOT (the stock skater, graphs and camera)"]
    fn a_forced_mode_switches_over_and_back() {
        let (mut app, ends) = app("forced");
        let world = app.world_mut();
        to_driving(world, &ends);
        world.resource_mut::<Skate2Physics>().set_force_skate3(true);
        switch(world);
        assert!(matches!(world.resource::<Skate2Physics>().state, HostState::Ready(_)), "Ready, not Driving");
        let (at, heading) = pending(world).expect("the engine's sim goes to the host's last root");
        assert!(at == AT && (heading - 0.5).abs() < 1e-4);
        land(world, 120);
        world.resource_mut::<Skate2Physics>().set_force_skate3(false);
        say(&ends, 0, frame(4, AT, Some((2, false))));
        switch(world);
        assert_eq!(places(&sent(&ends, 0)).len(), 1, "a new Place");
        say(&ends, 0, frame(5, AT, Some((2, true))));
        switch(world);
        assert!(matches!(world.resource::<Skate2Physics>().state, HostState::Driving(_)));

        // A vehicle: Skate 3 physics while it is occupied (its exit places the skater), Skate 2 after.
        world.resource_mut::<crate::modding::vehicles::Vehicles>().occupy_for_test(true);
        switch(world);
        assert!(matches!(world.resource::<Skate2Physics>().state, HostState::Ready(_)), "Ready, not Driving");
        assert!(pending(world).is_none(), "the vehicle's exit teleport places the skater, not the switch");
        world.resource_mut::<crate::modding::vehicles::Vehicles>().occupy_for_test(false);
        say(&ends, 0, frame(6, AT, Some((3, false))));
        switch(world);
        assert_eq!(places(&sent(&ends, 0)).len(), 1, "a new Place");
        say(&ends, 0, frame(7, AT, Some((3, true))));
        switch(world);
        assert!(matches!(world.resource::<Skate2Physics>().state, HostState::Driving(_)));
    }

    /// User 10-28 (spec P4 / section 7): online the switch follows the menu, which holds the session host's choice
    /// (online_moves::physics), on New San Vanelona; other maps ride Skate 3, online or not.
    #[test]
    #[ignore = "requires SKATE3_ASSET_ROOT (the stock skater, graphs and camera)"]
    fn online_follows_the_hosts_choice_and_other_maps_ride_skate_3() {
        let (mut app, ends) = app("modes");
        let world = app.world_mut();
        world.resource_mut::<Online>().0 = true;
        to_driving(world, &ends);
        *world.resource_mut::<CurrentMap>() = map("Skate_2_Maloof", 1);
        for _ in 0..5 {
            switch(world);
            assert!(matches!(world.resource::<Skate2Physics>().state, HostState::Ready(_)), "Maloof: never Driving");
        }
        assert!(!world.resource::<Menu>().on_nsv);
        assert!(places(&sent(&ends, 0)).is_empty());

        // Back on NSV, still online.
        *world.resource_mut::<CurrentMap>() = map(SKATE2_MAP, 2);
        say(&ends, 0, frame(4, AT, Some((2, false))));
        switch(world);
        assert!(world.resource::<Menu>().on_nsv);
        assert_eq!(places(&sent(&ends, 0)).len(), 1, "a new Place");
        say(&ends, 0, frame(5, AT, Some((2, true))));
        switch(world);
        assert!(matches!(world.resource::<Skate2Physics>().state, HostState::Driving(_)), "Driving again");
    }

    /// The spike script's pushes (A at 1.0, 1.9, 2.8 and 3.7 s, 12 ticks each); its flick tricks are left out.
    fn pushes(tick: u32) -> bool {
        [1.0f32, 1.9, 2.8, 3.7].iter().any(|at| ((at * 60.0) as u32..(at * 60.0) as u32 + 12).contains(&tick))
    }

    /// The engine on NSV with the real host driving the skater (SKATE3_ASSET_ROOT, S2HOST_TEST_DISC): the app and its
    /// settings folder, named `name` (removed by the caller).
    fn driving_on_nsv(name: &str) -> (App, PathBuf) {
        let root = PathBuf::from(std::env::var_os("SKATE3_ASSET_ROOT").expect("set SKATE3_ASSET_ROOT"));
        let data = PathBuf::from(std::env::var_os("S2HOST_TEST_DISC").expect("set S2HOST_TEST_DISC to the stage output"));
        let nsv = skate_data::skate_map::SkateMap::load(&root.parent().unwrap().join("maps").join("New_San_Vanelona.skate")).unwrap();
        let mut app = harness();
        let settings = scratch(name);
        {
            let world = app.world_mut();
            let physics = GamePhysics::load_with_map(&root, Some(&nsv)).unwrap();
            let graphs = world.resource::<crate::graph_runtime::StockGraphs>();
            let skater = SkaterRuntime::load(&root, graphs, &physics, "normal").unwrap();
            drop(nsv);
            world.insert_resource(physics);
            world.insert_resource(skater);
            world.insert_resource(Skate2Physics::default());
            world.insert_resource(crate::config::Config { skate2_physics_dir: Some(data), ..config(&settings) });
            world.insert_resource(crate::graphics_menu::test_menu());
            world.insert_resource(map(SKATE2_MAP, 0));
            world.insert_resource(Online(false));
        }
        let world = app.world_mut();
        tick(world);
        choose(world, Physics::Skate2);
        let started = Instant::now();
        while !matches!(world.resource::<Skate2Physics>().state, HostState::Driving(_)) {
            let physics = world.resource::<Skate2Physics>();
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "Driving within 10 s: {:?} {:?}",
                physics.notice,
                physics.last_failure
            );
            switch(world);
            std::thread::sleep(Duration::from_millis(5));
        }
        println!("Driving after {:?}", started.elapsed());
        (app, settings)
    }

    #[test]
    #[ignore = "needs SKATE3_ASSET_ROOT (the install's assets, read only; its maps beside them) and S2HOST_TEST_DISC (the stage output)"]
    fn rides_on_nsv_with_the_real_host() {
        let (mut app, settings) = driving_on_nsv("real");
        let world = app.world_mut();
        let deck =|world: &World| world.resource::<Skate2Physics>().last_good.as_ref().map(|frame| (frame.deck, frame.category));
        let height = |world: &World, deck: [f32; 3]| {
            let at = level(deck, 0.0);
            let face = crate::skate_world::probe_ground(world.resource::<GamePhysics>().world(), at).1;
            deck[1] - face.expect("our NSV ground under the port's deck")
        };
        // (tick, category, deck height above our face, deck position) on every tick.
        let mut series = Vec::new();
        for n in 0..600u32 {
            let buttons = if pushes(n) { 0x1000 } else { 0 };
            world.resource_mut::<crate::input::ControllerInput>().sample_raw_for_test(XboxState { buttons, triggers: [0; 2], left: [0; 2], right: [0; 2] });
            tick(world);
            switch(world);
            assert!(matches!(world.resource::<Skate2Physics>().state, HostState::Driving(_)), "tick {n}: still driving");
            let (at, category) = deck(world).expect("a frame");
            series.push((n, category, height(world, at), at));
        }
        let start = series[0].3;
        let end = series.last().unwrap().3;
        let moved = Vec3::from_array(end).distance(Vec3::from_array(start));
        for (n, category, h, at) in series.iter().take(60) {
            println!("tick {n}: category {category}, height {h:.4} m, deck {at:?}");
        }
        for (n, category, h, at) in series.iter().skip(60).step_by(20) {
            println!("tick {n}: category {category}, height {h:.4} m, deck {at:?}");
        }
        // Past the placement's settle and its ~35-tick Bumped window, on the ground. A vertical probe from the deck's
        // centre reads high wherever the board spans a transition (ruling), so only a steady grade is asserted: the
        // grade over the 0.4 m the deck travelled before this tick and the 0.4 m after it (about the deck's length)
        // differ by under 0.02. Transition ticks are logged and counted, not asserted.
        let grade = |from: [f32; 3], to: [f32; 3]| (to[1] - from[1]) / (to[0] - from[0]).hypot(to[2] - from[2]);
        let along = |i: usize, back: bool| {
            let mut j = i;
            loop {
                j = if back { j.checked_sub(1)? } else { (j + 1 < series.len()).then_some(j + 1)? };
                let (a, b) = (series[j].3, series[i].3);
                if (a[0] - b[0]).hypot(a[2] - b[2]) >= 0.4 {
                    return Some(j);
                }
            }
        };
        let (mut steady, mut transition) = (Vec::new(), 0);
        for (i, (n, _, h, at)) in series.iter().enumerate().filter(|(_, (n, c, ..))| *n >= 60 && *c == 1) {
            let grades = along(i, true).zip(along(i, false)).map(|(b, f)| (grade(series[b].3, *at), grade(*at, series[f].3)));
            match grades {
                Some((before, after)) if (before - after).abs() < 0.02 => steady.push((*n, *h, before)),
                _ => {
                    transition += 1;
                    println!("transition (or too slow to measure) tick {n}: height {h:.4} m, grades {grades:?}");
                }
            }
        }
        let (low, high) = steady.iter().fold((f32::MAX, f32::MIN), |(lo, hi), (_, h, _)| (lo.min(*h), hi.max(*h)));
        println!(
            "moved {moved:.2} m; ground ticks from 60: {} on a steady grade (height {low:.4}..{high:.4} m), {transition} transition",
            steady.len()
        );
        assert!(moved > 1.0, "the skater moved {moved} m");

        // Off on NSV (before the height check, so a height failure still shows this): the engine's sim takes the
        // skater back where the port left it and keeps running.
        choose(world, Physics::Skate3);
        switch(world);
        assert!(pending(world).is_some(), "the hand-back teleport");
        land(world, 300);
        println!("switched off on NSV: 340 engine ticks, no failure, drawn within 0.5 m of the port's root");
        let _ = std::fs::remove_dir_all(&settings);

        assert!(!steady.is_empty(), "no steady-grade ground ticks after 60");
        assert!(high - low <= 0.05, "on a steady grade the deck's height varies by {} m: {steady:?}", high - low);
        assert!((0.03..=0.12).contains(&low) && (0.03..=0.12).contains(&high), "the deck rides {low}..{high} m above our ground: {steady:?}");
    }

    /// User 10-22 ("there doesnt seem to be any bail sounds or any walking sounds or any wheel spinning sounds"): the
    /// record the sound engine runs on, from the real host's frames, while the skater pushes, ollies twice (right stick
    /// down, then up), steps off (Y) and walks (left stick forward). The wheels spin up in known air (+332) and the
    /// steps need the feet going down (`offboard_feet`).
    #[test]
    #[ignore = "needs SKATE3_ASSET_ROOT (the install's assets, read only; its maps beside them) and S2HOST_TEST_DISC (the stage output)"]
    fn sounds_on_nsv_with_the_real_host() {
        let (mut app, settings) = driving_on_nsv("sounds");
        let world = app.world_mut();
        let stick = |down: bool, up: bool| if down { -32767 } else if up { 32767 } else { 0 };
        let (mut air, mut steps, mut walking, mut grounds) = (0, 0, 0, 0);
        let mut flipped = Vec::new();
        let mut last = None;
        for n in 0..900u32 {
            let ollie = |at: u32| (at..at + 10).contains(&n);
            let flick = |at: u32| (at + 10..at + 16).contains(&n);
            let buttons = if pushes(n) { 0x1000 } else if (480..486).contains(&n) { 0x8000 } else { 0 };
            // The second is a kickflip (user 10-22: the wheel spin stops at once on a trick): the flick goes up-left.
            let right = [if flick(400) { -23170 } else { 0 }, stick(ollie(300) || ollie(400), flick(300) || flick(400))];
            let left = [0, if n >= 540 { 32767 } else { 0 }];
            world
                .resource_mut::<crate::input::ControllerInput>()
                .sample_raw_for_test(XboxState { buttons, triggers: [0; 2], left, right });
            tick(world);
            switch(world);
            let Some(frame) = world.resource::<Skate2Physics>().last_good.clone() else { continue };
            let wheels = crate::skate2_physics::audio::wheel_surfaces(world.resource::<GamePhysics>().world(), &frame.audio);
            let r = crate::skate2_physics::audio::observation(&frame, u64::from(frame.tick), 0, &wheels).retail;
            let a = &frame.audio;
            let row = (frame.category, a.num_wheels_in_contact, a.is_skateboard_airborne, r.in_known_air, r.offboard_feet, r.state_category);
            if last != Some(row) || n % 60 == 0 || (400..470).contains(&n) {
                println!(
                    "tick {n}: category {} wheels {} airborne {} known_air {} feet {:?} state {} surface speed {:.3} held {} ground tag {:#x}",
                    row.0, row.1, row.2, row.3, row.4, row.5, a.skateboard_surface_speed, r.offboard_311, wheels.body
                );
                last = Some(row);
            }
            air += usize::from(r.in_known_air);
            if r.in_known_air && n > 400 && r.scorable_id != -1 {
                flipped.push(r.scorable_id);
            }
            walking += usize::from(r.state_category == 500);
            steps += usize::from(r.offboard_feet.contains(&true));
            grounds += usize::from(r.state_category == 500 && r.offboard_surface != 0);
        }
        println!("known air {air} ticks, off board {walking}, a foot down {steps}, a surface under the walk {grounds}");

        // Skate 3's own walk for comparison: its animation's AudibleFootStepStrength, which the port has no value for.
        choose(world, Physics::Skate3);
        switch(world);
        let mut strengths = Vec::new();
        for _ in 0..300 {
            world
                .resource_mut::<crate::input::ControllerInput>()
                .sample_raw_for_test(XboxState { buttons: 0, triggers: [0; 2], left: [0, 32767], right: [0; 2] });
            tick(world);
            let skater = world.resource::<SkaterRuntime>();
            strengths.push((skater.player_input.physical.state.category_12, skater.animation_input.extra.footstep_strength));
        }
        let walked: Vec<f32> = strengths.iter().filter(|(c, s)| *c == 500 && *s != 0.0).map(|(_, s)| *s).collect();
        println!(
            "Skate 3 walk: {} of {} ticks off board, footstep strength non-zero on {}: min {:?} max {:?} values {:?}",
            strengths.iter().filter(|(c, _)| *c == 500).count(),
            strengths.len(),
            walked.len(),
            walked.iter().copied().reduce(f32::min),
            walked.iter().copied().reduce(f32::max),
            walked.iter().take(40).collect::<Vec<_>>()
        );
        let _ = std::fs::remove_dir_all(&settings);
        assert!(air > 0, "the ollies reach known air");
        assert!(walking > 0, "the skater stepped off");
        assert!(steps > 0, "walking puts the feet down");
        assert!(grounds > 0, "the steps have a surface");
        println!("the kickflip's air ticks scored as {flipped:?}");
        assert!(!flipped.is_empty(), "the kickflip is the trick being scored in the air");
    }
}
