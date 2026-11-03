//! The skater's pose while Edit Skater is open: Skate 3's editor clips, not the
//! paused gameplay pose. cac_edit_<gender>.abin holds CAS_STAND (board in hand)
//! and its posture variants; gesture/cac_<gender>_<name>.abin previews each
//! gesture. They play on real time while gameplay stays paused, on the editor's
//! 131-bone skeleton, and drive the skater's 36 shared bones by name.
use crate::{animation_pose::PoseEvaluator, config::Config, customiser::Customiser, physics::SkaterRuntime};
use bevy::prelude::*;
use skate_core::animation::{
    output::{NativeMatrix, sqt_to_matrix},
    playback_tree::PoseCommand,
};
use skate_data::{abin::Bank, animation_banks::AnimationBanks, animation_frames::SampleWords};
use std::{path::Path, sync::Arc};

/// Engine gesture label -> (game string, retail preview clip stem).
pub(crate) const GESTURES: [(&str, &str, &str); 37] = [
    ("Air guitar", "ID_GESTURE_AIRGUITAR", "airguitar"),
    ("Airplane", "ID_GESTURE_AIRPLANE", "airplane"),
    ("Boxing", "ID_GESTURE_BOXING", "boxing"),
    ("Bruce Lee", "ID_GESTURE_BRUCE", "bruce"),
    ("Check the time", "ID_GESTURE_CHECKTIME", "checktime"),
    ("Devil horns", "ID_GESTURE_DEVIL", "devil"),
    ("Finger guns", "ID_GESTURE_DOUBLE_GUN", "doublegun"),
    ("Dunno", "ID_GESTURE_DUNNO", "dunno"),
    ("Finger wag", "ID_GESTURE_FINGERWAG", "fingerwag"),
    ("Fists", "ID_GESTURE_FISTS", "fists"),
    ("Flex", "ID_GESTURE_BICEPFLEX", "flex"),
    ("Flip the table", "ID_GESTURE_FLIPTABLE", "fliptable"),
    ("The Fonz", "ID_GESTURE_FONZ", "fonz"),
    ("Freedom", "ID_GESTURE_FREEDOM", "freedom"),
    ("Raised fist", "ID_GESTURE_FU", "fu"),
    ("Get away", "ID_GESTURE_GETAWAY", "getouttheway"),
    ("Get outta here", "ID_GESTURE_GETOUTTAHERE", "getoutahere"),
    ("Handcuffs", "ID_GESTURE_HANDCUFFS", "handcuffs"),
    ("High fist pump", "ID_GESTURE_PUMP_HI", "pumphigh"),
    ("Low fist pump", "ID_GESTURE_PUMP_LO", "pumplow"),
    ("Wind up", "ID_GESTURE_PREWIND", "prewind"),
    ("Peace", "ID_GESTURE_PEACE", "peace"),
    ("Point", "ID_GESTURE_FINGERPOINT", "fingerpoint"),
    ("Raise the roof", "ID_GESTURE_RAISEROOF", "raisetheroof"),
    ("Shaka", "ID_GESTURE_SHAKA", "shaka"),
    ("Shrug", "ID_GESTURE_SHRUG", "shrug"),
    ("Point to the sky", "ID_GESTURE_SKY", "sky"),
    ("Snap", "ID_GESTURE_SNAP", "snap"),
    ("Soul arch", "ID_GESTURE_SOULARCH", "soularch"),
    ("Spock", "ID_GESTURE_SPOCK", "spock"),
    ("Surf's up", "ID_GESTURE_SURFSUP", "surfsup"),
    ("Swing high", "ID_GESTURE_SWINGHIGH", "swinghigh"),
    ("Swing low", "ID_GESTURE_SWINGLOW", "swinglow"),
    ("Throw your arms", "ID_GESTURE_THROWARMS", "throwarms"),
    ("Thumbs down", "ID_GESTURE_THMB_DWN", "thumbsdown"),
    ("Wings", "ID_GESTURE_WINGS", "wings"),
    ("Yard sale", "ID_GESTURE_YARDSALE", "yardsale"),
];

pub(crate) struct Poser {
    gender: String,
    evaluator: PoseEvaluator,
    /// Skater bone -> editor bone.
    map: Vec<Option<usize>>,
}
impl Poser {
    pub(crate) fn load(root: &Path, gender: &str, skater_bones: &[String]) -> Result<Self, String> {
        let anim = root.join("private/stock/data/anim");
        let mut banks = vec![Arc::new(Bank::load(&anim.join(format!("cac_edit_{gender}.abin"))).map_err(|e| e.to_string())?)];
        for (_, _, stem) in GESTURES {
            let path = anim.join(format!("gesture/cac_{gender}_{stem}.abin"));
            if let Ok(bank) = Bank::load(&path) {
                banks.push(Arc::new(bank));
            }
        }
        Self::from_banks(gender, banks, skater_bones)
    }
    /// The editor bank alone (CAS_STAND and the CAS_Single_Pose_N photo poses), without the gesture previews.
    pub(crate) fn editor(root: &Path, gender: &str, skater_bones: &[String]) -> Result<Self, String> {
        let path = root.join(format!("private/stock/data/anim/cac_edit_{gender}.abin"));
        let bank = Arc::new(Bank::load(&path).map_err(|e| e.to_string())?);
        Self::from_banks(gender, vec![bank], skater_bones)
    }
    /// Skate 2's editor bank (private/cas-skate2, old_cas.py): CAS_STAND, CAS_SITTING (the couch), CAS_ONTABLE (Pete's
    /// table) and CAS_GESTURESTAND_CYCLE, played whole as Skate 3's editor clips are (the same RAW bank layout).
    pub(crate) fn skate2(root: &Path, gender: &str, skater_bones: &[String]) -> Result<Self, String> {
        let path = root.join(format!("private/cas-skate2/cac_edit_{gender}.abin"));
        let bank = Arc::new(Bank::load(&path).map_err(|e| e.to_string())?);
        Self::from_banks(gender, vec![bank], skater_bones)
    }
    /// One Skate 2 scene bank (a cutscene actor track) and the name of the clip it holds.
    pub(crate) fn scene(path: &Path, skater_bones: &[String]) -> Result<(Self, String), String> {
        let bank = Arc::new(Bank::load(path).map_err(|e| e.to_string())?);
        let poser = Self::from_banks("", vec![bank], skater_bones)?;
        let clip = poser.evaluator.frames.clip_names().next().ok_or("scene bank has no clip")?.to_owned();
        Ok((poser, clip))
    }
    /// [`Poser::scene`] for the player (faces.md 9.2, retail 0x826c0550): each face bone's base is overwritten by name
    /// from `binds` (the player's head joints as bind worlds, native space), in the bank's own parenting; an OFFSET_
    /// parent the head lacks keeps its bank reference local. Binds equal to the bank's reference change nothing.
    pub(crate) fn scene_with_face(path: &Path, bones: &[String], binds: &[(String, Mat4)]) -> Result<(Self, String), String> {
        let (mut poser, clip) = Self::scene(path, bones)?;
        let frames = &poser.evaluator.frames;
        let reference = frames.reference_samples(0)?;
        let name = |i: usize| frames.bone_names[i].as_str();
        let parent = |i: usize| usize::try_from(frames.parents[i]).ok();
        let bind = |i: usize| binds.iter().find(|(n, _)| n.eq_ignore_ascii_case(name(i))).map(|(_, m)| *m);
        // A bone's world in the binds' frame: its bind, else its parent's world times its reference local. The
        // hierarchy lists parents first.
        let mut worlds: Vec<Option<Mat4>> = Vec::with_capacity(reference.len());
        for (i, words) in reference.iter().enumerate() {
            let local = crate::animation::native_matrix(sqt_to_matrix(crate::animation_pose::sqt(*words)));
            let up = parent(i).and_then(|p| worlds.get(p).copied().flatten());
            worlds.push(bind(i).or_else(|| Some(up? * local)));
        }
        let base = (0..reference.len())
            .filter(|&i| crate::animation::FACE_JOINTS.iter().any(|f| f.eq_ignore_ascii_case(name(i))))
            .filter_map(|i| Some((name(i).to_owned(), words(worlds[parent(i)?]?.inverse() * bind(i)?))))
            .collect();
        poser.evaluator.frames.set_base(base);
        Ok((poser, clip))
    }
    fn from_banks(gender: &str, banks: Vec<Arc<Bank>>, skater_bones: &[String]) -> Result<Self, String> {
        // Identities only label decoded-frame caches; editor clips are never cached.
        let identities = banks.iter().map(|_| String::from("cac")).collect();
        let evaluator = PoseEvaluator::from_banks(&AnimationBanks { banks, identities })?;
        let mut poser = Self { gender: gender.into(), evaluator, map: Vec::new() };
        poser.remap(skater_bones);
        Ok(poser)
    }
    /// Pose these bones from now on (a cast member's: the skater's, then its model's extra joints). Returns the
    /// ones the rig lacks.
    pub(crate) fn remap(&mut self, bones: &[String]) -> Vec<String> {
        let names = &self.evaluator.frames.bone_names;
        self.map = bones.iter().map(|b| names.iter().position(|n| n.eq_ignore_ascii_case(b))).collect();
        bones.iter().zip(&self.map).filter(|(_, m)| m.is_none()).map(|(b, _)| b.clone()).collect()
    }
    pub(crate) fn length(&self, clip: &str) -> Option<f32> {
        let c = self.evaluator.frames.clip(clip).ok()?;
        let fps = f32::from_bits(c.fps_bits);
        (fps > 0.).then(|| c.frames.len().saturating_sub(1) as f32 / fps)
    }

    /// `clip` at `time` on the skater's skeleton; bones the editor rig lacks keep `fallback`. An extra joint (past
    /// `fallback`) the rig lacks is NaN, which [`crate::animation::AnimationStatus::pose_transforms`] skips: it
    /// keeps its transform.
    pub(crate) fn pose(&self, clip: &str, time: f32, looping: bool, fallback: &[NativeMatrix]) -> Option<Vec<Mat4>> {
        self.pose_onto(None, clip, time, looping, fallback)
    }
    /// [`Poser::pose`] with the clip added onto the bank's `reference` pose, for a bank whose clips are deltas on it
    /// (VBR: Skate 3's pedestrian bank), as gameplay adds the skater's onto RIG_TPOSE (AddBindPose 82B98118). RAW
    /// banks (the editor's, Skate 2's scenes) decode whole: None.
    pub(crate) fn pose_onto(
        &self,
        reference: Option<&str>,
        clip: &str,
        time: f32,
        looping: bool,
        fallback: &[NativeMatrix],
    ) -> Option<Vec<Mat4>> {
        let mut commands = vec![PoseCommand::Clip { name: clip.to_owned(), time, previous_time: time, loops: looping as u32 as _ }];
        if let Some(name) = reference {
            commands.extend([PoseCommand::Pose { name: name.to_owned() }, PoseCommand::Add { motion_is_a: true }]);
        }
        let matrices = self.evaluator.evaluate(&commands).and_then(|p| self.evaluator.hierarchy(&p)).ok()?;
        Some(
            self.map
                .iter()
                .enumerate()
                .map(|(i, m)| m.map(|m| matrices[m]).or(fallback.get(i).copied()).map_or(Mat4::NAN, crate::animation::native_matrix))
                .collect(),
        )
    }
}

#[derive(Resource, Default)]
pub(crate) struct CasPose {
    poser: Option<Poser>,
    /// Skate 2's editor clips, for the Skate 2 rooms (none without its bank).
    skate2: Option<Poser>,
    failed: bool,
    time: f32,
    /// Gesture preview playing once, then back to the stand.
    gesture: Option<String>,
    /// How far the board has turned over for Grip Color ([`flip_step`]).
    flip: f32,
    /// The posers being loaded off the main thread (their gender, the worker): see [`apply`].
    #[allow(clippy::type_complexity)]
    loading: Option<(String, std::thread::JoinHandle<Result<(Poser, Option<Poser>), String>>)>,
}
impl CasPose {
    /// Preview a gesture by its engine label (Edit Style rows).
    pub(crate) fn preview(&mut self, label: &str) {
        if let Some((_, _, stem)) = GESTURES.iter().find(|g| g.0 == label) {
            self.gesture = Some(stem.to_string());
            self.time = 0.;
        }
    }
}

/// Not retail (Skate 2 had no grip page): the Skate 2 room turns the board over for Grip Color, over this long.
const FLIP_SECONDS: f32 = 0.4;

/// The board's turn toward the grip side (0 graphic side up to the camera, 1 grip side) after `dt` more seconds.
fn flip_step(progress: f32, grip: bool, dt: f32) -> f32 {
    let step = dt / FLIP_SECONDS;
    progress + (f32::from(u8::from(grip)) - progress).clamp(-step, step)
}

/// Turns the board (`bones`: SKATEBOARD_ROOT and every bone under it) by `angle` about the board's long axis (its
/// local Z, through its root), in the pose's own space.
fn turn_board(pose: &mut [Mat4], board: usize, bones: &[usize], angle: f32) {
    let Some(root) = pose.get(board).copied() else { return };
    let at = root.w_axis.truncate();
    let turn = Mat4::from_translation(at) * Mat4::from_axis_angle(root.z_axis.truncate().normalize(), angle) * Mat4::from_translation(-at);
    // Each board part binds the same bones: turn each one once.
    for bone in bones.iter().copied().collect::<std::collections::BTreeSet<_>>() {
        if let Some(m) = pose.get_mut(bone) {
            *m = turn * *m;
        }
    }
}

pub(crate) struct CasPosePlugin;
impl Plugin for CasPosePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CasPose>().add_systems(
            Update,
            apply.after(crate::animation::present).in_set(crate::app::FrameSet::Animation),
        );
    }
}

fn stand(profile: &serde_json::Value) -> &'static str {
    // Engine posture order: default, stiff, slouch, buff (retail Default,
    // Straight, Hunched, Macho).
    match profile["posture"].as_u64().unwrap_or(0) {
        1 => "CAS_STAND_STIFF",
        2 => "CAS_STAND_SLOUCH",
        3 => "CAS_STAND_BUFF",
        _ => "CAS_STAND",
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply(
    mut state: ResMut<CasPose>,
    customiser: Res<Customiser>,
    config: Res<Config>,
    skater: Res<SkaterRuntime>,
    animation: Res<crate::animation::AnimationStatus>,
    real: Res<Time<Real>>,
    history: Res<crate::presentation::Presentation>,
    mut transforms: ParamSet<(Query<&mut Transform>, Query<&mut Transform, With<crate::world::PlayerRoot>>)>,
    net: Option<Res<crate::multiplayer::Multiplayer>>,
    (skate2, menu, creator): (
        Option<Res<crate::cas_room::Skate2Cas>>,
        Option<Res<crate::graphics_menu::Menu>>,
        Option<Res<crate::graphic_creator::Creator>>,
    ),
) {
    // The poser's banks load on a worker. Offline the open waits for them (the world is paused); online (user 10-16)
    // they load in the background from the session's start, so Edit Skater opens without a stall (the skater keeps
    // its stance until they're in, if opened sooner).
    let online = net.is_some_and(|n| n.active());
    let gender = customiser.gender().to_owned();
    let needed = state.poser.as_ref().is_none_or(|p| p.gender != gender) && !state.failed && animation.ready;
    if needed && (online || customiser.open) && state.loading.as_ref().is_none_or(|(g, _)| *g != gender) {
        let (root, bones, g) = (config.asset_root.clone(), skater.animation.evaluator.frames.bone_names.clone(), gender.clone());
        state.loading = Some((
            gender.clone(),
            std::thread::spawn(move || Poser::load(&root, &g, &bones).map(|p| (p, Poser::skate2(&root, &g, &bones).ok()))),
        ));
    }
    let wait = customiser.open && !online;
    if let Some((loaded, worker)) = state.loading.take_if(|(_, w)| w.is_finished() || wait) {
        match worker.join().unwrap_or_else(|_| Err("the loader panicked".into())) {
            Ok((poser, skate2)) if loaded == gender => (state.poser, state.skate2) = (Some(poser), skate2),
            Ok(_) => {}
            Err(error) => {
                warn!("Edit Skater poses unavailable: {error}");
                state.failed = true;
            }
        }
    }
    // From the frame Edit Skater opens, while the original screen still loads too (10-11: the skater stood in its
    // riding pose for that second, then snapped); only the classic fallback panel keeps the riding pose.
    if !customiser.open || (!customiser.original && customiser.fallback) || !animation.ready {
        state.gesture = None;
        state.time = 0.;
        state.flip = 0.;
        return;
    }
    // A Skate 2 room puts the skater where Skate 2 does for the page (cas_room::Skate2Cas): couch, table or standing.
    let shot = crate::cas_room::skate2_shot_now(&customiser, creator.is_some_and(|c| c.is_open()));
    let stem = menu.as_ref().map_or(crate::graphics_menu::ROOMS[0].1, |m| m.edit_skater_room());
    let skate2 = history.pair().zip(skate2.as_ref())
        .and_then(|((_, current), cas)| cas.view(stem, &current.root, shot))
        .filter(|_| state.skate2.is_some());
    // Otherwise stand on the room's spot, upright wherever gameplay paused (on a ramp, mid-air lean), turned by Rotate
    // Skater and the page's framing: retail turns the skater, not the camera. The spot and heading are the camera
    // root's (cas_room places the floor there); gameplay draws PlayerRoot between the last two physics steps, which
    // sat the skater above or below the floor when Edit Skater opened mid-fall (10-11). Republished every frame, so
    // this never accumulates.
    let turn = customiser.preview_camera().2;
    if let Some(view) = &skate2 {
        let turn = crate::cas_room::skate2_turn(&customiser, shot);
        for mut root in &mut transforms.p1() {
            root.translation = view.feet;
            root.rotation = view.facing * Quat::from_rotation_y(-turn);
        }
    } else if let Some((_, current)) = history.pair() {
        let (yaw, _, _) = current.root.rotation.to_euler(EulerRot::YXZ);
        for mut root in &mut transforms.p1() {
            root.translation = current.root.translation;
            root.rotation = Quat::from_rotation_y(yaw - turn);
        }
    }
    state.time += real.delta_secs().min(0.1);
    let (clip, looping) = match (state.gesture.clone(), &skate2) {
        (Some(stem), _) => (format!("CAC_{}_{}", gender.to_uppercase(), stem.to_uppercase()), false),
        (None, Some(view)) => (view.clip.clone(), true),
        (None, None) => (stand(&customiser.draft).to_owned(), true),
    };
    // Gesture previews stay Skate 3's clips (its gesture list), played on Skate 2's standing spot.
    let poser = if skate2.is_some() && state.gesture.is_none() { state.skate2.as_ref() } else { state.poser.as_ref() };
    let Some(poser) = poser else { return };
    let Some(length) = poser.length(&clip) else {
        state.gesture = None;
        return;
    };
    let mut time = state.time;
    if time > length {
        if looping {
            time %= length.max(1e-3);
        } else {
            state.gesture = None;
            state.time = 0.;
            return;
        }
    }
    let Some(mut pose) = poser.pose(&clip, time, looping, &skater.render_pose) else { return };
    // The Skate 2 room turns the board over for Grip Color, and back after.
    state.flip = flip_step(state.flip, customiser.grip && skate2.is_some(), real.delta_secs().min(0.1));
    let board = skater.animation.evaluator.frames.bone_names.iter().position(|n| n == "SKATEBOARD_ROOT");
    if let Some(board) = board.filter(|_| state.flip > 0.) {
        let eased = state.flip * state.flip * (3. - 2. * state.flip);
        turn_board(&mut pose, board, &animation.subtree(board), std::f32::consts::PI * eased);
    }
    for (entity, transform) in animation.pose_transforms(&pose) {
        if let Ok(mut node) = transforms.p0().get_mut(entity) {
            *node = transform;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Not retail (user 10-31): Grip Color in the Skate 2 room turns the board over about its long axis, the trucks and
    /// wheels with it, and leaves the skater alone.
    #[test]
    fn the_board_turns_over_about_its_long_axis() {
        let root = Mat4::from_translation(Vec3::new(1., 2., 3.));
        let truck = root * Mat4::from_translation(Vec3::new(0.1, 0., 0.3));
        let hand = Mat4::from_translation(Vec3::new(1.2, 2.4, 3.));
        let mut pose = [root, truck, hand];
        turn_board(&mut pose, 0, &[0, 1], std::f32::consts::PI);
        assert!(pose[0].w_axis.abs_diff_eq(root.w_axis, 1e-5), "the board stays where it is");
        assert!(pose[0].y_axis.truncate().abs_diff_eq(-Vec3::Y, 1e-5), "the other face up");
        assert!(pose[0].z_axis.truncate().abs_diff_eq(Vec3::Z, 1e-5), "along the same line");
        assert!(pose[1].w_axis.truncate().abs_diff_eq(Vec3::new(0.9, 2., 3.3), 1e-5), "the truck goes over with it");
        assert_eq!(pose[2], hand, "the skater's hand is not the board's");
        // Playtest 10-31: the deck, truck and wheel parts each bind the board's bones, so a bone comes more than once;
        // it still turns over once (it spun and stopped askew).
        let mut twice = [root, truck, hand];
        turn_board(&mut twice, 0, &[0, 1, 0, 1, 0], std::f32::consts::PI);
        assert_eq!(&twice[..2], &pose[..2], "one half turn");
    }

    /// The turn takes FLIP_SECONDS each way and stops at either side.
    #[test]
    fn the_flip_takes_its_time_each_way() {
        let half = flip_step(0., true, FLIP_SECONDS / 2.);
        assert!((half - 0.5).abs() < 1e-5, "{half}");
        assert_eq!(flip_step(half, true, FLIP_SECONDS), 1., "over, and no further");
        assert!((flip_step(1., false, FLIP_SECONDS / 4.) - 0.75).abs() < 1e-5, "back when the page closes");
        assert_eq!(flip_step(0.1, false, 1.), 0.);
    }

    /// Needs the installed assets (SKATE3_ASSET_ROOT); not in CI.
    #[test]
    #[ignore]
    fn editor_stand_holds_the_board_and_postures_differ() {
        let root = std::path::PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap());
        let skater = Bank::load(&root.join("private/stock/data/anim/OffBoard.abin")).unwrap();
        let bones = skater.hierarchy().unwrap().bone_names.clone();
        let at = |name: &str| bones.iter().position(|b| b == name).unwrap();
        for gender in ["male", "female"] {
            let poser = Poser::load(&root, gender, &bones).unwrap();
            assert!(poser.map.iter().all(Option::is_some), "every skater bone exists in the editor rig");
            let pose = |clip: &str| {
                let t = poser.length(clip).unwrap() * 0.5;
                let m = poser.evaluator.hierarchy(&poser.evaluator.evaluate(&[PoseCommand::Clip { name: clip.into(), time: t, previous_time: t, loops: 1 }]).unwrap()).unwrap();
                poser.map.iter().map(|i| crate::animation::native_matrix(m[i.unwrap()])).collect::<Vec<Mat4>>()
            };
            let stand = pose("CAS_STAND");
            let p = |b: &str| stand[at(b)].w_axis.truncate();
            assert!(p("HIPS").y > 0.8 && p("HIPS").y < 1.0, "{gender} stands upright ({:?})", p("HIPS"));
            assert!(p("LEFTFOOT").y < 0.2 && p("RIGHTFOOT").y < 0.2, "{gender} feet on the ground");
            // As captured: the deck (long axis local Z) upright at the right side, hand on top.
            let long = stand[at("SKATEBOARD_ROOT")].z_axis.truncate().normalize();
            assert!(long.y.abs() > 0.9, "{gender} carries the board upright ({long:?})");
            let (board, hand) = (p("SKATEBOARD_ROOT"), p("RIGHTHAND"));
            assert!((board.x - hand.x).abs() < 0.2 && hand.y - board.y > 0.3 && hand.y - board.y < 0.6,
                "{gender} board hangs from the right hand ({board:?} vs {hand:?})");
            assert_ne!(pose("CAS_STAND_SLOUCH")[at("SPINE2")], stand[at("SPINE2")], "postures change the pose");
            // Gesture banks keep posture deltas before the CAC_<GENDER> reference pose.
            let pump = pose(&format!("CAC_{}_PUMPHIGH", gender.to_uppercase()));
            let q = |b: &str| pump[at(b)].w_axis.truncate();
            assert!(q("HIPS").y > 0.8 && q("LEFTFOOT").y < 0.2, "{gender} gesture stands ({:?})", q("HIPS"));
            assert!(q("RIGHTHAND").y > q("HEAD").y, "{gender} pump high raises the hand ({:?})", q("RIGHTHAND"));
        }
    }

    /// Skate 2's editor bank (old_cas.py) drives the skater whole: it stands in CAS_STAND, sits in CAS_SITTING facing
    /// away from the couch's back (its body row has no turn, and in Skate 2's room the backrest is at z 1.2-1.6 behind
    /// the seat's z 0.93, so the knees point -Z) and is up off the floor on Pete's table in CAS_ONTABLE. Needs
    /// SKATE3_ASSET_ROOT.
    #[test]
    #[ignore]
    fn skate2_editor_clips_stand_sit_and_take_the_table() {
        let root = std::path::PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap());
        let skater = Bank::load(&root.join("private/stock/data/anim/OffBoard.abin")).unwrap();
        let bones = skater.hierarchy().unwrap().bone_names.clone();
        let at = |name: &str| bones.iter().position(|b| b == name).unwrap();
        for gender in ["male", "female"] {
            let poser = Poser::skate2(&root, gender, &bones).unwrap();
            let pose = |clip: &str| poser.pose(clip, poser.length(clip).unwrap() * 0.5, true, &[]).unwrap();
            let (stand, sit, table) = (pose("CAS_STAND"), pose("CAS_SITTING"), pose("CAS_ONTABLE"));
            let p = |pose: &Vec<Mat4>, b: &str| pose[at(b)].w_axis.truncate();
            assert!(p(&stand, "HIPS").y > 0.8 && p(&stand, "HIPS").y < 1.05, "{gender} stands ({:?})", p(&stand, "HIPS"));
            assert!(p(&stand, "LEFTFOOT").y < 0.2 && p(&stand, "RIGHTFOOT").y < 0.2, "{gender} feet on the floor");
            let (hips, knee, head) = (p(&sit, "HIPS"), p(&sit, "LEFTLEG"), p(&sit, "HEAD"));
            assert!(hips.y > 0.3 && hips.y < 0.75 && head.y > hips.y + 0.4, "{gender} sits up on the seat ({hips:?}, {head:?})");
            assert!(knee.z < hips.z - 0.2, "{gender} faces -Z, away from the backrest ({hips:?}, knee {knee:?})");
            assert!(p(&table, "HIPS").y > 0.5, "{gender} is up on the table ({:?})", p(&table, "HIPS"));
        }
    }

    /// Edit Skaters' portrait poses (team_profile_00-04 -> CAS_Single_Pose_5/2/3/7/8, traced by prepare_boot.py) are
    /// in the editor bank: each stands, and faces the way CAS_STAND does. Needs SKATE3_ASSET_ROOT.
    #[test]
    #[ignore]
    fn photo_poses_are_in_the_editor_bank() {
        let root = std::path::PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap());
        let skater = Bank::load(&root.join("private/stock/data/anim/OffBoard.abin")).unwrap();
        let bones = skater.hierarchy().unwrap().bone_names.clone();
        let at = |name: &str| bones.iter().position(|b| b == name).unwrap();
        for gender in ["male", "female"] {
            let poser = Poser::editor(&root, gender, &bones).unwrap();
            let facing = |clip: &str| {
                let pose = poser.pose(clip, 0., false, &[]).unwrap_or_else(|| panic!("{gender} {clip} missing"));
                let p = |b: &str| pose[at(b)].w_axis.truncate();
                assert!(p("HIPS").y > 0.6 && p("HEAD").y > 1.3, "{gender} {clip} stands ({:?}, {:?})", p("HIPS"), p("HEAD"));
                let forward = Vec3::Y.cross(p("RIGHTUPLEG") - p("LEFTUPLEG")).normalize();
                eprintln!("{gender} {clip}: hips {:?} head {:?} forward {forward:?}", p("HIPS"), p("HEAD"));
                forward
            };
            let stand = facing("CAS_STAND");
            for n in [5, 2, 3, 7, 8] {
                let forward = facing(&format!("CAS_SINGLE_POSE_{n}"));
                assert!(forward.dot(stand) > 0.5, "{gender} pose {n} turns away from the stand ({forward:?} vs {stand:?})");
            }
        }
    }
}

/// A native-space local as a bank sample (scale, quaternion XYZW, translation): sqt_to_matrix's inverse.
fn words(local: Mat4) -> SampleWords {
    let (s, q, t) = local.to_scale_rotation_translation();
    [s.x, s.y, s.z, q.x, q.y, q.z, q.w, t.x, t.y, t.z].map(f32::to_bits)
}
