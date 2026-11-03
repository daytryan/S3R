//! Skate 1's online-area walls (online_boundary::skate1_walls, joined to the solve as network proxies) against the
//! riding and the walking skater: real raw input on the flat fixture, the wall line put ahead of where each one goes.
use super::*;
use skate_core::input::xbox::XboxState;
use skate_core::player::state::PhysicalStateId;

struct Run {
    physics: GamePhysics,
    skater: SkaterRuntime,
    controls: PlayerControls,
    input: crate::input::ControllerInput,
    camera: crate::camera::CameraRuntime,
    graphs: crate::graph_runtime::StockGraphs,
    polygon: Option<Vec<[f32; 2]>>,
}

impl Run {
    fn load() -> Self {
        let root = std::env::var_os("SKATE3_ASSET_ROOT").expect("set SKATE3_ASSET_ROOT");
        let root = std::path::Path::new(&root);
        let assets = skate_data::GameAssets::load(root).unwrap();
        let graphs = crate::graph_runtime::StockGraphs::load(root, &assets).unwrap();
        let mut physics = GamePhysics::load(root).unwrap();
        physics.world = ground::Terrain::Flat.world(physics.settings.floor_material);
        let skater = SkaterRuntime::load(root, &graphs, &physics, crate::physics::PHYSICS_MODE).unwrap();
        let camera = crate::camera::CameraRuntime::load(root).unwrap();
        Self {
            physics,
            skater,
            camera,
            graphs,
            controls: PlayerControls::default(),
            input: crate::input::ControllerInput::default(),
            polygon: None,
        }
    }
    fn root(&self) -> Vec2 {
        let r = self.skater.animated_skeleton.roots.animation_to_world[3];
        Vec2::new(r[0], r[2])
    }
    /// One tick; the walls as add_skate1_walls puts them in each step (multiplayer::prepare isn't running here).
    fn tick(&mut self, tick: usize, buttons: u16, left: [i16; 2]) {
        if let Some(polygon) = &self.polygon {
            self.physics.network_proxies.volumes = crate::online_boundary::skate1_walls(polygon, self.root().into());
        }
        self.input.sample_raw_for_test(XboxState { buttons, triggers: [0; 2], left, right: [0; 2] });
        let mut actions = self.input.player_actions();
        self.controls.update_for_physics(&mut actions, &self.physics, &self.skater, &self.camera).unwrap();
        frame::advance(&mut self.physics, &mut self.skater, &mut self.controls, &self.graphs, &mut actions, true, &mut self.camera)
            .unwrap_or_else(|e| panic!("tick{tick}: {e}"));
    }
    /// The way the skater went over the last `ticks`, then an area whose edge crosses that way `gap` ahead (a 100 m
    /// square behind it, so the skater is inside). Returns the edge point.
    fn edge_ahead(&mut self, start: Vec2, gap: f32, walls: bool) -> (Vec2, Vec2) {
        let dir = (self.root() - start).normalize();
        let edge = self.root() + dir * gap;
        let (side, back) = (dir.perp() * 50., edge - dir * 100.);
        if walls {
            self.polygon = Some([edge + side, back + side, back - side, edge - side].map(|v| v.to_array()).to_vec());
        }
        (edge, dir)
    }
}

/// Pushing on the board: how far past the edge line the root gets.
fn ride(walls: bool) -> f32 {
    let mut r = Run::load();
    let push = |tick: usize| if tick % 40 < 30 { 0x1000 } else { 0 };
    for tick in 0..90 {
        r.tick(tick, push(tick), [0; 2]);
    }
    let start = r.root();
    for tick in 90..120 {
        r.tick(tick, push(tick), [0; 2]);
    }
    let (edge, dir) = r.edge_ahead(start, 3., walls);
    (120..420)
        .map(|tick| {
            r.tick(tick, push(tick), [0; 2]);
            (r.root() - edge).dot(dir)
        })
        .fold(f32::MIN, f32::max)
}

/// Off the board (Y), walking with the stick held forward: how far past the edge line the root gets.
fn walk(walls: bool) -> f32 {
    let mut r = Run::load();
    for tick in 0..200 {
        r.tick(tick, if tick == 20 { 0x8000 } else { 0 }, [0; 2]);
    }
    assert_eq!(r.skater.player_state.current(), PhysicalStateId::BipedGround);
    for tick in 200..260 {
        r.tick(tick, 0, [0, 24000]);
    }
    let start = r.root();
    for tick in 260..320 {
        r.tick(tick, 0, [0, 24000]);
    }
    let (edge, dir) = r.edge_ahead(start, 2., walls);
    (320..800)
        .map(|tick| {
            r.tick(tick, 0, [0, 24000]);
            assert_eq!(r.skater.player_state.current(), PhysicalStateId::BipedGround, "tick{tick}: still walking");
            (r.root() - edge).dot(dir)
        })
        .fold(f32::MIN, f32::max)
}

#[test]
#[ignore = "requires private stock skater, animation banks and collections"]
fn skate1_walls_stop_the_riding_and_the_walking_skater() {
    let (ride_free, ride_walled, walk_free, walk_walled) = (ride(false), ride(true), walk(false), walk(true));
    eprintln!(
        "SKATE1_WALLS past the edge line (m): riding free {ride_free:.2} walled {ride_walled:.2}; walking free {walk_free:.2} walled {walk_walled:.2}"
    );
    assert!(ride_free > 1. && walk_free > 1., "without walls both cross the line");
    // Measured: riding free 38.35 walled -0.34; walking free 19.27 walled -0.32. Both held, so Skate 1 rows run no
    // countdown (online_boundary::update).
    assert!(ride_walled < 0.5, "the wall stops the board");
    assert!(walk_walled < 0.5, "the wall stops the walking skater");
}
