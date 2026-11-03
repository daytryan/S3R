//! Edit Skater's room. Skate 3 stands the skater in a concrete bay
//! (world/models/DIST_CreateSakter, prepared as native-backdrops/CreateSkater.skate)
//! while editing. The bay is anchored at the skater's feet with its back wall
//! behind them, facing the editor camera, and the map's geometry is hidden
//! meanwhile so nothing intrudes. Gameplay position is never touched.
use crate::{
    cas_menu::{Frame, Node},
    config::Config,
    customiser::Customiser,
    map_render::{MapEntity, PreparedScene},
    retail_render::{RetailScene, RetailSkyMaterial},
};
use bevy::prelude::*;
use serde::Deserialize;
use skate_data::skate_map::SkateMap;
use std::collections::{HashMap, HashSet};

/// Where the skater stands in the room's authored space, as in retail footage:
/// 0.9 m in front of the knee-high ledge (front face x = 0.1, wall x = 0.37),
/// with the wall's step edge (z = -10.08) about 2.3 m to the camera's left.
/// The open side (-X) faces the camera.
// ponytail: retail's locator is in the encrypted executable; measured off video.
const SPOT: Vec3 = Vec3::new(-0.8, 200., -7.29);
/// Skate 3's sun (DownTown, Industrial and the parks share it) in the room's authored space: the room is authored in
/// place, its floor 200 m up, so retail's world sun lights it from the camera's side, front right, 47 degrees up
/// (retail_character lights the skater in it so).
pub(crate) const SUN: Vec3 = Vec3::new(-0.4787, 0.736, 0.4787);

/// The room's meshes, Visible while it is shown.
#[derive(Component)]
pub(crate) struct Room;

#[derive(Resource, Default)]
struct Staging {
    /// The package stem that failed to load, so it is not retried every frame.
    failed: Option<String>,
    /// The package stem the spawned room came from.
    loaded: String,
    /// Map meshes hidden while the room is shown, with their prior visibility.
    hidden: Vec<(Entity, Visibility)>,
    shown: bool,
    /// The room package being read, parsed and built off the main thread: its stem and the worker.
    parsing: Option<(String, std::thread::JoinHandle<Result<PreparedScene, String>>)>,
}

pub(crate) struct CasRoomPlugin;
impl Plugin for CasRoomPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Staging>()
            .init_resource::<Skate2Cas>()
            .init_resource::<HiddenActors>()
            .add_systems(Startup, load_skate2)
            .add_systems(Update, (spawn, stage, report).chain().after(crate::app::FrameSet::Animation))
            .add_systems(PostUpdate, hide_actors.before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate))
            // Overnight 10-15 D: where PlayerRoot's position goes wrong between the frame's stages.
            .add_systems(
                Update,
                (
                    trace("physics").after(crate::app::FrameSet::Physics).before(crate::app::FrameSet::Animation),
                    trace("present").after(crate::animation::present).before(crate::cas_pose::apply),
                    trace("cas_apply").after(crate::cas_pose::apply).before(spawn),
                    trace("animation").after(crate::app::FrameSet::Animation).before(spawn),
                    trace("update_end").after(report),
                ),
            )
            .add_systems(PostUpdate, trace("post").before(bevy::transform::TransformSystems::Propagate))
            .add_systems(Last, trace("last"))
            .add_systems(First, trace("first"));
    }
}

/// Overnight 10-15 D diagnostic (the Skate 1 frontend's Edit Skater showed the room without the skater): one INFO line
/// when Edit Skater first has a frame and one a second later, with what decides whether the skater is drawn.
/// ponytail: remove once a preview log has settled it.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn report(
    customiser: Res<Customiser>,
    history: Res<crate::presentation::Presentation>,
    replay: Res<crate::replay::Replay>,
    animation: Res<crate::animation::AnimationStatus>,
    time: Res<Time<Virtual>>,
    real: Res<Time<Real>>,
    players: Query<(Entity, &Transform, &GlobalTransform, &Visibility, &InheritedVisibility), With<crate::world::PlayerRoot>>,
    cameras: Query<(&GlobalTransform, &Camera), With<crate::camera::GameplayCamera>>,
    skins: Query<(Entity, &ViewVisibility, &InheritedVisibility), With<bevy::mesh::skinning::SkinnedMesh>>,
    parents: Query<&ChildOf>,
    globals: Query<&GlobalTransform>,
    mut logged: Local<(u8, f32)>,
) {
    if !customiser.open {
        *logged = (0, 0.);
        return;
    }
    let Some((previous, current)) = history.pair() else { return };
    let now = real.elapsed_secs();
    match logged.0 {
        0 => *logged = (1, now),
        1 if now - logged.1 >= 1. => logged.0 = 2,
        _ => return,
    }
    let player = players.iter().next();
    let (mut drawn, mut total) = (0, 0);
    for (skin, view, inherited) in &skins {
        if player.is_some_and(|p| parents.iter_ancestors(skin).any(|a| a == p.0)) {
            total += 1;
            drawn += usize::from(view.get() && inherited.get());
        }
    }
    let (mut nonfinite, mut min_scale, mut far) = (0, f32::INFINITY, 0_f32);
    let root = player.map_or(Vec3::ZERO, |p| p.2.translation());
    for (joint, ..) in animation.online_bindings() {
        let Ok(g) = globals.get(joint) else { continue };
        let (scale, _, at) = g.to_scale_rotation_translation();
        if !at.is_finite() || !scale.is_finite() {
            nonfinite += 1;
        } else {
            min_scale = min_scale.min(scale.min_element());
            far = far.max(at.distance(root));
        }
    }
    let camera = cameras.iter().next();
    info!(
        "CAS_SKATER_DIAG pass={} paused={} replay_active={} anim_ready={} bindings={} pair_current={:?} pair_previous={:?} \
         player_local={:?} player_world={:?} player_scale={:?} player_vis={:?} player_inherited={:?} skins_drawn={drawn}/{total} \
         joints_nonfinite={nonfinite} joints_min_scale={min_scale} joints_far={far} camera={:?} camera_active={:?} original={} fallback={}",
        logged.0,
        time.is_paused(),
        replay.active,
        animation.ready,
        animation.online_bindings().len(),
        current.root.translation,
        previous.root.translation,
        player.map(|p| p.1.translation),
        player.map(|p| p.2.translation()),
        player.map(|p| p.1.scale),
        player.map(|p| *p.3),
        player.map(|p| p.4.get()),
        camera.map(|c| c.0.translation()),
        camera.map(|c| c.1.is_active),
        customiser.original,
        customiser.fallback,
    );
}

/// Diagnostic for overnight 10-15 D: every PlayerRoot's translation at one stage of the frame, logged when it changes
/// while Edit Skater is open (at most 12 lines a stage). ponytail: remove with `report`.
#[allow(clippy::type_complexity)]
fn trace(
    at: &'static str,
) -> impl FnMut(Res<Customiser>, Query<(Entity, &Transform), With<crate::world::PlayerRoot>>, Local<(Vec<(Entity, Vec3)>, u8)>) {
    move |customiser, roots, mut seen| {
        if !customiser.open || seen.1 >= 12 {
            return;
        }
        let now: Vec<_> = roots.iter().map(|(e, t)| (e, t.translation)).collect();
        if now.len() != seen.0.len() || now.iter().zip(&seen.0).any(|(a, b)| a.0 != b.0 || a.1.distance(b.1) > 1e-3) {
            info!("CAS_ROOT_TRACE at={at} roots={now:?}");
            seen.1 += 1;
            seen.0 = now;
        }
    }
}

/// Load the room the first time it is needed, again after a map switch
/// retires it with the rest of the scene, and when Game Settings picks another room. A worker reads, parses and
/// builds it (meshes, BC7 textures: G4), so only publishing it touches this thread. Online (user 10-16) it is readied
/// as soon as the session runs; offline as soon as the world pauses (the pause menu Edit Skater opens from, or the
/// boot), and it is published when Edit Skater opens. Opened offline before it is ready, the open waits for the rest,
/// as it always did (the world is paused); online the editor shows where the skater stands until it is.
// perf 10-18: the Room query is kept between frames, not built every frame (the same entities).
// SKATE_CAS_QUERY=0: built every frame, as before.
fn spawn(world: &mut World, rooms: &mut QueryState<Entity, With<Room>>) {
    let open = world.resource::<Customiser>().open;
    let online = world.get_resource::<crate::multiplayer::Multiplayer>().is_some_and(|n| n.active());
    let wanted = open || online;
    // Not while a map loads: the loader has the cores.
    let prefetch = wanted
        || (world.resource::<Time<Virtual>>().is_paused() && !world.get_resource::<crate::map_transition::MapTransition>().is_some_and(|t| t.busy()));
    let stem = world.get_resource::<crate::graphics_menu::Menu>()
        .map_or(crate::graphics_menu::ROOMS[0].1, |m| m.edit_skater_room()).to_string();
    let staging = world.resource::<Staging>();
    if !prefetch || !world.resource::<RetailScene>().0 || staging.failed.as_ref() == Some(&stem) {
        return;
    }
    let loaded = staging.loaded.clone();
    static KEPT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let rooms: Vec<Entity> = if *KEPT.get_or_init(|| std::env::var_os("SKATE_CAS_QUERY").is_none_or(|v| v != "0")) {
        rooms.iter(world).collect()
    } else {
        world.query_filtered::<Entity, With<Room>>().iter(world).collect()
    };
    if !rooms.is_empty() && loaded == stem {
        return;
    }
    if wanted {
        // ponytail: the old room's meshes/textures (~4 MB) stay allocated until the
        // next map switch retires the scene's assets.
        for entity in rooms {
            // A parent's despawn may already have taken its children.
            if let Ok(entity) = world.get_entity_mut(entity) {
                entity.despawn();
            }
        }
    }
    let root = world.resource::<Config>().asset_root.clone();
    let path = root.join(format!("private/native-backdrops/{stem}.skate"));
    // Another room's worker (Game Settings switched) is left to finish on its own.
    let pending = world.resource_mut::<Staging>().parsing.take().filter(|(s, _)| *s == stem);
    let (building, worker) = match pending {
        Some(pending) => pending,
        None => {
            let mut scene = PreparedScene::new(world);
            let file = path.clone();
            let worker = std::thread::Builder::new().name("cas-room-loader".into()).spawn(move || {
                let map = std::fs::read(&file).map_err(|e| e.to_string()).and_then(|d| SkateMap::parse_render_only(&d))?;
                scene.prepare_room(&map, &root);
                Ok(scene)
            });
            match worker {
                Ok(worker) => (stem.clone(), worker),
                Err(error) => {
                    warn!("Edit Skater room unavailable ({}: {error}); editing where play paused", path.display());
                    world.resource_mut::<Staging>().failed = Some(stem);
                    return;
                }
            }
        }
    };
    if !wanted || (!worker.is_finished() && online) {
        world.resource_mut::<Staging>().parsing = Some((building, worker));
        return;
    }
    let scene = match worker.join().unwrap_or_else(|_| Err("the room's loader panicked".into())) {
        Ok(scene) => scene,
        Err(error) => {
            warn!("Edit Skater room unavailable ({}: {error}); editing where play paused", path.display());
            world.resource_mut::<Staging>().failed = Some(stem);
            return;
        }
    };
    world.resource_mut::<Staging>().loaded = stem;
    let before: HashSet<Entity> = world.query_filtered::<Entity, With<MapEntity>>().iter(world).collect();
    // The package carries its own clear colour; keep the map's.
    let clear = world.get_resource::<ClearColor>().cloned();
    scene.publish_unowned(world);
    if let Some(clear) = clear {
        world.insert_resource(clear);
    }
    let added: Vec<Entity> = world.query_filtered::<Entity, With<MapEntity>>().iter(world).filter(|e| !before.contains(e)).collect();
    for entity in added {
        world.entity_mut(entity).insert((Room, Visibility::Hidden));
    }
}

/// The room's transform: SPOT at the skater's feet, the open side toward the
/// editor camera (which sits along the skater's local +Z), the wall behind.
fn place(root: &Transform) -> Transform {
    let (yaw, _, _) = root.rotation.to_euler(EulerRot::YXZ);
    Transform::from_translation(root.translation).with_rotation(Quat::from_rotation_y(yaw + std::f32::consts::FRAC_PI_2))
        * Transform::from_translation(-SPOT)
}

/// Skate 2's own Edit Skater (private/cas-skate2/cas.json, tools/pipeline/skate2port/old_cas.py), shown in the Skate 2
/// rooms: the skater's spot, heading and clip for each editor state (skater.vlt cac_body,
/// CASCharacterEntity::SetBodyState 8259e8c8) and each page's camera (cac_camera, CasCamera::SetTargetState 825874f0),
/// in Skate 2's room space (its DIST_petes model), and where that space lies in each converted room (`rooms`: the mod
/// moved the model whole). Empty when the file is missing: the Skate 2 rooms then keep Skate 3's presentation.
#[derive(Resource, Default, Deserialize)]
pub(crate) struct Skate2Cas {
    bodies: HashMap<String, Skate2Body>,
    cameras: HashMap<String, Skate2Camera>,
    rooms: HashMap<String, Skate2Fit>,
}
#[derive(Deserialize)]
struct Skate2Body {
    position: [f32; 3],
    /// Degrees about +Y (UpdateTransform 82587c88).
    rotation: f32,
    animation: String,
}
#[derive(Deserialize)]
struct Skate2Camera {
    position: [f32; 3],
    look: [f32; 3],
    up: [f32; 3],
    /// Vertical, degrees (GetFOVY 827f64b0).
    fov: f32,
}
/// A room package's model = Ry(yaw) * Skate 2's room + translation.
#[derive(Deserialize)]
struct Skate2Fit {
    yaw: f32,
    translation: [f32; 3],
}
/// One frame of Skate 2's presentation, in world space.
pub(crate) struct Skate2View {
    pub feet: Vec3,
    pub facing: Quat,
    pub clip: String,
    pub eye: Vec3,
    pub look: Vec3,
    pub up: Vec3,
    pub fov: f32,
}
/// The state gestures and side views use: Skate 2's gesture rows are children of `standing`.
const STANDING: (&str, &str) = ("standing", "standing");
impl Skate2Cas {
    /// `shot` (camera, body) in the room `stem` placed for the camera root `root`; None outside the Skate 2 rooms.
    pub(crate) fn view(&self, stem: &str, root: &Transform, (camera, body): (&str, &str)) -> Option<Skate2View> {
        let fit = self.rooms.get(stem)?;
        let (camera, body) = (self.cameras.get(camera)?, self.bodies.get(body)?);
        let world = place(root)
            * Transform::from_translation(fit.translation.into()).with_rotation(Quat::from_rotation_y(fit.yaw));
        Some(Skate2View {
            feet: world.transform_point(body.position.into()),
            facing: world.rotation * Quat::from_rotation_y(body.rotation.to_radians()),
            clip: body.animation.clone(),
            eye: world.transform_point(camera.position.into()),
            look: world.transform_point(camera.look.into()),
            up: world.rotation * Vec3::from(camera.up).normalize_or(Vec3::Y),
            fov: camera.fov.to_radians(),
        })
    }
}
fn load_skate2(mut cas: ResMut<Skate2Cas>, config: Option<Res<Config>>) {
    let Some(config) = config else { return };
    let path = config.asset_root.join("private/cas-skate2/cas.json");
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    match serde_json::from_str(&text) {
        Ok(loaded) => *cas = loaded,
        Err(error) => warn!("Skate 2 Edit Skater presentation unreadable ({}: {error}); the Skate 2 rooms keep Skate 3's", path.display()),
    }
}

/// Skate 2's (camera, body state) for an Edit Skater page. Its cameras are grouped under their state: Back Alley
/// Pete's table for the face and body (head with its eyes/brow/nose/jaw/mouth/chin, body), Slappy's couch for gear
/// (hat, necklace, chest, legs, feet, wrists, skateboard, wheels). Tattoo and side views turn the skater, which only
/// the standing state does (Skate 2 has no turned shots). INFERRED: hair and facial hair take the couch's head shot,
/// skin Pete's body shot, wristwear the left wrist; the vault's keys don't name those pages.
pub(crate) fn skate2_shot(node: Node, frame: Frame) -> (&'static str, &'static str) {
    const PETES: &str = "back_alley_petes";
    match node {
        Node::Body | Node::Skin | Node::Shape => ("body", PETES),
        Node::Presets | Node::Face => ("head", PETES),
        Node::Eyes => ("eyes", PETES),
        Node::Brow => ("brow", PETES),
        Node::Nose => ("nose", PETES),
        Node::Jaw => ("jaw", PETES),
        Node::Mouth => ("mouth", PETES),
        Node::Chin => ("chin", PETES),
        Node::Hair | Node::FacialHair => ("hat", "slappys"),
        // The idle the gesture rows return to (CAS_GestureStand_Cycle).
        Node::Style => ("standing", "idle"),
        Node::Tattoo(_) | Node::TattooKind(..) | Node::TattooGrid(..) | Node::TattooAdjust(_) => STANDING,
        _ => {
            let camera = match frame {
                Frame::Body => "slappys",
                Frame::Face => "necklace",
                Frame::Head => "hat",
                Frame::Torso => "chest",
                Frame::Legs => "legs",
                Frame::Feet => "feet",
                Frame::Hands => "wrist_left",
                Frame::Board => "skateboard",
                Frame::BoardSide => "skatewheel",
                Frame::Back | Frame::LeftSide | Frame::RightSide => return STANDING,
            };
            (camera, "slappys")
        }
    }
}
/// What a Skate 2 room shows now: the page's shot (the couch until a page sets one), standing while the Graphic
/// Creator turns the skater to the side it edits.
pub(crate) fn skate2_shot_now(customiser: &Customiser, creator: bool) -> (&'static str, &'static str) {
    if creator { STANDING } else { customiser.skate2.unwrap_or(("slappys", "slappys")) }
}
/// How far the skater turns in a Skate 2 room: standing, Rotate Skater and the page's turn as in Skate 3; seated and
/// on the table the authored cameras frame the page. INFERRED: Skate 2's button help has no Rotate Skater.
pub(crate) fn skate2_turn(customiser: &Customiser, (_, body): (&str, &str)) -> f32 {
    if matches!(body, "standing" | "idle") { customiser.preview_camera().2 } else { 0. }
}

/// The world's actors hidden while the room is shown, with their visibility before; the map's clear colour while
/// the Skate 2 room has it black.
#[derive(Resource, Default)]
struct HiddenActors(Vec<(Entity, Visibility)>, Option<Color>);

/// Not retail: the room is the skater's alone, so the world's own actors (traffic cars, movable props, the map's
/// props, Party Play's waiting skaters, the Skate 1/2 cast) aren't drawn in it either; the map's meshes are already
/// ([`stage`]) and other players' skaters too (multiplayer::render). Skate 2's room stands in black, as in Skate 2:
/// no sky and a black clear behind its glass. Re-hidden every frame, late, as some of them set their own visibility
/// (the map's props: retail_backdrop's hide_online) or colour each frame; each gets back what it had when the room closes.
#[allow(clippy::type_complexity)]
fn hide_actors(
    staging: Res<Staging>,
    mut hidden: ResMut<HiddenActors>,
    mut actors: Query<
        (
            Entity,
            &mut Visibility,
            Option<&Name>,
            Has<crate::traffic::TrafficCar>,
            Has<crate::physics::props::PropPiece>,
            Has<crate::retail_render::OnlineHidden>,
            Has<MeshMaterial3d<RetailSkyMaterial>>,
        ),
        (
            Without<Room>,
            Or<(
                With<crate::traffic::TrafficCar>,
                With<crate::physics::props::PropPiece>,
                With<crate::retail_render::OnlineHidden>,
                With<MeshMaterial3d<RetailSkyMaterial>>,
                With<Name>,
            )>,
        ),
    >,
    puppets: Query<(), With<crate::multiplayer::appearance::PuppetCharacter>>,
    clear: Option<ResMut<ClearColor>>,
) {
    if !staging.shown {
        for (entity, previous) in std::mem::take(&mut hidden.0) {
            if let Ok((_, mut visibility, ..)) = actors.get_mut(entity) {
                *visibility = previous;
            }
        }
        if let (Some(colour), Some(mut clear)) = (hidden.1.take(), clear) {
            clear.0 = colour;
        }
        return;
    }
    let skate2 = staging.loaded == crate::graphics_menu::ROOMS[1].1;
    if let Some(mut clear) = clear.filter(|c| skate2 && c.0 != Color::BLACK) {
        hidden.1 = Some(clear.0);
        clear.0 = Color::BLACK;
    }
    let cast = |name: Option<&Name>| name.is_some_and(|n| n.as_str().starts_with("Skate 1 cast") || n.as_str().starts_with("Skate 2 cast"));
    for (entity, mut visibility, name, car, prop, map_prop, sky) in &mut actors {
        if !(car || prop || map_prop || (sky && skate2) || puppets.contains(entity) || cast(name)) {
            continue;
        }
        match hidden.0.iter_mut().find(|(e, _)| *e == entity) {
            // Its own system turned it back on this frame: that's what it shows when the room closes.
            Some((_, previous)) if *visibility != Visibility::Hidden => *previous = *visibility,
            Some(_) => {}
            None => hidden.0.push((entity, *visibility)),
        }
        visibility.set_if_neq(Visibility::Hidden);
    }
}

#[allow(clippy::type_complexity)]
fn stage(
    customiser: Res<Customiser>,
    mut staging: ResMut<Staging>,
    history: Res<crate::presentation::Presentation>,
    mut room: Query<(&mut Transform, &mut Visibility), With<Room>>,
    mut scenery: Query<
        (Entity, &mut Visibility),
        (With<MapEntity>, With<Mesh3d>, Without<Room>, Without<MeshMaterial3d<RetailSkyMaterial>>),
    >,
) {
    let show = customiser.open && !room.is_empty();
    if show {
        // The camera's root, not PlayerRoot: Rotate Skater turns only the skater.
        if let Some((_, current)) = history.pair() {
            let place = place(&current.root);
            // Only a moved camera root moves the room: a written Transform re-propagates the whole room.
            for (mut transform, mut visibility) in &mut room {
                transform.set_if_neq(place);
                visibility.set_if_neq(Visibility::Visible);
            }
        }
        if !staging.shown {
            staging.shown = true;
            for (entity, mut visibility) in &mut scenery {
                if *visibility != Visibility::Hidden {
                    staging.hidden.push((entity, *visibility));
                    *visibility = Visibility::Hidden;
                }
            }
        }
    } else if staging.shown {
        staging.shown = false;
        for (entity, previous) in std::mem::take(&mut staging.hidden) {
            if let Ok((_, mut visibility)) = scenery.get_mut(entity) {
                *visibility = previous;
            }
        }
        for (_, mut visibility) in &mut room {
            *visibility = Visibility::Hidden;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// v12 list 2: cars, props and the cast stay out of the room, and get their own visibility back after it.
    #[test]
    fn the_worlds_actors_are_hidden_while_the_room_shows() {
        let mut app = App::new();
        app.init_resource::<Staging>().init_resource::<HiddenActors>().add_systems(Update, hide_actors);
        let car = app.world_mut().spawn((crate::traffic::TrafficCar::default(), Visibility::Inherited)).id();
        let cast = app.world_mut().spawn((Name::new("Skate 2 cast: slappy"), Visibility::Visible)).id();
        let sign = app.world_mut().spawn((Name::new("Shop sign"), Visibility::Visible)).id();
        let shown = |app: &App, e: Entity| *app.world().get::<Visibility>(e).unwrap();
        app.world_mut().resource_mut::<Staging>().shown = true;
        app.update();
        assert_eq!((shown(&app, car), shown(&app, cast)), (Visibility::Hidden, Visibility::Hidden));
        assert_eq!(shown(&app, sign), Visibility::Visible, "only the actors");
        // The cast's own system shows it again mid-visit: hidden again, and that's what it gets back.
        app.world_mut().entity_mut(cast).insert(Visibility::Inherited);
        app.update();
        assert_eq!(shown(&app, cast), Visibility::Hidden);
        app.world_mut().resource_mut::<Staging>().shown = false;
        app.update();
        assert_eq!((shown(&app, car), shown(&app, cast)), (Visibility::Inherited, Visibility::Inherited));
    }

    /// v12 (user 10-31): through the Skate 2 room's window a ramp and rails stood in the void (the map's props, which
    /// hide_online shows again every frame offline) and the glass showed the sky; in Skate 2 the room stands in black.
    #[test]
    fn the_skate2_room_stands_in_black_with_no_props_outside() {
        let mut app = App::new();
        app.init_resource::<Staging>().init_resource::<HiddenActors>().add_systems(Update, hide_actors);
        app.insert_resource(ClearColor(Color::srgb(0.2, 0.4, 0.8)));
        let prop = app.world_mut().spawn((crate::retail_render::OnlineHidden, Visibility::Inherited)).id();
        let sky = app.world_mut().spawn((MeshMaterial3d::<RetailSkyMaterial>::default(), Visibility::Visible)).id();
        let shown = |app: &App, e: Entity| *app.world().get::<Visibility>(e).unwrap();
        let clear = |app: &App| app.world().resource::<ClearColor>().0;
        // Skate 3's room keeps the sky.
        {
            let mut staging = app.world_mut().resource_mut::<Staging>();
            (staging.shown, staging.loaded) = (true, crate::graphics_menu::ROOMS[0].1.into());
        }
        app.update();
        assert_eq!((shown(&app, prop), shown(&app, sky)), (Visibility::Hidden, Visibility::Visible));
        app.world_mut().resource_mut::<Staging>().shown = false;
        app.update();
        // Skate 2's: no props (even as their own system shows them again), no sky, black behind the glass.
        {
            let mut staging = app.world_mut().resource_mut::<Staging>();
            (staging.shown, staging.loaded) = (true, crate::graphics_menu::ROOMS[1].1.into());
        }
        app.update();
        app.world_mut().entity_mut(prop).insert(Visibility::Inherited);
        app.update();
        assert_eq!((shown(&app, prop), shown(&app, sky)), (Visibility::Hidden, Visibility::Hidden));
        assert_eq!(clear(&app), Color::BLACK);
        app.world_mut().resource_mut::<Staging>().shown = false;
        app.update();
        assert_eq!((shown(&app, prop), shown(&app, sky)), (Visibility::Inherited, Visibility::Visible));
        assert_eq!(clear(&app), Color::srgb(0.2, 0.4, 0.8), "the map's colour back");
    }

    #[test]
    fn room_stands_the_skater_on_its_floor_with_the_wall_behind() {
        for yaw in [0., 0.7, -2.4, std::f32::consts::PI] {
            let root = Transform::from_xyz(12., 3.5, -40.).with_rotation(Quat::from_rotation_y(yaw) * Quat::from_rotation_x(0.3));
            let room = place(&root);
            assert!(room.transform_point(SPOT).distance(root.translation) < 1e-3, "skater on the spot");
            let camera = Quat::from_rotation_y(yaw) * Vec3::Z;
            // Ledge (x = 0.1) 0.9 m behind; open side (x = -7.43) toward the camera.
            let ledge = room.transform_point(Vec3::new(0.1, SPOT.y, SPOT.z)) - root.translation;
            let open = room.transform_point(Vec3::new(-7.43, SPOT.y, SPOT.z)) - root.translation;
            assert!((ledge.dot(camera) + 0.9).abs() < 1e-3 && open.dot(camera) > 6., "yaw {yaw}: ledge {ledge}, open {open}");
            assert!(ledge.y.abs() < 1e-3, "floor level");
        }
    }

    /// cas.json as old_cas.py writes it: a fit, one body and one camera.
    fn skate2(fit_yaw: f32, translation: Vec3) -> Skate2Cas {
        serde_json::from_value(serde_json::json!({
            "bodies": {"slappys": {"animation": "CAS_Sitting", "position": [0.391, 0.0, 0.929], "rotation": 90.0}},
            "cameras": {"hat": {"fov": 35.0, "look": [2.3, 1.4, 5.0], "position": [0.2, 1.0, 0.2], "up": [-10.0, 10.0, 0.1]}},
            "rooms": {"CreateSkater_Skate2": {"translation": translation.to_array(), "yaw": fit_yaw}},
        }))
        .unwrap()
    }

    #[test]
    fn skate2_room_space_goes_through_its_fit_then_the_room() {
        let seat = Vec3::new(0.391, 0., 0.929);
        for (yaw, fit_yaw) in [(0., 0.787), (0.7, 1.48), (-2.4, -3.14), (std::f32::consts::PI, 0.)] {
            let root = Transform::from_xyz(12., 3.5, -40.).with_rotation(Quat::from_rotation_y(yaw) * Quat::from_rotation_x(0.3));
            // A fit taking the seat onto Skate 3's spot sits the skater at the camera root.
            let cas = skate2(fit_yaw, SPOT - Quat::from_rotation_y(fit_yaw) * seat);
            let view = cas.view("CreateSkater_Skate2", &root, ("hat", "slappys")).unwrap();
            assert!(view.feet.distance(root.translation) < 1e-3, "yaw {yaw}: {} vs {}", view.feet, root.translation);
            let turn = Quat::from_rotation_y(yaw + std::f32::consts::FRAC_PI_2 + fit_yaw);
            assert!((view.facing * Vec3::Z).distance(turn * Vec3::X) < 1e-4, "yaw {yaw}: body heading 90 degrees");
            let back = place(&root) * Transform::from_translation(SPOT - Quat::from_rotation_y(fit_yaw) * seat)
                .with_rotation(Quat::from_rotation_y(fit_yaw));
            assert!(view.eye.distance(back.transform_point(Vec3::new(0.2, 1., 0.2))) < 1e-3);
            assert!((view.look - view.eye).normalize().distance(turn * Vec3::new(2.1, 0.4, 4.8).normalize()) < 1e-4);
            assert!((view.up.length() - 1.).abs() < 1e-5 && view.up.y > 0.7 && view.up.y < 0.71, "up {}", view.up);
            assert!((view.fov - 35_f32.to_radians()).abs() < 1e-6 && view.clip == "CAS_Sitting");
        }
        let cas = skate2(0., Vec3::ZERO);
        let root = Transform::default();
        assert!(cas.view("CreateSkater", &root, ("hat", "slappys")).is_none(), "Skate 3's room keeps its own");
        assert!(cas.view("CreateSkater_Skate2", &root, ("legs", "slappys")).is_none(), "no such camera: Skate 3's framing");
        assert!(Skate2Cas::default().view("CreateSkater_Skate2", &root, ("hat", "slappys")).is_none(), "no cas.json");
    }

    #[test]
    fn skate2_pages_take_skate2_shots() {
        let petes = "back_alley_petes";
        assert_eq!(skate2_shot(Node::Eyes, Frame::Face), ("eyes", petes));
        assert_eq!(skate2_shot(Node::Presets, Frame::Face), ("head", petes));
        assert_eq!(skate2_shot(Node::Shape, Frame::Body), ("body", petes));
        assert_eq!(skate2_shot(Node::Clothes("Hats"), Frame::Head), ("hat", "slappys"));
        assert_eq!(skate2_shot(Node::Clothes("Necklaces"), Frame::Face), ("necklace", "slappys"));
        assert_eq!(skate2_shot(Node::Clothes("Shoes"), Frame::Feet), ("feet", "slappys"));
        assert_eq!(skate2_shot(Node::Board("Decks"), Frame::Board), ("skateboard", "slappys"));
        assert_eq!(skate2_shot(Node::Root, Frame::Body), ("slappys", "slappys"));
        assert_eq!(skate2_shot(Node::Style, Frame::Body), ("standing", "idle"));
        assert_eq!(skate2_shot(Node::Clothes("Backpacks"), Frame::Back), STANDING);
        assert_eq!(skate2_shot(Node::TattooGrid(true, 3, 0), Frame::Torso), STANDING);
    }

    /// Every shot a page can ask for is in the installed cas.json (old_cas.py's names), and each Skate 2 room has its
    /// fit. Needs the installed assets (SKATE3_ASSET_ROOT).
    #[test]
    #[ignore]
    fn installed_skate2_cas_has_every_shot() {
        let root = std::path::PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap());
        let cas: Skate2Cas =
            serde_json::from_str(&std::fs::read_to_string(root.join("private/cas-skate2/cas.json")).unwrap()).unwrap();
        for (_, stem) in &crate::graphics_menu::ROOMS[1..] {
            assert!(cas.rooms.contains_key(*stem), "{stem}");
        }
        let frames = [Frame::Body, Frame::Face, Frame::Head, Frame::Torso, Frame::Legs, Frame::Feet, Frame::Hands, Frame::Board,
            Frame::BoardSide, Frame::Back, Frame::LeftSide, Frame::RightSide];
        let nodes = [Node::Root, Node::Body, Node::Skin, Node::Hair, Node::Shape, Node::Presets, Node::Face, Node::Eyes, Node::Brow,
            Node::Nose, Node::Jaw, Node::Mouth, Node::Chin, Node::FacialHair, Node::Style, Node::Tattoo(true), Node::Merch];
        for (node, frame) in nodes.iter().flat_map(|n| frames.iter().map(move |f| (*n, *f))) {
            let (camera, body) = skate2_shot(node, frame);
            assert!(cas.cameras.contains_key(camera) && cas.bodies.contains_key(body), "{node:?} {frame:?}: {camera} {body}");
        }
        assert!(cas.cameras.contains_key("slappys") && cas.bodies.contains_key("slappys"), "the default shot");
    }
}
