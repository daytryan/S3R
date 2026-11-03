use super::appearance::{Appearances, LocalPuppets, Look, PuppetCharacter, RemoteCharacter};
use super::*;
use crate::customiser_material::SkaterMaterial;
use bevy::{
    mesh::{morph::MorphWeights, skinning::SkinnedMesh},
    scene::SceneInstance,
};
struct Candidate {
    root: Entity,
    scenes: Vec<Entity>,
    look: Look,
    started: Instant,
}
#[derive(Component)]
pub(super) struct OutfitPiece {
    id: String,
    mid: String,
}

use skate_net::interpolation::{Buffer, Clock, position};
#[derive(Resource)]
pub(super) struct RemoteSkins {
    reported: f64,
    actors: BTreeMap<u64, RemoteSkin>,
    rest: Vec<Mat4>,
    parents: Vec<i32>,
    board: Option<usize>,
    /// Bug 10-17 #27: TRUCK_FRONT, TRUCK_BACK and LEFT_WHEELFRONT, for the board's size ([`board_pose`]).
    trucks: [Option<usize>; 2],
    wheel: Option<usize>,
    /// A skater went this frame: [`spawn`] frees the materials nobody wears, the next.
    left: bool,
}
#[derive(Default)]
struct RemoteSkin {
    root: Option<Entity>,
    bindings: Vec<(Entity, usize, Option<usize>)>,
    visible: Option<Entity>,
    pending: Option<Candidate>,
    requested: Option<[u8; 32]>,
    /// Last outfit change: a peer cycling looks only rebuilds every COOLDOWN.
    changed: Option<Instant>,

    positions: Buffer<[f32; 3]>,
    roots: Buffer<Transform>,
    poses: Buffer<Vec<Transform>>,
    /// The sender's own camera, on the same clock as its skater.
    cameras: Buffer<Transform>,
    clock: Clock,
    epoch: u64,
    /// Bug 10-17 #27: per binding, whether it is in the deck part (only its SKATEBOARD_ROOT is scaled; only the other
    /// parts' trucks move), and the board's shape from the last pose (None: Standard).
    on_deck: Vec<bool>,
    size: usize,
    shape: Option<crate::animation::BoardShape>,
}
pub(super) struct RemoteRenderPlugin;
impl Plugin for RemoteRenderPlugin {
    fn build(&self, app: &mut App) {
        let idle = app
            .world()
            .resource::<crate::assets::AssetManifest>()
            .0
            .initial_animation
            .clone();
        let skater = app.world().resource::<SkaterRuntime>();
        // The bind pose only when the idle can't be evaluated at all; puppets and boneless remotes retry it each frame
        // (present).
        let rest = idle_locals(skater, &idle, 0.).unwrap_or_else(|| {
            let pose = skater.animation.pose.iter().copied().map(skate_core::animation::output::sqt_to_matrix);
            pose.map(crate::animation::native_matrix).collect()
        });
        let parents = skater
            .animation
            .evaluator
            .frames
            .parents
            .iter()
            .map(|&p| p as i32)
            .collect();
        let bone = |name: &str| skater.animation.evaluator.frames.bone_names.iter().position(|n| n == name);
        let (board, trucks, wheel) = (bone("SKATEBOARD_ROOT"), [bone("TRUCK_FRONT"), bone("TRUCK_BACK")], bone("LEFT_WHEELFRONT"));
        app.init_resource::<LocalPuppets>().insert_resource(RemoteSkins {
            board,
            trucks,
            wheel,
            reported: 0.,
            actors: BTreeMap::new(),
            rest,
            parents,
            left: false,
        })
        .add_observer(respawned)
        .add_systems(
            Update,
            (spawn, bind, present)
                .chain()
                .after(crate::modding::vehicles::present)
                .after(crate::match_core::ask_watch)
                // Spectating: the camera takes the watched player's view at the time their skater is drawn this frame.
                .before(crate::match_core::observer_camera),
        );
    }
}
/// Bug 10-25 #7 online: a shown skater's piece that Bevy spawned again (its glb loaded a second time, see
/// customiser_parts::respawned; this player's Edit Skater loading an item the other wears is enough) has new joints and
/// the glTF's own material: build the look again, at once, rather than leave it white and hanging off the body.
fn respawned(ready: On<bevy::scene::SceneInstanceReady>, parents: Query<&ChildOf>, mut skins: ResMut<RemoteSkins>) {
    let Ok(parent) = parents.get(ready.entity) else { return };
    for skin in skins.actors.values_mut().filter(|s| s.visible == Some(parent.parent())) {
        (skin.requested, skin.changed) = (None, None);
    }
}
pub(super) fn spawn(
    mut commands: Commands,
    net: Res<Multiplayer>,
    looks: Res<Appearances>,
    mut skins: ResMut<RemoteSkins>,
    server: Res<AssetServer>,
    mut parts: ResMut<crate::customiser_parts::Parts>,
    models: Res<crate::custom_models::CustomModels>,
    puppets: Res<LocalPuppets>,
    pieces: Query<&OutfitPiece>,
) {
    // User 10-29: the frame after a skater went (its pieces despawned), its materials leave the cache.
    if std::mem::take(&mut skins.left) {
        parts.forget_unworn_materials(&pieces.iter().map(|p| p.mid.as_str()).collect::<std::collections::HashSet<_>>());
    }
    let removed: Vec<_> = skins
        .actors
        .keys()
        .filter(|id| !net.remotes.contains_key(id) && !puppets.0.contains_key(id))
        .copied()
        .collect();
    for id in removed {
        if let Some(root) = skins.actors.remove(&id).and_then(|s| s.root) {
            commands.entity(root).despawn();
            skins.left = true;
        }
    }
    if net.lobby.is_none() {
        crate::custom_graphic::forget_hidden();
        // A left lobby's design textures, once their skaters' materials are gone (a few frames).
        // ponytail: checked every offline frame; the cache is a few local sides then.
        // Puppets wear REMOTE_SIZE sides too, but a worn one is kept.
        parts.designs.forget_unworn(crate::custom_graphic::REMOTE_SIZE);
    }
    let show_designs = crate::custom_graphic::SHOW_OTHERS.load(std::sync::atomic::Ordering::Relaxed);
    // Party Play's puppets (local only) after the network's skaters, through the same loading and binding.
    for id in net.remotes.keys().copied().chain(puppets.0.keys().copied()) {
        let puppet = puppets.0.get(&id).filter(|_| !net.remotes.contains_key(&id));
        let skin = skins.actors.entry(id).or_insert_with(|| RemoteSkin {
            clock: Clock::for_connection(net.loopback),
            ..default()
        });
        let root = *skin.root.get_or_insert_with(|| match puppet {
            // Hidden until party_play places it: its look loads up front.
            Some(_) => commands
                .spawn((PuppetCharacter, Transform::default(), Visibility::Hidden, Name::new("Party Play skater")))
                .id(),
            None => commands
                .spawn((
                    RemoteCharacter(id),
                    Transform::default(),
                    Visibility::Inherited,
                    Name::new("Remote skater"),
                ))
                .id(),
        });
        let compatible = net
            .lobby
            .as_ref()
            .and_then(|l| l.actors.get(&id))
            .is_some_and(|a| a.info.rig == net.info.rig);
        // perf 10-18: the key from a borrow first; the look (an outfit's whole JSON) is cloned only when it changes,
        // not for every remote every frame. None: stock.
        let (mut key, look) = if let Some(p) = puppet {
            (puppet_key(&p.look), Some(&p.look))
        } else if skin.visible.is_some() && compatible {
            looks.looks.get(&id).map_or(([0; 32], None), |(key, look)| (*key, Some(look)))
        } else {
            ([0; 32], None)
        };
        // Hidden creator designs: the same outfit under another key, so toggling rebinds.
        let strip = puppet.is_none() && (!show_designs || crate::custom_graphic::hidden(id));
        if let (true, Some(Look::Outfit(profile))) = (strip, look) {
            if has_designs(profile) {
                key[0] ^= 0x80;
            }
        }
        // 3.2: Settings > Audio > Custom Models (and Hide / Block): their own model never reaches the gltf loader;
        // their stock skater shows instead.
        let blocked = matches!(look, Some(Look::Native(_) | Look::Imported(_))) && puppet.is_none() && !crate::player_list::models(id);
        if blocked {
            key = [0; 32];
        }
        if skin.requested == Some(key) {
            continue;
        }
        let look = match look {
            None => Look::Stock,
            Some(_) if blocked => Look::Stock,
            Some(Look::Outfit(profile)) if strip => {
                let mut profile = profile.clone();
                crate::custom_graphic::without_designs(&mut profile);
                Look::Outfit(profile)
            }
            Some(look) => look.clone(),
        };
        const COOLDOWN: Duration = Duration::from_secs(2);
        if puppet.is_none() && key != [0; 32] && skin.changed.is_some_and(|t| t.elapsed() < COOLDOWN) {
            continue; // The latest look applies once the cooldown ends.
        }
        if key != [0; 32] {
            skin.changed = Some(Instant::now());
        }
        skin.requested = Some(key);
        if let Some(old) = skin.pending.take() {
            commands.entity(old.root).despawn();
        }
        let mut scenes = vec![];
        let mut specs = vec![];
        let look = match look {
            Look::Outfit(profile) => match parts.resolve(&profile) {
                Ok(p) => Look::Outfit(p),
                Err(e) => {
                    warn!("Remote outfit: {e}");
                    Look::Stock
                }
            },
            other => other,
        };
        match &look {
            Look::Stock => specs.push(("private/skater.glb".to_owned(), None)),
            Look::Native(key) => {
                if let Some(path) = models.online_native_path(key) {
                    specs.push((path, None));
                } else {
                    specs.push(("private/skater.glb".to_owned(), None));
                }
            }
            Look::Imported(path) => specs.push((path.clone(), None)),
            Look::Outfit(profile) => {
                for v in profile["selections"]
                    .as_object()
                    .into_iter()
                    .flat_map(|s| s.values())
                {
                    let Some((id, mid)) = v["asset_id"].as_str().zip(v["material_id"].as_str())
                    else {
                        continue;
                    };
                    if let Some(part) = parts.library.models.get(id) {
                        specs.push((part.scene.clone(), Some((id.to_owned(), mid.to_owned()))));
                    }
                }
            }
        }
        if specs.is_empty() || specs.len() > 32 {
            continue;
        }
        let candidate = commands
            .spawn((Transform::default(), Visibility::Hidden, ChildOf(root)))
            .id();
        if let Look::Native(key) = &look {
            commands
                .entity(candidate)
                .insert(crate::custom_models::NativeModelRoot(key.clone()));
        }
        if matches!(look, Look::Imported(_)) {
            commands
                .entity(candidate)
                .insert(crate::custom_models::CustomModelRoot);
        }
        for (path, piece) in specs {
            // F1: another player's received model is drawn, never read: its meshes and textures (up to 256 MB of
            // each) keep no RAM copy after upload, as the maps do. Library paths stay shared with this player's own.
            let scene = GltfAssetLabel::Scene(0).from_asset(path.clone());
            let scene = if path.starts_with("online-characters://") {
                server.load_with_settings(scene, |s: &mut bevy::gltf::GltfLoaderSettings| {
                    s.load_meshes = bevy::asset::RenderAssetUsages::RENDER_WORLD;
                    s.load_materials = bevy::asset::RenderAssetUsages::RENDER_WORLD;
                })
            } else if let Some(part) = piece.as_ref().and_then(|(id, _)| parts.library.models.get(id)) {
                // perf 10-19: a library part through Part::load, as this player's own (SKATE_PARTS_GPU_ONLY).
                part.load(&server, Some(GltfAssetLabel::Scene(0)))
            } else if path == "private/skater.glb" {
                // perf 10-19: the stock skater, as assets.rs and world.rs load it.
                crate::customiser_parts::load_gltf(&server, scene, false)
            } else {
                server.load(scene)
            };
            let mut e = commands.spawn((
                SceneRoot(scene),
                ChildOf(candidate),
            ));
            if let Some((id, mid)) = piece {
                e.insert((
                    crate::customiser_parts::PartRoot(id.clone()),
                    OutfitPiece { id, mid },
                ));
            }
            scenes.push(e.id());
        }
        skin.pending = Some(Candidate {
            root: candidate,
            scenes,
            look,
            started: Instant::now(),
        });
    }
}

/// A puppet's look key: never the stock key, and new whenever its look changes.
/// Whether custom_graphic::without_designs would change `profile` (a side carrying a creator design).
fn has_designs(profile: &serde_json::Value) -> bool {
    profile["graphics"].as_object().is_some_and(|g| g.values().any(|side| side.get("design").is_some()))
}
fn puppet_key(look: &Look) -> [u8; 32] {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(look).unwrap_or_default().hash(&mut hasher);
    let mut key = [1; 32];
    key[..8].copy_from_slice(&hasher.finish().to_le_bytes());
    key
}

fn bind(
    mut commands: Commands,
    net: Res<Multiplayer>,
    mut skins: ResMut<RemoteSkins>,
    skater: Res<SkaterRuntime>,
    meshes: Query<(Entity, &SkinnedMesh)>,
    nodes: Query<(&Name, &Transform)>,
    parents: Query<&ChildOf>,
    instances: Query<&SceneInstance>,
    spawner: Res<SceneSpawner>,
    server: Res<AssetServer>,
    mut parts: ResMut<crate::customiser_parts::Parts>,
    mut materials: ResMut<Assets<SkaterMaterial>>,
    pieces: Query<&OutfitPiece>,
    mut morphs: Query<(Entity, &mut MorphWeights)>,
    mut images: ResMut<Assets<Image>>,
    (names, roots): (Query<&bevy::gltf::GltfMaterialName>, Query<&crate::customiser_parts::PartRoot>),
) {
    let initial_poses: BTreeMap<_, _> = skins
        .actors
        .iter()
        .filter(|(_, skin)| skin.pending.is_some())
        .map(|(&id, _)| {
            let bones = net
                .remotes
                .get(&id)
                .and_then(|r| r.poses.back())
                .map(|p| p.bones.as_slice())
                .unwrap_or(&[]);
            (id, globals(bones, &skins))
        })
        .collect();
    let (bones, board_bone) = (skins_index(&skins), skins.board);
    for (id, skin) in skins.actors.iter_mut() {
        let Some(p) = skin.pending.as_ref() else {
            continue;
        };
        if p.started.elapsed() > Duration::from_secs(120) {
            warn!("Remote appearance loading timed out; previous skater retained");
            commands.entity(p.root).despawn();
            skin.pending = None;
            continue;
        }
        if p.scenes.iter().any(|&e| {
            !instances
                .get(e)
                .is_ok_and(|i| spawner.instance_is_ready(**i))
        }) {
            continue;
        }
        if let Look::Outfit(profile) = &p.look {
            for e in &p.scenes {
                if let Ok(piece) = pieces.get(*e) {
                    parts.warm(&piece.mid, &server, &mut materials);
                }
            }
            let size = crate::custom_graphic::REMOTE_SIZE;
            if !parts.graphics_ready(profile, &server, &mut images, size)
                | !parts.tattoos_ready(profile, &server)
                || p.scenes.iter().any(|&e| {
                    pieces
                        .get(e)
                        .is_ok_and(|piece| !parts.material_ready(&piece.mid, &server))
                })
            {
                continue;
            }
        }
        let skater_bones = &skater.animation.evaluator.frames.bone_names;
        let bindings = match crate::animation::AnimationStatus::for_scene(
            p.root,
            &crate::animation::with_face(skater_bones),
            &meshes,
            &nodes,
            &parents,
        ) {
            // Face joints (past the skater's bones) hold their bind: network poses never carry them.
            Ok(b) => b.online_bindings().into_iter().filter(|&(_, bone, _)| bone < skater_bones.len()).collect::<Vec<_>>(),
            Err(e) => {
                warn!("Remote appearance rejected: {e}");
                commands.entity(p.root).despawn();
                skin.pending = None;
                continue;
            }
        };
        if let Look::Outfit(profile) = &p.look {
            for &scene in &p.scenes {
                let Ok(piece) = pieces.get(scene) else {
                    continue;
                };
                let Some(material) =
                    parts.profile_material(&piece.id, &piece.mid, profile, &materials)
                else {
                    continue;
                };
                // The deck's grip (customiser_parts::GRIP) wears the profile's Grip Color.
                let grip = parts.grip_material(&piece.id, &piece.mid, &material, profile).map(|m| materials.add(m));
                let handle = materials.add(material);
                for (e, _) in &meshes {
                    if parents.iter_ancestors(e).any(|p| p == scene) {
                        let grip = grip.as_ref().filter(|_| names.get(e).is_ok_and(|n| n.0 == crate::customiser_parts::GRIP));
                        let handle = grip.unwrap_or(&handle);
                        commands
                            .entity(e)
                            .remove::<MeshMaterial3d<StandardMaterial>>()
                            .insert(MeshMaterial3d(handle.clone()));
                    }
                }
            }
            let weights: Vec<f32> = parts
                .library
                .morphs
                .iter()
                .enumerate()
                .map(|(i, n)| {
                    profile["morphs"][n]
                        .as_f64()
                        .unwrap_or(if (2..19).contains(&i) { 0.25 } else { 0. })
                        .clamp(0., 0.5) as f32
                })
                .collect();
            for (e, mut m) in &mut morphs {
                if parents.iter_ancestors(e).any(|e| e == p.root)
                    && m.weights().len() == weights.len()
                {
                    m.weights_mut().copy_from_slice(&weights);
                }
            }
        }
        for (e, _) in &meshes {
            if parents.iter_ancestors(e).any(|e| e == p.root) {
                commands.entity(e).insert((
                    bevy::camera::visibility::NoFrustumCulling,
                    bevy::camera::visibility::RenderLayers::from_layers(&[0, 28]),
                ));
            }
        }
        let pose = &initial_poses[id];
        let basis = Mat4::from_cols(Vec4::X, -Vec4::Z, Vec4::Y, Vec4::W);
        let mut locals: Vec<Transform> = bindings
            .iter()
            .map(|&(_, bone, parent)| {
                let global = pose[bone] * basis;
                Transform::from_matrix(parent.map_or(global, |parent| (pose[parent] * basis).inverse() * global))
            })
            .collect();
        // Bug 10-17 #27: the board at the sender's size. Puppets keep this pose; remotes are re-shaped as they move.
        skin.size = match &p.look {
            Look::Outfit(profile) => crate::animation::board_size_index(profile),
            _ => 1,
        };
        skin.on_deck = bindings
            .iter()
            .map(|&(entity, _, _)| {
                let part = parents.iter_ancestors(entity).find_map(|e| roots.get(e).ok());
                part.and_then(|r| parts.library.models.get(&r.0)).is_some_and(|m| m.slot == "SkateBoard")
            })
            .collect();
        skin.shape = board_pose(skin.size, &bindings, &skin.on_deck, &mut locals, &bones);
        for ((&(entity, bone, _), mut local), &on_deck) in bindings.iter().zip(locals).zip(&skin.on_deck) {
            if board_bone == Some(bone) {
                local.scale = deck_scale(skin.shape, on_deck, false);
            }
            commands.entity(entity).insert(local);
        }
        commands.entity(p.root).insert(crate::animation::BoardSizeOf(skin.size));
        // perf 10-19: the replay records these joints, once per distinct pose (replay::gather).
        if let Some(root) = skin.root {
            let keys = bindings.iter().zip(&skin.on_deck).map(|(&(entity, bone, parent), &deck)| (entity, (bone, parent, deck)));
            commands.entity(root).insert(crate::replay::ReplayJoints::new(keys));
        }
        if let Some(old) = skin.visible.replace(p.root) {
            commands.entity(old).despawn();
        }
        commands.entity(p.root).insert(Visibility::Inherited);
        info!(
            "ONLINE_CHARACTER_VISIBLE peer={id} joints={} kind={}",
            bindings.len(),
            if matches!(p.look, Look::Imported(_)) {
                "import"
            } else {
                "retail"
            }
        );
        skin.bindings = bindings;
        skin.poses = Buffer::default();
        skin.pending = None;
    }
}

/// The skeleton's board joints (bone indices), for [`board_pose`].
#[derive(Clone, Copy, Default)]
struct BoardBones {
    trucks: [Option<usize>; 2],
    wheel: Option<usize>,
}
fn skins_index(skins: &RemoteSkins) -> BoardBones {
    BoardBones { trucks: skins.trucks, wheel: skins.wheel }
}
/// Bug 10-17 #27: another player's board at their Board size, on a pose just written (`locals`, one per binding): the
/// truck and wheel parts' trucks move out along the length, from the fresh pose each time (never added up). Returns the
/// shape, whose `scale` [`deck_scale`] puts on the deck part's SKATEBOARD_ROOT. Riding: no held-deck offset.
/// ponytail: Edit Skater's held-deck offset (BoardShape::hold) isn't applied to other players.
fn board_pose(
    size: usize,
    bindings: &[(Entity, usize, Option<usize>)],
    on_deck: &[bool],
    locals: &mut [Transform],
    bones: &BoardBones,
) -> Option<crate::animation::BoardShape> {
    let find = |bone: Option<usize>| bone.and_then(|b| bindings.iter().position(|&(_, i, _)| i == b)).map(|k| locals[k]);
    let shape = crate::animation::BoardShape::new(size, &find(bones.trucks[0])?, find(bones.wheel)?.translation, false)?;
    for ((&(_, bone, _), local), &deck) in bindings.iter().zip(locals.iter_mut()).zip(on_deck) {
        if !deck && bones.trucks.contains(&Some(bone)) {
            shape.truck(local);
        }
    }
    Some(shape)
}
/// A remote SKATEBOARD_ROOT's scale: the deck part's at the board's size, all of them shrunk away while seated.
fn deck_scale(shape: Option<crate::animation::BoardShape>, on_deck: bool, seated: bool) -> Vec3 {
    let size = shape.filter(|_| on_deck).map_or(Vec3::ONE, |s| s.scale);
    size * if seated { 0.001 } else { 1. }
}

/// The stock idle (the manifest's initial animation) looping at `time`, as [`RemoteSkins::rest`] holds a pose (each
/// bone's local matrix): the rest pose is this at 0. None when the idle can't be evaluated.
fn idle_locals(skater: &SkaterRuntime, idle: &str, time: f32) -> Option<Vec<Mat4>> {
    use skate_core::animation::playback_tree::PoseCommand;
    let evaluator = &skater.animation.evaluator;
    let clip = evaluator.frames.clip(idle).ok()?;
    let length = clip.frames.len().saturating_sub(1) as f32 / f32::from_bits(clip.fps_bits).max(1.);
    let time = if length > 0. { time % length } else { 0. };
    let commands = [
        PoseCommand::Clip { name: idle.to_owned(), previous_time: time, time, loops: 0 },
        PoseCommand::Pose { name: "RIG_TPOSE".into() },
        PoseCommand::Add { motion_is_a: true },
    ];
    let pose = evaluator.evaluate(&commands).ok()?;
    Some(pose.into_iter().map(skate_core::animation::output::sqt_to_matrix).map(crate::animation::native_matrix).collect())
}
/// User 10-25 (never a T-pose): the idle at `time` on a bound skin's joints, its board at the skin's size, as `bind`
/// writes its first pose. The board's shape, or None without the idle.
fn write_idle(
    skin: &RemoteSkin,
    skins: &RemoteSkins,
    (skater, idle, time): (&SkaterRuntime, &str, f32),
    nodes: &mut Query<&mut Transform>,
) -> Option<Option<crate::animation::BoardShape>> {
    let locals = idle_locals(skater, idle, time)?;
    let pose = compose(&[], &locals, &skins.parents);
    let basis = Mat4::from_cols(Vec4::X, -Vec4::Z, Vec4::Y, Vec4::W);
    let mut posed: Vec<Transform> = (skin.bindings.iter())
        .map(|&(_, bone, parent)| {
            let global = pose[bone] * basis;
            Transform::from_matrix(parent.map_or(global, |parent| (pose[parent] * basis).inverse() * global))
        })
        .collect();
    let shape = board_pose(skin.size, &skin.bindings, &skin.on_deck, &mut posed, &skins_index(skins));
    for ((&(entity, bone, _), mut local), &on_deck) in skin.bindings.iter().zip(posed).zip(&skin.on_deck) {
        if skins.board == Some(bone) {
            local.scale = deck_scale(shape, on_deck, false);
        }
        if let Ok(mut t) = nodes.get_mut(entity) {
            *t = local;
        }
    }
    Some(shape)
}

fn globals(bones: &[skate_net::Bone], skin: &RemoteSkins) -> Vec<Mat4> {
    compose(bones, &skin.rest, &skin.parents)
}
/// Global matrices: the sent `bones`, every other bone its `rest` local under its parent.
fn compose(bones: &[skate_net::Bone], rest: &[Mat4], parents: &[i32]) -> Vec<Mat4> {
    let mut result = vec![None; rest.len()];
    for b in bones {
        if let Some(slot) = result.get_mut(b.index as usize) {
            *slot = Some(network::matrix(b.pose));
        }
    }
    fn visit(i: usize, rest: &[Mat4], parents: &[i32], result: &mut [Option<Mat4>]) -> Mat4 {
        if let Some(m) = result[i] {
            return m;
        }
        let p = parents[i];
        let matrix = if p >= 0 {
            visit(p as usize, rest, parents, result) * rest[i]
        } else {
            rest[i]
        };
        result[i] = Some(matrix);
        matrix
    }
    for i in 0..result.len() {
        visit(i, rest, parents, &mut result);
    }
    result.into_iter().map(Option::unwrap).collect()
}
/// Remote skaters are skinned with NoFrustumCulling (bind), so every one in a 50-player lobby was drawn,
/// shadowed and re-posed each frame wherever it was. Cull whole skaters here instead.
/// ponytail: last frame's frustum (PostUpdate builds it), so a hard camera cut can show a remote a frame late;
/// the radius covers ordinary pans and a skater's own shadow.
const CULL_RADIUS: f32 = 4.;
fn in_view(camera: Option<(&bevy::camera::primitives::Frustum, &GlobalTransform)>, at: Vec3) -> bool {
    camera.is_none_or(|(frustum, eye)| {
        // Settings > Graphics > Draw Distance; High's 400 m: a skater is a few pixels tall there at 1080p.
        eye.translation().distance(at) < crate::graphics_menu::draw_distance().3
            && frustum.intersects_sphere(&bevy::camera::primitives::Sphere { center: at.into(), radius: CULL_RADIUS }, false)
    })
}
/// Spectating's handover: the longest the camera waits for an asked player's samples to come at the watched rate before
/// it cuts to them anyway (a sender that never speeds up: an older build, a stalled link). Room for our `@watch` to
/// reach that player through the host and HANDOVER_SPAN of its watched-rate samples to come back on a slow link.
const HANDOVER_WAIT: f64 = 2.;
/// The newest stretch of samples [`hand_over`] wants at the watched rate: the restarted clock plays 0.06-0.15 s behind
/// its newest sample (interpolation::Clock), and a sample's more.
const HANDOVER_SPAN: f64 = 0.2;

/// Whether the newest `span` seconds of `buffer` came at the watched rate: no gap over 0.1 s between two samples that
/// differ (a skater standing still repeats itself slowly, and that is no gap).
fn dense<T: PartialEq>(buffer: &Buffer<T>, span: f64) -> bool {
    let s = &buffer.samples;
    let Some(head) = s.back().map(|h| h.time) else { return false };
    s.iter()
        .rev()
        .zip(s.iter().rev().skip(1))
        .find_map(|(b, a)| if b.time - a.time > 0.1 && b.value != a.value { Some(false) } else { (a.time <= head - span).then_some(true) })
        .unwrap_or(false)
}

/// Spectating's handover (user 10-25: the player you watch is always smooth, cutting to a far one too): whether the
/// camera cuts to this skater, asked `waited` s ago: Some(true) once its newest samples came at the watched rate (roots,
/// its camera rows, its pose once bound), Some(false) when HANDOVER_WAIT passed without, None while it waits. Its clock
/// restarts on them at the cut, so the camera never shows a far player's sparse past (floaty, no tricks) nor the snap
/// forward once its rate comes up.
fn hand_over(skin: &mut RemoteSkin, waited: f64) -> Option<bool> {
    let ready = dense(&skin.roots, HANDOVER_SPAN)
        && (skin.cameras.samples.is_empty() || dense(&skin.cameras, HANDOVER_SPAN))
        && (skin.bindings.is_empty() || dense(&skin.poses, HANDOVER_SPAN));
    (ready || waited > HANDOVER_WAIT).then(|| {
        skin.clock.restart();
        ready
    })
}

#[allow(clippy::type_complexity)]
fn present(
    mut net: ResMut<Multiplayer>,
    vehicles: Res<crate::modding::vehicles::Vehicles>,
    mut skins: ResMut<RemoteSkins>,
    mut nodes: Query<&mut Transform>,
    mut shown: Query<&mut Visibility, Or<(With<RemoteCharacter>, With<PuppetCharacter>)>>,
    camera: Query<(&bevy::camera::primitives::Frustum, &GlobalTransform), With<crate::camera::GameplayCamera>>,
    puppets: Res<LocalPuppets>,
    mut views: ResMut<super::RemoteCameras>,
    cas: Option<Res<crate::customiser::Customiser>>,
    mut watching: ResMut<crate::match_core::Watching>,
    (skater, manifest): (Res<SkaterRuntime>, Res<crate::assets::AssetManifest>),
) {
    let camera = camera.single().ok();
    let idle = manifest.0.initial_animation.as_str();
    // Each skater standing in the idle a little out of step with the others, as the Skate 2 cast breathes.
    let offset = |id: u64| (id % 7) as f32 * 1.7;
    // Edit Skater open online (user 10-16): the room is this player's alone, so the others aren't drawn in it.
    let editing = cas.is_some_and(|c| c.open);
    views.0.clear();
    let basis = Mat4::from_cols(Vec4::X, -Vec4::Z, Vec4::Y, Vec4::W);
    let now = net.started.elapsed().as_secs_f64();
    // Our own camera (or a player not drawn here) needs no samples: the camera goes there at once.
    if watching.asked.is_none_or(|a| !net.remotes.contains_key(&a) || !skins.actors.contains_key(&a)) {
        watching.shown = watching.asked;
    }
    for (&id, remote) in &net.remotes {
        let Some(mut skin) = skins.actors.remove(&id) else {
            continue;
        };
        if skin.epoch != remote.epoch {
            skin.positions = Buffer::default();
            skin.roots = Buffer::default();
            skin.poses = Buffer::default();
            skin.cameras = Buffer::default();
            skin.clock = Clock::for_connection(net.loopback);
            skin.epoch = remote.epoch;
        }
        for sample in &remote.roots {
            if skin
                .roots
                .samples
                .back()
                .is_some_and(|p| p.time >= sample.captured)
            {
                continue;
            }
            let root = Transform::from_matrix(network::matrix(sample.pose));
            // perf 10-18: an unchanged sample (the sender idling) is no delivery gap (interpolation::Clock).
            let moved = skin.roots.samples.back().is_none_or(|p| p.value != root);
            if skin.roots.insert(sample.captured, root) {
                skin.positions.insert(sample.captured, sample.pose.p);
                skin.clock.observe_sample(0, sample.captured, sample.received, moved);
            }
        }
        for sample in &remote.poses {
            let Some(c) = sample.camera else { continue };
            if skin.cameras.samples.back().is_none_or(|p| p.time < sample.captured) {
                skin.cameras.insert(sample.captured, Transform::from_translation(Vec3::from_array(c.p)).with_rotation(Quat::from_array(c.q)));
            }
        }
        // Each animation sample is resolved once; physics arrivals never duplicate it.
        if !skin.bindings.is_empty() {
            for sample in &remote.poses {
                if skin
                    .poses
                    .samples
                    .back()
                    .is_some_and(|p| p.time >= sample.captured)
                {
                    continue;
                }
                let g = globals(&sample.bones, &skins);
                let locals: Vec<Transform> = skin
                    .bindings
                    .iter()
                    .map(|&(_, i, parent)| {
                        Transform::from_matrix(
                            parent
                                .map_or(g[i] * basis, |p| (g[p] * basis).inverse() * g[i] * basis),
                        )
                    })
                    .collect();
                let moved = skin.poses.samples.back().is_none_or(|p| p.value != locals);
                if skin.poses.insert(sample.captured, locals) {
                    skin.clock.observe_sample(1, sample.captured, sample.received, moved);
                }
            }
        }
        if watching.asked == Some(id)
            && watching.shown != Some(id)
            && let Some(ready) = hand_over(&mut skin, now - watching.since)
        {
            if !ready {
                info!("MULTIPLAYER_SPECTATE handover to {id} without the watched rate after {HANDOVER_WAIT} s");
            }
            watching.shown = Some(id);
        }
        let time = skin.clock.step(now);
        // Every player's camera rows are its own skater's gameplay camera, a spectator's too (multiplayer::sent_camera):
        // watching one shows its parked skater, as retail's spectate camera follows the skater picked (audit 10-30).
        if let Some((a, b, alpha)) = time.and_then(|t| skin.cameras.pair(t)) {
            let view = crate::presentation::blend(skin.cameras.samples[a].value, skin.cameras.samples[b].value, alpha);
            views.0.insert(id, view);
        }
        if let (Some(root), Some(time)) = (skin.root, time) {
            if let Some((a, b, alpha)) = skin.roots.pair(time) {
                let mut transform = crate::presentation::blend(
                    skin.roots.samples[a].value,
                    skin.roots.samples[b].value,
                    alpha,
                );
                if let Some(p) = position(&skin.positions, time) {
                    transform.translation = Vec3::from_array(p);
                }
                if let Ok(mut t) = nodes.get_mut(root) {
                    *t = transform;
                }
            }
        }
        if let Some(root) = skin.root {
            if let Some(attached) = crate::modding::vehicles::network::attached_root(&vehicles, id)
            {
                if let Ok(mut t) = nodes.get_mut(root) {
                    *t = attached;
                }
            }
        }
        // A culled skater keeps buffering samples but costs no joint writes (and no transform propagation). The one the
        // camera shows is never culled: the cull reads last frame's camera, which at a cut was elsewhere.
        let visible = !editing
            && !crate::player_list::ghost(id)
            && (watching.shown == Some(id) || skin.root.and_then(|root| nodes.get(root).ok()).is_none_or(|t| in_view(camera, t.translation)));
        if let Some(mut shown) = skin.root.and_then(|root| shown.get_mut(root).ok()) {
            shown.set_if_neq(if visible { Visibility::Inherited } else { Visibility::Hidden });
        }
        if let (true, Some(time)) = (visible, time) {
            if let Some((a, b, alpha)) = skin.poses.pair(time) {
                let ag = &skin.poses.samples[a].value;
                let bg = &skin.poses.samples[b].value;
                let mut posed: Vec<Transform> = ag.iter().zip(bg).map(|(a, b)| crate::presentation::blend(*a, *b, alpha)).collect();
                // Bug 10-17 #27: the sender's Board size, on the pose just written (a frame without one keeps the last).
                skin.shape = board_pose(skin.size, &skin.bindings, &skin.on_deck, &mut posed, &skins_index(&skins));
                for (&(entity, _, _), pose) in skin.bindings.iter().zip(posed) {
                    if let Ok(mut t) = nodes.get_mut(entity) {
                        *t = pose;
                    }
                }
            } else if skin.poses.samples.is_empty() && !skin.bindings.is_empty() {
                // User 10-25 (never a T-pose): a player whose samples carry no bones (another build's skeleton:
                // multiplayer drops them) or none yet stands in the idle, looping, not one held pose.
                if let Some(shape) = write_idle(&skin, &skins, (&skater, idle, now as f32 + offset(id)), &mut nodes) {
                    skin.shape = shape;
                }
            }
        }
        let seated = remote.body.enabled & (1u64 << 62) != 0;
        for (k, &(entity, i, _)) in skin.bindings.iter().enumerate() {
            if skins.board == Some(i) {
                let scale = deck_scale(skin.shape, skin.on_deck.get(k).copied().unwrap_or(false), seated);
                if let Ok(mut t) = nodes.get_mut(entity) {
                    if t.scale != scale {
                        t.scale = scale;
                    }
                }
            }
        }
        skins.actors.insert(id, skin);
    }
    // Party Play's puppets stand where party_play puts them, in the idle, looping (user 10-25: posed once at bind, a
    // puppet stood in one frame forever, and in the bind pose once a piece of its outfit loaded again).
    // ponytail: the stock idle (the manifest's initial animation); retail's AI.PinAI (TU3 0x8284adc8) only sends
    // cMsgPinPlayer, and no waiting-skater idle of its own was traced.
    for (id, puppet) in &puppets.0 {
        let Some(skin) = skins.actors.get(id) else { continue };
        let Some(root) = skin.root else { continue };
        if let (Some(at), Ok(mut t)) = (puppet.at, nodes.get_mut(root)) {
            t.set_if_neq(at);
        }
        let visible = puppet.at.is_some_and(|at| in_view(camera, at.translation));
        if let Ok(mut shown) = shown.get_mut(root) {
            shown.set_if_neq(if visible { Visibility::Inherited } else { Visibility::Hidden });
        }
        if visible && !skin.bindings.is_empty() {
            write_idle(skin, &skins, (&skater, idle, now as f32 + offset(*id)), &mut nodes);
        }
    }
    if now < skins.reported || now - skins.reported >= 1. {
        let delay = skins
            .actors
            .values()
            .map(|s| s.clock.delay)
            .fold(0., f64::max);
        let stalls: u64 = skins.actors.values().map(|s| s.clock.underruns).sum();
        net.visual_status = format!(
            "Visual interpolation: {:.0} ms target buffer | stalls {}",
            delay * 1000.,
            stalls
        );
        if net.active() {
            info!("MULTIPLAYER_INTERPOLATION {}", net.visual_status);
        }
        skins.reported = now;
    }
}

#[cfg(test)]
mod board_size_tests {
    use super::*;

    /// A board's joints as the stock skin has them: SKATEBOARD_ROOT 0, TRUCK_FRONT 1 (out along z, turned so the
    /// wheel's y is the deck's x), TRUCK_BACK 2, LEFT_WHEELFRONT 3; the deck part's three, then the truck part's.
    fn board() -> (Vec<(Entity, usize, Option<usize>)>, Vec<bool>, Vec<Transform>, BoardBones) {
        let truck = |z: f32| Transform::from_xyz(0., -0.05, z).with_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2));
        let joints = [Transform::IDENTITY, truck(0.3), truck(-0.3), Transform::from_xyz(0., 0.1, 0.)];
        let bindings: Vec<_> = (0..8u32).map(|k| (Entity::from_raw_u32(k + 1).unwrap(), (k % 4) as usize, None)).collect();
        let on_deck = (0..8).map(|k| k < 4).collect();
        let locals = (0..8).map(|k| joints[k % 4]).collect();
        (bindings, on_deck, locals, BoardBones { trucks: [Some(1), Some(2)], wheel: Some(3) })
    }

    /// Bug 10-17 #27: another player's Longboard scales their deck part's root and moves the truck part's trucks out;
    /// Standard changes nothing, and posing again from a fresh pose never adds up.
    #[test]
    fn a_remote_longboard_is_shaped_from_each_fresh_pose() {
        let (bindings, on_deck, fresh, bones) = board();
        let mut locals = fresh.clone();
        assert_eq!(board_pose(1, &bindings, &on_deck, &mut locals, &bones), None);
        assert_eq!(locals, fresh, "Standard leaves the pose alone");
        assert_eq!(deck_scale(None, true, false), Vec3::ONE);
        let shape = board_pose(3, &bindings, &on_deck, &mut locals, &bones).unwrap();
        let (_, length, width) = crate::animation::BOARD_SIZES[3];
        assert!(deck_scale(Some(shape), true, false).abs_diff_eq(Vec3::new(width, 1., length), 1e-6), "length on z, width on x");
        assert_eq!(deck_scale(Some(shape), false, false), Vec3::ONE, "only the deck part's root");
        assert!(deck_scale(Some(shape), true, true).abs_diff_eq(Vec3::new(width, 1., length) * 0.001, 1e-6), "seated");
        assert!((locals[5].translation.z - 0.3 * length).abs() < 1e-6 && (locals[6].translation.z + 0.3 * length).abs() < 1e-6);
        assert_eq!(locals[1], fresh[1], "the deck part's own trucks follow its scaled root");
        let mut again = fresh.clone();
        board_pose(3, &bindings, &on_deck, &mut again, &bones);
        assert_eq!(again, locals, "absolute: the same fresh pose gives the same board");
    }
}

#[cfg(test)]
mod cull_tests {
    use super::*;
    #[test]
    fn remote_skaters_cull_behind_the_camera_and_past_the_far_distance() {
        let eye = GlobalTransform::from(Transform::from_xyz(0., 2., 0.).looking_to(Vec3::NEG_Z, Vec3::Y));
        let clip = Mat4::perspective_infinite_reverse_rh(1., 16. / 9., 0.1) * Mat4::from(eye.affine().inverse());
        let frustum = bevy::camera::primitives::Frustum::from_clip_from_world(&clip);
        let camera = Some((&frustum, &eye));
        assert!(in_view(camera, Vec3::new(0., 0., -10.)));
        // Just behind the camera still draws: limbs and its shadow can reach into view.
        assert!(in_view(camera, Vec3::new(0., 0., 2.)));
        assert!(!in_view(camera, Vec3::new(0., 0., 20.)));
        assert!(!in_view(camera, Vec3::new(0., 0., -500.)));
        assert!(in_view(None, Vec3::new(0., 0., 20.)));
    }
    /// 10-12 audit item 4: CPU per frame of 49 remote skaters, headless. The real pieces of `present` (Buffer, Clock,
    /// `globals` + binding locals per new pose sample, root and joint blends) at the lobby's rates (nearest 8 at 10 Hz
    /// POSE, the crowd at 3 Hz: lobby B3), every skater in view (no culling), on a 160-joint rig.
    /// ponytail: not the ECS writes or transform propagation (needs the game's assets); those scale with the same blends.
    #[test]
    fn forty_nine_remote_skaters_cost_little_cpu_per_frame() {
        let joints: usize = 160;
        let skins = RemoteSkins {
            reported: 0.,
            actors: BTreeMap::new(),
            rest: (0..joints).map(|i| Mat4::from_translation(Vec3::new(0., 0.05 * i as f32, 0.))).collect(),
            parents: (0..joints as i32).map(|i| i - 1).collect(),
            board: None,
            trucks: [None, None],
            wheel: None,
            left: false,
        };
        let basis = Mat4::from_cols(Vec4::X, -Vec4::Z, Vec4::Y, Vec4::W);
        let bindings: Vec<(Entity, usize, Option<usize>)> =
            (0..joints).map(|i| (Entity::PLACEHOLDER, i, i.checked_sub(1))).collect();
        let bones: Vec<skate_net::Bone> = (0..32)
            .map(|i| skate_net::Bone { index: i * 5, pose: skate_net::Pose { p: [0., 0.1, 0.], q: [0., 0., 0., 1.] } })
            .collect();
        let mut remotes: Vec<RemoteSkin> =
            (0..49).map(|_| RemoteSkin { clock: Clock::for_connection(false), ..Default::default() }).collect();
        let (frames, mut blends) = (600, 0usize);
        let started = Instant::now();
        for frame in 0..frames {
            let now = frame as f64 / 60.;
            for (n, skin) in remotes.iter_mut().enumerate() {
                let period = if n < 8 { 0.1 } else { 1. / 3. };
                let captured = (now / period).floor() * period;
                if skin.poses.samples.back().is_none_or(|s| s.time < captured) {
                    if skin.roots.insert(captured, Transform::from_xyz(captured as f32, 0., n as f32)) {
                        skin.clock.observe(0, captured, now);
                    }
                    let g = globals(&bones, &skins);
                    let locals = bindings
                        .iter()
                        .map(|&(_, i, parent)| {
                            Transform::from_matrix(parent.map_or(g[i] * basis, |p| (g[p] * basis).inverse() * g[i] * basis))
                        })
                        .collect();
                    if skin.poses.insert(captured, locals) {
                        skin.clock.observe(1, captured, now);
                    }
                }
                let Some(time) = skin.clock.step(now) else { continue };
                if let Some((a, b, alpha)) = skin.roots.pair(time) {
                    std::hint::black_box(crate::presentation::blend(skin.roots.samples[a].value, skin.roots.samples[b].value, alpha));
                }
                if let Some((a, b, alpha)) = skin.poses.pair(time) {
                    for (a, b) in skin.poses.samples[a].value.iter().zip(&skin.poses.samples[b].value) {
                        std::hint::black_box(crate::presentation::blend(*a, *b, alpha));
                        blends += 1;
                    }
                }
            }
        }
        let ms = started.elapsed().as_secs_f64() * 1000. / frames as f64;
        eprintln!("49 remote skaters: {ms:.3} ms per frame (debug build), {} joint blends per frame", blends / frames);
        assert!(blends > 0, "playout started");
        assert!(ms < 5., "{ms} ms per frame");
    }
}

#[cfg(test)]
mod handover_tests {
    use super::*;

    #[test]
    fn dense_wants_the_watched_rate_over_the_newest_span() {
        let buffer = |times: &[f64], still: bool| {
            let mut b = Buffer::default();
            for &t in times {
                b.insert(t, if still { 0. } else { t });
            }
            b
        };
        let run = |from: f64, n: usize| (0..n).map(|k| from + 0.04 * k as f64).collect::<Vec<_>>();
        assert!(!dense(&buffer(&[], false), HANDOVER_SPAN), "nothing yet");
        assert!(!dense(&buffer(&[0., 1., 2., 3.], false), HANDOVER_SPAN), "the far rate, once a second");
        assert!(dense(&buffer(&[0., 1., 2., 3.], true), HANDOVER_SPAN), "standing still: a slow repeat is no gap");
        let mut times = vec![0., 1., 2.];
        times.extend(run(2.15, 4));
        assert!(!dense(&buffer(&times, false), HANDOVER_SPAN), "0.12 s at the watched rate: not yet");
        times.extend(run(2.31, 3));
        assert!(dense(&buffer(&times, false), HANDOVER_SPAN), "0.24 s at the watched rate");
    }

    /// User 10-25: cutting to a far player is seamless. It streams at the far rates (BODY 2 Hz, POSE 1 Hz, riding 8 m/s),
    /// so its clock plays 1.5 s deep; asked at 10 s, it sends at 25 Hz from 10.15 s (arriving 50 ms later). The camera
    /// cuts once 0.2 s of that is in, and from the cut the playout never jumps or starves and never blends across a
    /// far-rate gap. A clock that is not handed over (the old cut) shows the sparse past there instead.
    #[test]
    fn cutting_to_a_far_player_waits_for_the_watched_rate_then_plays_it_smoothly() {
        let fresh = || RemoteSkin { clock: Clock::for_connection(false), bindings: vec![(Entity::PLACEHOLDER, 0, None)], ..Default::default() };
        let (mut skin, mut old) = (fresh(), fresh());
        // (captured s, a POSE): BODY and POSE both carry the root; a POSE also the camera row and the bones.
        let mut sent: Vec<(f64, bool)> = (0..21).map(|k| (0.5 * k as f64, k % 2 == 0)).filter(|s| s.0 < 10.15).collect();
        sent.extend((0..71).map(|k| (10.15 + 0.04 * k as f64, true)));
        let feed = |skin: &mut RemoteSkin, &(c, pose): &(f64, bool)| {
            let x = 8. * c as f32;
            if skin.roots.insert(c, Transform::from_xyz(x, 0., 0.)) {
                skin.positions.insert(c, [x, 0., 0.]);
                skin.clock.observe_sample(0, c, c + 0.05, true);
            }
            if pose {
                skin.cameras.insert(c, Transform::from_xyz(x - 4., 1.5, 0.));
                if skin.poses.insert(c, vec![Transform::from_xyz(0., (7. * c).sin() as f32, 0.)]) {
                    skin.clock.observe_sample(1, c, c + 0.05, true);
                }
            }
        };
        let gap = |b: &Buffer<Transform>, t: f64| b.pair(t).map_or(f64::MAX, |(a, b2, _)| b.samples[b2].time - b.samples[a].time);
        let (mut delivered, mut cut, mut last): (usize, Option<(f64, u64)>, Option<f64>) = (0, None, None);
        for frame in 0..(13 * 60) {
            let now = frame as f64 / 60.;
            while sent.get(delivered).is_some_and(|s| s.0 + 0.05 <= now) {
                feed(&mut skin, &sent[delivered]);
                feed(&mut old, &sent[delivered]);
                delivered += 1;
            }
            if now >= 10. && cut.is_none() && hand_over(&mut skin, now - 10.) == Some(true) {
                cut = Some((now, skin.clock.underruns));
                let shown_old = old.clock.step(now).unwrap();
                assert!(gap(&old.roots, shown_old) > 0.1, "the old cut blends far-rate samples ({shown_old:.2} s)");
            } else {
                old.clock.step(now);
            }
            let time = skin.clock.step(now).unwrap_or(0.);
            let Some((at, underruns)) = cut else { continue };
            assert!(gap(&skin.roots, time) <= 0.1 && gap(&skin.cameras, time) <= 0.1, "{now:.3}: blends across a gap at {time:.3}");
            if let Some(last) = last {
                let step = time - last;
                assert!((0. ..=1.1 / 60.).contains(&step), "{now:.3}: the playout moved {step:.4} s in a frame");
            }
            assert_eq!(skin.clock.underruns, underruns, "{now:.3}: starved after the cut at {at:.3}");
            last = Some(time);
        }
        let (at, _) = cut.expect("the camera cut");
        assert!((10.3..10.5).contains(&at), "cut {:.3} s after the ask", at - 10.);
    }
}

#[cfg(test)]
mod online_owned_tests {
    use super::*;
    #[test]
    #[ignore = "requires SKATE3_ASSET_ROOT prepared owned assets"]
    fn online_appearance_owned_male_and_female_outfits_bind_every_clothing_rig() {
        use bevy::{
            asset::AssetPlugin, ecs::system::SystemState, gltf::GltfPlugin, image::ImagePlugin,
            mesh::MeshPlugin, scene::ScenePlugin,
        };
        let assets = std::path::PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap());
        let banks = skate_data::animation_banks::AnimationBanks::load(&assets).unwrap();
        let names = skate_data::animation_frames::AnimationFrames::from_banks(&banks)
            .unwrap()
            .bone_names;
        let library: crate::customiser_parts::Library = serde_json::from_slice(
            &std::fs::read(
                crate::customiser_parts::asset_directory(&assets).join("library-v3.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let parts = crate::customiser_parts::Parts::for_test(library);
        let mut app = App::new();
        crate::custom_models::register_source(&mut app);
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin {
                file_path: assets.to_string_lossy().into_owned(),
                ..default()
            },
            ImagePlugin::default(),
            MeshPlugin,
            ScenePlugin,
            GltfPlugin::default(),
        ));
        app.init_asset::<StandardMaterial>()
            .init_asset::<AnimationClip>()
            .register_type::<MeshMaterial3d<StandardMaterial>>();
        app.finish();
        app.cleanup();
        if let Ok(path) = std::env::var("SKATE_ONLINE_TEST_GLB") {
            let bytes = std::fs::read(path).unwrap();
            super::super::appearance::validate_glb(&bytes).unwrap();
            let cache = super::super::appearance::cache_directory();
            std::fs::create_dir_all(cache).unwrap();
            let mut payload = vec![0];
            payload.extend(bytes);
            let received = super::super::appearance_transfer::transfer_over_udp(payload);
            assert_eq!(received[0], 0);
            super::super::appearance::validate_glb(&received[1..]).unwrap();
            std::fs::write(cache.join("received-test.glb"), &received[1..]).unwrap();
            let handle = app
                .world()
                .resource::<AssetServer>()
                .load(GltfAssetLabel::Scene(0).from_asset("online-characters://received-test.glb"));
            let root = app
                .world_mut()
                .spawn((SceneRoot(handle), Transform::default(), Visibility::Hidden))
                .id();
            let started = Instant::now();
            loop {
                app.update();
                let w = app.world();
                if w.get::<SceneInstance>(root)
                    .is_some_and(|i| w.resource::<SceneSpawner>().instance_is_ready(**i))
                {
                    break;
                }
                assert!(
                    started.elapsed().as_secs() < 45,
                    "received online import failed to load"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            let mut queries: SystemState<(
                Query<(Entity, &SkinnedMesh)>,
                Query<(&Name, &Transform)>,
                Query<&ChildOf>,
            )> = SystemState::new(app.world_mut());
            let (meshes, nodes, parents) = queries.get(app.world());
            // The re-exported CAS heads and eyes carry the face joints (Task 5), so bind as the game does.
            let animation = crate::animation::AnimationStatus::for_scene(
                root, &crate::animation::with_face(&names), &meshes, &nodes, &parents,
            )
            .unwrap();
            let joints = animation.online_bindings();
            assert!(!joints.is_empty());
            // A received model must accept poses before display, then keep moving.
            let first: Vec<_> = (0..names.len())
                .map(|i| Mat4::from_translation(Vec3::new(i as f32 * 0.01, 1., 0.)))
                .collect();
            for (joint, pose) in animation.pose_transforms(&first) {
                assert!(pose.to_matrix().is_finite());
                *app.world_mut().get_mut::<Transform>(joint).unwrap() = pose;
            }
            *app.world_mut().get_mut::<Visibility>(root).unwrap() = Visibility::Inherited;
            let second: Vec<_> = first
                .iter()
                .enumerate()
                .map(|(i, m)| *m * Mat4::from_rotation_z(0.01 * (i + 1) as f32))
                .collect();
            for (joint, pose) in animation.pose_transforms(&second) {
                assert!(pose.to_matrix().is_finite());
                assert_ne!(*app.world().get::<Transform>(joint).unwrap(), pose);
                *app.world_mut().get_mut::<Transform>(joint).unwrap() = pose;
            }
            app.world_mut().entity_mut(root).despawn();
            app.update();
            std::fs::remove_file(cache.join("received-test.glb")).unwrap();
        }
        for gender in ["male", "female"] {
            let profile = parts.resolve(&parts.library.defaults[gender]).unwrap();
            let candidate = app
                .world_mut()
                .spawn((Transform::default(), Visibility::Hidden))
                .id();
            let paths: Vec<_> = profile["selections"]
                .as_object()
                .unwrap()
                .values()
                .map(|v| {
                    parts.library.models[v["asset_id"].as_str().unwrap()]
                        .scene
                        .clone()
                })
                .collect();
            assert!(paths.len() > 3);
            let mut scenes = vec![];
            for path in paths {
                let handle = app
                    .world()
                    .resource::<AssetServer>()
                    .load(GltfAssetLabel::Scene(0).from_asset(path));
                scenes.push(
                    app.world_mut()
                        .spawn((SceneRoot(handle), ChildOf(candidate)))
                        .id(),
                );
            }
            let started = Instant::now();
            loop {
                app.update();
                let world = app.world();
                if scenes.iter().all(|&e| {
                    world
                        .get::<SceneInstance>(e)
                        .is_some_and(|i| world.resource::<SceneSpawner>().instance_is_ready(**i))
                }) {
                    break;
                }
                assert!(
                    started.elapsed().as_secs() < 45,
                    "{gender} scene loading timed out"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            let mut queries: SystemState<(
                Query<(Entity, &SkinnedMesh)>,
                Query<(&Name, &Transform)>,
                Query<&ChildOf>,
            )> = SystemState::new(app.world_mut());
            let (meshes, nodes, parents) = queries.get(app.world());
            let binding = crate::animation::AnimationStatus::for_scene(
                candidate, &crate::animation::with_face(&names), &meshes, &nodes, &parents,
            )
            .unwrap()
            .online_bindings();
            let expected: std::collections::HashSet<_> = meshes
                .iter()
                .filter(|(e, _)| parents.iter_ancestors(*e).any(|e| e == candidate))
                .flat_map(|(_, m)| m.joints.iter().copied())
                .collect();
            assert_eq!(binding.len(), expected.len());
            for scene in scenes {
                assert!(
                    binding
                        .iter()
                        .any(|(e, _, _)| parents.iter_ancestors(*e).any(|e| e == scene)),
                    "all outfit scene rigs must animate"
                );
            }
            app.world_mut().entity_mut(candidate).despawn();
            app.update();
        }
    }
}

#[cfg(test)]
mod look_key_tests {
    use super::*;
    /// perf 10-18: `has_designs` (the key, from a borrow) agrees with whether stripping changes the outfit.
    #[test]
    fn has_designs_matches_what_stripping_changes() {
        for profile in [
            serde_json::json!({}),
            serde_json::json!({"graphics": {}}),
            serde_json::json!({"graphics": {"top": {"logo": 3}}}),
            serde_json::json!({"graphics": {"top": {"logo": 3, "design": "x"}}}),
            serde_json::json!({"graphics": {"top": {"design": "x"}, "deck": {"logo": 1}}}),
            serde_json::json!({"graphics": {"top": 5}}),
        ] {
            let mut stripped = profile.clone();
            crate::custom_graphic::without_designs(&mut stripped);
            assert_eq!(has_designs(&profile), stripped != profile, "{profile}");
        }
    }
}

#[cfg(test)]
mod respawn_tests {
    use super::*;
    /// Bug 10-25 #7 online: another player's shown piece whose scene Bevy spawns again (its glb loaded a second time,
    /// e.g. this player's Edit Skater loading an item they wear) has new joints and the glTF material: its look is
    /// built again rather than left white and hanging off the body.
    #[test]
    fn a_shown_piece_spawned_again_rebuilds_the_look() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default(), bevy::scene::ScenePlugin));
        app.register_type::<MeshMaterial3d<StandardMaterial>>().add_observer(respawned);
        let piece = || {
            let mut world = World::new();
            let joint = world.spawn((Name::new("HIPS"), Transform::default())).id();
            world.spawn((Mesh3d::default(), MeshMaterial3d::<StandardMaterial>::default(), SkinnedMesh { inverse_bindposes: default(), joints: vec![joint] }));
            Scene::new(world)
        };
        let world = app.world_mut();
        let (scene, next) = (world.resource_mut::<Assets<Scene>>().add(piece()), world.resource_mut::<Assets<Scene>>().add(piece()));
        let root = world.spawn((RemoteCharacter(7), Transform::default(), Visibility::Inherited)).id();
        // The shown look and a newer one still loading (spawn's candidate), as `bind` leaves them.
        let shown = world.spawn((Transform::default(), Visibility::Hidden, ChildOf(root))).id();
        let pending = world.spawn((Transform::default(), Visibility::Hidden, ChildOf(root))).id();
        world.spawn((SceneRoot(scene.clone()), ChildOf(shown)));
        world.spawn((SceneRoot(next.clone()), ChildOf(pending)));
        let skin = RemoteSkin { root: Some(root), requested: Some([3; 32]), changed: Some(Instant::now()), ..default() };
        world.insert_resource(RemoteSkins { reported: 0., actors: BTreeMap::from([(7, skin)]), rest: vec![], parents: vec![], board: None, trucks: [None; 2], wheel: None, left: false });
        // Past SceneSpawner's 3-frame debounce of a new scene's events; then `bind` shows the first look.
        for _ in 0..8 {
            app.update();
        }
        app.world_mut().resource_mut::<RemoteSkins>().actors.get_mut(&7).unwrap().visible = Some(shown);
        app.world_mut().resource_mut::<Assets<Scene>>().insert(next.id(), piece()).unwrap();
        for _ in 0..5 {
            app.update();
        }
        assert_eq!(app.world().resource::<RemoteSkins>().actors[&7].requested, Some([3; 32]), "a loading look spawned again is no rebuild");
        app.world_mut().resource_mut::<Assets<Scene>>().insert(scene.id(), piece()).unwrap();
        for _ in 0..5 {
            app.update();
        }
        let skin = &app.world().resource::<RemoteSkins>().actors[&7];
        assert_eq!((skin.requested, skin.changed), (None, None), "the shown look is built again, without waiting for the cooldown");
    }
}
