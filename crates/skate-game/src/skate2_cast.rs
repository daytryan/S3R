//! The Skate 2 tutorial's characters: Slappy, Reda and the arriving pros, built from Skate 2's own marquee
//! models by skate2port/old_tutorial_cast.py (`private/skate2-tutorial/cast/<key>.glb`, with the face, finger and
//! twist joints the cutscenes pose). The tutorial binding
//! places them at locators; they stand in Skate 3's Create-a-Skater idle (CAS_STAND, board in hand).
use crate::{animation::AnimationStatus, app::FrameSet, cas_pose::Poser, config::Config, physics::SkaterRuntime};
use bevy::{
    camera::visibility::{NoFrustumCulling, RenderLayers},
    mesh::skinning::SkinnedMesh,
    prelude::*,
    scene::SceneInstance,
};
use skate_core::animation::{output::NativeMatrix, playback_tree::PoseCommand};
use std::collections::BTreeMap;

const STAND: &str = "CAS_STAND";
/// Skate 3's pedestrian bank: the marquee graph's IdleBasicCyc clips (round2-research2 Q1).
const PEDESTRIAN_BANK: &str = "private/stock/data/anim/PedestrianSkeletonPres.abin";
/// Its reference pose (named as its hierarchy): the bank's clips are deltas on it (user 10-28 bug 1).
const PEDESTRIAN_REFERENCE: &str = "PEDESTRIAN_RIG_TPOSE";
const HAND_PROP: &str = "LEFTHANDPROP";
const RIGHT_HAND_PROP: &str = "RIGHTHANDPROP";
/// marquee_board's IdleBasicCyc. Its IDLE1..3 pick is UNKNOWN: variant 1.
const BOARD_IDLE: &str = "IP_WNDR_STNDWBOARD_IDLE1_0_CYC";

/// A member's idle (remap class 53E332F6 IdleBasicCyc by its marquee row) and whether it carries the board.
/// `None`: not a marquee character (CAS_STAND). ponytail: no GreetChannel gesture (every 16 s facing within 6 m).
fn idle_clip(key: &str) -> Option<(&'static str, bool)> {
    match key.split('#').next().unwrap_or(key) {
        "slappy" | "eric_koston" | "mike_carroll" | "rob_dyrdek" => Some((BOARD_IDLE, true)),
        // Reda's marquee row lists IDLE1..6; the pick is UNKNOWN: variant 1.
        "giovanni_reda" => Some(("IP_WNDR_STND_IDLE1_0_CYC", false)),
        "attiba_jefferson" => Some(("PHOT_STAND_IDLE_N_0_CYC", false)),
        _ => None,
    }
}

/// Skate 2's `livingworld_models` `Hide_Skateboard` (vault 0x0626FA10792BBA58, set on these two rows only): a pedestrian
/// with it leaves its SkateBoard component out (s2.bin 0x8267E9C8..EA1C). Scenes still draw the board (their own
/// SkaterPresEntity), so it shows only while a cutscene holds the member.
fn hides_board(key: &str) -> bool {
    matches!(key.split('#').next().unwrap_or(key), "attiba_jefferson" | "giovanni_reda")
}

/// A member's meshes shown only while a cutscene holds it: its board under [`hides_board`], and Reda's video camera
/// (his scenes name giovanni_reda_prop, whose Accessory it is; his standing row's plain model has none, user 10-29).
fn scene_only(key: &str, material: &str) -> bool {
    let model = key.split('#').next().unwrap_or(key);
    (hides_board(key) && material == "Retail_SkateBoard") || (model == "giovanni_reda" && material == "Retail_Accessory")
}

/// Moves the board's bones (`board`, `root` among them) so `root` sits at `prop`, children keeping their offsets.
fn carry_board(pose: &mut [Mat4], board: &[usize], root: usize, prop: Mat4) {
    let carry = prop * pose[root].inverse();
    for &b in board {
        pose[b] = carry * pose[b];
    }
}

/// The pedestrian bank on the skater's bones plus the hand prop (last), the board's bones and the prop's parent.
struct Pedestrian {
    poser: Poser,
    board: Vec<usize>,
    root: usize,
    hand: usize,
    right: usize,
}
impl Pedestrian {
    fn load(root: &std::path::Path, skater: &skate_data::animation_frames::AnimationFrames) -> Result<Self, String> {
        let mut bones = skater.bone_names.clone();
        bones.extend([HAND_PROP.into(), RIGHT_HAND_PROP.into()]);
        let (mut poser, _) = Poser::scene(&root.join(PEDESTRIAN_BANK), &bones)?;
        if let Some(missing) = poser.remap(&bones).into_iter().find(|b| b == HAND_PROP || b == RIGHT_HAND_PROP) {
            return Err(format!("no {missing}"));
        }
        let root = skater.bone_names.iter().position(|n| n == "SKATEBOARD_ROOT").ok_or("no SKATEBOARD_ROOT")?;
        let hand = skater.bone_names.iter().position(|n| n == "LEFTHAND").ok_or("no LEFTHAND")?;
        let right = skater.bone_names.iter().position(|n| n == "RIGHTHAND").ok_or("no RIGHTHAND")?;
        // Parents come before their children.
        let mut on = vec![false; skater.bone_names.len()];
        for i in 0..on.len() {
            on[i] = i == root || usize::try_from(skater.parents[i]).is_ok_and(|p| on[p]);
        }
        Ok(Self { poser, board: (0..on.len()).filter(|&i| on[i]).collect(), root, hand, right })
    }
}

/// Who should stand where, by cast key (`slappy`, `giovanni_reda`, ...): a locator matrix.
#[derive(Resource, Default)]
pub(crate) struct Cast {
    pub wanted: BTreeMap<String, [[f32; 4]; 4]>,
    /// Cast keys a cutscene animates: their idle stands aside.
    pub held: std::collections::BTreeSet<String>,
    /// Cast keys on a board (the competitors): the skater's on-board idle, not the stand.
    pub riding: std::collections::BTreeSet<String>,
    /// The folder the members' models load from (`<folder>/cast/<model>.glb`): empty = Skate 2's tutorial;
    /// skate1_cast sets Skate 1's while it owns the cast.
    pub folder: &'static str,
    /// Poses a binding writes for its members (skate1_cast's idles), ahead of the rider and stand poses.
    pub poses: BTreeMap<String, Vec<Mat4>>,
    members: BTreeMap<String, Member>,
    poser: Option<Poser>,
    pedestrian: Option<Pedestrian>,
    /// The skater's idle at rest (as the online skaters start): bones the editor rig lacks hold this, not the
    /// player's live pose.
    rest: Vec<NativeMatrix>,
    failed: bool,
    time: f32,
}

impl Cast {
    /// The character's root entity, once spawned (cutscene actor binding).
    pub(crate) fn entity(&self, key: &str) -> Option<Entity> {
        self.members.get(key).map(|m| m.root)
    }
    /// `key`'s marquee idle `time` s in, the board in hand where it carries one; None without the bank or a clip.
    fn idle(&self, key: &str, time: f32, names: &[String]) -> Option<Vec<Mat4>> {
        let p = self.pedestrian.as_ref()?;
        let (clip, board) = idle_clip(key)?;
        let length = p.poser.length(clip).filter(|l| *l > 1e-3)?;
        let mut pose = p.poser.pose_onto(Some(PEDESTRIAN_REFERENCE), clip, time % length, true, &self.rest)?;
        let right = pose.pop()?;
        let prop = pose.pop()?;
        // RETAIL (s2.bin 0x8267D240..D25C): the marquee rig's RIGHT_PROP (a root) is RIGHTHANDPROP's local; Atiba's
        // camera is skinned to it. The extra joints before it keep their transforms (NaN: pose_transforms skips them).
        if let Some(i) = names.iter().skip(pose.len()).position(|n| n.eq_ignore_ascii_case("RIGHT_PROP")) {
            let local = pose[p.right].inverse() * right;
            pose.resize(pose.len() + i, Mat4::NAN);
            pose.push(local);
        }
        if board {
            // RETAIL (s2.bin 0x8267A814, 0x8267D1C4): the marquee rig takes the pedestrian rig's locals TRAJECTORY..=
            // LEFTHANDPROP as its first 26 bones, so its SKATEBOARD_ROOT (bone 25, a root) is LEFTHANDPROP's local, not
            // the prop under LEFTHAND: in the STNDWBOARD idles the board stands on its tail, the nose in the left hand.
            let local = pose[p.hand].inverse() * prop;
            carry_board(&mut pose, &p.board, p.root, local);
        }
        Some(pose)
    }
    /// The bones a spawned character binds: the skater's, then its model's extra joints (a Skate 2 scene rig's
    /// face, fingers and twists, which only cutscenes pose).
    pub(crate) fn bones(&self, key: &str) -> Option<&[String]> {
        self.members.get(key).filter(|m| m.bones.is_some()).map(|m| &m.names[..])
    }
}

struct Member {
    root: Entity,
    scene: Entity,
    at: [[f32; 4]; 4],
    bones: Option<AnimationStatus>,
    names: Vec<String>,
    /// Its meshes shown only in scenes ([`scene_only`]: a hidden board, Reda's video camera).
    board: Vec<Entity>,
}

pub(crate) struct Skate2CastPlugin;
impl Plugin for Skate2CastPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Cast>()
            .add_systems(Update, (place, pose).chain().after(crate::animation::present).in_set(FrameSet::Animation));
    }
}

fn place(
    mut commands: Commands,
    mut cast: ResMut<Cast>,
    server: Res<AssetServer>,
    skater: Res<SkaterRuntime>,
    instances: Query<&SceneInstance>,
    spawner: Res<SceneSpawner>,
    skins: Query<(Entity, &SkinnedMesh)>,
    nodes: Query<(&Name, &Transform)>,
    parents: Query<&ChildOf>,
    physics: Option<Res<crate::physics::GamePhysics>>,
    materials: Query<&bevy::gltf::GltfMaterialName>,
) {
    let cast = &mut *cast;
    let ground = |p| physics.as_deref().and_then(|physics| crate::challenge::cutscene::ground(physics, p));
    cast.members.retain(|key, m| {
        let keep = cast.wanted.get(key) == Some(&m.at);
        if !keep {
            commands.entity(m.root).despawn();
        }
        keep
    });
    for (key, at) in &cast.wanted {
        cast.members.entry(key.clone()).or_insert_with(|| {
            // Riders are placed on their path every frame (skate2_challenge place_cast); the path is already at retail height.
            // ponytail: grounded once at spawn; a member spawned before the map collision keeps its locator height.
            let placed = if cast.riding.contains(key) { locator_transform(at) } else { stand(at, ground) };
            let root = commands.spawn((Name::new(format!("Skate 2 cast: {key}")), placed, Visibility::Hidden)).id();
            if cast.folder.is_empty() {
                // Lit like the player's skater (retail_character::bind): Skate 2's marquee models are character.* shaders.
                let model = skate2_model(key.split('#').next().unwrap_or(key.as_str()));
                commands.entity(root).insert(crate::custom_models::NativeModelRoot(format!("skate2-tutorial/{model}")));
            }
            let path = model_path(cast.folder, key);
            let scene = commands
                .spawn((
                    SceneRoot(server.load(GltfAssetLabel::Scene(0).from_asset(path))),
                    ChildOf(root),
                ))
                .id();
            Member { root, scene, at: *at, bones: None, names: Vec::new(), board: Vec::new() }
        });
    }
    for (key, m) in cast.members.iter_mut().filter(|(_, m)| m.bones.is_none()) {
        if !instances.get(m.scene).is_ok_and(|i| spawner.instance_is_ready(**i)) {
            continue;
        }
        // The stand poses the skater's bones; extra joints hang at bind off them until a cutscene poses them.
        let mut names = skater.animation.evaluator.frames.bone_names.clone();
        for (e, skin) in &skins {
            if parents.iter_ancestors(e).any(|p| p == m.root) {
                for (name, _) in skin.joints.iter().filter_map(|&j| nodes.get(j).ok()) {
                    if !names.iter().any(|n| n.eq_ignore_ascii_case(name.as_str())) {
                        names.push(name.as_str().to_owned());
                    }
                }
            }
        }
        match AnimationStatus::for_scene(m.root, &names, &skins, &nodes, &parents) {
            Ok(bones) => {
                for (e, _) in &skins {
                    if parents.iter_ancestors(e).any(|p| p == m.root) {
                        // Same layers as the other skaters (28 = character lighting/shadow pass).
                        commands.entity(e).insert((NoFrustumCulling, RenderLayers::from_layers(&[0, 28])));
                        if materials.get(e).is_ok_and(|n| scene_only(key, &n.0)) {
                            m.board.push(e);
                        }
                    }
                }
                commands.entity(m.root).insert(Visibility::Inherited);
                m.bones = Some(bones);
                m.names = names;
            }
            Err(error) => {
                warn!("Skate 2 cast: {key} cannot be posed ({error})");
                commands.entity(m.root).despawn();
                cast.wanted.remove(key);
            }
        }
    }
}

/// `<folder>/cast/<model>.glb`; `key#n` is another of the same character (a cutscene's extras, a rider's copy).
fn model_path(folder: &str, key: &str) -> String {
    let model = key.split('#').next().unwrap_or(key);
    let (folder, model) = if folder.is_empty() { ("private/skate2-tutorial", skate2_model(model)) } else { (folder, model) };
    format!("{folder}/cast/{model}.glb")
}
/// The model a member loads: Atiba's livingworld_models row names attiba_jefferson_prop (field 0x62F7C585686F3371),
/// whose Accessory is his camera (skinned to Right_Prop); Reda's scenes name giovanni_reda_prop (his video camera,
/// shown only in scenes: [`scene_only`]).
fn skate2_model(model: &str) -> &str {
    match model {
        "attiba_jefferson" => "attiba_jefferson_prop",
        "giovanni_reda" => "giovanni_reda_prop",
        _ => model,
    }
}

fn pose(
    mut cast: ResMut<Cast>,
    physics: Option<Res<crate::physics::GamePhysics>>,
    config: Res<Config>,
    manifest: Res<crate::assets::AssetManifest>,
    skater: Res<SkaterRuntime>,
    real: Res<Time<Real>>,
    mut transforms: Query<&mut Transform>,
    riders: Option<Res<crate::physics::riders::Riders>>,
    mut shown: Query<&mut Visibility>,
) {
    if cast.members.values().all(|m| m.bones.is_none()) {
        return;
    }
    if cast.poser.is_none() && !cast.failed {
        match Poser::load(&config.asset_root, "male", &skater.animation.evaluator.frames.bone_names) {
            Ok(poser) => cast.poser = Some(poser),
            Err(error) => {
                warn!("Skate 2 cast: stand pose unavailable ({error}); characters keep their bind pose");
                cast.failed = true;
            }
        }
        match Pedestrian::load(&config.asset_root, &skater.animation.evaluator.frames) {
            Ok(p) => cast.pedestrian = Some(p),
            Err(error) => warn!("Skate 2 cast: marquee idles unavailable ({error}); the cast stands in CAS_STAND"),
        }
        let evaluator = &skater.animation.evaluator;
        let idle = PoseCommand::Clip { name: manifest.0.initial_animation.clone(), previous_time: 0., time: 0., loops: 0 };
        let commands = [idle, PoseCommand::Pose { name: "RIG_TPOSE".into() }, PoseCommand::Add { motion_is_a: true }];
        cast.rest = evaluator.evaluate(&commands).and_then(|p| evaluator.hierarchy(&p)).unwrap_or_else(|_| skater.render_pose.clone());
    }
    cast.time += real.delta_secs().min(0.1);
    // User 10-25 (never a T-pose): without the stand clip, riders and riding members still pose and the rest stand in
    // the idle's first frame, rather than every member keeping its bind pose.
    let stand = cast.poser.as_ref().and_then(|poser| Some((poser, poser.length(STAND).filter(|l| *l > 1e-3)?)));
    let rest = || cast.rest.iter().copied().map(crate::animation::native_matrix).collect::<Vec<Mat4>>();
    let riding = |time: f32| {
        ride_pose(&skater, &manifest.0.initial_animation, time).map(|mut pose| {
            let radius = physics.as_deref().map_or(0., crate::physics::GamePhysics::wheel_radius);
            wheels_down(&mut pose, &skater.animation.evaluator.frames.bone_names, radius);
            pose
        })
    };
    // Each character is offset in the loop so they don't breathe in step.
    for (i, (key, m)) in cast.members.iter().enumerate() {
        let board = if cast.held.contains(key) { Visibility::Inherited } else { Visibility::Hidden };
        for &e in &m.board {
            if let Ok(mut v) = shown.get_mut(e) {
                v.set_if_neq(board);
            }
        }
        let Some(bones) = m.bones.as_ref().filter(|_| !cast.held.contains(key)) else { continue };
        let at = cast.time + i as f32 * 1.7;
        // round2 T5: a full-skater rider's own animation, world space under its origin root (skate2_challenge place_cast).
        let pose = if let Some(pose) = cast.poses.get(key) {
            Some(pose.clone())
        } else if let Some(rider) = riders.as_deref().and_then(|r| r.get(key)) {
            // Before its first tick a rider's pose is the skeleton's initial T-pose: it rides the idle under its root.
            if rider.posed() {
                Some(rider.pose())
            } else {
                let root = crate::animation::native_matrix(rider.root());
                riding(at).map(|pose| pose.into_iter().map(|bone| root * bone).collect())
            }
        } else if cast.riding.contains(key) {
            riding(at)
        } else {
            (cast.idle(key, at, &m.names))
                .or_else(|| stand.and_then(|(poser, length)| poser.pose(STAND, at % length, true, &cast.rest)))
                .or_else(|| Some(rest()))
        };
        let Some(pose) = pose else { continue };
        for (entity, transform) in bones.pose_transforms(&pose) {
            if let Ok(mut node) = transforms.get_mut(entity) {
                *node = transform;
            }
        }
    }
}

/// `clip` (the game's on-board idle) looping at `time` on the skater's own skeleton: the cast rig carries the board.
pub(crate) fn ride_pose(skater: &SkaterRuntime, clip: &str, time: f32) -> Option<Vec<Mat4>> {
    let evaluator = &skater.animation.evaluator;
    let c = evaluator.frames.clip(clip).ok()?;
    let length = c.frames.len().saturating_sub(1) as f32 / f32::from_bits(c.fps_bits).max(1.);
    let time = if length > 0. { time % length } else { 0. };
    let clip = PoseCommand::Clip { name: clip.to_owned(), previous_time: time, time, loops: 0 };
    let commands = [clip, PoseCommand::Pose { name: "RIG_TPOSE".into() }, PoseCommand::Add { motion_is_a: true }];
    let pose = evaluator.evaluate(&commands).and_then(|p| evaluator.hierarchy(&p)).ok()?;
    Some(pose.into_iter().map(crate::animation::native_matrix).collect())
}

/// Lowers a riding pose until its lowest wheel touches the root (a rider's root is the ground under its path).
fn wheels_down(pose: &mut [Mat4], names: &[String], radius: f32) {
    const WHEELS: [&str; 4] = ["LEFT_WHEELFRONT", "RIGHT_WHEELFRONT", "LEFT_WHEELBACK", "RIGHT_WHEELBACK"];
    let wheels = names.iter().zip(pose.iter()).filter(|(n, _)| WHEELS.iter().any(|w| w.eq_ignore_ascii_case(n)));
    let Some(low) = wheels.map(|(_, m)| m.w_axis.y - radius).reduce(f32::min) else { return };
    for bone in pose {
        bone.w_axis.y -= low;
    }
}

/// A standing member's root: its locator brought down onto the ground under it (retail's locators sit 0.3-0.8 m up,
/// round2-research R1; how retail grounds them, capsule or spawn probe, is UNTRACED). A miss keeps the locator.
fn stand(at: &[[f32; 4]; 4], ground: impl Fn(Vec3) -> Option<Vec3>) -> Transform {
    let mut root = locator_transform(at);
    if let Some(g) = ground(root.translation) {
        root.translation.y = g.y;
    }
    root
}

/// A placement locator (row-major, translation in row 3, world space) as the character root.
fn locator_transform(m: &[[f32; 4]; 4]) -> Transform {
    Transform::from_matrix(Mat4::from_cols_array_2d(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Skate 1's people (skate1_cast) load from their own folder; Skate 2's default folder and keys are unchanged.
    #[test]
    fn a_member_loads_from_the_cast_folder() {
        assert_eq!(model_path("", "slappy#2"), "private/skate2-tutorial/cast/slappy.glb");
        assert_eq!(model_path("", "mike_carroll"), "private/skate2-tutorial/cast/mike_carroll.glb");
        assert_eq!(model_path("private/challenges-skate1", "IP_shingo#s1ai1"), "private/challenges-skate1/cast/IP_shingo.glb");
        let cast = Cast::default();
        assert!(cast.folder.is_empty() && cast.poses.is_empty());
    }

    /// User 10-13 (bug 3): retail's placement locators sit 0.3-0.8 m above the ground (round2-research R1); a standing
    /// member's root goes down onto the ground under its locator, keeping the locator's facing. A miss keeps the locator.
    #[test]
    fn the_cast_stands_on_the_ground_under_its_locator() {
        let at = [[0., 0., -1., 0.], [0., 1., 0., 0.], [1., 0., 0., 0.], [5., 0.8, -3., 1.]];
        let stood = stand(&at, |p| Some(Vec3::new(p.x, 0.02, p.z)));
        assert_eq!(stood.translation, Vec3::new(5., 0.02, -3.));
        assert_eq!(stood.rotation, locator_transform(&at).rotation);
        assert_eq!(stand(&at, |_| None), locator_transform(&at));
    }

    /// User 10-13 (bug 3): each marquee member idles in its vault remap row's IdleBasicCyc (round2-research2 Q1):
    /// marquee_board (Slappy and the three pros), marquee (Reda), marquee_photographer (Atiba). Others keep CAS_STAND.
    #[test]
    fn the_marquee_cast_idles_in_its_remap_row() {
        for key in ["slappy", "eric_koston", "mike_carroll", "rob_dyrdek", "slappy#2"] {
            assert_eq!(idle_clip(key), Some((BOARD_IDLE, true)), "{key}");
        }
        assert_eq!(idle_clip("giovanni_reda"), Some(("IP_WNDR_STND_IDLE1_0_CYC", false)));
        assert_eq!(idle_clip("attiba_jefferson"), Some(("PHOT_STAND_IDLE_N_0_CYC", false)));
        assert_eq!(idle_clip("ai_skater_01"), None);
    }

    /// The board's bones follow the hand prop: SKATEBOARD_ROOT lands on it, its children keep their offsets.
    #[test]
    fn a_carried_board_hangs_on_the_hand_prop() {
        let mut pose = vec![
            Mat4::from_translation(Vec3::Y),
            Mat4::from_translation(Vec3::new(0., 0.1, 0.)),
            Mat4::from_translation(Vec3::new(0., 0.1, 0.4)),
        ];
        let prop = Mat4::from_rotation_translation(Quat::from_rotation_x(1.), Vec3::new(0.3, 0.9, 0.1));
        carry_board(&mut pose, &[1, 2], 1, prop);
        assert!(pose[1].abs_diff_eq(prop, 1e-6));
        assert!(pose[2].abs_diff_eq(prop * Mat4::from_translation(Vec3::new(0., 0., 0.4)), 1e-6));
        assert_eq!(pose[0], Mat4::from_translation(Vec3::Y), "the body is the clip's");
    }

    /// The idles and the hand prop are in Skate 3's pedestrian bank. Needs SKATE3_ASSET_ROOT.
    #[test]
    #[ignore]
    fn the_marquee_idles_are_in_the_pedestrian_bank() {
        let root = std::path::PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap());
        let (mut poser, _) = Poser::scene(&root.join(PEDESTRIAN_BANK), &[]).unwrap();
        assert!(poser.remap(&[HAND_PROP.into()]).is_empty(), "the rig has {HAND_PROP}");
        for key in ["slappy", "giovanni_reda", "attiba_jefferson"] {
            let (clip, _) = idle_clip(key).unwrap();
            assert!(poser.length(clip).is_some_and(|l| l > 1.), "{clip}");
        }
    }

    /// User 10-28 (bug 1, Slappy "sunken and crushed in the ground"): the pedestrian bank's clips are deltas on its
    /// PEDESTRIAN_RIG_TPOSE (its T-pose clip decodes to zero translations; the skater's clips go onto RIG_TPOSE the
    /// same way). Played alone, every body joint sat at the root, 3 cm under the ground. Each marquee idle stands the
    /// hips 0.85-0.95 m up, the head over 1.4 m and the toes on the ground, as Skate 1's do. Needs SKATE3_ASSET_ROOT.
    #[test]
    #[ignore]
    fn the_marquee_idles_stand_on_the_ground() {
        let root = std::path::PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap());
        let offboard = skate_data::abin::Bank::load(&root.join("private/stock/data/anim/OffBoard.abin")).unwrap();
        let banks = skate_data::animation_banks::AnimationBanks {
            banks: vec![std::sync::Arc::new(offboard)],
            identities: vec!["skater".into()],
        };
        let skater = skate_data::animation_frames::AnimationFrames::from_banks(&banks).unwrap();
        let cast = Cast { pedestrian: Some(Pedestrian::load(&root, &skater).unwrap()), ..Default::default() };
        let at = |name: &str| skater.bone_names.iter().position(|b| b == name).unwrap();
        for key in ["slappy", "giovanni_reda", "attiba_jefferson"] {
            for time in [0., 3.] {
                let pose = cast.idle(key, time, &[]).unwrap();
                let y = |bone: &str| pose[at(bone)].w_axis.y;
                assert!((0.85..0.95).contains(&y("HIPS")), "{key} at {time} s: hips {}", y("HIPS"));
                assert!(y("HEAD") > 1.4, "{key} at {time} s: head {}", y("HEAD"));
                for toe in ["LEFTTOEBASE", "RIGHTTOEBASE"] {
                    assert!((0. ..0.06).contains(&y(toe)), "{key} at {time} s: {toe} {}", y(toe));
                }
            }
        }
    }

    /// User 10-13 (bugs 9, 10): a rider's root is the ground under its path; its on-board pose comes down until the
    /// lowest wheel (centre less the radius) touches it. A rig without wheels keeps its pose.
    #[test]
    fn a_rider_stands_its_wheels_on_its_root() {
        let names: Vec<String> = ["HIPS", "Left_WheelFront", "RIGHT_WHEELFRONT", "LEFT_WHEELBACK", "RIGHT_WHEELBACK"].map(String::from).into();
        let at = |y| Mat4::from_translation(Vec3::new(0., y, 0.));
        let mut pose = vec![at(1.), at(0.31), at(0.30), at(0.32), at(0.31)];
        wheels_down(&mut pose, &names, 0.03);
        assert!((pose[0].w_axis.y - 0.73).abs() < 1e-5, "the body comes down with the board");
        assert!((pose[2].w_axis.y - 0.03).abs() < 1e-5, "the lowest wheel sits its radius up");
        let mut bare = vec![at(1.)];
        wheels_down(&mut bare, &names[..1], 0.03);
        assert_eq!(bare, vec![at(1.)]);
    }

    /// User 10-29 (bug 3, Slappy "isnt holding his board correctly"): retail Skate 2's marquee rig takes the pedestrian
    /// clip's LEFTHANDPROP local as its root SKATEBOARD_ROOT (s2.bin 0x8267A814, 0x8267D1C4), so in the STNDWBOARD idle
    /// the deck (along the board root's z, +-0.45 m in the cast GLBs) stands on its tail by the left foot, the nose in
    /// the left hand. Composed through LEFTHAND it floated 0.7-1.3 m up, tilted 60 degrees. Needs SKATE3_ASSET_ROOT.
    #[test]
    #[ignore]
    fn the_board_idle_stands_the_board_on_its_tail_in_the_left_hand() {
        let root = std::path::PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap());
        let offboard = skate_data::abin::Bank::load(&root.join("private/stock/data/anim/OffBoard.abin")).unwrap();
        let banks = skate_data::animation_banks::AnimationBanks {
            banks: vec![std::sync::Arc::new(offboard)],
            identities: vec!["skater".into()],
        };
        let skater = skate_data::animation_frames::AnimationFrames::from_banks(&banks).unwrap();
        // The board's bones are not in the pedestrian rig: they start from `rest` (the game's is the skater's idle).
        // Identity is enough, since the carry moves the root onto the prop whatever it was.
        let identity = [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [0., 0., 0., 1.]];
        let cast = Cast {
            pedestrian: Some(Pedestrian::load(&root, &skater).unwrap()),
            rest: vec![identity; skater.bone_names.len()],
            ..Default::default()
        };
        let at = |name: &str| skater.bone_names.iter().position(|b| b == name).unwrap();
        for time in [0., 3.] {
            let pose = cast.idle("slappy", time, &[]).unwrap();
            let board = pose[at("SKATEBOARD_ROOT")];
            assert!(board.is_finite(), "at {time} s SKATEBOARD_ROOT is {board}");
            let deck = board.z_axis.truncate().normalize();
            assert!(deck.y.abs() > 0.95, "at {time} s the deck runs {deck}, not upright");
            let ends = [-0.45f32, 0.45].map(|t| board.w_axis.truncate() + deck * t);
            let (tail, nose) = if ends[0].y < ends[1].y { (ends[0], ends[1]) } else { (ends[1], ends[0]) };
            assert!((-0.03..0.06).contains(&tail.y), "at {time} s the tail is {} m up", tail.y);
            let hand = pose[at("LEFTHAND")].w_axis.truncate();
            assert!(nose.distance(hand) < 0.12, "at {time} s the nose is {} m from the left hand", nose.distance(hand));
        }
    }

    /// User 10-29 (bug 5, No Mugshots' pro has "his board inbetween his feet"): Skate 2's livingworld_models rows set
    /// Hide_Skateboard (0x0626FA10792BBA58) for Atiba and Reda only, and a pedestrian with it leaves its SkateBoard
    /// component out (s2.bin 0x8267E9C8). The board idlers keep theirs.
    #[test]
    fn the_photographer_and_the_filmer_leave_their_boards_out() {
        for key in ["attiba_jefferson", "giovanni_reda", "giovanni_reda#1"] {
            assert!(hides_board(key), "{key}");
        }
        for key in ["slappy", "eric_koston", "mike_carroll#ai1", "rob_dyrdek", "ai_skater_01"] {
            assert!(!hides_board(key), "{key}");
        }
    }

    /// User 10-29 (Atiba "looks like he is supposed to be holding something"): Skate 2's livingworld_models row for
    /// Atiba names attiba_jefferson_prop, the recipe with his camera; Skate 1's folder and the other keys are unchanged.
    #[test]
    fn atiba_stands_as_his_camera_model() {
        assert_eq!(model_path("", "attiba_jefferson"), "private/skate2-tutorial/cast/attiba_jefferson_prop.glb");
        assert_eq!(model_path("", "slappy"), "private/skate2-tutorial/cast/slappy.glb");
        assert_eq!(model_path("private/challenges-skate1", "attiba_jefferson"), "private/challenges-skate1/cast/attiba_jefferson.glb");
    }

    /// User 10-29 ("reda looks like hes supposed to be holding something in some of the cutscenes"): Reda loads his
    /// scenes' giovanni_reda_prop and shows its video camera (Accessory) only while a cutscene holds him, as his board.
    #[test]
    fn redas_video_camera_shows_only_in_scenes() {
        assert_eq!(model_path("", "giovanni_reda#1"), "private/skate2-tutorial/cast/giovanni_reda_prop.glb");
        assert!(scene_only("giovanni_reda", "Retail_Accessory"));
        assert!(scene_only("giovanni_reda", "Retail_SkateBoard"));
        assert!(!scene_only("attiba_jefferson", "Retail_Accessory"), "Atiba holds his camera standing too");
        assert!(scene_only("attiba_jefferson", "Retail_SkateBoard"));
        assert!(!scene_only("slappy", "Retail_SkateBoard"));
    }

    /// User 10-29: retail puts the marquee rig's RIGHT_PROP (Atiba's camera) on the photographer idle's RIGHTHANDPROP
    /// local (s2.bin 0x8267D240): at chest height, in front of and beside his right hand. Needs SKATE3_ASSET_ROOT.
    #[test]
    #[ignore]
    fn atibas_camera_rides_his_right_hand_prop() {
        let root = std::path::PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap());
        let offboard = skate_data::abin::Bank::load(&root.join("private/stock/data/anim/OffBoard.abin")).unwrap();
        let banks = skate_data::animation_banks::AnimationBanks { banks: vec![std::sync::Arc::new(offboard)], identities: vec!["skater".into()] };
        let skater = skate_data::animation_frames::AnimationFrames::from_banks(&banks).unwrap();
        let cast = Cast { pedestrian: Some(Pedestrian::load(&root, &skater).unwrap()), ..Default::default() };
        let names: Vec<String> = skater.bone_names.iter().cloned().chain(["FACE".into(), "RIGHT_PROP".into()]).collect();
        let at = |name: &str| skater.bone_names.iter().position(|b| b == name).unwrap();
        for time in [0., 3.] {
            let pose = cast.idle("attiba_jefferson", time, &names).unwrap();
            assert_eq!(pose.len(), names.len());
            assert!(pose[names.len() - 2].is_nan(), "FACE keeps its transform");
            let camera = pose[names.len() - 1].w_axis.truncate();
            let hand = pose[at("RIGHTHAND")].w_axis.truncate();
            assert!((0.95..1.2).contains(&camera.y), "at {time} s the camera is {} m up", camera.y);
            assert!(camera.distance(hand) < 0.15 && camera.z > hand.z, "at {time} s camera {camera}, right hand {hand}");
        }
    }

    /// User 10-29 (bug 1): every material of every cast model has its character rows in cast/lighting.json
    /// (skate2port/old_tutorial_cast.py), hair with its coverage map. Needs SKATE3_ASSET_ROOT.
    #[test]
    #[ignore]
    fn every_cast_material_has_character_rows() {
        let root = std::path::PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap());
        let cast = root.join("private/skate2-tutorial/cast");
        let table: serde_json::Value = serde_json::from_slice(&std::fs::read(cast.join("lighting.json")).unwrap()).unwrap();
        for key in ["slappy", "giovanni_reda", "giovanni_reda_prop", "attiba_jefferson", "attiba_jefferson_prop", "mike_carroll", "eric_koston", "rob_dyrdek"] {
            let glb = std::fs::read(cast.join(format!("{key}.glb"))).unwrap();
            let length = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
            let gltf: serde_json::Value = serde_json::from_slice(&glb[20..20 + length]).unwrap();
            for material in gltf["materials"].as_array().unwrap() {
                let name = material["name"].as_str().unwrap();
                let rows = &table[key][name];
                assert_eq!(rows["params"].as_array().map(Vec::len), Some(9), "{key} {name}");
                if material["alphaMode"] == "MASK" {
                    let mask = rows["opacity"].as_str().unwrap_or_else(|| panic!("{key} {name}: no coverage"));
                    assert!(root.join(mask).is_file(), "{key} {name}: {mask}");
                }
            }
        }
    }
}
