//! Skate 2 scenes, played as Skate 2 plays them. scene_000.bin gives a scene its origin and actor tracks (a
//! character and a scene/anim bank holding its one clip); the scene's ICE camera take, evaluated by
//! skate2port/old_tutorial.py, is `scene/camera/<scene>.json` (tutorial_recovery.md, "Cameras").
//!
//! - A scene with an origin is authored on the ground about it: the origin is snapped down to the ground (as the
//!   exe does for the camera), and the actors and a scene-space camera hang off that point.
//! - A scene at origin (0, 0, 0) is in world space: an actor's TRAJECTORY bone carries its world placement.
//! - `current_player` is the player's own skater (the Female take for a female skater), posed over gameplay's pose.
//! - The camera drives the gameplay camera (eye, look, roll, fov), its TIME_SCALE the scene's clock, its fisheye,
//!   tonemap, bloom and sun overrides the map's post ([`crate::retail_render::ShotPost`]) and its OVERLAY the HUD
//!   overlay ([`Cutscene::overlay`]).
//!
//! Scene time runs on the clock Skate 2 and Skate 1 run ([`Clock`]). The queue calls [`Cutscene::start`] when a scene
//! begins and [`Cutscene::stop`] when it ends ([`real_length`]); a highlight ([`Cutscene::highlight`]) ends itself.
//! [`Cutscene::actors`] lists the cast members the scene needs, so the binding can put them in `Cast.wanted`.
use crate::{
    animation::AnimationStatus,
    camera::GameplayCamera,
    cas_pose::Poser,
    challenge::movie::read_wav,
    physics::{GamePhysics, SkaterRuntime},
    skate2_cast::Cast,
    skate_audio::StreamPcm,
    world::PlayerRoot,
};
use bevy::{
    mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes},
    prelude::*,
};
use serde::Deserialize;
use skate_core::{
    animation::{output::NativeMatrix, playback_tree::PoseCommand},
    math::Vector3,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
struct Scenes {
    scenes: BTreeMap<String, SceneData>,
}
#[derive(Deserialize)]
struct SceneData {
    origin: [f32; 3],
    /// The record's length: where a scene without a camera ends (nis-clock.md 2, "no sync").
    #[serde(default)]
    duration: f32,
    /// Yaw (radians) the scene is turned by about its origin: a NIS definition placing a scene facing another way
    /// (the Death Race intros). 0 for the tutorial's scenes.
    #[serde(default)]
    turn: f32,
    tracks: Vec<TrackData>,
    camera: Option<String>,
    /// The streams that play with the scene from its start (the online NIS's post.big SFX bed, tools/skate3_online.py).
    #[serde(default)]
    audio: Vec<SceneAudio>,
    /// SceneManagerImpl's screen transition around the cut (tools/skate3_online.py `nis_transition`); none in the
    /// Skate 2 / Skate 1 data.
    #[serde(default)]
    transition: Option<Transition>,
    /// Skate 1's OVERLAY track (channel 101; old_online1.py keeps it out of the camera): its 2..5 are the same SpotFound
    /// corners as Skate 2's (item10-flashes.md D1).
    #[serde(default)]
    s1_overlay: Vec<(f32, u8)>,
    /// The scene's own `.sub` cues [from, to, text] in scene seconds (the Skate 3 career export, spec 15.5).
    #[serde(default)]
    subtitles: Vec<Cue>,
}
#[derive(Deserialize, Clone)]
#[serde(untagged)]
enum Cue {
    Line(f32, f32, String),
    /// Skate 2's and Skate 1's string-id cues ({start, end, label}): their modes show them themselves.
    Labelled(#[allow(dead_code)] serde::de::IgnoredAny),
}
#[derive(Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
enum Transition {
    /// ColorFadeTransitionEffect type 1, colour (0, 0, 0).
    Black,
    /// BloomToWhiteTransitionEffect ([`crate::retail_render::ScreenFade`]).
    Bloom,
}
/// Each transition leg's length (TU3 0x82314d90).
const TRANSITION_SECONDS: f32 = 1.;
#[derive(Deserialize)]
struct SceneAudio {
    audio: String,
}
#[derive(Deserialize)]
struct TrackData {
    character: String,
    anim: String,
    /// A Skate 1 track's clip (P1 old_scenes1.py; `anim` is then the disc bank's name, not exported).
    #[serde(default)]
    clip1: Option<Clip1Meta>,
}
/// A Skate 1 actor track's clip (P1 s1abin `export_clip`: the track's `clip1`).
#[derive(Deserialize, Clone)]
pub(crate) struct Clip1Meta {
    /// `anim1/<CLIP>.bin`, beside the scenes' tutorial.json.
    pub(crate) file: String,
    pub(crate) fps: f32,
    pub(crate) frames: usize,
    pub(crate) bones: Vec<String>,
    /// Each bone's parent (-1: a root; parents come first).
    pub(crate) parents: Vec<i32>,
}
/// Its frames: `frames` x `bones` locals [rotation xyzw, translation xyz, scale xyz] (LE f32, s1abin `write_clip`),
/// already the bank's reference o the clip (research.md 4.3a "Bind compose").
pub(crate) struct Clip1 {
    meta: Clip1Meta,
    locals: Vec<[f32; 10]>,
    /// Target bone -> clip bone ([`Clip1::remap`]).
    map: Vec<Option<usize>>,
}
impl Clip1 {
    pub(crate) fn load(dir: &Path, meta: Clip1Meta) -> Result<Self, String> {
        let bytes = std::fs::read(dir.join(&meta.file)).map_err(|e| format!("{}: {e}", meta.file))?;
        let n = meta.bones.len();
        let parents = meta.parents.len() == n && meta.parents.iter().enumerate().all(|(i, &p)| p < i as i32);
        if meta.frames == 0 || !(meta.fps > 0.) || !parents || bytes.len() != meta.frames * n * 40 {
            return Err(format!("{}: {} bytes for {} frames of {n} bones", meta.file, bytes.len(), meta.frames));
        }
        let locals = bytes
            .chunks_exact(40)
            .map(|c| std::array::from_fn(|k| f32::from_le_bytes(c[4 * k..4 * k + 4].try_into().unwrap())))
            .collect();
        Ok(Self { map: (0..n).map(Some).collect(), meta, locals })
    }
    /// Pose these bones from now on, by name (case-insensitive); returns the ones the clip lacks.
    pub(crate) fn remap(&mut self, bones: &[String]) -> Vec<String> {
        let names = &self.meta.bones;
        self.map = bones.iter().map(|b| names.iter().position(|n| n.eq_ignore_ascii_case(b))).collect();
        bones.iter().zip(&self.map).filter(|(_, m)| m.is_none()).map(|(b, _)| b.clone()).collect()
    }
    /// The last frame's time.
    pub(crate) fn length(&self) -> f32 {
        (self.meta.frames - 1) as f32 / self.meta.fps
    }
    /// The clip at `time` (retail sampler 0x8225ec98, research.md 4.3a: frame = fps x t clamped to the last, B = A + 1
    /// clamped, translation and scale lerped, rotation nlerped on the shorter arc), composed down its parents, on
    /// [`Clip1::remap`]'s bones; one the clip lacks takes `fallback`, else NaN (kept by `pose_transforms`).
    /// ponytail: retail's weight is u16/65536 (U14); f32 here.
    pub(crate) fn pose(&self, time: f32, fallback: &[NativeMatrix]) -> Vec<Mat4> {
        let (n, last) = (self.meta.bones.len(), self.meta.frames - 1);
        let f = (self.meta.fps * time.max(0.)).min(last as f32);
        let (a, w) = (f as usize, f.fract());
        let b = (a + 1).min(last);
        let mut worlds = Vec::<Mat4>::with_capacity(n);
        for i in 0..n {
            let (p, q) = (self.locals[a * n + i], self.locals[b * n + i]);
            // glam's Quat::lerp is retail's nlerp: the shorter arc, normalised.
            let r = Quat::from_array([p[0], p[1], p[2], p[3]]).lerp(Quat::from_array([q[0], q[1], q[2], q[3]]), w);
            let t = Vec3::new(p[4], p[5], p[6]).lerp(Vec3::new(q[4], q[5], q[6]), w);
            let s = Vec3::new(p[7], p[8], p[9]).lerp(Vec3::new(q[7], q[8], q[9]), w);
            let local = Mat4::from_scale_rotation_translation(s, r, t);
            worlds.push(usize::try_from(self.meta.parents[i]).map_or(local, |p| worlds[p] * local));
        }
        self.map
            .iter()
            .enumerate()
            .map(|(i, m)| m.map(|m| worlds[m]).or_else(|| fallback.get(i).copied().map(crate::animation::native_matrix)).unwrap_or(Mat4::NAN))
            .collect()
    }
}
/// An actor's clip: a scene bank (Skate 2, the online intros) or a Skate 1 anim1 clip.
enum Anim {
    Bank(Poser, String),
    Skate1(Clip1),
}
impl Anim {
    fn remap(&mut self, bones: &[String]) -> Vec<String> {
        match self {
            Anim::Bank(poser, _) => poser.remap(bones),
            Anim::Skate1(clip) => clip.remap(bones),
        }
    }
    /// The clip at `time`, its last frame held.
    fn pose(&self, time: f32, rest: &[NativeMatrix]) -> Option<Vec<Mat4>> {
        match self {
            Anim::Bank(poser, clip) => poser.pose(clip, time.min(poser.length(clip).unwrap_or(0.)), false, rest),
            Anim::Skate1(clip) => Some(clip.pose(time, rest)),
        }
    }
}
/// A baked ICE camera: samples of [t, eye xyz, look xyz, roll, fov, time scale, fisheye, tonemap, bloom threshold,
/// bloom intensity, sun xyz] in engine axes (the post columns 0 where the shot doesn't override them), about the
/// space's base (1 world, 2 the player's hips, 3 the scene's ground-snapped origin). Two samples at one time are a cut.
#[derive(Deserialize)]
struct Camera {
    space: u8,
    /// The look point's space when it differs from the eye's (a world eye watching the player: 2).
    #[serde(default)]
    look_space: Option<u8>,
    #[serde(deserialize_with = "samples")]
    samples: Vec<[f32; 17]>,
    /// The ICE OVERLAY track: [t, HUD overlay id] at each change (0 Clear; 80/81 the ollie gesture, nis/gestollie).
    #[serde(default)]
    overlay: Vec<(f32, u8)>,
    /// The FREEZE keys [t, command, time in frames]: the scene's holds and a photo scene's snaps ([`Clock`]).
    #[serde(default)]
    freeze: Vec<(f32, f32, f32)>,
    /// An edit's shots [edit time, subtake start] (old_tutorial.py scene_camera): where each cut sets the actors.
    #[serde(default)]
    shots: Vec<(f32, f32)>,
    /// The Skate 3 career export's extra ICE columns (spec 15.7). Only the brightness dip is drawn; its DOF, SHADOW_*,
    /// NEAR_CLIP / CLIPPING_FAR and AVATAR_EYE rows are left unread (spec 18 row 4).
    #[serde(default)]
    effects: Effects,
}
#[derive(Deserialize, Default)]
struct Effects {
    /// [t, BRIGHTNESS_OVERRIDE, BRIGHTNESS_NEAR, BRIGHTNESS_FAR] keys (03_Reda_Leaves_Shingo's dip, 39.33-43.84 s).
    #[serde(default)]
    brightness: Vec<[f32; 4]>,
}
/// Camera rows; ones baked before the post columns keep the defaults (lens 1, no overrides).
fn samples<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<[f32; 17]>, D::Error> {
    let rows = Vec::<Vec<f32>>::deserialize(d)?;
    rows.into_iter()
        .map(|r| match r.len() {
            10..=17 => Ok(std::array::from_fn(|k| r.get(k).copied().unwrap_or(if k == 10 { 1. } else { 0. }))),
            n => Err(serde::de::Error::custom(format!("a camera row of {n} values"))),
        })
        .collect()
}
impl Camera {
    /// The sample at `t`: linear between neighbours, the last one after the end.
    fn at(&self, t: f32) -> [f32; 17] {
        let i = self.samples.partition_point(|s| s[0] <= t).max(1) - 1;
        let a = self.samples[i];
        let Some(b) = self.samples.get(i + 1).filter(|b| b[0] > a[0]) else { return a };
        let f = ((t - a[0]) / (b[0] - a[0])).clamp(0., 1.);
        std::array::from_fn(|k| a[k] + (b[k] - a[k]) * f)
    }
    /// The take over `length` seconds instead (nis-clock.md 2, RETAIL: a replay scene's take duration is overwritten
    /// at sync start, S1 0x824b0048 at 0x824b0138): every key keeps its place in the take.
    fn stretch(&mut self, length: f32) {
        let Some(end) = self.samples.last().map(|s| s[0]).filter(|&end| end > 0.) else { return };
        let k = length / end;
        self.samples.iter_mut().for_each(|s| s[0] *= k);
        self.overlay.iter_mut().for_each(|o| o.0 *= k);
        self.freeze.iter_mut().for_each(|f| f.0 *= k);
        self.shots.iter_mut().for_each(|s| s.0 *= k);
    }
}

/// The NIS clock as the Skate 2 and Skate 1 exes run it (nis-clock.md 1 and 6, RETAIL in both): scene time advances by
/// real dt x TIME_SCALE (the camera's where the scene is: the value its mover set last frame, 0.01..1) x freeze, and
/// TIME_SCALE goes nowhere else (never the game's speed). A FREEZE hold counts down in real time before the scene time
/// moves. Skate 3's scenes keep real time and set the game's speed, as before: its clock isn't traced (tu3.pe is gone,
/// user 10-14).
#[derive(Default)]
struct Clock {
    game: Game,
    time: f32,
    /// Real seconds the FREEZE hold has left (infinite for CMD 2).
    hold: f32,
    /// The FREEZE keys passed.
    keys: usize,
}
impl Clock {
    /// TIME_SCALE x freeze now.
    fn rate(&self, camera: Option<&Camera>) -> f32 {
        match camera {
            _ if self.hold > 0. => 0.,
            Some(camera) if self.game != Game::Skate3 => camera.at(self.time)[9].clamp(0.01, 1.),
            _ => 1.,
        }
    }
    fn tick(&mut self, dt: f32, camera: Option<&Camera>) {
        if self.hold > 0. {
            self.hold -= dt;
        }
        self.time += dt * self.rate(camera);
        let (Some(camera), true) = (camera, self.game != Game::Skate3) else { return };
        let time = self.time;
        for &(_, cmd, frames) in camera.freeze[self.keys..].iter().take_while(|f| f.0 <= time) {
            self.keys += 1;
            // CMD 4 and 5 also post the FE photo events ([`Cutscene::take_freeze`]); CMD 5's skater check
            // (0x8269fe40 != 64) is UNTRACED, taken as passing. ponytail: 6 (notify the challenge) and 7 (jump the
            // scene) are in no scene the port plays.
            match (cmd as u8, self.game) {
                // nis-clock.md 6 (RETAIL, S1 table 0x824bc0d0): Skate 1's CMD 5 does nothing.
                (5, Game::Skate1) => {}
                // TIME x f32 1/30 (fmuls, S1 0x824bc384 / S2 0x8259f39c), not TIME / 30: one ulp apart for 374 of
                // 0..600; at 60 Hz that moves a 21-frame hold by a frame.
                (1 | 3 | 4 | 5, _) => self.hold = frames * (1. / 30.),
                (2, _) => self.hold = f32::INFINITY,
                _ => {}
            }
        }
    }
}

/// Which game's rules a scene folder plays by, from its folder names.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Game {
    Skate1,
    #[default]
    Skate2,
    Skate3,
    /// The Skate 3 career's scenes (the career export, `skate3-career/scenes`).
    Skate3Career,
}
impl Game {
    pub(crate) fn of(root: &Path) -> Self {
        let named = |name: &str| root.components().any(|c| c.as_os_str() == name);
        match () {
            _ if named("skate3-career") => Game::Skate3Career,
            _ if named("skate3-online") => Game::Skate3,
            _ if named("challenges-skate1") || named("skate1-online") => Game::Skate1,
            _ => Game::Skate2,
        }
    }
    /// The music slot a scene's streams play on. nis-clock.md 4 (RETAIL): Skate 1's post channel plays them through
    /// the speech system (bank 34), so the Dialog slot (lead ruling 10-14: the Skate 1 online intro beds too). Skate 2
    /// and 3 do the same through bank 99 (TU3 post channel 0x8267E688 CMD 2 -> 0x82482D88 -> 0x824A6728; bug 10-17 #43).
    pub(crate) fn audio_slot(self) -> usize {
        crate::music::DIALOG
    }
    /// The cast model key a track's character uses.
    fn cast_key(self, character: &str) -> &str {
        match self {
            // Spec 15.6: the career's `_prop` characters are their own recipes (shingo_prop is not shingo).
            Game::Skate3Career => character,
            // The disc names a few characters by a prop variant of a cast member.
            _ => character.strip_suffix("_prop").unwrap_or(character),
        }
    }
    /// The female player's take of the player's bank `anim`.
    fn female_anim(self, anim: &str) -> String {
        let anim = anim.replace("_Player", "_Female");
        // Spec 15.6: Skate 3's player banks are _Male1 / _Female1.
        if self == Game::Skate3Career { anim.replace("_Male1", "_Female1") } else { anim }
    }
    /// The game's speed while the camera's sample `s` plays.
    fn speed(self, s: &[f32; 17]) -> f32 {
        // Skate 3's old clock: its TIME_SCALE slows the game. Skate 2's and Skate 1's slows only the scene ([`Clock`]),
        // and so does the career's (spec 15.4, INFERRED from the bed lengths, stage1_nis 3.4).
        if self != Game::Skate3 { 1. } else { s[9].clamp(0.05, 4.) }
    }
}

/// How long `scene` in `root`/tutorial.json runs, in real seconds at 60 Hz on its clock: to its camera's end (an edit's
/// tail key or a take's end, nis-clock.md 2), or its record's length without a camera. Neither the record's length nor
/// the audio ends a scene, so the bindings end theirs by this (read as a scene starts, as [`Cutscene::start`] reads it).
pub(crate) fn real_length(root: &Path, scene: &str) -> Option<f32> {
    let text = std::fs::read_to_string(root.join("tutorial.json")).ok()?;
    let data = serde_json::from_str::<Scenes>(&text).ok()?.scenes.remove(scene)?;
    let camera = data.camera.and_then(|path| serde_json::from_str::<Camera>(&std::fs::read_to_string(root.join(path)).ok()?).ok());
    let camera = camera.filter(|c| !c.samples.is_empty());
    let end = camera.as_ref().and_then(|c| c.samples.last()).map_or(data.duration, |s| s[0]);
    let (mut clock, mut real) = (Clock { game: Game::of(root), ..default() }, 0f32);
    // ponytail: a CMD 2 hold never lets go; 10 minutes ends it.
    while clock.time < end && real < 600. {
        clock.tick(1. / 60., camera.as_ref());
        real += 1. / 60.;
    }
    Some(real)
}

#[derive(Resource, Default)]
pub(crate) struct Cutscene {
    request: Option<Request>,
    running: Option<Running>,
    stop: bool,
    /// The skater's idle at rest: bones a scene rig lacks hold this (as the cast does).
    rest: Vec<NativeMatrix>,
    /// A Skate 3 scene runs the game at its camera's TIME_SCALE (its old clock); back to 1 when it ends.
    scaled: bool,
    /// A BloomToWhite transition holds the screen fade; back to 0 when it ends.
    bloomed: bool,
}

struct Request {
    root: PathBuf,
    scene: String,
    /// Whether the scene's audio plays with it ([`Cutscene::start_muted`]: the queue plays it itself).
    audio: bool,
    /// The take's length when it is overwritten ([`Cutscene::stretch`]).
    length: Option<f32>,
}

struct Running {
    /// The folder the scene came from (`private/skate2-online/intros`, ...).
    root: PathBuf,
    /// What the actors and a scene-space camera hang off: the origin on the ground, or zero for a world-space scene.
    base: Vec3,
    /// The scene's turn about `base` (actors and a scene-space camera).
    turn: Quat,
    clock: Clock,
    tracks: Vec<Track>,
    /// The player's own track.
    player: Option<Anim>,
    /// The player's face joints at bind, put back when the scene ends (gameplay never poses them).
    face_rest: Vec<(Entity, Transform)>,
    camera: Option<Camera>,
    /// The player's hips in world space, as gameplay presents them (a player-space camera's base).
    hips: Vec3,
    /// The player's presented root rotation (a Player-space (0) camera point turns with it).
    facing: Quat,
    /// Where the scene's clock ends ([`Cutscene::ended`]).
    end: f32,
    /// FREEZE keys already handed out by [`Cutscene::take_freeze`].
    frozen: usize,
    /// The scene's audio files, and what plays them (stopped with the scene).
    audio: Vec<PathBuf>,
    playing: Vec<Entity>,
    transition: Option<Transition>,
    subtitles: Vec<Cue>,
}

struct Track {
    key: String,
    anim: Anim,
    /// The cast model's TRAJECTORY bone (its own bone list), set at bind.
    trajectory: Option<usize>,
    /// The cast member's root, its bone bindings and its locator transform before the scene.
    bound: Option<(Entity, AnimationStatus, Transform)>,
    /// Every bound joint's transform before the scene: the stand doesn't pose the extra joints (face, fingers,
    /// twists), so they're put back at the end.
    joints: Vec<(Entity, Transform)>,
}

impl Running {
    fn tick(&mut self, dt: f32) {
        self.clock.tick(dt, self.camera.as_ref());
    }
    /// Where the actors are in their clips: an edit's cut sets them to the new shot's subtake start (nis-clock.md 5,
    /// RETAIL calls; that an actor samples its clip there is INFERRED), so each shot replays its window of the clip.
    fn actor_time(&self) -> f32 {
        let shots = self.camera.as_ref().map_or(&[][..], |c| &c.shots);
        let time = self.clock.time;
        shots.iter().rev().find(|s| s.0 <= time).map_or(time, |&(t0, start)| start + time - t0)
    }
    /// The camera's eye and look at sample `s`, each about its space (research.md 4.4a, S1 0x82c38130 RETAIL):
    /// 0 Player = the avatar's position and rotation, 1 World, 2 Hybrid = the avatar's position, 3 Scene = the
    /// ground-snapped origin, turned by the scene's turn. INFERRED (U10): the avatar's position is the presented hips
    /// (as space 2 has always been here), its rotation the presented root's.
    fn eye_look(&self, camera: &Camera, s: &[f32; 17]) -> (Vec3, Vec3) {
        let place = |space: u8, offset: Vec3| match space {
            0 => self.hips + self.facing * offset,
            1 => offset,
            2 => self.hips + offset,
            _ => self.base + self.turn * offset,
        };
        let look_space = camera.look_space.unwrap_or(camera.space);
        (place(camera.space, Vec3::new(s[1], s[2], s[3])), place(look_space, Vec3::new(s[4], s[5], s[6])))
    }
}

impl Cutscene {
    /// Begin `scene` from `root`/tutorial.json (root: private/skate2-tutorial), until [`Cutscene::stop`].
    /// The scene's audio plays with it.
    pub(crate) fn start(&mut self, root: &Path, scene: &str) {
        self.request = Some(Request { root: root.to_owned(), scene: scene.to_owned(), audio: true, length: None });
        self.stop = false;
    }
    /// [`Cutscene::start`] without the scene's audio, for a queue that plays it itself (the Skate 2 tutorial).
    pub(crate) fn start_muted(&mut self, root: &Path, scene: &str) {
        self.start(root, scene);
        self.request.as_mut().expect("just set").audio = false;
    }
    /// Run the starting scene's take over `length` seconds (a replay scene: its record duration overwrites the take's,
    /// nis-clock.md 2).
    pub(crate) fn stretch(&mut self, length: f32) {
        if let Some(request) = self.request.as_mut() {
            request.length = Some(length);
        }
    }
    /// Whether the running scene's clock has reached its end: its camera's last key (an edit's tail key or a take's
    /// end) or, without a camera, its record's length (nis-clock.md 2). It holds there until [`Cutscene::stop`].
    pub(crate) fn ended(&self) -> bool {
        self.running.as_ref().filter(|_| !self.stop).is_some_and(|r| r.clock.time >= r.end)
    }
    /// For tests outside this file: what [`play`] does to the clock, without a world (no actors, audio or ground).
    #[cfg(test)]
    pub(crate) fn run_for_test(&mut self, seconds: f32) {
        if std::mem::take(&mut self.stop) {
            self.running = None;
        }
        if let Some(request) = self.request.take() {
            let running = load(&request, &[], false, &[]).ok();
            self.running = running.filter(|r| !r.tracks.is_empty() || r.player.is_some() || r.camera.is_some());
        }
        for _ in 0..(seconds * 60.).round() as usize {
            if let Some(running) = self.running.as_mut() {
                running.tick(1. / 60.);
            }
        }
    }
    /// End the scene: actors go back to their locators and the cast idle, the player to gameplay.
    pub(crate) fn stop(&mut self) {
        self.request = None;
        self.stop = true;
    }
    /// The cast keys the running scene animates, with the matrix to spawn one at that isn't already placed
    /// (row-major, translation in row 3, as the locators): the scene's base, which its clip moves the actor from.
    pub(crate) fn actors(&self) -> Vec<(String, [[f32; 4]; 4])> {
        let Some(running) = self.running.as_ref().filter(|_| !self.stop) else { return Vec::new() };
        let o = running.base;
        let at = [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [o.x, o.y, o.z, 1.]];
        running.tracks.iter().map(|t| (t.key.clone(), at)).collect()
    }
    /// Whether a scene (or a highlight's camera) has the view.
    pub(crate) fn active(&self) -> bool {
        self.request.is_some() || self.running.is_some() && !self.stop
    }
    /// The folder the running scene came from (intro_overlay.rs: whose online intros).
    pub(crate) fn root(&self) -> Option<&Path> {
        self.running.as_ref().filter(|_| !self.stop).map(|r| r.root.as_path())
    }
    /// Whether the starting or running scene comes from `root`: an owner stops only its own scenes.
    pub(crate) fn plays_from(&self, root: &Path) -> bool {
        self.request.as_ref().is_some_and(|r| r.root == root) || self.root() == Some(root)
    }
    /// The HUD overlay the running scene's camera asks for now (0 = none).
    pub(crate) fn overlay(&self) -> u8 {
        let Some(running) = self.running.as_ref().filter(|_| !self.stop) else { return 0 };
        let steps = running.camera.as_ref().map_or(&[][..], |c| &c.overlay);
        steps.iter().rev().find(|(t, _)| *t <= running.clock.time).map_or(0, |&(_, id)| id)
    }
    /// The transition over the screen now, (black, bloom to white): its out leg, 1 -> 0 over the scene's first second.
    /// ponytail: retail runs the in leg (0 -> 1, 1 s) before the cut and cuts at full (0x82772078); the modes time a
    /// scene from [`Cutscene::start`], so the cut is at once and the scene opens at full.
    pub(crate) fn transition(&self) -> (f32, f32) {
        let Some(running) = self.running.as_ref().filter(|_| !self.stop) else { return (0., 0.) };
        let level = (1. - running.clock.time / TRANSITION_SECONDS).max(0.);
        match running.transition {
            Some(Transition::Black) => (level, 0.),
            Some(Transition::Bloom) => (0., level),
            None => (0., 0.),
        }
    }
    /// Whether the running scene's camera passed another photo FREEZE key (CMD 4/5, the FE photo events: nis-clock.md
    /// 6) since the last call.
    pub(crate) fn take_freeze(&mut self) -> bool {
        let Some(running) = self.running.as_mut().filter(|_| !self.stop) else { return false };
        // nis-clock.md 6 (RETAIL): only CMD 4 posts the FE event in Skate 1 (0x824bc0d0); Skate 2 posts 4 and 5.
        let game = running.clock.game;
        let snap = |f: &&(f32, f32, f32)| f.0 <= running.clock.time && (f.1 as u8 == 4 || f.1 as u8 == 5 && game != Game::Skate1);
        let due = running.camera.as_ref().map_or(0, |c| c.freeze.iter().filter(snap).count());
        let taken = due > running.frozen;
        running.frozen += taken as usize;
        taken
    }
    /// The running scene's time on its clock (subtitle cues are tenths of it: nis-clock.md 3).
    pub(crate) fn time(&self) -> f32 {
        self.running.as_ref().filter(|_| !self.stop).map_or(0., |r| r.clock.time)
    }
    /// How fast the running scene's clock runs now (TIME_SCALE x freeze): a replay-type scene's replay moves with it.
    pub(crate) fn rate(&self) -> f32 {
        self.running.as_ref().filter(|_| !self.stop).map_or(1., |r| r.clock.rate(r.camera.as_ref()))
    }
    /// The running scene's own subtitle cue now (spec 15.5), on its clock and cut at its end.
    pub(crate) fn subtitle(&self) -> Option<&str> {
        let running = self.running.as_ref().filter(|r| !self.stop && r.clock.time < r.end)?;
        let t = running.clock.time;
        running.subtitles.iter().find_map(|cue| match cue {
            Cue::Line(from, to, text) if *from <= t && t < *to => Some(text.as_str()),
            _ => None,
        })
    }
    /// The running scene's brightness now (its camera's BRIGHTNESS keys): 1 outside the keys, 0 black.
    pub(crate) fn brightness(&self) -> f32 {
        let Some(running) = self.running.as_ref().filter(|_| !self.stop) else { return 1. };
        let keys = running.camera.as_ref().map_or(&[][..], |c| &c.effects.brightness);
        let t = running.clock.time;
        let (Some(first), Some(last)) = (keys.first(), keys.last()) else { return 1. };
        if t < first[0] || t > last[0] {
            return 1.;
        }
        // INFERRED (spec 18 row 4): linear between keys, as the camera's samples; a second key at one time is a cut.
        let i = keys.partition_point(|k| k[0] <= t) - 1;
        let a = keys[i];
        let k = keys.get(i + 1).filter(|b| b[0] > a[0]).map_or(a, |b| {
            let f = (t - a[0]) / (b[0] - a[0]);
            std::array::from_fn(|n| a[n] + (b[n] - a[n]) * f)
        });
        if k[1] < 0.5 { 1. } else { (1. + (k[2] + k[3]) / 2.).clamp(0., 1.) }
    }
}

pub(crate) struct CutscenePlugin;
impl Plugin for CutscenePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Cutscene>().add_systems(
            Update,
            (
                play.after(crate::animation::present).in_set(crate::app::FrameSet::Animation),
                frame.after(crate::camera::present).after(play).before(crate::app::FrameSet::Verification),
            ),
        );
    }
}

fn load(request: &Request, bones: &[String], female: bool, face: &[(String, Mat4)]) -> Result<Running, String> {
    let (root, scene) = (&request.root, &request.scene);
    let text = std::fs::read_to_string(root.join("tutorial.json")).map_err(|e| e.to_string())?;
    let scenes: Scenes = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let data = scenes.scenes.get(scene).ok_or_else(|| format!("no scene {scene}"))?;
    let (mut tracks, mut player, game) = (Vec::<Track>::new(), None, Game::of(root));
    for track in &data.tracks {
        let key = game.cast_key(&track.character);
        let skate1 = track.clip1.clone().map(|meta| Clip1::load(root, meta).map(Anim::Skate1));
        if key == "current_player" {
            let anim = match skate1 {
                // UNTRACED (U4): Skate 1's player face base (SkaterFacePres). The clip is composed on its bank's own
                // reference, so it poses the skater's bones only and the face keeps its bind. Skate 1 has no female
                // skater, so every player takes this one take.
                Some(anim) => anim.map(|mut anim| {
                    anim.remap(bones);
                    anim
                }),
                None => {
                    let female_anim = game.female_anim(&track.anim);
                    let anim = if female && root.join(&female_anim).exists() { &female_anim } else { &track.anim };
                    // Retail 0x826c0550: the player's face takes its base from the head, the clip's keys on top.
                    Poser::scene_with_face(&root.join(anim), &crate::animation::with_face(bones), face).map(|(p, c)| Anim::Bank(p, c))
                }
            };
            match anim {
                Ok(anim) => player = Some(anim),
                Err(error) => warn!("cutscene {scene}: the player keeps gameplay's pose ({error})"),
            }
            continue;
        }
        // Skate 2's cast is in the scenes' folder, Skate 1's beside it (challenges-skate1/cast, `nis/` holding the scenes).
        let model = format!("cast/{key}.glb");
        if !root.join(&model).exists() && !root.parent().is_some_and(|p| p.join(&model).exists()) {
            warn!("cutscene {scene}: no cast model for {key}");
            continue;
        }
        // A character in several tracks is that many people (outro_4's four skaters): key, key#1, key#2...
        let n = tracks.iter().filter(|t| t.key.split('#').next() == Some(key)).count();
        let key = if n == 0 { key.to_owned() } else { format!("{key}#{n}") };
        let anim = skate1.unwrap_or_else(|| Poser::scene(&root.join(&track.anim), bones).map(|(p, c)| Anim::Bank(p, c)));
        match anim {
            Ok(anim) => tracks.push(Track { key, anim, trajectory: None, bound: None, joints: Vec::new() }),
            Err(error) => warn!("cutscene {scene}: {key} keeps its idle ({error})"),
        }
    }
    let mut camera = data.camera.as_ref().and_then(|path| {
        let camera = std::fs::read_to_string(root.join(path))
            .map_err(|e| e.to_string())
            .and_then(|text| serde_json::from_str::<Camera>(&text).map_err(|e| e.to_string()));
        camera.inspect_err(|error| warn!("Skate 2 cutscene {scene}: no camera ({error})")).ok().filter(|c| !c.samples.is_empty())
    });
    if let Some(camera) = camera.as_mut().filter(|c| c.overlay.is_empty()) {
        camera.overlay.clone_from(&data.s1_overlay);
    }
    if let (Some(camera), Some(length)) = (camera.as_mut(), request.length) {
        camera.stretch(length);
    }
    // The expression real_length uses.
    let end = camera.as_ref().and_then(|c| c.samples.last()).map_or(data.duration, |s| s[0]);
    Ok(Running {
        root: root.clone(),
        base: Vec3::from_array(data.origin),
        turn: Quat::from_rotation_y(data.turn),
        clock: Clock { game, ..default() },
        tracks,
        player,
        face_rest: Vec::new(),
        camera,
        hips: Vec3::ZERO,
        facing: Quat::IDENTITY,
        end,
        frozen: 0,
        // Each stream once (bug 10-17 #43): Skate 3's online intros list their bed once per take, all at 0, which
        // played it up to 5x over (+14 dB).
        audio: if request.audio {
            data.audio.iter().map(|a| root.join(&a.audio)).collect::<std::collections::BTreeSet<_>>().into_iter().collect()
        } else {
            Vec::new()
        },
        playing: Vec::new(),
        transition: data.transition,
        subtitles: data.subtitles.clone(),
    })
}

/// The ground under `at`, a scene origin.
pub(crate) fn ground(physics: &GamePhysics, at: Vec3) -> Option<Vec3> {
    let hit = physics.world().query_thin_line(Vector3::new(at.x, at.y + 1., at.z), Vector3::new(at.x, at.y - 20., at.z));
    Some(Vec3::new(at.x, hit.ok()??.geometry.position.y, at.z))
}

/// `clip` at `time` (holding its last frame), every bone carried by TRAJECTORY: identity in an origin scene, the
/// actor's world placement in a world-space one. Both TRAJECTORY and HIPS are roots of the scene rig.
/// The board needs no off-board hand adjust (gameplay's `pose_adjust::adjustment`): the scene banks key
/// SKATEBOARD_ROOT in the hand and leave the board-parented hands (SKATEBOARD_PROCEDURAL) unkeyed.
fn scene_pose(anim: &Anim, time: f32, rest: &[NativeMatrix], trajectory: Option<usize>) -> Option<Vec<Mat4>> {
    let mut pose = anim.pose(time, rest)?;
    if let Some(t) = trajectory {
        let carry = pose[t];
        for (_, bone) in pose.iter_mut().enumerate().filter(|(i, _)| *i != t) {
            *bone = carry * *bone;
        }
    }
    Some(pose)
}

/// The player's face joints as gameplay leaves them (bind): bound past the skater's bones, which gameplay never poses.
fn face_rest(player: &AnimationStatus, skater_bones: usize, transforms: &Query<&mut Transform>) -> Vec<(Entity, Transform)> {
    player
        .online_bindings()
        .into_iter()
        .filter(|&(_, bone, _)| bone >= skater_bones)
        .filter_map(|(joint, ..)| Some((joint, *transforms.get(joint).ok()?)))
        .collect()
}

/// The player's head joints as bind worlds in native space (the skin's inverse binds, back through render_basis),
/// for [`Poser::scene_with_face`]; empty when no player skin has a face.
fn player_face(
    skins: &Query<(Entity, &SkinnedMesh)>,
    parents: &Query<&ChildOf>,
    roots: &Query<Entity, With<PlayerRoot>>,
    names: &Query<(&Name, &Transform)>,
    inverse_binds: &Assets<SkinnedMeshInverseBindposes>,
) -> Vec<(String, Mat4)> {
    let basis = Mat4::from_cols(Vec4::X, -Vec4::Z, Vec4::Y, Vec4::W).inverse();
    skins
        .iter()
        .filter(|(skin, _)| parents.iter_ancestors(*skin).any(|p| roots.contains(p)))
        .filter_map(|(_, skin)| {
            let binds = inverse_binds.get(&skin.inverse_bindposes)?;
            let joints: Vec<(String, Mat4)> = skin
                .joints
                .iter()
                .zip(binds.iter())
                .filter_map(|(joint, bind)| Some((names.get(*joint).ok()?.0.as_str().to_ascii_uppercase(), bind.inverse() * basis)))
                .collect();
            joints.iter().any(|(name, _)| name == "FACE").then_some(joints)
        })
        .next()
        .unwrap_or_default()
}

/// Whether an actor track binds (again): its cast member exists and isn't the entity it is bound to.
fn needs_bind(bound: Option<Entity>, member: Option<Entity>) -> bool {
    member.is_some() && bound != member
}

fn release(running: Running, cast: &mut Cast, transforms: &mut Query<&mut Transform>, commands: &mut Commands) {
    for (joint, bind) in &running.face_rest {
        if let Ok(mut transform) = transforms.get_mut(*joint) {
            *transform = *bind;
        }
    }
    for audio in running.playing {
        commands.entity(audio).try_despawn();
    }
    for track in running.tracks {
        cast.held.remove(&track.key);
        for (joint, before) in &track.joints {
            if let Ok(mut transform) = transforms.get_mut(*joint) {
                *transform = *before;
            }
        }
        if let Some((root, _, before)) = track.bound
            && let Ok(mut transform) = transforms.get_mut(root)
        {
            *transform = before;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn play(
    mut cutscene: ResMut<Cutscene>,
    mut cast: ResMut<Cast>,
    skater: Res<SkaterRuntime>,
    physics: Option<Res<GamePhysics>>,
    player: Res<AnimationStatus>,
    customiser: Option<Res<crate::customiser::Customiser>>,
    manifest: Res<crate::assets::AssetManifest>,
    (history, replay, fixed): (Res<crate::presentation::Presentation>, Res<crate::replay::Replay>, Res<Time<Fixed>>),
    (real, menu, inverse_binds): (Res<Time<Real>>, Option<Res<crate::graphics_menu::Menu>>, Res<Assets<SkinnedMeshInverseBindposes>>),
    (mut commands, mut pcm): (Commands, ResMut<Assets<StreamPcm>>),
    skins: Query<(Entity, &SkinnedMesh)>,
    parents: Query<&ChildOf>,
    player_roots: Query<Entity, With<PlayerRoot>>,
    mut nodes: ParamSet<(Query<(&Name, &Transform)>, Query<&mut Transform>)>,
) {
    let cutscene = &mut *cutscene;
    if std::mem::take(&mut cutscene.stop)
        && let Some(running) = cutscene.running.take()
    {
        release(running, &mut cast, &mut nodes.p1(), &mut commands);
    }
    let bones = &skater.animation.evaluator.frames.bone_names;
    if let Some(request) = cutscene.request.take() {
        if let Some(running) = cutscene.running.take() {
            release(running, &mut cast, &mut nodes.p1(), &mut commands);
        }
        let female = customiser.as_ref().is_some_and(|c| c.gender() == "female");
        let face = player_face(&skins, &parents, &player_roots, &nodes.p0(), &inverse_binds);
        match load(&request, bones, female, &face) {
            Ok(mut running) if !running.tracks.is_empty() || running.player.is_some() || running.camera.is_some() => {
                if running.base != Vec3::ZERO {
                    running.base = physics.as_ref().and_then(|p| ground(p, running.base)).unwrap_or(running.base);
                }
                for path in &running.audio {
                    match read_wav(path) {
                        Ok(audio) => {
                            let settings = crate::music::one_shot(menu.as_deref(), running.clock.game.audio_slot());
                            running.playing.push(commands.spawn((AudioPlayer(pcm.add(audio)), settings)).id());
                        }
                        Err(error) => warn!("cutscene {}: {}: {error}", request.scene, path.display()),
                    }
                }
                cutscene.running = Some(running);
            }
            Ok(_) => {}
            Err(error) => warn!("Skate 2 cutscene: {error}"),
        }
        if cutscene.rest.is_empty() {
            let evaluator = &skater.animation.evaluator;
            let idle = PoseCommand::Clip { name: manifest.0.initial_animation.clone(), previous_time: 0., time: 0., loops: 0 };
            let commands = [idle, PoseCommand::Pose { name: "RIG_TPOSE".into() }, PoseCommand::Add { motion_is_a: true }];
            cutscene.rest =
                evaluator.evaluate(&commands).and_then(|p| evaluator.hierarchy(&p)).unwrap_or_else(|_| skater.render_pose.clone());
        }
    }
    let Some(running) = cutscene.running.as_mut() else { return };
    running.tick(real.delta_secs().min(0.1));
    let trajectory = bones.iter().position(|b| b == "TRAJECTORY");
    // A player-space camera follows the hips as gameplay presents them (interpolated, as the skater is drawn).
    if let Some(h) = bones.iter().position(|b| b == "HIPS")
        && let Some((previous, current, alpha)) = history.view(&replay, fixed.overstep_fraction())
        && let (Some(a), Some(b)) = (previous.bones.get(h), current.bones.get(h))
    {
        let root = crate::presentation::blend(previous.root, current.root, alpha);
        running.hips = root.transform_point(a.w_axis.truncate().lerp(b.w_axis.truncate(), alpha));
        running.facing = root.rotation;
    }
    // Bind each actor once its cast member has spawned; until then the scene runs on without it. A member the cast
    // respawned under the scene (a Skate 1 pro's stand leaving or moving at his challenge's start, research 4.1) is
    // bound again: the old binding poses a despawned entity (overnight-1015 C2).
    for track in running.tracks.iter_mut() {
        if !needs_bind(track.bound.as_ref().map(|b| b.0), cast.entity(&track.key)) {
            continue;
        }
        let (Some(root), Some(names)) = (cast.entity(&track.key), cast.bones(&track.key)) else { continue };
        let Ok(binding) = AnimationStatus::for_scene(root, names, &skins, &nodes.p0(), &parents) else { continue };
        let missing = track.anim.remap(names);
        track.trajectory = names.iter().position(|n| n.eq_ignore_ascii_case("TRAJECTORY"));
        if !missing.is_empty() {
            warn!("Skate 2 cutscene: {}'s clip has no {missing:?}; they keep their bind pose", track.key);
        }
        let joints = nodes.p0();
        track.joints = binding.online_bindings().into_iter().filter_map(|(e, ..)| Some((e, *joints.get(e).ok()?.1))).collect();
        let mut transforms = nodes.p1();
        let Ok(mut transform) = transforms.get_mut(root) else { continue };
        track.bound = Some((root, binding, *transform));
        *transform = Transform::from_translation(running.base).with_rotation(running.turn);
        cast.held.insert(track.key.clone());
    }
    let rest = &cutscene.rest;
    let mut transforms = nodes.p1();
    // The actors hold their last frame until the queue ends the scene (the audio may run longer).
    let actor = running.actor_time();
    for track in &running.tracks {
        let Some((_, binding, _)) = &track.bound else { continue };
        let Some(pose) = scene_pose(&track.anim, actor, rest, track.trajectory) else { continue };
        for (entity, transform) in binding.pose_transforms(&pose) {
            if let Ok(mut node) = transforms.get_mut(entity) {
                *node = transform;
            }
        }
    }
    if running.player.is_some() && player.ready && running.face_rest.is_empty() {
        running.face_rest = face_rest(&player, bones.len(), &transforms);
    }
    // The player acts over gameplay's pose and place; both are republished every frame, so this never sticks (the
    // face, which gameplay never poses, goes back to bind in `release`).
    if let Some(anim) = &running.player
        && player.ready
        && let Some(pose) = scene_pose(anim, actor, rest, trajectory)
    {
        for (entity, transform) in player.pose_transforms(&pose) {
            if let Ok(mut node) = transforms.get_mut(entity) {
                *node = transform;
            }
        }
        for root in &player_roots {
            if let Ok(mut transform) = transforms.get_mut(root) {
                *transform = Transform::from_translation(running.base).with_rotation(running.turn);
            }
        }
    }
}

/// The scene's camera over the gameplay camera (and a Skate 3 scene's TIME_SCALE over the game).
fn frame(
    mut cutscene: ResMut<Cutscene>,
    mut time: ResMut<Time<Virtual>>,
    mut cameras: Query<(&mut Transform, &mut Projection), With<GameplayCamera>>,
    post: Option<ResMut<crate::retail_render::ShotPost>>,
    fade: Option<ResMut<crate::retail_render::ScreenFade>>,
) {
    let cutscene = &mut *cutscene;
    // A BloomToWhite transition over the screen fade (the Skate 2 tutorial drives it itself otherwise).
    let bloom = cutscene.transition().1;
    if let Some(mut fade) = fade
        && (bloom > 0. || cutscene.bloomed)
    {
        fade.set_if_neq(crate::retail_render::ScreenFade(bloom));
        cutscene.bloomed = bloom > 0.;
    }
    let Some((running, camera)) = cutscene.running.as_ref().and_then(|r| Some((r, r.camera.as_ref()?))) else {
        if std::mem::take(&mut cutscene.scaled) {
            time.set_relative_speed(1.);
        }
        if let Some(mut post) = post {
            post.set_if_neq(default());
        }
        return;
    };
    let s = camera.at(running.clock.time);
    let (eye, look) = running.eye_look(camera, &s);
    for (mut transform, mut projection) in &mut cameras {
        *transform = Transform::from_translation(eye).looking_at(look, Vec3::Y);
        transform.rotate_local_z(s[7]);
        if let Projection::Perspective(p) = &mut *projection {
            p.fov = s[8];
        }
    }
    // The shot's post overrides over the map's (Skate 2 maps' grading draws them).
    if let Some(mut post) = post {
        post.set_if_neq(crate::retail_render::ShotPost { lens: s[10], exposure: s[11], bloom: Vec2::new(s[12], s[13]),
            sun: Vec3::new(s[14], s[15], s[16]), scene: true });
    }
    let speed = running.clock.game.speed(&s);
    if speed != 1. || cutscene.scaled {
        time.set_relative_speed(speed);
        cutscene.scaled = speed != 1.;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skate_data::abin::Bank;

    /// The installed assets (SKATE3_ASSET_ROOT) with the Skate 2 tutorial; these tests are not in CI.
    fn assets() -> PathBuf {
        PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap())
    }

    fn skater_bones() -> Vec<String> {
        Bank::load(&assets().join("private/stock/data/anim/OnBoard.abin")).unwrap().hierarchy().unwrap().bone_names.clone()
    }

    /// A scene bank's clip on the skater's bones and `extra`.
    fn scene_bank(name: &str, extra: &[&str]) -> (Poser, String, Vec<String>) {
        let mut bones = skater_bones();
        bones.extend(extra.iter().map(|b| b.to_string()));
        let bank = assets().join("private/skate2-tutorial/scene/anim").join(name);
        let (poser, clip) = Poser::scene(&bank, &bones).unwrap();
        (poser, clip, bones)
    }

    fn samples(poser: &Poser, clip: &str) -> Vec<f32> {
        let length = poser.length(clip).unwrap();
        (0..=120).map(|k| length * k as f32 / 120.).collect()
    }

    /// Bug 10-09 #10: an online NIS opens out of SceneManagerImpl's transition (black, or BloomToWhite), 1 s.
    #[test]
    fn a_scene_opens_out_of_its_transition() {
        let dir = std::env::temp_dir().join(format!("cutscene-transition-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let scenes = r#"{"scenes":{"b":{"origin":[0,0,0],"tracks":[],"camera":null,"transition":"black"},
            "w":{"origin":[0,0,0],"tracks":[],"camera":null,"transition":"bloom"},
            "n":{"origin":[0,0,0],"tracks":[],"camera":null}}}"#;
        std::fs::write(dir.join("tutorial.json"), scenes).unwrap();
        let mut cutscene = Cutscene::default();
        let mut at = |scene: &str, time: f32| {
            cutscene.start(&dir, scene);
            let mut running = load(cutscene.request.take().as_ref().unwrap(), &[], false, &[]).unwrap();
            running.clock.time = time;
            cutscene.running = Some(running);
            cutscene.transition()
        };
        assert_eq!(at("b", 0.), (1., 0.));
        assert_eq!(at("b", 0.25), (0.75, 0.));
        assert_eq!(at("b", 1.5), (0., 0.));
        assert_eq!(at("w", 0.5), (0., 0.5));
        assert_eq!(at("n", 0.), (0., 0.));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Bug 10-09 #10: a scene's audio (the online NIS's post.big SFX bed) plays with it, for every caller of
    /// [`Cutscene::start`]; [`Cutscene::start_muted`] is for a queue that plays the scene's audio itself.
    #[test]
    fn a_scene_plays_its_audio_unless_muted() {
        let dir = std::env::temp_dir().join(format!("cutscene-audio-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let scenes = r#"{"scenes":{"s":{"origin":[0,0,0],"tracks":[],"camera":null,"audio":[{"audio":"nis/a.wav","seconds":5}]},
            "quiet":{"origin":[0,0,0],"tracks":[],"camera":null}}}"#;
        std::fs::write(dir.join("tutorial.json"), scenes).unwrap();
        let mut cutscene = Cutscene::default();
        let audio = |c: &Cutscene| load(c.request.as_ref().unwrap(), &[], false, &[]).unwrap().audio;
        cutscene.start(&dir, "s");
        assert_eq!(audio(&cutscene), vec![dir.join("nis/a.wav")]);
        cutscene.start_muted(&dir, "s");
        assert!(audio(&cutscene).is_empty());
        cutscene.start(&dir, "quiet");
        assert!(audio(&cutscene).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Item 10: a Skate 1 scene's OVERLAY track (`s1_overlay`, kept out of its camera) keys the overlays as a camera's.
    /// overnight-1015 C2: an actor whose cast member was respawned (new entity) binds again; one still on its member,
    /// or whose member is gone, doesn't.
    #[test]
    fn a_respawned_cast_member_is_bound_again() {
        let mut world = World::new();
        let (old, new) = (world.spawn_empty().id(), world.spawn_empty().id());
        assert!(needs_bind(None, Some(old)));
        assert!(!needs_bind(Some(old), Some(old)));
        assert!(needs_bind(Some(old), Some(new)), "the stand left and the scene's actor respawned");
        assert!(!needs_bind(Some(old), None));
    }

    #[test]
    fn a_skate1_scene_keys_its_overlay_track() {
        let dir = std::env::temp_dir().join(format!("cutscene-s1-overlay-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("cam.json"), r#"{"space":0,"samples":[[0,0,0,0,0,0,0,0,0,0]]}"#).unwrap();
        let scenes = r#"{"scenes":{"s1":{"origin":[0,0,0],"tracks":[],"camera":"cam.json","s1_overlay":[[0,0],[3.8435,4]]}}}"#;
        std::fs::write(dir.join("tutorial.json"), scenes).unwrap();
        let mut cutscene = Cutscene::default();
        cutscene.start(&dir, "s1");
        cutscene.running = Some(load(cutscene.request.take().as_ref().unwrap(), &[], false, &[]).unwrap());
        assert_eq!(cutscene.overlay(), 0);
        cutscene.running.as_mut().unwrap().clock.time = 4.;
        assert_eq!(cutscene.overlay(), 4, "SpotFound_TopLeft from 3.84 s");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    #[ignore]
    fn slappy_talks_with_his_jaw() {
        let (poser, clip, bones) = scene_bank("WelcomeBack_Slappy1.abin", &["JAW"]);
        let at = |name: &str| bones.iter().position(|b| b == name).unwrap();
        let jaw = |t| {
            let p = poser.pose(&clip, t, false, &[]).unwrap();
            (p[at("HEAD")].inverse() * p[at("JAW")]).to_scale_rotation_translation().1
        };
        let first = jaw(0.);
        let most = samples(&poser, &clip).into_iter().map(|t| jaw(t).angle_between(first)).fold(0., f32::max);
        assert!(most > 5f32.to_radians(), "the jaw turns {} degrees", most.to_degrees());
    }

    /// faces.md 1.4: the scene clips key eye look, blinks and lips themselves (RETAIL); the cast must carry the joints.
    #[test]
    #[ignore = "requires SKATE3_ASSET_ROOT with private/skate2-tutorial"]
    fn welcome_back_slappy_blinks_looks_and_moves_his_lips() {
        let face = ["FACE", "JAW", "LEFTEYE", "RIGHTEYE", "LEFTUPEYELID", "LOWERLIP"];
        let (poser, clip, bones) = scene_bank("WelcomeBack_Slappy1.abin", &face);
        let at = |name: &str| bones.iter().position(|b| b == name).unwrap();
        let local = |pose: &[Mat4], bone: &str, parent: &str| pose[at(parent)].inverse() * pose[at(bone)];
        let rotation = |m: Mat4| m.to_scale_rotation_translation().1;
        let (mut eye, mut lid, mut lip_follows) = (0f32, 0f32, true);
        let rest = poser.pose(&clip, 0., false, &[]).unwrap();
        for frame in 0..=(poser.length(&clip).unwrap() * 60.) as usize {
            let pose = poser.pose(&clip, frame as f32 / 60., false, &[]).unwrap();
            let turn = |b: &str, p: &str| rotation(local(&pose, b, p)).angle_between(rotation(local(&rest, b, p))).to_degrees();
            eye = eye.max(turn("LEFTEYE", "FACE"));
            lid = lid.max(turn("LEFTUPEYELID", "FACE"));
            // With the jaw open, the lower lip rides the jaw (it is no longer welded to FACE).
            if turn("JAW", "FACE") > 5. {
                let lip_in_jaw = local(&pose, "LOWERLIP", "JAW").w_axis;
                lip_follows &= (lip_in_jaw - local(&rest, "LOWERLIP", "JAW").w_axis).length() < 0.01;
            }
        }
        assert!(eye > 20., "eye look {eye} deg");
        assert!(lid > 50., "blink {lid} deg");
        assert!(lip_follows, "lower lip left the jaw");
    }

    /// faces.md 9.1 (RETAIL, 0x826c0550 gate): a marquee track's face is bank reference + clip delta, never a
    /// per-character face base. Koston (A02, RIG_TPOSE bank) carries his own shape in the clip's OFFSET_ keys.
    #[test]
    #[ignore = "requires SKATE3_ASSET_ROOT with private/skate2-tutorial"]
    fn marquee_faces_use_the_bank_reference_only() {
        let (poser, clip, bones) = scene_bank("tutorial_m_1a_05_photo_rail_A02_01.abin", &["FACE", "LEFTEYE"]);
        let at = |name: &str| bones.iter().position(|b| b == name).unwrap();
        let pose = poser.pose(&clip, 0., false, &[]).unwrap();
        let eye = (pose[at("FACE")].inverse() * pose[at("LEFTEYE")]).w_axis.truncate();
        let bind = koston_bind_eye_in_face();
        assert!((eye - bind).length() < 1e-4, "{eye} vs Koston's bind {bind}");
    }

    /// Koston's LEFTEYE in FACE at bind (cast/eric_koston.glb's inverse binds), in the banks' basis: the GLB's bind
    /// worlds are the skater's times render_basis (character_glb.py), so the offset turns back by it.
    fn koston_bind_eye_in_face() -> Vec3 {
        let glb = std::fs::read(assets().join("private/skate2-tutorial/cast/eric_koston.glb")).unwrap();
        let length = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        let doc: serde_json::Value = serde_json::from_slice(&glb[20..20 + length]).unwrap();
        let bin = &glb[20 + length + 8..];
        let skin = &doc["skins"][0];
        let joints = skin["joints"].as_array().unwrap();
        let joint = |name: &str| {
            joints.iter().position(|j| doc["nodes"][j.as_u64().unwrap() as usize]["name"].as_str().unwrap().eq_ignore_ascii_case(name)).unwrap()
        };
        let accessor = &doc["accessors"][skin["inverseBindMatrices"].as_u64().unwrap() as usize];
        let view = &doc["bufferViews"][accessor["bufferView"].as_u64().unwrap() as usize];
        let base = (view["byteOffset"].as_u64().unwrap_or(0) + accessor["byteOffset"].as_u64().unwrap_or(0)) as usize;
        let ibm = |i: usize| {
            Mat4::from_cols_array(&std::array::from_fn(|k| f32::from_le_bytes(bin[base + 64 * i + 4 * k..][..4].try_into().unwrap())))
        };
        let local = ibm(joint("FACE")) * ibm(joint("LEFTEYE")).inverse();
        Mat4::from_cols(Vec4::X, -Vec4::Z, Vec4::Y, Vec4::W).transform_vector3(local.w_axis.truncate())
    }

    /// No off-board hand adjust is needed: the board-parented hands are unkeyed in every Slappy bank, and where he
    /// holds the board (WelcomeBack) it is keyed in his hand.
    #[test]
    #[ignore]
    fn slappys_board_is_keyed_in_his_hand() {
        for bank in ["WelcomeBack_Slappy1.abin", "ProsArrival_Slappy1.abin", "ProsLeave_Slappy1.abin"] {
            let (poser, clip, bones) = scene_bank(bank, &[]);
            let at = |name: &str| bones.iter().position(|b| b == name).unwrap();
            let trajectory = Some(at("TRAJECTORY"));
            let times = samples(&poser, &clip);
            let anim = Anim::Bank(poser, clip);
            for t in times {
                let p = scene_pose(&anim, t, &[], trajectory).unwrap();
                let board = p[at("SKATEBOARD_ROOT")].w_axis;
                let gap = ["RIGHTHAND", "LEFTHAND"].map(|h| p[at(h)].w_axis.distance(board)).into_iter().fold(f32::MAX, f32::min);
                // ProsArrival leaves the board aside (3-18 m off), ProsLeave keeps it 0.2-0.5 m off the hand and hip.
                if bank.starts_with("WelcomeBack") {
                    assert!(gap < 0.1, "{bank} at {t}s: the board is {gap} m from a hand");
                }
                for hand in ["RIGHTHAND_REPARENTED", "LEFTHAND_REPARENTED"] {
                    assert_eq!(p[at(hand)], p[at("TRAJECTORY")], "{bank}: {hand} is unkeyed");
                }
            }
        }
    }

    /// Every extra joint of a cast model (`cast/<key>.glb`) has a bone in each scene bank its tracks play.
    #[test]
    #[ignore]
    fn cast_extra_joints_resolve_in_their_scene_banks() {
        let root = assets().join("private/skate2-tutorial");
        let scenes: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("tutorial.json")).unwrap()).unwrap();
        let skater = skater_bones();
        let mut checked = 0;
        for track in scenes["scenes"].as_object().unwrap().values().flat_map(|s| s["tracks"].as_array().unwrap()) {
            let key = track["character"].as_str().unwrap();
            let key = key.strip_suffix("_prop").unwrap_or(key);
            let Ok(glb) = std::fs::read(root.join(format!("cast/{key}.glb"))) else { continue };
            let length = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
            let doc: serde_json::Value = serde_json::from_slice(&glb[20..20 + length]).unwrap();
            let joints = doc["skins"][0]["joints"].as_array().unwrap().iter().map(|j| doc["nodes"][j.as_u64().unwrap() as usize]["name"].as_str().unwrap());
            let mut names = skater.clone();
            names.extend(joints.filter(|j| !skater.iter().any(|s| s.eq_ignore_ascii_case(j))).map(str::to_owned));
            assert!(names.len() >= 109, "{key}: {} joints", names.len());
            let anim = track["anim"].as_str().unwrap();
            let (mut poser, _) = Poser::scene(&root.join(anim), &skater).unwrap();
            assert_eq!(poser.remap(&names), Vec::<String>::new(), "{key} in {anim}");
            checked += 1;
        }
        assert!(checked > 0, "no cast/<key>.glb under {}", root.display());
    }

    /// A bank's reference pose as worlds by bone name (native space), composed as the bank parents them.
    fn reference_worlds(bank: &Path) -> Vec<(String, Mat4)> {
        let banks = skate_data::animation_banks::AnimationBanks {
            banks: vec![std::sync::Arc::new(Bank::load(bank).unwrap())],
            identities: vec!["t".into()],
        };
        let frames = skate_data::animation_frames::AnimationFrames::from_banks(&banks).unwrap();
        let mut worlds: Vec<Mat4> = Vec::new();
        for (i, words) in frames.reference_samples(0).unwrap().into_iter().enumerate() {
            let local = crate::animation::native_matrix(skate_core::animation::output::sqt_to_matrix(crate::animation_pose::sqt(words)));
            worlds.push(usize::try_from(frames.parents[i]).map_or(local, |p| worlds[p] * local));
        }
        frames.bone_names.into_iter().zip(worlds).collect()
    }

    /// Retail 0x826c0550 on a head whose face binds are the bank's own reference: the overwrite changes nothing.
    #[test]
    #[ignore = "requires SKATE3_ASSET_ROOT with private/skate2-tutorial"]
    fn a_face_base_equal_to_the_bank_reference_changes_nothing() {
        let bones = crate::animation::with_face(&skater_bones());
        let bank = assets().join("private/skate2-tutorial/scene/anim/WelcomeBack_Player1.abin");
        let (plain, clip) = Poser::scene(&bank, &bones).unwrap();
        let (faced, _) = Poser::scene_with_face(&bank, &bones, &reference_worlds(&bank)).unwrap();
        for t in samples(&plain, &clip) {
            let (a, b) = (plain.pose(&clip, t, false, &[]).unwrap(), faced.pose(&clip, t, false, &[]).unwrap());
            for (i, (x, y)) in a.iter().zip(&b).enumerate().filter(|(_, (x, _))| x.is_finite()) {
                assert!(x.abs_diff_eq(*y, 1e-4), "{} at {t}: {x} vs {y}", bones[i]);
            }
        }
    }

    /// faces.md section 6, Rust 5: the player's clip covers every face joint; nothing is left unposed.
    #[test]
    #[ignore = "requires SKATE3_ASSET_ROOT with private/skate2-tutorial"]
    fn the_player_clip_poses_every_face_joint() {
        let bones = crate::animation::with_face(&skater_bones());
        let root = assets().join("private/skate2-tutorial/scene/anim");
        let (mut poser, _) = Poser::scene(&root.join("WelcomeBack_Player1.abin"), &bones).unwrap();
        assert_eq!(poser.remap(&bones), Vec::<String>::new());
    }

    /// Review Focus 4: scenes pose the player's face and gameplay never does, so a scene's end puts it back to bind.
    #[test]
    fn a_scene_end_puts_the_players_face_back() {
        use bevy::ecs::system::SystemState;
        let mut world = World::new();
        let root = world.spawn_empty().id();
        let head = world.spawn((Name::new("HEAD"), Transform::default(), ChildOf(root))).id();
        let bind = Transform::from_xyz(0., 0.1, 0.02);
        let face = world.spawn((Name::new("FACE"), bind, ChildOf(head))).id();
        world.spawn((ChildOf(root), SkinnedMesh { inverse_bindposes: default(), joints: vec![head, face] }));
        let skater = vec!["HEAD".to_string()];
        let mut queries: SystemState<(Query<(Entity, &SkinnedMesh)>, Query<(&Name, &Transform)>, Query<&ChildOf>)> =
            SystemState::new(&mut world);
        let (skins, nodes, parents) = queries.get(&world);
        let player = AnimationStatus::for_scene(root, &crate::animation::with_face(&skater), &skins, &nodes, &parents).unwrap();
        let dir = std::env::temp_dir().join(format!("cutscene-face-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("tutorial.json"), r#"{"scenes":{"s":{"origin":[0,0,0],"tracks":[],"camera":null}}}"#).unwrap();
        let mut cutscene = Cutscene::default();
        cutscene.start(&dir, "s");
        let mut running = load(cutscene.request.take().as_ref().unwrap(), &[], false, &[]).unwrap();
        let mut writes: SystemState<(Query<&mut Transform>, Commands)> = SystemState::new(&mut world);
        let (mut transforms, mut commands) = writes.get_mut(&mut world);
        running.face_rest = face_rest(&player, skater.len(), &transforms);
        assert_eq!(running.face_rest, vec![(face, bind)]);
        transforms.get_mut(face).unwrap().translation.x = 0.5; // the scene's pose
        release(running, &mut Cast::default(), &mut transforms, &mut commands);
        writes.apply(&mut world);
        assert_eq!(*world.get::<Transform>(face).unwrap(), bind);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A scene folder `tutorial.json` = `scenes`, `cam.json` = `camera`, under `parent` (the folder name picks the
    /// game: skate2-online, skate3-online, challenges-skate1).
    fn clock_scene(name: &str, scenes: &str, camera: &str, parent: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(parent).join(format!("cutscene-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("tutorial.json"), scenes).unwrap();
        std::fs::write(dir.join("cam.json"), camera).unwrap();
        dir
    }
    fn running(dir: &Path, scene: &str) -> Running {
        let mut cutscene = Cutscene::default();
        cutscene.start(dir, scene);
        load(cutscene.request.as_ref().unwrap(), &[], false, &[]).unwrap()
    }
    /// `seconds` of real time at 60 Hz.
    fn run(running: &mut Running, seconds: f32) {
        for _ in 0..(seconds * 60.).round() as usize {
            running.tick(1. / 60.);
        }
    }
    const ONE_SCENE: &str = r#"{"scenes":{"s":{"origin":[0,0,0],"tracks":[],"camera":"cam.json"}}}"#;
    /// A world camera at TIME_SCALE 0.5 (column 9, already / 100) over 4 s of scene time.
    const HALF_SPEED: &str = r#"{"space":1,"samples":[[0,0,0,0,0,0,1,0,1,0.5],[4,0,0,0,0,0,1,0,1,0.5]]}"#;

    /// nis-clock.md 0.1-0.2 (RETAIL in the Skate 2 and Skate 1 exes): scene time runs at real dt x the camera's
    /// TIME_SCALE; the scene is slowed, not the game.
    #[test]
    fn a_scene_runs_at_its_cameras_time_scale() {
        let dir = clock_scene("scaled", ONE_SCENE, HALF_SPEED, "skate2-online");
        let mut scene = running(&dir, "s");
        run(&mut scene, 2.);
        assert!((scene.clock.time - 1.).abs() < 1e-3, "2 s at TIME_SCALE 0.5: {}", scene.clock.time);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Skate 3's scenes keep real time (its clock isn't traced: tu3.pe is gone, user 10-14).
    #[test]
    fn a_skate3_scene_keeps_real_time() {
        let dir = clock_scene("skate3", ONE_SCENE, HALF_SPEED, "skate3-online");
        let mut scene = running(&dir, "s");
        run(&mut scene, 2.);
        assert!((scene.clock.time - 2.).abs() < 1e-3, "{}", scene.clock.time);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// nis-clock.md 6 (RETAIL): FREEZE CMD 1/3/4/5 hold the scene TIME/30 real seconds and CMD 2 holds it for good;
    /// only CMD 4/5 (the FE photo events) snap a photo.
    #[test]
    fn freeze_keys_hold_the_scene() {
        let camera = r#"{"space":1,"samples":[[0,0,0,0,0,0,1,0,1,1],[9,0,0,0,0,0,1,0,1,1]],"freeze":[[0.5,4,15],[2,2,0]]}"#;
        let dir = clock_scene("freeze", ONE_SCENE, camera, "skate2-online");
        let mut cutscene = Cutscene { running: Some(running(&dir, "s")), ..default() };
        let scene = cutscene.running.as_mut().unwrap();
        run(scene, 1.);
        assert!((scene.clock.time - 0.5).abs() < 0.02, "held 0.5 s at the CMD 4 key: {}", scene.clock.time);
        run(scene, 1.);
        assert!((scene.clock.time - 1.5).abs() < 0.04, "running again: {}", scene.clock.time);
        run(scene, 5.);
        assert!((scene.clock.time - 2.).abs() < 0.02, "CMD 2 holds: {}", scene.clock.time);
        assert!(cutscene.take_freeze(), "CMD 4 snaps");
        assert!(!cutscene.take_freeze(), "CMD 2 doesn't");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_scene_folder_names_its_game() {
        assert_eq!(Game::of(Path::new("private/challenges-skate1/nis")), Game::Skate1);
        assert_eq!(Game::of(Path::new("private/skate1-online/intros")), Game::Skate1);
        assert_eq!(Game::of(Path::new("private/skate2-tutorial")), Game::Skate2);
        assert_eq!(Game::of(Path::new("private/skate2-online/intros")), Game::Skate2);
        assert_eq!(Game::of(Path::new("private/skate3-online/intros")), Game::Skate3);
        // nis-clock.md 4 (RETAIL): every game's post channel plays its streams through the speech system (S1 bank 34,
        // S2/S3 bank 99).
        for game in [Game::Skate1, Game::Skate2, Game::Skate3] {
            assert_eq!(game.audio_slot(), crate::music::DIALOG);
        }
    }

    /// Spec 15.4 (INFERRED from the bed lengths, stage1_nis 3.4): the career's scenes run on Skate 2's clock, scene
    /// time += dt x TIME_SCALE, and the game's speed stays 1.
    #[test]
    fn a_career_scene_runs_on_the_skate_2_clock() {
        let dir = clock_scene("career", ONE_SCENE, HALF_SPEED, "skate3-career");
        assert_eq!(Game::of(&dir), Game::Skate3Career);
        let mut scene = running(&dir, "s");
        run(&mut scene, 2.);
        assert!((scene.clock.time - 1.).abs() < 1e-3, "2 s at TIME_SCALE 0.5: {}", scene.clock.time);
        let s = scene.camera.as_ref().unwrap().at(scene.clock.time);
        assert_eq!(scene.clock.game.speed(&s), 1., "Time<Virtual> keeps its speed");
        assert!((real_length(&dir, "s").unwrap() - 8.).abs() < 0.02, "TIME_SCALE 0.5 doubles the length");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The online Skate 3 scenes keep their clock (a_skate3_scene_keeps_real_time) and still set the game's speed.
    #[test]
    fn an_online_skate_3_scene_keeps_its_clock() {
        let mut s = [0.; 17];
        s[9] = 0.5;
        assert_eq!(Game::Skate3.speed(&s), 0.5);
        assert_eq!(Game::Skate2.speed(&s), 1.);
    }

    /// Spec 15.6: `shingo` and `shingo_prop` are different recipes in the career's cast; Skate 1 / 2 and online keys
    /// still drop `_prop`.
    #[test]
    fn shingo_and_shingo_prop_load_different_models() {
        assert_eq!(Game::Skate3Career.cast_key("shingo_prop"), "shingo_prop");
        assert_eq!(Game::Skate3Career.cast_key("shingo"), "shingo");
        for game in [Game::Skate1, Game::Skate2, Game::Skate3] {
            assert_eq!(game.cast_key("shingo_prop"), "shingo", "{game:?}");
        }
    }

    /// Spec 15.6: Skate 3's player banks are `_Male1` / `_Female1`; Skate 2's `_Player` / `_Female` swap is unchanged.
    #[test]
    fn male1_swaps_to_female1() {
        let career = Game::Skate3Career;
        assert_eq!(career.female_anim("scene/anim/02_PassesBaton_Male1.abin"), "scene/anim/02_PassesBaton_Female1.abin");
        assert_eq!(Game::Skate2.female_anim("scene/anim/intro_Player.abin"), "scene/anim/intro_Female.abin");
        assert_eq!(Game::Skate2.female_anim("scene/anim/x_Male1.abin"), "scene/anim/x_Male1.abin");
    }

    /// Spec 15.5: a scene's cues show on its clock (TIME_SCALE 0.5 here) and one that outlasts the scene is cut at
    /// its end.
    #[test]
    fn a_sub_cue_shows_on_the_scene_clock_and_stops_at_the_end() {
        let scenes =
            r#"{"scenes":{"s":{"origin":[0,0,0],"tracks":[],"camera":"cam.json","subtitles":[[0,1,"first"],[1,9,"second"]]}}}"#;
        let dir = clock_scene("subs", scenes, HALF_SPEED, "skate3-career");
        let mut cutscene = Cutscene { running: Some(running(&dir, "s")), ..default() };
        run(cutscene.running.as_mut().unwrap(), 1.);
        assert_eq!(cutscene.subtitle(), Some("first"), "scene 0.5 s");
        run(cutscene.running.as_mut().unwrap(), 2.);
        assert_eq!(cutscene.subtitle(), Some("second"), "scene 1.5 s");
        run(cutscene.running.as_mut().unwrap(), 6.);
        assert!(cutscene.ended());
        assert_eq!(cutscene.subtitle(), None, "the cue to 9 s is cut at the scene's 4 s end");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Spec 18 row 4 (INFERRED): BRIGHTNESS_OVERRIDE keys darken the scene from NEAR/FAR 0 to -1 linearly between
    /// keys (03_Reda_Leaves_Shingo's 39.33 -> 40.92, held to 43.84, cut back); 1 outside the keys.
    #[test]
    fn the_dip_darkens_between_its_keys() {
        let camera = r#"{"space":1,"samples":[[0,0,0,0,0,0,1,0,1,1],[5,0,0,0,0,0,1,0,1,1]],
            "effects":{"brightness":[[1,1,0,0],[2,1,-1,-1],[3,1,-1,-1],[3,1,0,0]]}}"#;
        let dir = clock_scene("dip", ONE_SCENE, camera, "skate3-career");
        let mut cutscene = Cutscene { running: Some(running(&dir, "s")), ..default() };
        let at = |cutscene: &mut Cutscene, seconds: f32| {
            run(cutscene.running.as_mut().unwrap(), seconds);
            cutscene.brightness()
        };
        assert_eq!(at(&mut cutscene, 0.5), 1., "before the first key");
        assert!((at(&mut cutscene, 1.) - 0.5).abs() < 0.02, "half way down at 1.5 s");
        assert!(at(&mut cutscene, 1.) < 0.02, "held black at 2.5 s");
        assert_eq!(at(&mut cutscene, 1.), 1., "cut back at 3 s");
        assert_eq!(Cutscene::default().brightness(), 1., "no scene");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// stage1_nis 3.4: each stage 1 scene's length on the Skate 2 clock matches the table's, and the five beds the
    /// table shows ending with it (02, 06, 03 and the two Reda scenes) end within 0.06 s of it. tut_sksc_01's bed
    /// runs 0.58 s past its scene and tut_sksc_04's ends 0.61 s before (the same table; spec 15.4's "seven" counts
    /// them too). SKATE3_CAREER_DIR = the career export.
    #[test]
    #[ignore]
    fn the_stage_one_lengths_match_their_beds() {
        let root = PathBuf::from(std::env::var("SKATE3_CAREER_DIR").expect("SKATE3_CAREER_DIR")).join("scenes");
        assert_eq!(Game::of(&root), Game::Skate3Career);
        let data: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("tutorial.json")).unwrap()).unwrap();
        let table = [
            ("tut_sksc_01", 12.65, false),
            ("tut_sksc_02", 14.00, true),
            ("tut_sksc_06", 14.63, true),
            ("tut_sksc_03", 12.50, true),
            ("tut_sksc_04", 13.00, false),
            ("02_Reda_Passes_Baton", 25.02, true),
            ("03_Reda_Leaves_Shingo", 50.02, true),
        ];
        for (scene, length, bed_matches) in table {
            let real = real_length(&root, scene).unwrap();
            assert!((real - length).abs() < 0.02, "{scene}: {real} on the Skate 2 clock, table {length}");
            let bed = data["scenes"][scene]["audio"][0]["seconds"].as_f64().unwrap() as f32;
            assert_eq!((bed - real).abs() < 0.06, bed_matches, "{scene}: bed {bed}, scene {real}");
        }
    }

    /// Skate 2's and Skate 1's scenes carry string-id cues ({start, end, label}, shown by their own modes): they
    /// still load, and give no career line.
    #[test]
    fn labelled_cues_still_load() {
        let scenes = r#"{"scenes":{"s":{"origin":[0,0,0],"tracks":[],"camera":"cam.json",
            "subtitles":[{"start":0,"end":2,"label":"ID_NISSUBTITLE_GOOLLIE_1"}]}}}"#;
        let dir = clock_scene("labelled", scenes, HALF_SPEED, "skate2-online");
        let mut cutscene = Cutscene { running: Some(running(&dir, "s")), ..default() };
        run(cutscene.running.as_mut().unwrap(), 1.);
        assert_eq!(cutscene.subtitle(), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Every installed scene file (private/*/tutorial.json, private/*/*/tutorial.json) still parses.
    #[test]
    #[ignore]
    fn every_installed_scene_file_parses() {
        let mut checked = 0;
        for game in std::fs::read_dir(assets().join("private")).unwrap().flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
            let below = std::fs::read_dir(&game).unwrap().flatten().map(|e| e.path()).filter(|p| p.is_dir());
            for path in std::iter::once(game.clone()).chain(below).map(|d| d.join("tutorial.json")).filter(|p| p.exists()) {
                let text = std::fs::read_to_string(&path).unwrap();
                if !serde_json::from_str::<serde_json::Value>(&text).unwrap()["scenes"].is_object() {
                    continue;
                }
                serde_json::from_str::<Scenes>(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                checked += 1;
            }
        }
        assert!(checked >= 15, "{checked} scene files");
    }

    /// nis-clock.md 6 (RETAIL): a hold is TIME x f32 1/30 (fmuls by 0.033333335: S1 0x824bc384 / 0x82190e40, S2
    /// 0x8259f39c), counted down in f32 by the frame's dt before the scene moves (fsubs: S1 0x8278393c in 0x82783910,
    /// called from the ScenePlayer update ahead of the ICE tick, 0x8277dad8; S2 0x8231a360). A 21-frame hold at 60 Hz
    /// keeps the scene still 42 frames; TIME / 30 (one ulp short) lets go a frame early.
    #[test]
    fn a_freeze_hold_counts_down_as_retail() {
        let camera = r#"{"space":1,"samples":[[0,0,0,0,0,0,1,0,1,1],[9,0,0,0,0,0,1,0,1,1]],"freeze":[[0.01,1,21]]}"#;
        let dir = clock_scene("hold21", ONE_SCENE, camera, "challenges-skate1");
        let mut scene = running(&dir, "s");
        scene.tick(1. / 60.);
        let at = scene.clock.time;
        for frame in 0..42 {
            scene.tick(1. / 60.);
            assert_eq!(scene.clock.time, at, "held at frame {frame}");
        }
        scene.tick(1. / 60.);
        assert!(scene.clock.time > at, "moves on the 43rd");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// nis-clock.md 6 (RETAIL, S1 0x824bc0d0): Skate 1's FREEZE CMD 5 does nothing (Skate 2's holds), and only CMD 4
    /// posts the FE photo event.
    #[test]
    fn a_skate1_scene_runs_through_freeze_cmd_5() {
        let camera = r#"{"space":1,"samples":[[0,0,0,0,0,0,1,0,1,1],[9,0,0,0,0,0,1,0,1,1]],"freeze":[[0.5,4,15],[1,5,30]]}"#;
        let dir = clock_scene("s1-freeze", ONE_SCENE, camera, "challenges-skate1");
        let mut cutscene = Cutscene { running: Some(running(&dir, "s")), ..default() };
        run(cutscene.running.as_mut().unwrap(), 2.);
        let time = cutscene.running.as_ref().unwrap().clock.time;
        assert!((time - 1.5).abs() < 0.04, "held 0.5 s at CMD 4, not at CMD 5: {time}");
        assert!(cutscene.take_freeze(), "CMD 4 is the FE event");
        assert!(!cutscene.take_freeze(), "CMD 5 posts nothing in Skate 1");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// nis-clock.md 5 (RETAIL calls; that an actor samples its clip there is INFERRED): each cut of an edit sets the
    /// actors to the new shot's subtake start, so each shot replays its window of the clip (Go to Slappy's shots).
    #[test]
    fn an_edits_actors_replay_each_shots_window() {
        let camera = r#"{"space":1,"samples":[[0,0,0,0,0,0,1,0,1,1],[12.4,0,0,0,0,0,1,0,1,1]],
            "shots":[[0,0.91],[3.75,0.63],[7.44,1.59]]}"#;
        let dir = clock_scene("shots", ONE_SCENE, camera, "skate2-online");
        let mut scene = running(&dir, "s");
        for (t, actor) in [(1., 1.91), (4., 0.88), (8., 2.15)] {
            scene.clock.time = t;
            assert!((scene.actor_time() - actor).abs() < 1e-4, "at {t}: {}", scene.actor_time());
        }
        let single = clock_scene("single", ONE_SCENE, HALF_SPEED, "skate2-online");
        let mut scene = running(&single, "s");
        scene.clock.time = 3.;
        assert_eq!(scene.actor_time(), 3., "a single take's actors run on scene time");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&single).ok();
    }

    /// nis-clock.md 2 and 10.3: a scene ends at its camera's end on its clock (neither its record's length nor its
    /// audio ends it), a camera-less one at its record's length; the bindings time scenes by these real lengths.
    #[test]
    fn real_length_follows_the_clock() {
        let scenes = r#"{"scenes":{"slow":{"origin":[0,0,0],"tracks":[],"camera":"cam.json","duration":2},
            "plain":{"origin":[0,0,0],"tracks":[],"camera":null,"duration":3}}}"#;
        let dir = clock_scene("lengths", scenes, HALF_SPEED, "skate2-online");
        let length = |scene| real_length(&dir, scene).unwrap();
        assert!((length("slow") - 8.).abs() < 0.05, "4 s of camera at 0.5: {}", length("slow"));
        assert!((length("plain") - 3.).abs() < 0.05, "{}", length("plain"));
        assert_eq!(real_length(&dir, "missing"), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// nis-clock.md 9-10 on the installed Skate 2 tutorial: outro 2 runs 6.5 s and Go to Slappy's 13.9 s (the port
    /// played 5.3 s and 15.0 s); Go to Slappy's is the one edit, with three shots.
    #[test]
    #[ignore = "requires SKATE3_ASSET_ROOT with private/skate2-tutorial"]
    fn the_skate2_tutorial_scenes_run_their_retail_lengths() {
        let root = assets().join("private/skate2-tutorial");
        let length = |scene| real_length(&root, scene).unwrap();
        assert!((length("Goto_m_1a_01_goto_slappys_outro_2") - 6.52).abs() < 0.05, "{}", length("Goto_m_1a_01_goto_slappys_outro_2"));
        assert!((length("tutorial_m_1a_goto_slappys") - 13.85).abs() < 0.05, "{}", length("tutorial_m_1a_goto_slappys"));
        let text = std::fs::read_to_string(root.join("scene/camera/tutorial_m_1a_goto_slappys.json")).unwrap();
        assert_eq!(serde_json::from_str::<Camera>(&text).unwrap().shots.len(), 3);
    }

    /// A clip file as P1's s1abin.write_clip writes it: frames x bones x [rotation xyzw, translation xyz, scale xyz],
    /// LE f32.
    fn write_clip1(dir: &Path, name: &str, frames: &[Vec<(Quat, Vec3, Vec3)>]) -> String {
        let bytes: Vec<u8> = frames
            .iter()
            .flatten()
            .flat_map(|(q, t, s)| [q.x, q.y, q.z, q.w, t.x, t.y, t.z, s.x, s.y, s.z])
            .flat_map(f32::to_le_bytes)
            .collect();
        std::fs::create_dir_all(dir.join("anim1")).unwrap();
        std::fs::write(dir.join(format!("anim1/{name}.bin")), bytes).unwrap();
        format!("anim1/{name}.bin")
    }
    fn meta(file: &str, frames: usize, bones: &[&str], parents: &[i32]) -> Clip1Meta {
        Clip1Meta { file: file.into(), fps: 30., frames, bones: bones.iter().map(|b| b.to_string()).collect(), parents: parents.to_vec() }
    }

    /// research.md 4.3a (RETAIL sampler 0x8225ec98): frame = fps x t clamped to the last, B = A + 1 clamped,
    /// translation and scale lerped, rotation nlerped on the shorter arc; locals composed down the parents; bones mapped
    /// by name, one the clip lacks taking the fallback, else NaN.
    #[test]
    fn a_skate1_clip_samples_as_retail() {
        use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};
        let dir = std::env::temp_dir().join(format!("cutscene-clip1-{}", std::process::id()));
        let (one, three) = (Vec3::ONE, Vec3::splat(3.));
        // Frame 1 stores the root's turn negated: the same rotation, on the far arc. The child grows 1 -> 3.
        let frames = [
            vec![(Quat::IDENTITY, Vec3::ZERO, one), (Quat::IDENTITY, Vec3::Y, one)],
            vec![(-Quat::from_rotation_y(FRAC_PI_2), Vec3::X * 3., one), (Quat::IDENTITY, Vec3::Y, three)],
        ];
        let file = write_clip1(&dir, "C", &frames);
        let mut clip = Clip1::load(&dir, meta(&file, 2, &["ROOT", "CHILD"], &[-1, 0])).unwrap();
        let missing = clip.remap(&["CHILD".into(), "root".into(), "REST".into(), "EXTRA".into()]);
        assert_eq!(missing, ["REST", "EXTRA"]);
        assert!((clip.length() - 1. / 30.).abs() < 1e-6);
        // A fallback for the first three bones only: REST (2) takes it, EXTRA (3) has none.
        let mut rest = [[[0f32; 4]; 4]; 3];
        rest[2] = [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [7., 8., 9., 1.]];
        let half = clip.pose(1. / 60., &rest);
        let (_, r, t) = half[1].to_scale_rotation_translation();
        assert!(t.abs_diff_eq(Vec3::new(1.5, 0., 0.), 1e-5), "{t}");
        let q = Quat::from_rotation_y(FRAC_PI_4);
        assert!(r.abs_diff_eq(q, 1e-5) || r.abs_diff_eq(-q, 1e-5), "the shorter arc: {r}");
        let (s, _, t) = half[0].to_scale_rotation_translation();
        assert!(t.abs_diff_eq(Vec3::new(1.5, 1., 0.), 1e-5), "the child hangs off the root: {t}");
        assert!(s.abs_diff_eq(Vec3::splat(2.), 1e-5), "scale is lerped: {s}");
        assert_eq!(half[2].w_axis.truncate(), Vec3::new(7., 8., 9.), "the fallback");
        assert!(half[3].is_nan(), "a bone neither the clip nor the fallback has keeps its transform");
        let held = clip.pose(10., &[]);
        assert!(held[1].w_axis.truncate().abs_diff_eq(Vec3::X * 3., 1e-5), "the last frame holds");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Review Focus 1: a clip file that doesn't match its meta is refused (the actor keeps its idle), never sampled.
    #[test]
    fn a_broken_clip_file_is_refused() {
        let dir = std::env::temp_dir().join(format!("cutscene-clip1-broken-{}", std::process::id()));
        let file = write_clip1(&dir, "B", &[vec![(Quat::IDENTITY, Vec3::ZERO, Vec3::ONE)]]);
        assert!(Clip1::load(&dir, meta(&file, 1, &["ROOT"], &[-1])).is_ok(), "40 bytes a bone");
        assert!(Clip1::load(&dir, meta(&file, 2, &["ROOT"], &[-1])).is_err(), "1 frame on disc, 2 in the meta");
        assert!(Clip1::load(&dir, meta(&file, 1, &["ROOT"], &[-1, 0])).is_err(), "parents for 2 bones");
        assert!(Clip1::load(&dir, meta(&file, 1, &["ROOT"], &[0])).is_err(), "a bone can't be its own parent");
        assert!(Clip1::load(&dir, meta(&file, 0, &["ROOT"], &[-1])).is_err(), "no frames");
        assert!(Clip1::load(&dir, meta("anim1/none.bin", 1, &["ROOT"], &[-1])).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A Skate 1 scene (P1 `clip1` tracks): an actor plays its anim1 clip when its cast model is in
    /// challenges-skate1/cast (beside `nis/`); the player (female too: Skate 1 has one take) is posed on the skater's
    /// bones only, so the face keeps its bind (U4: the S1 face base is UNTRACED).
    #[test]
    fn a_skate1_scene_plays_its_clip1_tracks() {
        let top = std::env::temp_dir().join("challenges-skate1").join(format!("cutscene-s1-tracks-{}", std::process::id()));
        let dir = top.join("nis");
        let frame = vec![(Quat::IDENTITY, Vec3::ZERO, Vec3::ONE), (Quat::IDENTITY, Vec3::Y, Vec3::ONE), (Quat::IDENTITY, Vec3::Z, Vec3::ONE)];
        let file = write_clip1(&dir, "P", &[frame.clone(), frame]);
        std::fs::create_dir_all(top.join("cast")).unwrap();
        std::fs::write(top.join("cast/IP_shingo.glb"), b"").unwrap();
        let clip1 = format!(r#"{{"file":"{file}","fps":30,"frames":2,"bones":["TRAJECTORY","HIPS","FACE"],"parents":[-1,-1,1]}}"#);
        let scenes = format!(
            r#"{{"scenes":{{"rop":{{"origin":[0,0,0],"camera":null,"tracks":[
            {{"character":"current_player","anim":"scene/anim/p.abin","clip1":{clip1}}},
            {{"character":"IP_shingo","anim":"scene/anim/s.abin","clip1":{clip1}}},
            {{"character":"nobody","anim":"scene/anim/n.abin","clip1":{clip1}}}]}}}}}}"#
        );
        std::fs::write(dir.join("tutorial.json"), scenes).unwrap();
        let mut cutscene = Cutscene::default();
        cutscene.start(&dir, "rop");
        let bones = ["TRAJECTORY".to_string(), "HIPS".to_string()];
        let running = load(cutscene.request.as_ref().unwrap(), &bones, true, &[]).unwrap();
        let keys: Vec<_> = running.tracks.iter().map(|t| t.key.as_str()).collect();
        assert_eq!(keys, ["IP_shingo"], "no cast model, no actor");
        assert!(matches!(running.tracks[0].anim, Anim::Skate1(_)));
        let player = running.player.as_ref().expect("a female skater takes Skate 1's one take");
        let pose = player.pose(0., &[]).unwrap();
        assert_eq!(pose.len(), 2, "no face matrices: the face joints keep their bind");
        assert!(pose[1].w_axis.truncate().abs_diff_eq(Vec3::Y, 1e-6));
        std::fs::remove_dir_all(&top).ok();
    }

    /// research.md 4.4a (RETAIL 0x82c38130): a Player-space (0) point moves and turns with the avatar; World (1)
    /// stays. he_u2_photo01's camera has a world eye looking in space 0.
    #[test]
    fn a_player_space_point_turns_with_the_player() {
        let camera = r#"{"space":1,"look_space":0,"samples":[[0,5,6,7,1,0,0,0,1,1]]}"#;
        let dir = clock_scene("space0", ONE_SCENE, camera, "challenges-skate1");
        let mut scene = running(&dir, "s");
        scene.hips = Vec3::new(10., 1., 0.);
        scene.facing = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let camera = scene.camera.as_ref().unwrap();
        let (eye, look) = scene.eye_look(camera, &camera.at(0.));
        assert_eq!(eye, Vec3::new(5., 6., 7.));
        assert!(look.abs_diff_eq(Vec3::new(10., 1., -1.), 1e-5), "1 m along the avatar's x, turned 90 degrees: {look}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// nis-clock.md 2 (RETAIL): a scene ends when its clock reaches its camera's end (4 s of camera at TIME_SCALE 0.5
    /// = 8 s); it holds there until stopped.
    #[test]
    fn a_scene_ends_at_its_cameras_end_on_its_clock() {
        let dir = clock_scene("ended", ONE_SCENE, HALF_SPEED, "skate2-online");
        let mut cutscene = Cutscene::default();
        cutscene.start(&dir, "s");
        cutscene.run_for_test(7.9);
        assert!(cutscene.active() && !cutscene.ended());
        cutscene.run_for_test(0.2);
        assert!(cutscene.ended());
        cutscene.stop();
        cutscene.run_for_test(0.);
        assert!(!cutscene.ended() && !cutscene.active());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// nis-clock.md 2 (RETAIL, S1 0x824b0048 at 0x824b0138): a replay scene's take duration is overwritten at sync
    /// start, so its camera, FREEZE and OVERLAY keys stretch over the new length.
    #[test]
    fn a_stretched_take_runs_over_its_new_length() {
        let camera = r#"{"space":1,"samples":[[0,0,0,0,0,0,1,0,1,1],[3,0,0,0,0,0,1,0,1,1]],"freeze":[[1.5,4,0]]}"#;
        let dir = clock_scene("stretch", ONE_SCENE, camera, "challenges-skate1");
        let mut cutscene = Cutscene::default();
        cutscene.start(&dir, "s");
        cutscene.stretch(6.);
        cutscene.run_for_test(2.9);
        assert!(!cutscene.take_freeze(), "the key moved from 1.5 s to 3 s");
        cutscene.run_for_test(0.2);
        assert!(cutscene.take_freeze());
        cutscene.run_for_test(2.8);
        assert!(!cutscene.ended());
        cutscene.run_for_test(0.2);
        assert!(cutscene.ended(), "6 s, not 3");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// P1's nine Skate 1 scenes load on the skater's bones: every actor has its cast model and clip, the ROP scenes
    /// pose the player, all have a camera; Basics' frame 0 puts TRAJECTORY at p1-progress Task 6's
    /// (-485.65, 194.80, -1973.15).
    #[test]
    #[ignore = "requires SKATE3_ASSET_ROOT with private/challenges-skate1"]
    fn the_skate1_scenes_load_their_clips() {
        let root = assets().join("private/challenges-skate1/nis");
        let bones = skater_bones();
        let scenes: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("tutorial.json")).unwrap()).unwrap();
        for (name, scene) in scenes["scenes"].as_object().unwrap() {
            let mut cutscene = Cutscene::default();
            cutscene.start(&root, name);
            let running = load(cutscene.request.as_ref().unwrap(), &bones, false, &[]).unwrap();
            let actors = scene["tracks"].as_array().unwrap().iter().filter(|t| t["character"] != "current_player").count();
            assert_eq!(running.tracks.len(), actors, "{name}");
            assert_eq!(running.player.is_some(), name.starts_with("NIS_ROP_"), "{name}");
            assert!(running.camera.is_some(), "{name}");
        }
        let mut cutscene = Cutscene::default();
        cutscene.start(&root, "Basic_Basics_Tutorial");
        let mut running = load(cutscene.request.as_ref().unwrap(), &bones, false, &[]).unwrap();
        let anim = &mut running.tracks[0].anim;
        anim.remap(&bones);
        let at = bones.iter().position(|b| b == "TRAJECTORY").unwrap();
        let trajectory = anim.pose(0., &[]).unwrap()[at].w_axis.truncate();
        assert!(trajectory.abs_diff_eq(Vec3::new(-485.65, 194.80, -1973.15), 0.01), "{trajectory}");
    }
}
