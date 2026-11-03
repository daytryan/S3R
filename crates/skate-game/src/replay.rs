//! Retail Skate 3's replay: a presentation-only recording (never a second simulation or a rewind of live state) in
//! a ring sized like retail's DataMan ring (record.rs), and the Replay Editor that plays it back (editor.rs, hud.rs).
//! Root, skin (including the board), camera, remote skaters, props and the local audio record share one playhead.
use crate::{
    app::SimulationSet,
    presentation::{Snapshot, blend},
    skate_audio::PlayerAudioObservation,
};
use bevy::prelude::*;
use record::{Actor, Prop, RecFrame, Ring};

/// Stage 0 §5 [disc]: 0x8250b8f8 opens the editor from Back only with >= 150 recorded frames (2.5 s at 60 Hz).
pub(crate) const MIN_BACK_FRAMES: usize = 150;
/// One audio record per sim tick (Stage 0 §1, 60 Hz).
const AUDIO_FRAME: f32 = 1. / record::RATE_HZ as f32;

/// The decoded frames bracketing the playhead, so [`Replay::sample`] can hand out references.
struct View {
    index: usize,
    frames: (RecFrame, RecFrame),
    poses: (Snapshot, Snapshot),
    alpha: f32,
}

#[derive(Resource, Default)]
pub(crate) struct Replay {
    ring: Ring,
    pub active: bool,
    /// The editor's camera (Task 6) or the old free camera: drawn in place of the recorded one.
    pub free_camera: Option<Transform>,
    /// The editor camera's vertical field of view (radians), with `free_camera`.
    pub camera_fov: Option<f32>,
    cursor: f64,
    pub playing: bool,
    /// Playback speed, 0.1..1 (the editor's Speed channel / 100); the audio time scale (Stage 0 §3).
    pub speed: f32,
    /// After the editor closes, gameplay input waits until the pad is released (hud.rs).
    release_controls: bool,
    /// A binding drives the playhead (a Skate 2 photo challenge's highlight reel); the replay controls are off.
    pub scripted: bool,
    /// Retail state 20 (kOnlineInstantReplayMode): online the simulation keeps running under the replay.
    pub online: bool,
    /// The recording clock of the newest frame; it keeps counting across [`Replay::reset_recording`] so times a
    /// script took earlier never point into the new recording.
    clock: Option<f64>,
    /// This tick's remote skaters, props and audio record, joined to the next [`Replay::record`].
    pending: RecFrame,
    view: Option<View>,
    audio_carry: f32,
    audio_sent: Option<usize>,
    /// The replay was open: the next live frame is a cut, never blended across the gap (review M6).
    gap: bool,
    /// The ring is a loaded save: it is dropped when the replay closes, not appended to (review M6).
    loaded: bool,
}

impl Replay {
    pub fn record(&mut self, pose: Snapshot, dt: f64, cut: bool) {
        if self.active || !dt.is_finite() || dt <= 0.0 {
            return;
        }
        let _perf = crate::performance::Scope::new("replay_record");
        let cut = cut || std::mem::take(&mut self.gap);
        let time = self.clock.map_or(0.0, |t| t + dt);
        self.clock = Some(time);
        let pending = std::mem::take(&mut self.pending);
        // The rain holds until weather audio reports a change; a strike is recorded once.
        let rain = pending.rain;
        let mut actors = vec![Actor { id: 0, root: pose.root, bones: pose.bones, joints: Vec::new(), visible: true }];
        actors.extend(pending.actors);
        self.ring.push(RecFrame { time, actors, camera: pose.camera, fov: pose.fov, cut, ..pending });
        self.pending.rain = rain;
    }

    /// Weather audio's rain and any strike, for the next recorded frame.
    pub(crate) fn note_weather(&mut self, rain: f32, strike: Option<crate::retail_render::LightningStrike>) {
        self.pending.rain = rain;
        self.pending.strike = self.pending.strike.or(strike);
    }

    /// The recorded frame at the playhead and the playhead's time (seconds, the recording clock).
    pub(crate) fn playhead(&self) -> Option<(usize, f64)> {
        self.view.as_ref().filter(|_| self.active).map(|v| (v.index, self.cursor))
    }

    /// Recorded frame `index`'s time, rain and strike (weather audio's replay path).
    pub(crate) fn weather_at(&self, index: usize) -> Option<(f64, f32, Option<crate::retail_render::LightningStrike>)> {
        self.ring.get(index).map(|f| (f.time, f.rain, f.strike))
    }

    /// The frame index and time at playhead `fraction` (0..1), as [`Replay::audio_at`] finds it.
    pub(crate) fn index_at(&self, fraction: f32) -> (usize, f64) {
        let (start, end) = self.bounds();
        let time = start + (end - start) * f64::from(fraction.clamp(0., 1.));
        (self.ring.times().partition_point(|&t| t <= time).saturating_sub(1), time)
    }

    fn bounds(&self) -> (f64, f64) {
        let n = self.ring.len();
        (self.ring.time(0).unwrap_or(0.0), n.checked_sub(1).and_then(|i| self.ring.time(i)).unwrap_or(0.0))
    }

    /// The recording (oldest frame first).
    pub(crate) fn frames(&self) -> &Ring {
        &self.ring
    }

    /// The playhead in seconds from the oldest frame, and the recording's length.
    pub(crate) fn position(&self) -> (f64, f64) {
        let (start, end) = self.bounds();
        (self.cursor - start, end - start)
    }

    /// Drops the recording (a race heat restart, a map change); the clock keeps counting.
    pub(crate) fn reset_recording(&mut self) {
        if self.active {
            warn!("replay: recording reset ignored while the replay is open");
            return;
        }
        self.ring.clear();
        self.pending = RecFrame::default();
        self.view = None;
        self.loaded = false;
    }

    /// A saved recording (file.rs) in place of the live one; the clock carries on after it.
    pub(crate) fn install(&mut self, ring: Ring) {
        if self.active {
            self.exit();
        }
        self.clock = ring.len().checked_sub(1).and_then(|i| ring.time(i)).or(self.clock);
        self.ring = ring;
        self.pending = RecFrame::default();
        self.view = None;
        self.loaded = true;
    }

    /// Retail's Back gate (Stage 0 §5): >= 150 frames, not already in a replay. Online too, as retail's free skate
    /// (0x825EBC0C: session type 22; hud.rs keeps online matches out).
    pub(crate) fn can_open_from_back(&self) -> bool {
        !self.active && !self.scripted && self.ring.len() >= MIN_BACK_FRAMES
    }

    pub(crate) fn enter(&mut self) {
        if self.ring.is_empty() {
            return;
        }
        self.active = true;
        self.playing = false;
        self.speed = 1.;
        self.free_camera = None;
        self.camera_fov = None;
        self.audio_sent = None;
        self.audio_carry = 0.;
        self.cursor = self.bounds().1;
        self.refresh();
    }

    pub(crate) fn exit(&mut self) {
        self.active = false;
        self.playing = false;
        self.scripted = false;
        self.online = false;
        self.free_camera = None;
        self.camera_fov = None;
        self.release_controls = true;
        self.view = None;
        self.gap = true;
        if std::mem::take(&mut self.loaded) {
            self.reset_recording();
        }
    }

    /// The recording's clock: the time of the newest frame.
    pub(crate) fn now(&self) -> f64 {
        self.bounds().1
    }

    /// A binding takes the replay over at `time`; it moves the playhead with [`Replay::seek`] and ends with
    /// [`Replay::exit`]. False when nothing is recorded.
    pub(crate) fn script(&mut self, time: f64) -> bool {
        self.enter();
        self.scripted = self.active;
        self.seek(time);
        self.active
    }

    pub(crate) fn seek(&mut self, time: f64) {
        let (start, end) = self.bounds();
        // max/min, never clamp: a NaN or reversed bound can't panic here (review I4b; from_blocks refuses both).
        self.cursor = time.max(start).min(end.max(start));
        self.refresh();
    }

    /// Seek to `fraction` (0..1) of the recording: the editor's playhead.
    pub(crate) fn seek_fraction(&mut self, fraction: f32) {
        let (start, end) = self.bounds();
        self.seek(start + (end - start) * f64::from(fraction.clamp(0., 1.)));
    }

    /// One recorded frame forward / back (tests: cuts stay discrete).
    #[cfg(test)]
    fn step(&mut self, direction: i32) {
        self.playing = false;
        let times = self.ring.times();
        let next = if direction > 0 {
            times.iter().find(|&&t| t > self.cursor + 1e-6)
        } else {
            times.iter().rev().find(|&&t| t < self.cursor - 1e-6)
        };
        if let Some(&next) = next {
            self.seek(next);
        }
    }

    /// Decode the frames around the playhead (one block decode, cached by the ring).
    fn refresh(&mut self) {
        if !self.active {
            self.view = None;
            return;
        }
        let times = self.ring.times();
        let upper = times.partition_point(|&t| t <= self.cursor);
        let index = upper.saturating_sub(1);
        if let Some(view) = &mut self.view
            && view.index == index
        {
            view.alpha = alpha(times, index, self.cursor);
            if view.poses.1.root == view.poses.0.root && view.frames.1.cut {
                view.alpha = 0.;
            }
            return;
        }
        let Some(a) = self.ring.get(index) else {
            self.view = None;
            return;
        };
        let b = self.ring.get(upper).unwrap_or_else(|| a.clone());
        let pa = snapshot(&a);
        // Teleports, camera cuts and cadence changes remain discrete.
        let (pb, alpha) = if b.cut || a.actors.first().map(|x| x.bones.len()) != b.actors.first().map(|x| x.bones.len())
        {
            (pa.clone(), 0.)
        } else {
            (snapshot(&b), alpha(times, index, self.cursor))
        };
        self.view = Some(View { index, frames: (a, b), poses: (pa, pb), alpha });
    }

    pub fn sample(&self) -> Option<(&Snapshot, &Snapshot, f32)> {
        if !self.active {
            return None;
        }
        self.view.as_ref().map(|v| (&v.poses.0, &v.poses.1, v.alpha))
    }

    /// The decoded frames around the playhead and the blend between them.
    pub(crate) fn sample_frames(&self) -> Option<(&RecFrame, &RecFrame, f32)> {
        let v = self.view.as_ref().filter(|_| self.active)?;
        let alpha = if v.poses.0.root == v.poses.1.root && v.frames.1.cut { 0. } else { v.alpha };
        Some((&v.frames.0, &v.frames.1, alpha))
    }

    /// One 60 Hz audio record for the playhead (Stage 0 §3, lead ruling): the recorded PhysOut-equivalent with
    /// the playback speed as its time scale. Paused, reverse scrub or a repeated frame (slow motion) carries no
    /// one-shot events; a paused or reversed playhead is silent.
    fn audio_record(&mut self) -> Option<PlayerAudioObservation> {
        let view = self.view.as_ref()?;
        let forward = self.playing && self.audio_sent.is_none_or(|sent| view.index >= sent);
        audio_from(&self.ring, view.index, &view.frames.0, forward, self.speed, &mut self.audio_sent)
    }

    /// The export's audio record for the 60 Hz tick at playhead `fraction` (review I2): the same rules as live
    /// playback, always moving forward at `speed`; `sent` is the export's own last frame.
    pub(crate) fn audio_at(&self, fraction: f32, speed: f32, sent: &mut Option<usize>) -> Option<PlayerAudioObservation> {
        let (start, end) = self.bounds();
        let time = start + (end - start) * f64::from(fraction.clamp(0., 1.));
        let index = self.ring.times().partition_point(|&t| t <= time).saturating_sub(1);
        let frame = self.ring.get(index)?;
        audio_from(&self.ring, index, &frame, true, speed, sent)
    }
}

/// One audio record for recorded frame `index`: paused or reversed is silent, a repeated frame (slow motion) drops
/// its one-shots, frames jumped over still land theirs.
fn audio_from(
    ring: &Ring,
    index: usize,
    frame: &RecFrame,
    forward: bool,
    speed: f32,
    sent: &mut Option<usize>,
) -> Option<PlayerAudioObservation> {
    let mut record = *frame.audio.clone()?;
    record.retail.time_scale = speed;
    match *sent {
        _ if !forward => {
            record.retail.paused = true;
            strip(&mut record);
        }
        Some(last) if last == index => strip(&mut record),
        Some(last) if index > last + 1 && index - last < 30 => {
            for skipped in last + 1..index {
                if let Some(audio) = ring.get(skipped).and_then(|f| f.audio) {
                    record.events.splice(0..0, audio.events);
                    record.landed |= audio.landed;
                }
            }
        }
        _ => {}
    }
    *sent = Some(index);
    Some(record)
}

fn alpha(times: &std::collections::VecDeque<f64>, index: usize, cursor: f64) -> f32 {
    match (times.get(index), times.get(index + 1)) {
        (Some(&a), Some(&b)) if b > a => ((cursor - a) / (b - a)).clamp(0.0, 1.0) as f32,
        _ => 0.0,
    }
}

fn snapshot(frame: &RecFrame) -> Snapshot {
    let skater = frame.actors.first();
    Snapshot {
        root: skater.map_or(Transform::IDENTITY, |a| a.root),
        bones: skater.map(|a| a.bones.clone()).unwrap_or_default(),
        camera: frame.camera,
        fov: frame.fov,
    }
}

fn strip(record: &mut PlayerAudioObservation) {
    record.events.clear();
    record.frontend_sounds.clear();
    record.landed = false;
    record.trick_id = None;
}

pub(crate) struct ReplayPlugin;
impl Plugin for ReplayPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Replay>()
            .configure_sets(
                FixedUpdate,
                (
                    SimulationSet::Input,
                    SimulationSet::Controls,
                    SimulationSet::Physics,
                )
                    .run_if(live),
            )
            .add_systems(
                FixedUpdate,
                gather
                    .after(SimulationSet::Physics)
                    .before(crate::presentation::capture),
            )
            .add_systems(Update, feed_audio.before(crate::skate_audio::player::forward))
            .add_systems(
                PostUpdate,
                play_back
                    .before(bevy::transform::TransformSystems::Propagate)
                    .before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate),
            );
    }
}

/// Online (retail state 20) the replay never pauses the simulation (Review Focus 5).
fn live(replay: Res<Replay>) -> bool {
    !replay.active || replay.online
}

/// perf 10-19: a remote skater's posed joints, put on its root by multiplayer::render's bind: per binding its entity
/// and its entry in `read`, the first binding of each distinct (bone, parent bone, deck part). bind and present
/// pose every binding from exactly that key (render.rs `locals`, `board_pose`, `deck_scale`), so bindings sharing it
/// (each outfit part's own skeleton copy) always hold the same Transform.
#[derive(Component, Default)]
pub(crate) struct ReplayJoints {
    joints: Vec<(Entity, usize)>,
    read: Vec<Entity>,
}
impl ReplayJoints {
    pub(crate) fn new(bindings: impl IntoIterator<Item = (Entity, (usize, Option<usize>, bool))>) -> Self {
        let (mut out, mut keys) = (Self::default(), Vec::new());
        for (entity, key) in bindings {
            let k = keys.iter().position(|&k| k == key).unwrap_or_else(|| {
                keys.push(key);
                out.read.push(entity);
                keys.len() - 1
            });
            out.joints.push((entity, k));
        }
        out
    }
}

/// Remote skaters (root + their distinct posed joints), props and the local audio record for the frame
/// `presentation::capture` records next. A frame's audio is the last record published, at most one tick behind.
fn gather(
    mut replay: ResMut<Replay>,
    mut audio: MessageReader<PlayerAudioObservation>,
    remotes: Query<(Entity, &crate::multiplayer::appearance::RemoteCharacter, &Transform, Option<&ReplayJoints>)>,
    children: Query<&Children>,
    nodes: Query<&Transform>,
    props: Query<(Entity, &crate::physics::props::PropPiece, &Transform, &Visibility)>,
) {
    let last = audio.read().last().cloned();
    if replay.active {
        return;
    }
    let pending = &mut replay.pending;
    if let Some(last) = last {
        pending.audio = Some(Box::new(last));
    }
    // perf 10-19: SKATE_REPLAY_BONES_ONLY: the distinct posed joints as Transforms, not every node below the root as a
    // Mat4 (each outfit part's own skeleton copy plus its static scene nodes, ~500 a remote at 60 Hz). Only render.rs
    // bind and present write a remote's nodes, and only its bindings: the rest hold their scene values, so replaying
    // them changed nothing. A remote not bound yet has no posed joints.
    let bones_only = crate::config::perf_on("SKATE_REPLAY_BONES_ONLY");
    pending.actors = remotes
        .iter()
        .map(|(entity, remote, root, joints)| Actor {
            id: remote.0 as u32,
            root: *root,
            bones: if bones_only {
                Vec::new()
            } else {
                children
                    .iter_descendants_depth_first(entity)
                    .filter_map(|node| nodes.get(node).ok())
                    .map(|t| Mat4::from_scale_rotation_translation(t.scale, t.rotation, t.translation))
                    .collect()
            },
            joints: joints
                .filter(|_| bones_only)
                .map_or_else(Vec::new, |j| j.read.iter().filter_map(|&e| nodes.get(e).ok().copied()).collect()),
            visible: true,
        })
        .collect();
    pending.props = prop_order(props.iter().map(|(e, piece, t, v)| (e, piece.0, *t, *v != Visibility::Hidden)))
        .into_iter()
        .map(|(_, id, transform, visible)| Prop { id, transform, visible })
        .collect();
}

/// Props in a stable order, keyed by prop and piece (the nth piece of a prop): id = prop << 8 | piece.
fn prop_order<T>(pieces: impl Iterator<Item = (Entity, u32, Transform, T)>) -> Vec<(Entity, u32, Transform, T)> {
    let mut pieces: Vec<_> = pieces.collect();
    pieces.sort_by_key(|(entity, prop, ..)| (*prop, entity.index()));
    let mut out = Vec::with_capacity(pieces.len());
    let mut previous = None;
    let mut piece = 0;
    for (entity, prop, transform, extra) in pieces {
        piece = if previous == Some(prop) { piece + 1 } else { 0 };
        previous = Some(prop);
        out.push((entity, prop << 8 | piece.min(255), transform, extra));
    }
    out
}

/// The recorded remote skaters and props at the playhead; the props go back where they were on exit.
fn play_back(
    replay: Res<Replay>,
    remotes: Query<(Entity, &crate::multiplayer::appearance::RemoteCharacter, Option<&ReplayJoints>)>,
    children: Query<&Children>,
    mut nodes: Query<&mut Transform>,
    mut shown: Query<&mut Visibility>,
    props: Query<(Entity, &crate::physics::props::PropPiece)>,
    mut saved: Local<Option<Vec<(Entity, Transform, Visibility)>>>,
) {
    let Some((a, b, alpha)) = replay.sample_frames() else {
        for (entity, transform, visibility) in saved.take().into_iter().flatten() {
            if let Ok(mut t) = nodes.get_mut(entity) {
                *t = transform;
            }
            if let Ok(mut v) = shown.get_mut(entity) {
                *v = visibility;
            }
        }
        return;
    };
    let live = prop_order(props.iter().map(|(e, piece)| (e, piece.0, Transform::IDENTITY, ())));
    if saved.is_none() {
        *saved = Some(
            live.iter()
                .filter_map(|(e, ..)| Some((*e, *nodes.get(*e).ok()?, *shown.get(*e).ok()?)))
                .collect(),
        );
    }
    for (entity, id, ..) in &live {
        let Some(p) = a.props.iter().find(|p| p.id == *id) else { continue };
        let to = b.props.iter().find(|q| q.id == *id).map_or(p.transform, |q| blend(p.transform, q.transform, alpha));
        if let Ok(mut t) = nodes.get_mut(*entity) {
            *t = to;
        }
        if let Ok(mut v) = shown.get_mut(*entity) {
            v.set_if_neq(if p.visible { Visibility::Inherited } else { Visibility::Hidden });
        }
    }
    for (entity, remote, joints) in &remotes {
        let id = remote.0 as u32;
        let Some(x) = a.actors.iter().skip(1).find(|x| x.id == id) else { continue };
        let y = b.actors
            .iter()
            .skip(1)
            .find(|y| y.id == id && y.bones.len() == x.bones.len() && y.joints.len() == x.joints.len())
            .unwrap_or(x);
        if let Ok(mut t) = nodes.get_mut(entity) {
            *t = blend(x.root, y.root, alpha);
        }
        if let Ok(mut v) = shown.get_mut(entity) {
            v.set_if_neq(Visibility::Inherited);
        }
        // perf 10-19: each recorded joint back on every binding that shares it.
        if let Some(joints) = joints.filter(|j| !x.joints.is_empty() && j.read.len() == x.joints.len()) {
            let posed: Vec<Transform> = x.joints.iter().zip(&y.joints).map(|(p, q)| blend(*p, *q, alpha)).collect();
            for &(node, k) in &joints.joints {
                if let Ok(mut t) = nodes.get_mut(node) {
                    *t = posed[k];
                }
            }
            continue;
        }
        if x.bones.is_empty() {
            continue;
        }
        let descendants: Vec<Entity> = children.iter_descendants_depth_first(entity).collect();
        if descendants.len() != x.bones.len() {
            continue;
        }
        for ((node, p), q) in descendants.into_iter().zip(&x.bones).zip(&y.bones) {
            if let Ok(mut t) = nodes.get_mut(node) {
                *t = blend(Transform::from_matrix(*p), Transform::from_matrix(*q), alpha);
            }
        }
    }
}

/// Playback audio (Stage 0 §3, lead ruling): one recorded audio record per 60 Hz tick into `player_audio`'s own
/// input, in place of the frozen simulation's. Online the simulation keeps publishing its own; a scripted replay
/// (the Skate 2 highlight reel) is left as it was.
fn feed_audio(mut replay: ResMut<Replay>, time: Res<Time<Real>>, mut out: MessageWriter<PlayerAudioObservation>) {
    if !replay.active || replay.scripted || replay.online {
        replay.audio_sent = None;
        replay.audio_carry = 0.;
        return;
    }
    replay.audio_carry = (replay.audio_carry + time.delta_secs()).min(0.25);
    while replay.audio_carry >= AUDIO_FRAME {
        replay.audio_carry -= AUDIO_FRAME;
        if let Some(record) = replay.audio_record() {
            out.write(record);
        }
    }
}

pub(crate) mod cameras;
pub(crate) mod editor;
pub(crate) mod export;
pub(crate) mod file;
pub(crate) mod grade;
pub(crate) mod hud;
pub(crate) mod mf;
pub(crate) mod record;
pub(crate) mod reel;
pub(crate) mod timeline;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod video_tests;
