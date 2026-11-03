//! Render the same stock skeleton that animation and physics update.
//! The GLB supplies the mesh, skin weights and hierarchy; it does not play a
//! separate idle clip or own the skater's pose.
use crate::{app::FrameSet, assets::AssetStatus, physics::SkaterRuntime, world::PlayerRoot};
use bevy::{camera::visibility::NoFrustumCulling, mesh::skinning::SkinnedMesh, prelude::*};
use skate_core::animation::output::NativeMatrix;

#[derive(Resource, Default)]
pub(crate) struct AnimationStatus {
    pub ready: bool,
    bindings: Vec<BoneBinding>,
}
struct BoneBinding {
    entity: Entity,
    bone: usize,
    parent_bone: Option<usize>,
}
impl AnimationStatus {
    pub(crate) fn pose_transforms(&self, pose: &[Mat4]) -> Vec<(Entity, Transform)> {
        self.bindings
            .iter()
            .filter_map(|b| {
                let global = *pose.get(b.bone)? * render_basis();
                let local = if let Some(parent) = b.parent_bone {
                    (*pose.get(parent)? * render_basis()).inverse() * global
                } else {
                    global
                };
                // A joint the pose has no matrix for (NaN, see cas_pose::Poser::pose) keeps its transform.
                local.is_finite().then(|| (b.entity, Transform::from_matrix(local)))
            })
            .collect()
    }
    /// `root` and every bound bone under it.
    pub(crate) fn subtree(&self, root: usize) -> Vec<usize> {
        let parent = |bone: usize| self.bindings.iter().find(|b| b.bone == bone).and_then(|b| b.parent_bone);
        let under = |mut bone: usize| loop {
            if bone == root {
                break true;
            }
            match parent(bone) {
                Some(p) => bone = p,
                None => break false,
            }
        };
        self.bindings.iter().map(|b| b.bone).filter(|&b| under(b)).collect()
    }

    pub(crate) fn online_bindings(&self) -> Vec<(Entity, usize, Option<usize>)> {
        self.bindings
            .iter()
            .map(|b| (b.entity, b.bone, b.parent_bone))
            .collect()
    }

    /// Prepare a hidden imported scene without disturbing the live bindings.
    pub(crate) fn for_scene(
        root: Entity,
        names: &[String],
        skins: &Query<(Entity, &SkinnedMesh)>,
        nodes: &Query<(&Name, &Transform)>,
        parents: &Query<&ChildOf>,
    ) -> Result<Self, String> {
        let mut bindings = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (entity, skin) in skins.iter() {
            if !parents.iter_ancestors(entity).any(|p| p == root) {
                continue;
            }
            for &joint in &skin.joints {
                if !seen.insert(joint) {
                    continue;
                }
                if !parents.iter_ancestors(joint).any(|p| p == root) {
                    return Err("Imported skin refers to a joint outside its scene".into());
                }
                let (name, _) = nodes
                    .get(joint)
                    .map_err(|_| "Imported joint has no name/transform")?;
                let bone = names
                    .iter()
                    .position(|n| n.eq_ignore_ascii_case(name.as_str()))
                    .ok_or_else(|| format!("Unsupported imported bone: {name}"))?;
                let mut parent_bone = None;
                for p in parents.iter_ancestors(joint) {
                    if p == root {
                        break;
                    }
                    if let Ok((name, transform)) = nodes.get(p) {
                        if let Some(i) = names
                            .iter()
                            .position(|n| n.eq_ignore_ascii_case(name.as_str()))
                        {
                            parent_bone = Some(i);
                            break;
                        }
                        if !transform.to_matrix().abs_diff_eq(Mat4::IDENTITY, 0.00001) {
                            return Err(
                                "Imported armature has an unsupported ancestor transform".into()
                            );
                        }
                    }
                }
                bindings.push(BoneBinding {
                    entity: joint,
                    bone,
                    parent_bone,
                });
            }
        }
        if bindings.is_empty() {
            return Err("Imported scene has no skinned character".into());
        }
        Ok(Self {
            ready: true,
            bindings,
        })
    }
}
/// The head's face rig (Skate 2/3 Rostral: Face, the jaw chain, the 27 leaves on Face; faces.md section 2.1). Skate 2's
/// cutscene clips key them on the player; gameplay never poses them, so they hold the head's bind there.
pub(crate) const FACE_JOINTS: [&str; 33] = [
    "FACE", "OFFSET_JAW", "JAW", "OFFSET_CHIN", "CHIN", "TONGUE", "TONGUETIP",
    "LEFTEYE", "RIGHTEYE", "LEFTUPEYELID", "RIGHTUPEYELID", "LEFTLOWEYELID", "RIGHTLOWEYELID",
    "LEFTINNEREYEBROW", "RIGHTINNEREYEBROW", "LEFTOUTEREYEBROW", "RIGHTOUTEREYEBROW",
    "LEFTCHEEK", "RIGHTCHEEK", "LEFTUPCHEEK", "RIGHTUPCHEEK", "LEFTNOSE", "RIGHTNOSE",
    "LEFTCREASE", "RIGHTCREASE", "LEFTMOUTH", "RIGHTMOUTH",
    "UPPERLIP", "LOWERLIP", "LEFTUPPERLIP", "RIGHTUPPERLIP", "LEFTLOWERLIP", "RIGHTLOWERLIP",
];

/// `bones` then the face joints it lacks: a pose indexed by `bones` leaves the face at bind.
pub(crate) fn with_face(bones: &[String]) -> Vec<String> {
    let mut names = bones.to_vec();
    names.extend(FACE_JOINTS.iter().filter(|f| !bones.iter().any(|b| b.eq_ignore_ascii_case(f))).map(|f| f.to_string()));
    names
}

#[cfg(test)]
mod custom_model_binding_tests {
    use super::*;
    use bevy::ecs::system::SystemState;
    #[test]
    fn custom_models_bind_only_the_candidate_and_reject_foreign_joints() {
        let mut world = World::new();
        let root = world.spawn_empty().id();
        let hips = world
            .spawn((Name::new("HIPS"), Transform::default(), ChildOf(root)))
            .id();
        let head = world
            .spawn((Name::new("HEAD"), Transform::default(), ChildOf(hips)))
            .id();
        for _ in 0..2 {
            world.spawn((
                ChildOf(root),
                SkinnedMesh {
                    inverse_bindposes: default(),
                    joints: vec![hips, head],
                },
            ));
        }
        let foreign = world
            .spawn((Name::new("unrelated"), Transform::default()))
            .id();
        world.spawn(SkinnedMesh {
            inverse_bindposes: default(),
            joints: vec![foreign],
        });
        let names = vec!["HIPS".into(), "HEAD".into()];
        let mut queries: SystemState<(
            Query<(Entity, &SkinnedMesh)>,
            Query<(&Name, &Transform)>,
            Query<&ChildOf>,
        )> = SystemState::new(&mut world);
        let (skins, nodes, parents) = queries.get(&world);
        let prepared = AnimationStatus::for_scene(root, &names, &skins, &nodes, &parents).unwrap();
        assert!(prepared.ready);
        assert_eq!(prepared.bindings.len(), 2);
        assert_eq!(
            prepared
                .bindings
                .iter()
                .find(|b| b.entity == head)
                .unwrap()
                .parent_bone,
            Some(0)
        );
        world.spawn((
            ChildOf(root),
            SkinnedMesh {
                inverse_bindposes: default(),
                joints: vec![foreign],
            },
        ));
        let (skins, nodes, parents) = queries.get(&world);
        assert!(AnimationStatus::for_scene(root, &names, &skins, &nodes, &parents).is_err());
        assert_eq!(prepared.bindings.len(), 2);
    }
    #[test]
    fn a_head_with_face_joints_binds_and_holds_bind_in_gameplay() {
        // Review Focus 3: the remote player binds the same head part; an unknown face joint must not fail the bind.
        let mut world = World::new();
        let root = world.spawn_empty().id();
        let head = world.spawn((Name::new("HEAD"), Transform::default(), ChildOf(root))).id();
        let face = world.spawn((Name::new("Face"), Transform::default(), ChildOf(head))).id();
        let eye = world.spawn((Name::new("LeftEye"), Transform::default(), ChildOf(face))).id();
        world.spawn((ChildOf(root), SkinnedMesh { inverse_bindposes: default(), joints: vec![head, face, eye] }));
        let skater = vec!["HEAD".to_string()];
        let mut queries: SystemState<(Query<(Entity, &SkinnedMesh)>, Query<(&Name, &Transform)>, Query<&ChildOf>)> =
            SystemState::new(&mut world);
        let (skins, nodes, parents) = queries.get(&world);
        assert!(AnimationStatus::for_scene(root, &skater, &skins, &nodes, &parents).is_err(), "unsupported bone without the face");
        let binding = AnimationStatus::for_scene(root, &with_face(&skater), &skins, &nodes, &parents).unwrap();
        assert_eq!(binding.bindings.len(), 3);
        // A gameplay pose has only the skater's bones: the face joints are left alone (bind).
        let gameplay = vec![Mat4::IDENTITY; skater.len()];
        assert_eq!(binding.pose_transforms(&gameplay).len(), skater.len());
    }
}
pub(crate) struct AnimationPlugin;
impl Plugin for AnimationPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AnimationStatus>()
            .add_systems(
                Update,
                (disable_skater_culling, bind, present)
                    .chain()
                    .in_set(FrameSet::Animation),
            )
            .add_systems(
                Update,
                (board_size, longboard_shape)
                    .after(present)
                    .after(crate::cas_pose::apply)
                    .in_set(FrameSet::Animation),
            );
    }
}
fn disable_skater_culling(
    mut commands: Commands,
    skins: Query<Entity, (With<SkinnedMesh>, Without<NoFrustumCulling>)>,
    parents: Query<&ChildOf>,
    roots: Query<Entity, With<PlayerRoot>>,
) {
    // GLB primitive bounds describe the bind pose, not the animated skin.
    // Apply to every skater mesh (not just the first skin used to bind bones).
    // World meshes retain their normal culling behavior.
    for entity in &skins {
        if parents.iter_ancestors(entity).any(|e| roots.contains(e)) {
            commands.entity(entity).insert(NoFrustumCulling);
        }
    }
}

fn bind(
    status: Res<AssetStatus>,
    skater: Res<SkaterRuntime>,
    mut animation: ResMut<AnimationStatus>,
    skins: Query<(Entity, &SkinnedMesh)>,
    nodes: Query<(&Name, &Transform)>,
    parents: Query<&ChildOf>,
    visibility: Query<&Visibility>,
    roots: Query<Entity, With<PlayerRoot>>,
    mut exit: MessageWriter<AppExit>,
) {
    if *status != AssetStatus::Ready || animation.ready {
        return;
    }
    // A head with a face rig (Skate 2 cutscenes pose it) binds its face joints; gameplay never poses them.
    let names = &with_face(&skater.animation.evaluator.frames.bone_names);
    let result = (|| -> Result<Option<Vec<BoneBinding>>, String> {
        let mut bindings = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (entity, skin) in &skins {
            if !parents.iter_ancestors(entity).any(|e| roots.contains(e)) {
                continue;
            }
            if parents
                .iter_ancestors(entity)
                .any(|e| visibility.get(e).is_ok_and(|v| *v == Visibility::Hidden))
            {
                continue;
            }
            let mut binding = Vec::with_capacity(skin.joints.len());
            for &joint in &skin.joints {
                if !seen.insert(joint) {
                    continue;
                }
                let (name, _) = nodes
                    .get(joint)
                    .map_err(|_| "Skater skin joint is missing its name or transform")?;
                let bone = names
                    .iter()
                    .position(|n| n.eq_ignore_ascii_case(name.as_str()))
                    .ok_or_else(|| {
                        format!("Skater skin bone {name} is absent from the stock rig")
                    })?;
                let mut parent_bone = None;
                for parent in parents.iter_ancestors(joint) {
                    if roots.contains(parent) {
                        break;
                    }
                    if let Ok((name, transform)) = nodes.get(parent) {
                        if let Some(index) = names
                            .iter()
                            .position(|n| n.eq_ignore_ascii_case(name.as_str()))
                        {
                            parent_bone = Some(index);
                            break;
                        }
                        // The supplied GLB's armature node is identity. Reject
                        // another import convention rather than double a pose.
                        if !transform.to_matrix().abs_diff_eq(Mat4::IDENTITY, 0.00001) {
                            return Err(format!(
                                "Skater armature ancestor {name} has an unsupported transform"
                            ));
                        }
                    }
                }
                binding.push(BoneBinding {
                    entity: joint,
                    bone,
                    parent_bone,
                });
            }
            // Bind each visible modular rig, retaining its authored inverse binds.
            bindings.extend(binding);
        }
        Ok(if bindings.is_empty() {
            None
        } else {
            Some(bindings)
        })
    })();
    match result {
        Ok(Some(binding)) => {
            animation.bindings = binding;
            animation.ready = true;
            info!(
                "GAME_CHARACTER_READY bones={} source=physical_stock_pose",
                animation.bindings.len()
            );
        }
        Ok(None) => (),
        Err(message) => {
            error!("{message}");
            exit.write(AppExit::error());
        }
    }
}
pub(crate) fn present(
    history: Res<crate::presentation::Presentation>,
    skater: Res<SkaterRuntime>,
    replay: Res<crate::replay::Replay>,
    time: Res<Time<Fixed>>,
    animation: Res<AnimationStatus>,
    mut nodes: Query<&mut Transform>,
    mut kept: Local<Vec<(Vec<Mat4>, Vec<Option<Transform>>)>>,
) {
    if !animation.ready {
        return;
    }
    // perf 10-18: an unchanged bone keeps its Transform unchanged (no transform propagation or skin upload for a
    // paused or still skater), and each snapshot's bone-local transforms are worked out once, not every frame.
    // SKATE_BONE_CACHE=0: both as before.
    static CACHE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let cache = *CACHE.get_or_init(|| std::env::var_os("SKATE_BONE_CACHE").is_none_or(|v| v != "0"));
    let mut write = |entity: Entity, transform: Transform| {
        if let Ok(mut node) = nodes.get_mut(entity) {
            if cache {
                node.set_if_neq(transform);
            } else {
                *node = transform;
            }
        }
    };
    // Existing GLB was exported through Blender: its bone-local axes are
    // rotated -90 degrees about X relative to the native frames. Both files'
    // world positions are Y-up. This is a skin basis change, not a physics turn.
    let basis = render_basis();
    let Some((previous, current, alpha)) = history.view(&replay, time.overstep_fraction()) else {
        let pose: Vec<_> = skater
            .render_pose
            .iter()
            .copied()
            .map(native_matrix)
            .collect();
        for (entity, transform) in animation.pose_transforms(&pose) {
            write(entity, transform);
        }
        return;
    };
    // Blend bone-local rotations, not matrix entries or independent world
    // positions: joints retain their hierarchy while limbs turn.
    // A replay frame with fewer bones than the rig (a foreign save, review I4c) leaves that bone as it was.
    let locals = |bones: &[Mat4]| -> Vec<Option<Transform>> {
        animation
            .bindings
            .iter()
            .map(|binding| {
                let global = *bones.get(binding.bone)? * basis;
                Some(Transform::from_matrix(if let Some(parent) = binding.parent_bone {
                    (*bones.get(parent)? * basis).inverse() * global
                } else {
                    global
                }))
            })
            .collect()
    };
    if !cache || animation.is_changed() {
        kept.clear();
    }
    for bones in [&previous.bones, &current.bones] {
        if !kept.iter().any(|(b, _)| same_bones(b, bones)) {
            kept.push((bones.clone(), locals(bones)));
        }
    }
    kept.retain(|(b, _)| same_bones(b, &previous.bones) || same_bones(b, &current.bones));
    let find = |bones: &[Mat4]| &kept.iter().find(|(b, _)| same_bones(b, bones)).unwrap().1;
    let (from, to) = (find(&previous.bones), find(&current.bones));
    for ((binding, from), to) in animation.bindings.iter().zip(from).zip(to) {
        let (Some(from), Some(to)) = (from, to) else { continue };
        write(binding.entity, crate::presentation::blend(*from, *to, alpha));
    }
}
/// Bit for bit: equal bones give equal bone-local transforms.
fn same_bones(a: &[Mat4], b: &[Mat4]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.to_cols_array().map(f32::to_bits) == b.to_cols_array().map(f32::to_bits))
}
/// Not retail (user 10-13, feature #1): Edit Skater's Board size, (name, length, width) against the stock deck
/// (31.5" x 8"): a 22" x 6" penny, a 10" wide deck, a 40" x 9.5" longboard. The trucks move out with the length.
/// ponytail: the look only. Physics keeps the stock deck and wheelbase (drive_frames / AuthoredTransformInputs::STOCK
/// is shared by every board system), so a longboard rides and flips as a standard deck does. Other players see the
/// size too (bug 10-17 #27): it rides in their outfit's profile, and multiplayer/render.rs shapes their board with
/// [`BoardShape`] as [`board_size`] shapes this player's.
pub(crate) const BOARD_SIZES: [(&str, f32, f32); 4] =
    [("Penny", 0.7, 0.75), ("Standard", 1., 1.), ("Wide", 1., 1.25), ("Longboard", 1.27, 1.19)];

/// A profile's Board size, an index into [`BOARD_SIZES`]; Standard (1) when it has none (Stock, Native and Imported
/// looks, older builds).
pub(crate) fn board_size_index(profile: &serde_json::Value) -> usize {
    crate::customiser::scalar_of(profile, "board_size", 1.).round().clamp(0., 3.) as usize
}

/// A board's size on one skater's joints, shared by this player's ([`board_size`]) and other players' boards
/// (multiplayer/render.rs): the deck part's SKATEBOARD_ROOT is scaled by `scale` and moved by `hold`, and the truck
/// joints of the truck and wheel parts move out along the length ([`BoardShape::truck`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BoardShape {
    pub scale: Vec3,
    /// In the deck joint's parent frame (apply as `rotation * hold`).
    pub hold: Vec3,
    long: Vec3,
    length: f32,
}
impl BoardShape {
    /// From the skater's TRUCK_FRONT joint and LEFT_WHEELFRONT's translation; None for Standard. `held`: Edit Skater's
    /// CAS_STAND holds the deck upright by its front end.
    pub(crate) fn new(size: usize, truck: &Transform, wheel: Vec3, held: bool) -> Option<Self> {
        let (_, length, width) = BOARD_SIZES[size.min(3)];
        if size == 1 {
            return None;
        }
        // The skin's own axes: the trucks sit out along the length, the wheels out along the width.
        let axis = |v: Vec3| {
            let a = v.abs();
            if a.x >= a.y && a.x >= a.z { Vec3::X } else if a.y >= a.z { Vec3::Y } else { Vec3::Z }
        };
        // The wheel sits in the truck's frame (its width is the truck's Y): turned into the deck's first. Taken raw, the
        // width scale went to the deck's thickness, so Wide looked like Standard (bug 10-17 #25).
        let (long, wide) = (axis(truck.translation), axis(truck.rotation * wheel));
        // A deck sized about its centre left the held end short of the hand (the penny floated below it, bug 10-17
        // #26), so held it moves along its length to keep that end in place. Riding, it stays centred over its trucks.
        let hold = if held { truck.translation.dot(long).signum() * long * DECK_HALF_LENGTH * (1. - length) } else { Vec3::ZERO };
        Some(Self { scale: Vec3::ONE + long * (length - 1.) + wide * (width - 1.), hold, long, length })
    }
    /// A truck joint of the truck and wheel parts (posed this frame), moved out with the length. The deck part's own
    /// trucks follow its scaled root instead.
    pub(crate) fn truck(&self, t: &mut Transform) {
        let along = t.translation.dot(self.long);
        t.translation += self.long * along * (self.length - 1.);
    }
}

/// Another player's (or a Party Play puppet's) Board size, on its appearance root: [`longboard_shape`] tapers its deck.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub(crate) struct BoardSizeOf(pub usize);

/// The local skater's board at its size, after every pose writer ([`BoardShape`]).
fn board_size(
    animation: Res<AnimationStatus>,
    skater: Res<SkaterRuntime>,
    customiser: Option<Res<crate::customiser::Customiser>>,
    parts: Option<Res<crate::customiser_parts::Parts>>,
    roots: Query<&crate::customiser_parts::PartRoot>,
    parents: Query<&ChildOf>,
    mut nodes: Query<&mut Transform>,
) {
    let (Some(customiser), Some(parts)) = (customiser, parts) else { return };
    let size = board_size_index(&customiser.draft);
    if !animation.ready || size == 1 {
        return;
    }
    let names = &skater.animation.evaluator.frames.bone_names;
    let name = |b: &BoneBinding| names.get(b.bone).map(String::as_str);
    let joint = |n: &str| {
        let binding = animation.bindings.iter().find(|b| name(b) == Some(n))?;
        nodes.get(binding.entity).ok().copied()
    };
    let (Some(truck), Some(wheel)) = (joint("TRUCK_FRONT"), joint("LEFT_WHEELFRONT")) else { return };
    // Edit Skater's CAS_STAND holds the deck by its front end (the hand past TRUCK_FRONT).
    let Some(shape) = BoardShape::new(size, &truck, wheel.translation, customiser.open) else { return };
    for b in &animation.bindings {
        let deck = match name(b) {
            Some("SKATEBOARD_ROOT") => true,
            Some("TRUCK_FRONT" | "TRUCK_BACK") => false,
            _ => continue,
        };
        let part = parents.iter_ancestors(b.entity).find_map(|e| roots.get(e).ok());
        let on_deck = part.and_then(|p| parts.library.models.get(&p.0)).is_some_and(|m| m.slot == "SkateBoard");
        let Ok(mut t) = nodes.get_mut(b.entity) else { continue };
        if deck {
            // The truck and wheel parts' root slides with the deck when Edit Skater holds it by its end (their trucks
            // hung off the penny there); only the deck part's is scaled.
            let shift = t.rotation * shape.hold;
            t.translation += shift;
            if on_deck {
                t.scale *= shape.scale;
            }
        } else if !on_deck {
            shape.truck(&mut t);
        }
    }
}

/// The stock deck mesh's half-length along its joint's long axis (f2c517ee8b8359da_v4.glb: z -0.452..0.450).
/// ponytail: one value for every deck; a deck much longer or shorter than stock keeps its held end within a few cm.
const DECK_HALF_LENGTH: f32 = 0.45;
/// How much narrower the longboard's nose and tail are than its middle (feature #1, bug 10-17 #24). Not retail (no
/// longboard in Skate 3). ponytail: a look picked here, not measured; raise it for a sharper pintail.
const LONGBOARD_TAPER: f32 = 0.45;

/// The longboard's pintail (bug 10-17 #24: a stretched deck didn't look like a longboard): a deck part's mesh is
/// swapped for a copy whose width narrows toward both ends, `1 - LONGBOARD_TAPER * t^2` at `t` = |along| / half-length,
/// in the mesh's own axes (longest extent the length, the next the width); any other size gets the original back.
/// Each deck follows its own skater (bug 10-17 #27): under the PlayerRoot the local draft, under another player's or a
/// puppet's appearance root its [`BoardSizeOf`]; any other deck is left alone.
#[allow(clippy::too_many_arguments)]
fn longboard_shape(
    customiser: Option<Res<crate::customiser::Customiser>>,
    parts: Option<Res<crate::customiser_parts::Parts>>,
    roots: Query<(Entity, &crate::customiser_parts::PartRoot)>,
    owners: Query<(Option<&BoardSizeOf>, Has<PlayerRoot>)>,
    parents: Query<&ChildOf>,
    children: Query<&Children>,
    mut decks: Query<(&mut Mesh3d, Option<&Untapered>)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut tapered: Local<std::collections::HashMap<AssetId<Mesh>, Handle<Mesh>>>,
    mut commands: Commands,
) {
    let Some(parts) = parts else { return };
    let local = customiser.is_some_and(|c| board_size_index(&c.draft) == 3);
    for (root, part) in &roots {
        if parts.library.models.get(&part.0).is_none_or(|m| m.slot != "SkateBoard") {
            continue;
        }
        let owner = parents.iter_ancestors(root).find_map(|e| match owners.get(e) {
            Ok((Some(size), _)) => Some(size.0 == 3),
            Ok((None, true)) => Some(local),
            _ => None,
        });
        let Some(longboard) = owner else { continue };
        for entity in children.iter_descendants(root) {
            let Ok((mut mesh, original)) = decks.get_mut(entity) else { continue };
            match (longboard, original) {
                (true, None) => {
                    let source = mesh.0.clone();
                    let shaped = match tapered.get(&source.id()) {
                        Some(h) => h.clone(),
                        None => {
                            let Some(copy) = meshes.get(&source).and_then(taper) else { continue };
                            let h = meshes.add(copy);
                            tapered.insert(source.id(), h.clone());
                            h
                        }
                    };
                    mesh.0 = shaped;
                    commands.entity(entity).insert(Untapered(source));
                }
                (false, Some(original)) => {
                    mesh.0 = original.0.clone();
                    commands.entity(entity).remove::<Untapered>();
                }
                _ => {}
            }
        }
    }
}

/// A deck mesh whose longboard copy is showing: the original to put back.
#[derive(Component)]
struct Untapered(Handle<Mesh>);

/// `mesh` narrowed toward both ends ([`longboard_shape`]); None without float3 positions.
fn taper(mesh: &Mesh) -> Option<Mesh> {
    use bevy::mesh::VertexAttributeValues;
    let mut out = mesh.clone();
    let Some(VertexAttributeValues::Float32x3(positions)) = out.attribute_mut(Mesh::ATTRIBUTE_POSITION) else { return None };
    let (lo, hi) = positions.iter().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), p| (lo.min(Vec3::from(*p)), hi.max(Vec3::from(*p))));
    let size = hi - lo;
    let mut order = [0, 1, 2];
    order.sort_by(|&a, &b| size[b].total_cmp(&size[a]));
    let (long, wide) = (order[0], order[1]);
    let (mid, half) = ((lo[long] + hi[long]) / 2., size[long] / 2.);
    let centre = (lo[wide] + hi[wide]) / 2.;
    if half <= 0. {
        return None;
    }
    for p in positions.iter_mut() {
        let t = ((p[long] - mid) / half).abs().min(1.);
        p[wide] = centre + (p[wide] - centre) * (1. - LONGBOARD_TAPER * t * t);
    }
    Some(out)
}

fn render_basis() -> Mat4 {
    Mat4::from_cols(Vec4::X, -Vec4::Z, Vec4::Y, Vec4::W)
}
pub(crate) fn native_matrix(matrix: NativeMatrix) -> Mat4 {
    Mat4::from_cols(
        Vec3::from_array(matrix[0][..3].try_into().unwrap()).extend(0.0),
        Vec3::from_array(matrix[1][..3].try_into().unwrap()).extend(0.0),
        Vec3::from_array(matrix[2][..3].try_into().unwrap()).extend(0.0),
        Vec3::from_array(matrix[3][..3].try_into().unwrap()).extend(1.0),
    )
}

/// Vehicle clips use the same native model-space skeleton matrices as the SDK.
///
/// `layers` are applied in order, each easing the result towards one target pose
/// by its own weight. Steering is one layer and an air trick is another, which
/// is what lets a trick be a single authored frame rather than an animation: the
/// weight is how far the rider has thrown it, so a half-extended trick reads as
/// half-extended instead of snapping between two poses.
pub(crate) fn vehicle_pose(world: &mut World, pose: &[Mat4], layers: &[(&[Mat4], f32)]) {
    let board = world
        .resource::<crate::physics::SkaterRuntime>()
        .animation
        .evaluator
        .frames
        .bone_names
        .iter()
        .position(|n| n == "SKATEBOARD_ROOT");
    world.resource_scope(|world, animation: Mut<AnimationStatus>| {
        let basis = render_basis();
        for binding in &animation.bindings {
            let Some(global) = pose.get(binding.bone).map(|m| *m * basis) else {
                continue;
            };
            let local = if let Some(parent) = binding.parent_bone {
                let Some(parent) = pose.get(parent) else {
                    continue;
                };
                (*parent * basis).inverse() * global
            } else {
                global
            };
            let mut target = Transform::from_matrix(local);
            for (layer, weight) in layers {
                let weight = weight.clamp(0., 1.);
                if weight <= 0. {
                    continue;
                }
                if let Some(global) = layer.get(binding.bone).map(|m| *m * basis) {
                    let local = if let Some(parent) = binding.parent_bone {
                        layer
                            .get(parent)
                            .map(|p| (*p * basis).inverse() * global)
                            .unwrap_or(global)
                    } else {
                        global
                    };
                    target =
                        crate::presentation::blend(target, Transform::from_matrix(local), weight);
                }
            }
            if let Some(mut transform) = world.get_mut::<Transform>(binding.entity) {
                *transform = target;
                if Some(binding.bone) == board {
                    transform.scale = Vec3::splat(0.001);
                }
            }
        }
    });
}

pub(crate) fn capture_vehicle_visual(world: &mut World) -> Vec<(Entity, Transform)> {
    let mut ids: Vec<_> = world
        .resource::<AnimationStatus>()
        .bindings
        .iter()
        .map(|b| b.entity)
        .collect();
    ids.extend(
        world
            .query_filtered::<Entity, With<crate::world::PlayerRoot>>()
            .iter(world),
    );
    ids.into_iter()
        .filter_map(|id| world.get::<Transform>(id).copied().map(|t| (id, t)))
        .collect()
}
pub(crate) fn blend_vehicle_visual(world: &mut World, from: &[(Entity, Transform)], alpha: f32) {
    for &(id, previous) in from {
        if let Some(mut current) = world.get_mut::<Transform>(id) {
            *current = crate::presentation::blend(previous, *current, alpha);
        }
    }
}

/// Read the final displayed hierarchy, including vehicle steering and stance blends.
pub(crate) fn network_visual(world: &mut World) -> skate_net::packed::PoseState {
    let root = world
        .query_filtered::<&Transform, With<PlayerRoot>>()
        .iter(world)
        .next()
        .copied()
        .unwrap_or_default();
    let status = world.resource::<AnimationStatus>();
    let mut locals = std::collections::BTreeMap::new();
    for b in &status.bindings {
        if let Some(t) = world.get::<Transform>(b.entity) {
            locals
                .entry(b.bone)
                .or_insert((b.parent_bone, t.to_matrix()));
        }
    }
    fn global(
        i: usize,
        locals: &std::collections::BTreeMap<usize, (Option<usize>, Mat4)>,
        depth: usize,
    ) -> Mat4 {
        if depth > 128 {
            return Mat4::IDENTITY;
        }
        let Some(&(parent, m)) = locals.get(&i) else {
            return Mat4::IDENTITY;
        };
        parent.map_or(m, |p| global(p, locals, depth + 1) * m)
    }
    let anchors = crate::physics::network::anchors(world.resource::<SkaterRuntime>());
    let basis = render_basis().inverse();
    skate_net::packed::PoseState {
        root: crate::physics::network::pose(root.to_matrix()),
        bones: anchors
            .into_iter()
            .map(|i| skate_net::Bone {
                index: i as u16,
                pose: crate::physics::network::pose(if locals.contains_key(&i) {
                    global(i, &locals, 0) * basis
                } else {
                    native_matrix(world.resource::<SkaterRuntime>().render_pose[i])
                }),
            })
            .collect(),
    }
}
#[cfg(test)]
mod board_shape_tests {
    use super::*;
    use bevy::{asset::RenderAssetUsages, mesh::PrimitiveTopology};

    /// Bug 10-17 #24: the longboard narrows toward nose and tail (the deck's length on z, width on x, as stock).
    #[test]
    fn the_longboard_narrows_toward_both_ends() {
        let deck = |z: f32| [[-0.11, 0.1, z], [0.11, 0.1, z]];
        let positions: Vec<[f32; 3]> = [-0.45, 0.0, 0.45].into_iter().flat_map(deck).collect();
        let mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        let shaped = taper(&mesh).unwrap();
        let Some(bevy::mesh::VertexAttributeValues::Float32x3(p)) = shaped.attribute(Mesh::ATTRIBUTE_POSITION) else { panic!() };
        let width = |i: usize| p[2 * i + 1][0] - p[2 * i][0];
        assert!((width(1) - 0.22).abs() < 1e-6, "the middle keeps its width");
        assert!((width(0) - 0.22 * (1. - LONGBOARD_TAPER)).abs() < 1e-6 && (width(2) - width(0)).abs() < 1e-6, "both ends narrow");
        assert!(p.iter().zip(mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap().as_float3().unwrap()).all(|(a, b)| a[1] == b[1] && a[2] == b[2]));
    }
}

#[cfg(test)]
mod board_size_tests {
    use super::*;
    use bevy::{asset::RenderAssetUsages, ecs::system::RunSystemOnce, mesh::PrimitiveTopology};

    /// Bug 10-17 #27: the size math both boards share. Wide widens across the wheels (the truck's y turned onto the
    /// deck's x), Longboard lengthens along the trucks, Standard changes nothing.
    #[test]
    fn board_shapes_follow_the_skins_own_axes() {
        let truck = Transform::from_xyz(0., -0.05, 0.3).with_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2));
        let wheel = Vec3::new(0., 0.1, 0.);
        assert_eq!(BoardShape::new(1, &truck, wheel, false), None);
        assert!(BoardShape::new(2, &truck, wheel, false).unwrap().scale.abs_diff_eq(Vec3::new(1.25, 1., 1.), 1e-6));
        let long = BoardShape::new(3, &truck, wheel, false).unwrap();
        assert!(long.scale.abs_diff_eq(Vec3::new(1.19, 1., 1.27), 1e-6) && long.hold == Vec3::ZERO, "{long:?}");
        let mut t = Transform::from_xyz(0., -0.05, -0.3);
        long.truck(&mut t);
        assert!((t.translation.z + 0.3 * 1.27).abs() < 1e-6 && t.translation.y == -0.05);
        assert_ne!(BoardShape::new(0, &truck, wheel, true).unwrap().hold, Vec3::ZERO, "held in Edit Skater");
        assert_eq!(board_size_index(&serde_json::json!({"board_size": 3.0})), 3);
        assert_eq!(board_size_index(&serde_json::json!({})), 1, "no size: Standard");
    }

    /// Bug 10-17 #27: picking Longboard tapers this player's deck only; another player's deck follows its own size.
    #[test]
    fn the_local_longboard_leaves_other_decks_alone() {
        let mut world = World::new();
        let mut parts = crate::customiser_parts::Parts::default();
        let deck = crate::customiser_parts::Part { slot: "SkateBoard".into(), name: "deck".into(), flags: default(), materials: vec![], scene: String::new() };
        parts.library.models.insert("deck".into(), deck);
        world.insert_resource(parts);
        world.insert_resource(crate::customiser::Customiser::for_draft(serde_json::json!({"board_size": 3})));
        let mut meshes = Assets::<Mesh>::default();
        let positions: Vec<[f32; 3]> = vec![[-0.1, 0., -0.45], [0.1, 0., -0.45], [-0.1, 0., 0.45], [0.1, 0., 0.45]];
        let mesh = meshes.add(Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default()).with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions));
        world.insert_resource(meshes);
        let mut skater = |owner: Option<usize>| {
            let root = match owner {
                Some(size) => world.spawn(BoardSizeOf(size)).id(),
                None => world.spawn(PlayerRoot).id(),
            };
            let part = world.spawn((crate::customiser_parts::PartRoot("deck".into()), ChildOf(root))).id();
            world.spawn((Mesh3d(mesh.clone()), ChildOf(part))).id()
        };
        let (local, stock, other_long) = (skater(None), skater(Some(1)), skater(Some(3)));
        world.run_system_once(longboard_shape).unwrap();
        world.flush();
        let tapered = |w: &World, e: Entity| w.get::<Untapered>(e).is_some();
        assert!(tapered(&world, local), "this player's Longboard");
        assert!(!tapered(&world, stock), "another player's Standard deck is left alone");
        assert!(tapered(&world, other_long), "another player's own Longboard");
    }
}

#[cfg(test)]
mod online_swap_tests {
    use super::*;
    use bevy::{ecs::system::SystemState, mesh::skinning::SkinnedMesh};
    #[test]
    fn online_appearance_swap_seeds_new_rig_and_keeps_animating() {
        let mut world = World::new();
        let mut roots = vec![];
        for _ in 0..2 {
            let root = world.spawn_empty().id();
            let hip = world
                .spawn((Name::new("HIPS"), Transform::default(), ChildOf(root)))
                .id();
            let head = world
                .spawn((Name::new("HEAD"), Transform::default(), ChildOf(hip)))
                .id();
            world.spawn((
                ChildOf(root),
                SkinnedMesh {
                    inverse_bindposes: default(),
                    joints: vec![hip, head],
                },
            ));
            roots.push((root, hip, head));
        }
        let names = vec!["HIPS".to_owned(), "HEAD".to_owned()];
        let bind = |world: &mut World, root| {
            let mut query: SystemState<(
                Query<(Entity, &SkinnedMesh)>,
                Query<(&Name, &Transform)>,
                Query<&ChildOf>,
            )> = SystemState::new(world);
            let (skins, nodes, parents) = query.get(world);
            AnimationStatus::for_scene(root, &names, &skins, &nodes, &parents).unwrap()
        };
        let old = bind(&mut world, roots[0].0);
        let pose = [
            Mat4::from_translation(Vec3::new(1., 2., 3.)),
            Mat4::from_rotation_z(0.7),
        ];
        for (entity, t) in old.pose_transforms(&pose) {
            *world.get_mut::<Transform>(entity).unwrap() = t;
        }
        let replacement = bind(&mut world, roots[1].0);
        assert_eq!(
            *world.get::<Transform>(roots[1].1).unwrap(),
            Transform::default()
        );
        for (entity, t) in replacement.pose_transforms(&pose) {
            *world.get_mut::<Transform>(entity).unwrap() = t;
        }
        assert_eq!(
            world.get::<Transform>(roots[0].1),
            world.get::<Transform>(roots[1].1)
        );
        assert_eq!(
            world.get::<Transform>(roots[0].2),
            world.get::<Transform>(roots[1].2)
        );
        let next = [
            Mat4::from_translation(Vec3::new(2., 3., 4.)),
            Mat4::from_rotation_z(1.2),
        ];
        for (entity, t) in replacement.pose_transforms(&next) {
            *world.get_mut::<Transform>(entity).unwrap() = t;
        }
        assert_ne!(
            world.get::<Transform>(roots[0].1),
            world.get::<Transform>(roots[1].1)
        );
        assert_ne!(
            world.get::<Transform>(roots[0].2),
            world.get::<Transform>(roots[1].2)
        );
    }

    #[test]
    fn pose_transforms_keep_a_keyed_scale() {
        let local = Mat4::from_scale_rotation_translation(Vec3::splat(0.58), Quat::IDENTITY, Vec3::X);
        let t = Transform::from_matrix(render_basis().inverse() * local * render_basis());
        assert!((t.scale - Vec3::splat(0.58)).abs().max_element() < 1e-5);
    }

    #[test]
    fn same_bones_is_bit_for_bit() {
        let a = vec![Mat4::from_translation(Vec3::X), Mat4::IDENTITY];
        assert!(same_bones(&a, &a.clone()));
        assert!(!same_bones(&a, &a[..1]));
        let mut b = a.clone();
        b[1].w_axis.x = -0.0;
        assert!(!same_bones(&a, &b), "-0.0 is another bit pattern, so another (equal) snapshot, never a stale one");
        b[1].w_axis.x = 1e-7;
        assert!(!same_bones(&a, &b));
    }
}
