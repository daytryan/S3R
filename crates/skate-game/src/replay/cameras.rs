//! The Replay Editor's cameras (research Q6, Stage 0 §2 and §8): Follow, Tripod, Free, Game (the recorded game
//! camera) and the keyed Adv* cameras, the camera edit input with retail's gains and clamps (research Q3), the lens
//! and shake. Pure: hud.rs feeds it the recorded frame and writes the result to `Replay::free_camera`.
use super::editor::{CUT_HOLD, CameraKind, Ease, Editor, HEIGHT_STEP, LENS_MAX, LENS_MIN, LENS_STEP, ROLL_STEP, Shot, Value, wrap_degrees};
use bevy::prelude::*;

/// research Q6 [disc]: the `replay` camera shot's distance (2.0 m) behind the skater (heading 180). Before 10-27 a
/// Follow shot orbited the skater's heading from there; only [`migrate_follow`] reads it now (older saves).
const LEGACY_FOLLOW_DISTANCE: f32 = 2.0;
/// RULING (build-progress, Task 6): the look-at height over the skater's root (the shot's COM target was not
/// decoded); 1 m, about the rider's chest.
const LOOK_HEIGHT: f32 = 1.0;
/// research Q3 [disc] 0x82610c08: right stick ×2.5 clamped ±3 (yaw / pitch, degrees per update), left stick Y ×1
/// clamped ±0.2 and negated (dolly, metres per update).
const ORBIT_GAIN: f32 = 2.5;
const ORBIT_CLAMP: f32 = 3.;
const DOLLY_CLAMP: f32 = 0.2;
/// TU3 0x82c826d0, the ICE orbit every editor camera's sticks go through: the pitch (cam+0x22, degrees x 65536 / 360)
/// clamps to +-15473 = +-85 degrees, the distance (cam+0x24) to 0.5..1000 m.
const ORBIT_PITCH_MAX: f32 = 85.;
const ORBIT_DISTANCE: (f32, f32) = (0.5, 1000.);
/// TU3 0x8261173c (the Free branch, mode 10): look = clamp(stick ×2.5, ±0.8) ×2.0 (ed+104 / +108); move =
/// clamp(stick, ±0.2) with no factor (ed+96 / +100); the height velocity ed+124 gains ±0.015 per held update,
/// clamped ±0.2, moves the camera and decays ×0.6 (0x8261197c: research Q3's "0.6 factor").
const FREE_LOOK_CLAMP: f32 = 0.8;
const FREE_LOOK_FACTOR: f32 = 2.0;
const FREE_MOVE_CLAMP: f32 = 0.2;
const FREE_HEIGHT_STEP: f32 = 0.015;
const FREE_RISE_DECAY: f32 = 0.6;
/// TU3 0x82c80de0 (mode 10): the Free camera's rise totals in cam+0x38, clamped +-10 m (0x821963E4 / 0x821A0314).
const FREE_HEIGHT_MAX: f32 = 10.;
/// TU3 0x82c80ea0 (modes 5, 7, 8, 9): D-pad up / down moves the aim point on the skater (ICE LOOK_Z through
/// 0x82c826d0), totalled in cam+0x38 and clamped +-1.0 (0x8231A844 / 0x8216DEE0).
const AIM_MAX: f32 = 1.;
/// TU3 0x82683680 (ICELITEWorldManager +0x10; 0x821EB794): while a camera is edited its eye stays within 40 m of the
/// skater.
const TETHER: f32 = 40.;
/// RULING (build-progress, Task 6): the 35 mm still frame's 24 mm height turns the lens into a vertical field of
/// view (ICE element 9 LENS_LENGTH; the ICELITE projection was not decoded).
const FILM_HEIGHT_MM: f32 = 24.;
/// RULING (build-progress, Task 6): ChannelShake's levels as rotation noise (degrees) Off / Low / Medium / High.
const SHAKE_DEGREES: [f32; 4] = [0., 0.4, 0.9, 1.8];

/// The skater the cameras look at (the recorded root at the playhead) and the recorded game camera (retail's avatar
/// 2, TU3 0x82c84cf8): a Follow eye is kept in the game camera's yaw frame (its position, horizontal forward, world up).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Subject {
    pub position: Vec3,
    pub game: Transform,
}
impl Subject {
    pub fn new(root: &Transform, game: Transform) -> Self {
        Self { position: root.translation, game }
    }
    fn target(&self) -> Vec3 {
        self.position + Vec3::Y * LOOK_HEIGHT
    }
    fn yaw(&self) -> Quat {
        Quat::from_rotation_y(self.game.rotation.to_euler(EulerRot::YXZ).0)
    }
    /// A Follow eye from the game camera's yaw frame to the world, and back.
    fn from_game(&self, eye: Vec3) -> Vec3 {
        self.game.translation + self.yaw() * eye
    }
    fn to_game(&self, eye: Vec3) -> Vec3 {
        self.yaw().inverse() * (eye - self.game.translation)
    }
}

/// One update of the camera edit sticks and D-pad (held), -1..1.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Sticks {
    pub left: Vec2,
    pub right: Vec2,
    /// D-pad held: x = right - left (lens), y = up - down (height).
    pub dpad: Vec2,
}

pub(crate) fn lens_fov(lens_mm: f32) -> f32 {
    2. * (FILM_HEIGHT_MM * 0.5 / lens_mm.clamp(LENS_MIN, LENS_MAX)).atan()
}

/// research Q2: ±0.5 mm per update, clamped 24..75 (0x82c80b48).
pub(crate) fn step_lens(lens: f32, direction: f32) -> f32 {
    (lens + LENS_STEP * direction.signum() * f32::from(direction != 0.)).clamp(LENS_MIN, LENS_MAX)
}

/// Where a shot puts the eye and what it looks at, for the skater at the playhead. Follow and Tripod look at the skater
/// with `height` the aim's offset up (ICE LOOK_Z, 1c). A Follow eye rides with the recorded game camera, kept in its
/// yaw frame (0x82c7ffb8: SPACE_EYE Player, AVATAR_EYE 2; the eye-to-skater distance is not held); a Tripod eye stays
/// where it was put.
pub(crate) fn view(shot: &Shot, subject: Subject) -> (Vec3, Vec3) {
    let aim = subject.target() + Vec3::Y * shot.height;
    match shot.kind {
        CameraKind::Follow | CameraKind::AdvFollow => (subject.from_game(shot.eye), aim),
        CameraKind::Tripod | CameraKind::AdvTripod => (shot.eye, aim),
        CameraKind::Free | CameraKind::AdvFree | CameraKind::Game => (shot.eye, shot.look),
    }
}

/// `shot` (a new one) where `camera` is: a Tripod / Free eye and look on it; a Follow eye on it, kept in the game
/// camera's frame and looking at the skater (0x82c7ffb8 keeps the eye's world position and zeroes LOOK; from the Game
/// camera the eye is the game camera's own). The Game camera is the recorded one.
pub(crate) fn start_at(shot: &mut Shot, camera: &Transform, subject: Subject) {
    match shot.kind {
        CameraKind::Tripod | CameraKind::AdvTripod | CameraKind::Free | CameraKind::AdvFree => {
            shot.eye = camera.translation;
            shot.look = camera.translation + camera.forward() * 1.;
        }
        CameraKind::Follow | CameraKind::AdvFollow => {
            shot.eye = subject.to_game(camera.translation);
            shot.height = 0.;
        }
        CameraKind::Game => {}
    }
}

/// 1a / 1b (user 10-27: the cameras started "under the map"): a camera edit's new shot starts at the camera on screen
/// (`camera`, last frame's), before anything draws it. Unplaced, a Tripod / Free shot sat at the world origin
/// (Shot::new's eye) on frames without a 60 Hz step, and the origin then became its start. Retail starts each from the
/// current view (Free 0x82c852f0, Tripod 0x82c80298, Follow 0x82c7ffb8). Call every frame before the updates.
pub(crate) fn start_edit(editor: &mut Editor, camera: Transform, subject: Subject) {
    if std::mem::take(&mut editor.unplaced) {
        start_at(&mut editor.shot, &camera, subject);
        // #28: nothing to blend from.
        editor.before = None;
    }
}

/// The stick orbit of `eye` about `aim` (0x82c826d0's ops -1, 0 and 1, each placing the eye through 0x82c82e40,
/// cam+0xc = 1): LY dollies, RX turns, RY tilts, with retail's gains. The angles and distance are eye - aim's
/// (0x82c82a50, unclamped), each clamped only when its stick moves it; stick up raises the eye (the view tilts down).
/// An idle stick leaves the eye (0x82509558 skips a 0).
fn orbit(eye: Vec3, aim: Vec3, sticks: Sticks) -> Vec3 {
    let step = |v: f32| (v * ORBIT_GAIN).clamp(-ORBIT_CLAMP, ORBIT_CLAMP);
    let (dolly, turn, tilt) = ((-sticks.left.y).clamp(-DOLLY_CLAMP, DOLLY_CLAMP), step(sticks.right.x), step(sticks.right.y));
    if dolly == 0. && turn == 0. && tilt == 0. {
        return eye;
    }
    let offset = eye - aim;
    let mut distance = offset.length();
    let yaw = offset.x.atan2(offset.z) + turn.to_radians();
    let mut pitch = offset.y.atan2(Vec2::new(offset.x, offset.z).length());
    if dolly != 0. {
        distance = (distance + dolly).clamp(ORBIT_DISTANCE.0, ORBIT_DISTANCE.1);
    }
    if tilt != 0. {
        let max = ORBIT_PITCH_MAX.to_radians();
        pitch = (pitch + tilt.to_radians()).clamp(-max, max);
    }
    aim + Vec3::new(yaw.sin() * pitch.cos(), pitch.sin(), yaw.cos() * pitch.cos()) * distance
}

/// 0x82c80ea0 / 0x82c80de0: one D-pad step against the edit's total (cam+0x38, zeroed each time a camera is
/// entered); the step that reaches +-`limit` is dropped and the total stops there. Whether the step applies.
fn aim_step(total: &mut f32, step: f32, limit: f32) -> bool {
    if step == 0. {
        return false;
    }
    *total += step;
    if total.abs() >= limit {
        *total = total.clamp(-limit, limit);
        return false;
    }
    true
}

/// The camera edit: one 60 Hz update of the sticks on the shot being edited (per-mode updates, 0x82610f18's table).
/// `rise` is the Free camera's height velocity (ed+124), `aim` the D-pad's total this edit (cam+0x38).
///
/// Follow and Tripod (modes 7 and 8: 0x82611a1c reads LY, RX, RY and the D-pad with Follow's gains, 0x82509558 makes
/// the ICE calls) [`orbit`] the eye about the skater's aim point, then the D-pad steps the aim up or down
/// (0x82c80ea0, ICE LOOK_Z) with the eye left where it is. The Free camera (cam+0xc = 0: 0x82c805ac) turns about its
/// eye with the angle steps negated (0x82c827b8), look = eye - offset (0x82c8312c): stick up lifts a Free look.
pub(crate) fn steer(shot: &mut Shot, sticks: Sticks, subject: Subject, rise: &mut f32, aim: &mut f32) {
    shot.lens = step_lens(shot.lens, sticks.dpad.x);
    let lift = HEIGHT_STEP * sticks.dpad.y.signum() * f32::from(sticks.dpad.y != 0.);
    let target = subject.target() + Vec3::Y * shot.height;
    match shot.kind {
        CameraKind::Follow | CameraKind::AdvFollow => {
            shot.eye = subject.to_game(orbit(subject.from_game(shot.eye), target, sticks));
            if aim_step(aim, lift, AIM_MAX) {
                shot.height += lift;
            }
        }
        CameraKind::Tripod | CameraKind::AdvTripod => {
            shot.eye = orbit(shot.eye, target, sticks);
            if aim_step(aim, lift, AIM_MAX) {
                shot.height += lift;
            }
        }
        CameraKind::Free | CameraKind::AdvFree => {
            let forward = (shot.look - shot.eye).normalize_or(Vec3::NEG_Z);
            let flat = Vec3::new(forward.x, 0., forward.z).normalize_or(Vec3::NEG_Z);
            let right = flat.cross(Vec3::Y);
            let step = |v: f32| v.clamp(-FREE_MOVE_CLAMP, FREE_MOVE_CLAMP);
            shot.eye += right * step(sticks.left.x) + flat * step(sticks.left.y);
            *rise = (*rise + FREE_HEIGHT_STEP * sticks.dpad.y.signum() * f32::from(sticks.dpad.y != 0.)).clamp(-FREE_MOVE_CLAMP, FREE_MOVE_CLAMP);
            // The rise steps EYE_Z (the look follows below), +-10 m in all this edit.
            if aim_step(aim, *rise, FREE_HEIGHT_MAX) {
                shot.eye.y += *rise;
            }
            *rise *= FREE_RISE_DECAY;
            let (yaw, pitch) = (forward.x.atan2(-forward.z), forward.y.asin());
            let look = |v: f32| ((v * ORBIT_GAIN).clamp(-FREE_LOOK_CLAMP, FREE_LOOK_CLAMP) * FREE_LOOK_FACTOR).to_radians();
            let yaw = yaw + look(sticks.right.x);
            let max = ORBIT_PITCH_MAX.to_radians();
            let pitch = (pitch + look(sticks.right.y)).clamp(-max, max);
            let direction = Vec3::new(yaw.sin() * pitch.cos(), pitch.sin(), -yaw.cos() * pitch.cos());
            shot.look = shot.eye + direction;
        }
        CameraKind::Game => {}
    }
}

/// TU3 0x82683680 (ICELITEWorldManager +0x10, on while a camera is edited): the eye stays within 40 m of the skater
/// (3D). A Free eye comes in onto that sphere keeping its view direction (0x82c7f530 / 0x82c7f5a0); a Follow or
/// Tripod eye comes in along its line to the aim (0x82683450 re-derives the orbit, 0x82683660 places it). The edit
/// keeps it; nothing switches to the Game camera. Retail also pushes the eye out of static geometry (0x82682d80, not
/// decoded).
fn tether(shot: &mut Shot, subject: Subject) {
    let (eye, look) = view(shot, subject);
    let away = eye - subject.position;
    // A Follow or Tripod eye is held 40 m from the aim it comes in along: measured from the feet, a 0.2 m dolly in
    // from above still left it past 40 m, and the pull put it back out (user 10-27: stuck at the limit).
    let free = matches!(shot.kind, CameraKind::Free | CameraKind::AdvFree);
    if if free { away.length() } else { eye.distance(look) } <= TETHER {
        return;
    }
    let pulled = look + (eye - look).normalize_or_zero() * TETHER;
    match shot.kind {
        CameraKind::Free | CameraKind::AdvFree => {
            let eye = subject.position + away.normalize() * TETHER;
            shot.look += eye - shot.eye;
            shot.eye = eye;
        }
        CameraKind::Follow | CameraKind::AdvFollow => shot.eye = subject.to_game(pulled),
        CameraKind::Tripod | CameraKind::AdvTripod => shot.eye = pulled,
        CameraKind::Game => {}
    }
}

/// One 60 Hz update of a camera edit (modes 7-10, from hud.rs present; a new shot already started by
/// [`start_edit`]): the sticks and D-pad ([`steer`]), the 40 m [`tether`], then the item 16 LB / RB roll (`roll` =
/// RB - LB held; bindings no camera mode reads). No camera mode reads the triggers: no scrubbing here.
pub(crate) fn edit_update(editor: &mut Editor, sticks: Sticks, roll: f32, subject: Subject) {
    steer(&mut editor.shot, sticks, subject, &mut editor.rise, &mut editor.aim_total);
    tether(&mut editor.shot, subject);
    if roll != 0. {
        editor.shot.roll = wrap_degrees(editor.shot.roll + roll * ROLL_STEP);
    }
}

/// Mode 5's update after the scrub (0x8261135c). Scrubbing commits the stick orbit: the clip under the playhead
/// becomes that Follow camera (0x8261141c). At rest inside the margins the sticks (0x82610c08: LY, RX, RY, D-pad)
/// first enter Follow and move nothing that update (0x82611494): from a Follow clip as it is, else from the view on
/// screen (`start`, 0x82c7ffb8). Later updates steer it ([`steer`], LX not read) and [`tether`] it.
pub(crate) fn orbit_update(editor: &mut Editor, sticks: Sticks, subject: Subject, start: Transform) {
    if editor.scrub_velocity != 0. {
        editor.commit_orbit();
    } else if !editor.playing && editor.inside() && (sticks.left.y != 0. || sticks.right != Vec2::ZERO || sticks.dpad != Vec2::ZERO) {
        match editor.orbit.as_mut() {
            Some(orbit) => {
                steer(orbit, Sticks { left: Vec2::new(0., sticks.left.y), ..sticks }, subject, &mut editor.rise, &mut editor.aim_total);
                tether(orbit, subject);
            }
            None => {
                let clip = editor.camera_at(editor.playhead);
                let orbit = if clip.kind == CameraKind::Follow {
                    clip
                } else {
                    let mut shot = Shot::new(CameraKind::Follow);
                    start_at(&mut shot, &start, subject);
                    shot
                };
                (editor.orbit, editor.aim_total, editor.rise) = (Some(orbit), 0., 0.);
            }
        }
    }
}

/// A save from before 10-27 (no `"cameras": 2`, file.rs): its Follow shots orbited the skater's heading
/// (LEGACY_FOLLOW_DISTANCE + `distance` out, the eye raised by `height`) and looked at the skater. Each becomes the eye
/// it drew at its key, kept in the game camera's frame as retail's are. `at` gives the recorded root and game camera
/// at a playhead.
pub(crate) fn migrate_follow(editor: &mut Editor, mut at: impl FnMut(f32) -> Option<(Transform, Transform)>) {
    for key in &mut editor.keys[0] {
        let Value::Camera(shot) = &mut key.value else { continue };
        if !matches!(shot.kind, CameraKind::Follow | CameraKind::AdvFollow) {
            continue;
        }
        let Some((root, game)) = at(key.t) else { continue };
        let forward = root.rotation * Vec3::Z;
        let yaw = forward.x.atan2(forward.z) + std::f32::consts::PI + shot.yaw.to_radians();
        let back = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(-shot.pitch.to_radians()) * Vec3::Z;
        let subject = Subject::new(&root, game);
        let eye = subject.target() + back * (LEGACY_FOLLOW_DISTANCE + shot.distance).max(ORBIT_DISTANCE.0) + Vec3::Y * shot.height;
        shot.eye = subject.to_game(eye);
        (shot.yaw, shot.pitch, shot.distance, shot.height) = (0., 0., 0., 0.);
    }
    editor.legacy_follow = false;
}

/// Stage 0 §8 RULING: Catmull-Rom (Hermite, tangent 1) on eye and look, linear lens, a cut holds for CUT_HOLD.
/// Item 16: each key's ease shapes its segment (Smooth = that retail curve, Linear, Hold); roll turns the short way.
pub(crate) fn interpolate(keys: &[(f32, Shot, bool, Ease)], t: f32) -> Option<Shot> {
    // A cut key gets a second key with its shot at t + 0.02 (0x8250c890): the shot holds there.
    let keys: Vec<(f32, Shot, bool, Ease)> =
        keys.iter().flat_map(|&k| std::iter::once(k).chain(k.2.then_some((k.0 + CUT_HOLD, k.1, false, k.3)))).collect();
    let (first, last) = (keys.first()?, keys.last()?);
    if t <= first.0 {
        return Some(first.1);
    }
    if t >= last.0 {
        return Some(last.1);
    }
    let i = keys.iter().rposition(|k| k.0 <= t)?;
    let (a, b) = (keys[i], keys[i + 1]);
    if a.3 == Ease::Hold {
        return Some(a.1);
    }
    let u = ((t - a.0) / (b.0 - a.0).max(1e-6)).clamp(0., 1.);
    let p0 = keys.get(i.wrapping_sub(1)).map_or(a.1, |k| k.1);
    let p3 = keys.get(i + 2).map_or(b.1, |k| k.1);
    let curve = |p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3| match a.3 {
        Ease::Smooth => {
            let (u2, u3) = (u * u, u * u * u);
            0.5 * ((2. * p1) + (-p0 + p2) * u + (2. * p0 - 5. * p1 + 4. * p2 - p3) * u2 + (-p0 + 3. * p1 - 3. * p2 + p3) * u3)
        }
        _ => p1.lerp(p2, u),
    };
    let mut shot = a.1;
    shot.eye = curve(p0.eye, a.1.eye, b.1.eye, p3.eye);
    shot.look = curve(p0.look, a.1.look, b.1.look, p3.look);
    shot.lens = a.1.lens + (b.1.lens - a.1.lens) * u;
    for (field, from, to) in [(&mut shot.yaw, a.1.yaw, b.1.yaw), (&mut shot.pitch, a.1.pitch, b.1.pitch), (&mut shot.distance, a.1.distance, b.1.distance), (&mut shot.height, a.1.height, b.1.height)] {
        *field = from + (to - from) * u;
    }
    if a.1.roll != b.1.roll {
        shot.roll = wrap_degrees(a.1.roll + wrap_degrees(b.1.roll - a.1.roll) * u);
    }
    Some(shot)
}

/// The shake channel's rotation at `seconds` of recording time.
pub(crate) fn shake(level: i32, seconds: f32) -> Quat {
    let amplitude = SHAKE_DEGREES[level.clamp(0, 3) as usize].to_radians();
    if amplitude == 0. {
        return Quat::IDENTITY;
    }
    let s = seconds;
    let x = (s * 7.3).sin() * 0.6 + (s * 13.1).sin() * 0.4;
    let y = (s * 5.7).sin() * 0.6 + (s * 11.9 + 1.).sin() * 0.4;
    Quat::from_euler(EulerRot::YXZ, y * amplitude, x * amplitude, 0.)
}

/// The editor camera at playhead `t`: None keeps the recorded camera (`subject.game`: the Game camera without shake,
/// zoom or roll). `recorded_fov` is the recorded frame's vertical field of view. `blend` (0..1: the time since the
/// last 60 Hz update over [`super::editor::UPDATE`]) carries a hand-steered shot on by that much of its last update's
/// step.
pub(crate) fn evaluate(editor: &Editor, t: f32, blend: f32, subject: Subject, recorded_fov: f32, seconds: f32) -> Option<(Transform, Option<f32>)> {
    let shot = match editor.hand() {
        // Bug 10-17 #28: display only, as Editor::shown carries the playhead on; the edit itself stays on the updates.
        // blend 0 is the update's own pose, so a 60 Hz draw shows what retail does.
        Some(shot) => match editor.before {
            Some(before) if before.kind == shot.kind => {
                let on = |from: f32, to: f32| to + (to - from) * blend;
                let mut ahead = shot;
                (ahead.eye, ahead.look) = (shot.eye + (shot.eye - before.eye) * blend, shot.look + (shot.look - before.look) * blend);
                (ahead.yaw, ahead.pitch, ahead.distance, ahead.height) = (on(before.yaw, shot.yaw), on(before.pitch, shot.pitch), on(before.distance, shot.distance), on(before.height, shot.height));
                ahead.lens = on(before.lens, shot.lens).clamp(LENS_MIN, LENS_MAX);
                if shot.roll != before.roll {
                    ahead.roll = wrap_degrees(shot.roll + wrap_degrees(shot.roll - before.roll) * blend);
                }
                ahead
            }
            _ => shot,
        },
        None => {
            let clip = editor.camera_at(t);
            if clip.kind.advanced() {
                interpolate(&editor.camera_keys(t), t).unwrap_or(clip)
            } else {
                clip
            }
        }
    };
    let level = editor.int_at(super::editor::Channel::Shake, t);
    let (mut transform, fov) = if shot.kind == CameraKind::Game {
        // Mode 9 steps the Game camera's lens (0x826116a0). RULING: 24 mm is the recorded lens, a longer one
        // narrows the recorded field of view by 24 / lens (the ICE game camera's projection is UNTRACED).
        let zoom = shot.lens > LENS_MIN;
        if level == 0 && shot.roll == 0. && !zoom {
            return None;
        }
        (subject.game, zoom.then(|| 2. * ((recorded_fov * 0.5).tan() * LENS_MIN / shot.lens).atan()))
    } else {
        let (eye, look) = view(&shot, subject);
        let transform = Transform::from_translation(eye).looking_at(if (look - eye).length_squared() < 1e-6 { eye + Vec3::NEG_Z } else { look }, Vec3::Y);
        (transform, Some(lens_fov(shot.lens)))
    };
    if shot.roll != 0. {
        transform.rotation *= Quat::from_rotation_z(shot.roll.to_radians());
    }
    transform.rotation *= shake(level, seconds);
    Some((transform, fov))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::editor::{Key, OPEN_PLAYHEAD, State};

    /// The skater at the origin and the game camera chasing it: 3 m behind, 0.5 m over its aim, looking at it.
    fn subject() -> Subject {
        Subject { position: Vec3::ZERO, game: Transform::from_xyz(0., 1.5, -3.).looking_at(Vec3::Y, Vec3::Y) }
    }
    /// The camera on screen when the game camera is.
    fn on_game() -> Transform {
        subject().game
    }
    /// One update with this edit's own D-pad total.
    fn steer1(shot: &mut Shot, sticks: Sticks) {
        steer(shot, sticks, subject(), &mut 0., &mut 0.);
    }
    fn elevation(v: Vec3) -> f32 {
        v.normalize().y.asin().to_degrees()
    }
    /// The eye's world yaw about the aim, from straight behind (the game camera's side): 0 at the start.
    fn turned(eye: Vec3) -> f32 {
        let offset = eye - subject().target();
        wrap_degrees(offset.x.atan2(offset.z).to_degrees() - 180.)
    }

    #[test]
    fn lens_clamps_24_75_in_half_mm() {
        let mut lens = LENS_MIN;
        lens = step_lens(lens, -1.);
        assert_eq!(lens, 24.);
        lens = step_lens(lens, 1.);
        assert_eq!(lens, 24.5);
        for _ in 0..200 {
            lens = step_lens(lens, 1.);
        }
        assert_eq!(lens, 75.);
        assert!(lens_fov(24.) > lens_fov(75.));
        assert!((lens_fov(24.).to_degrees() - 53.13).abs() < 0.1);
    }

    #[test]
    fn camera_edit_gains_match_retail() {
        // Follow from the game camera: the eye on it, 3.04 m from the aim, 9.46 degrees up.
        let mut shot = Shot::new(CameraKind::Follow);
        start_at(&mut shot, &on_game(), subject());
        let eye = |shot: &Shot| view(shot, subject()).0;
        let distance0 = (eye(&shot) - subject().target()).length();
        steer1(&mut shot, Sticks { right: Vec2::new(1., 0.), ..default() });
        assert!((turned(eye(&shot)) - 2.5).abs() < 1e-3, "x2.5 per update");
        assert!(((eye(&shot) - subject().target()).length() - distance0).abs() < 1e-4, "an orbit keeps the distance");
        let pitch0 = elevation(eye(&shot) - subject().target());
        steer1(&mut shot, Sticks { right: Vec2::new(4., -4.), ..default() });
        assert!((turned(eye(&shot)) - 5.5).abs() < 1e-3, "clamped to 3 per update");
        assert!((elevation(eye(&shot) - subject().target()) - (pitch0 - 3.)).abs() < 1e-3);
        steer1(&mut shot, Sticks { left: Vec2::new(0., 1.), ..default() });
        assert!(((eye(&shot) - subject().target()).length() - (distance0 - 0.2)).abs() < 1e-4, "dolly x1 clamped +-0.2, negated");
        let before = eye(&shot);
        steer1(&mut shot, Sticks { dpad: Vec2::new(1., 1.), ..default() });
        assert!((shot.height - 0.01).abs() < 1e-6 && shot.lens == 24.5);
        assert!(eye(&shot).distance(before) < 1e-5, "the aim step leaves the eye");
        let mut free = Shot::new(CameraKind::Free);
        free.look = Vec3::NEG_Z;
        let (rise, total) = (&mut 0., &mut 0.);
        steer(&mut free, Sticks { left: Vec2::new(1., 0.), dpad: Vec2::new(0., 1.), ..default() }, subject(), rise, total);
        assert!((free.eye.x - 0.2).abs() < 1e-6, "move clamped ±0.2");
        assert!((free.eye.y - 0.015).abs() < 1e-6);
        // 0x8261173c: no factor on the move; the height velocity decays x0.6 once the D-pad is released.
        let x = free.eye.x;
        steer(&mut free, Sticks { left: Vec2::new(0.05, 0.), ..default() }, subject(), rise, total);
        assert!((free.eye.x - x - 0.05).abs() < 1e-6, "move x1, not x2");
        assert!((free.eye.y - 0.015 - 0.009).abs() < 1e-6, "0.015 x 0.6 coasts on");
        assert!((*rise - 0.0054).abs() < 1e-6);
        // Held, the x0.6 decay settles the velocity at 0.015 / 0.4 = 0.0375 per update (0.0225 after the decay),
        // under its 0.2 clamp; look is clamp(x2.5, 0.8) x 2.0 degrees.
        for _ in 0..200 {
            steer(&mut free, Sticks { dpad: Vec2::new(0., 1.), ..default() }, subject(), rise, total);
        }
        assert!((*rise - 0.0225).abs() < 1e-5, "{rise}");
        let mut look = Shot::new(CameraKind::Free);
        look.look = Vec3::NEG_Z;
        steer1(&mut look, Sticks { right: Vec2::new(1., 0.), ..default() });
        let turned = (look.look - look.eye).normalize();
        assert!((turned.x.atan2(-turned.z).to_degrees() - 1.6).abs() < 1e-3, "0.8 x 2.0 = 1.6 degrees");
        // Tripod: height ±0.01 per update (0x82611a1c), not the Free velocity.
        let mut tripod = Shot::new(CameraKind::Tripod);
        steer1(&mut tripod, Sticks { dpad: Vec2::new(0., 1.), ..default() });
        assert!((tripod.height - 0.01).abs() < 1e-6 && tripod.eye == Vec3::ZERO);
    }

    /// #31: mode 8 (0x82611a1c, Tripod and Adv Tripod) hands LY / RX / RY to Follow's ICE orbit (0x82509558 ->
    /// 0x82c826d0): the right stick orbits the eye about the skater, LY dollies, LX is not read.
    #[test]
    fn tripod_right_stick_orbits_the_skater() {
        let target = subject().target();
        for kind in [CameraKind::Tripod, CameraKind::AdvTripod] {
            let mut shot = Shot::new(kind);
            shot.eye = target + Vec3::Z * 4.;
            steer1(&mut shot, Sticks { right: Vec2::new(1., 0.), ..default() });
            let offset = shot.eye - target;
            assert!((offset.length() - 4.).abs() < 1e-4, "{kind:?}: an orbit keeps the distance");
            assert!((offset.x.atan2(offset.z).to_degrees() - 2.5).abs() < 1e-3, "{kind:?}: x2.5 per update, Follow's way round");
            steer1(&mut shot, Sticks { right: Vec2::new(0., 4.), ..default() });
            assert!((elevation(shot.eye - target) - 3.).abs() < 1e-3, "{kind:?}: clamped 3 per update; up raises the eye");
            for _ in 0..40 {
                steer1(&mut shot, Sticks { right: Vec2::new(0., 1.), ..default() });
            }
            assert!((elevation(shot.eye - target) - ORBIT_PITCH_MAX).abs() < 1e-2, "{kind:?}: +-15473 BAM");
            let (before, direction) = ((shot.eye - target).length(), (shot.eye - target).normalize());
            steer1(&mut shot, Sticks { left: Vec2::new(1., 1.), ..default() });
            assert!(((shot.eye - target).length() - (before - 0.2)).abs() < 1e-4, "{kind:?}: LY dollies in 0.2");
            assert!((shot.eye - target).normalize().dot(direction) > 0.99999, "{kind:?}: LX does not truck");
            for _ in 0..100 {
                steer1(&mut shot, Sticks { left: Vec2::new(0., 1.), ..default() });
            }
            assert!(((shot.eye - target).length() - ORBIT_DISTANCE.0).abs() < 1e-4, "{kind:?}: 0.5 m at the closest");
            // Played back, the eye stays where the edit left it and looks at the skater.
            let (eye, look) = view(&shot, Subject { position: Vec3::X * 3., game: Transform::IDENTITY });
            assert_eq!((eye, look), (shot.eye, Vec3::new(3., LOOK_HEIGHT, 0.)));
        }
    }

    /// #31 / E: the stick-up direction per camera (0x82c826d0): Follow and Tripod raise the eye (the view tilts down),
    /// the Free camera lifts its look (tilts up); every pitch stops at +-85 degrees; Follow dollies 0.5..1000 m.
    #[test]
    fn stick_up_per_camera_and_the_pitch_limit() {
        let mut follow = Shot::new(CameraKind::Follow);
        start_at(&mut follow, &on_game(), subject());
        let (eye, look) = view(&follow, subject());
        let start = elevation(eye - look);
        steer1(&mut follow, Sticks { right: Vec2::new(0., 1.), ..default() });
        let (eye, look) = view(&follow, subject());
        assert!(elevation(look - eye) < 0. && (elevation(eye - look) - start - 2.5).abs() < 1e-3, "Follow: stick up tilts down");
        for _ in 0..100 {
            steer1(&mut follow, Sticks { right: Vec2::new(0., 1.), ..default() });
        }
        let (eye, look) = view(&follow, subject());
        assert!((elevation(eye - look) - ORBIT_PITCH_MAX).abs() < 1e-3);
        for _ in 0..100 {
            steer1(&mut follow, Sticks { left: Vec2::new(0., 1.), ..default() });
        }
        let (eye, look) = view(&follow, subject());
        assert!(((eye - look).length() - ORBIT_DISTANCE.0).abs() < 1e-4, "no dead travel past 0.5 m");
        steer1(&mut follow, Sticks { left: Vec2::new(0., -1.), ..default() });
        let (eye, look) = view(&follow, subject());
        assert!(((eye - look).length() - (ORBIT_DISTANCE.0 + 0.2)).abs() < 1e-4, "backs out at once");
        let mut free = Shot::new(CameraKind::Free);
        free.look = Vec3::NEG_Z;
        steer1(&mut free, Sticks { right: Vec2::new(0., 1.), ..default() });
        assert!((elevation(free.look - free.eye) - 1.6).abs() < 1e-3, "Free: stick up tilts up 0.8 x 2.0");
        for _ in 0..100 {
            steer1(&mut free, Sticks { right: Vec2::new(0., 1.), ..default() });
        }
        assert!((elevation(free.look - free.eye) - ORBIT_PITCH_MAX).abs() < 1e-3, "Free: +-85 degrees");
        for _ in 0..200 {
            steer1(&mut free, Sticks { right: Vec2::new(0., -1.), ..default() });
        }
        assert!((elevation(free.look - free.eye) + ORBIT_PITCH_MAX).abs() < 1e-3);
    }

    /// Bug 10-17 #28: a hand-steered camera (a camera edit, Instant Replay's stick orbit) is drawn `blend` of its last
    /// 60 Hz step past its update (0 = the update's own pose, as a 60 Hz draw shows); with nothing (or another camera)
    /// to carry on from it shows the current pose.
    #[test]
    fn hand_camera_moves_between_updates() {
        let mut editor = Editor::new(false);
        editor.state = State::Camera(CameraKind::Free);
        editor.shot = Shot::new(CameraKind::Free);
        editor.shot.eye = Vec3::new(0., 1., 5.);
        editor.shot.look = Vec3::new(0., 1., 0.);
        editor.before = editor.hand();
        steer1(&mut editor.shot, Sticks { left: Vec2::new(1., 0.), ..default() });
        let x = |editor: &Editor, blend| evaluate(editor, 0.5, blend, subject(), 1., 0.).unwrap().0.translation.x;
        assert_eq!(x(&editor, 0.), 0.2, "the update's own pose");
        assert!((x(&editor, 0.5) - 0.3).abs() < 1e-6);
        editor.before = None;
        assert_eq!(x(&editor, 0.5), 0.2, "nothing to carry on from");
        editor.before = Some(Shot::new(CameraKind::Tripod));
        assert_eq!(x(&editor, 0.5), 0.2, "another camera");
        // Instant Replay: the stick orbit moves on too (its first update only enters it).
        let mut editor = Editor::new(false);
        editor.playhead = 0.5;
        let turn = Sticks { right: Vec2::new(1., 0.), ..default() };
        for _ in 0..2 {
            orbit_update(&mut editor, turn, subject(), on_game());
        }
        editor.before = editor.hand();
        orbit_update(&mut editor, turn, subject(), on_game());
        let yaw = |blend| turned(evaluate(&editor, 0.5, blend, subject(), 1., 0.).unwrap().0.translation);
        assert!((yaw(0.) - 5.).abs() < 1e-3 && (yaw(0.5) - 6.25).abs() < 1e-2, "{} {}", yaw(0.), yaw(0.5));
    }

    /// E: LB / RB held roll a camera edit 1 degree per update (item 16), wrapping at +-180.
    #[test]
    fn camera_edit_update_rolls() {
        let mut editor = Editor::new(false);
        editor.state = State::Camera(CameraKind::Tripod);
        editor.shot = Shot::new(CameraKind::Tripod);
        editor.shot.eye = Vec3::new(2., 3., 4.);
        edit_update(&mut editor, Sticks::default(), 1., subject());
        assert_eq!(editor.shot.roll, ROLL_STEP);
        for _ in 0..180 {
            edit_update(&mut editor, Sticks::default(), 1., subject());
        }
        assert_eq!(editor.shot.roll, -179., "RB wraps through 180");
        edit_update(&mut editor, Sticks::default(), -1., subject());
        edit_update(&mut editor, Sticks::default(), 0., subject());
        assert_eq!((editor.shot.roll, editor.shot.eye), (-180., Vec3::new(2., 3., 4.)), "LB rolls back; neither held holds");
    }

    /// User 10-27 (audit 1a: "free camera ... sends you to a random place under the map"): a new Tripod / Free shot
    /// drew at the world origin on frames without a 60 Hz step, and the origin then became its start. start_edit puts
    /// every new shot at the camera on screen before anything draws it (1b: a Follow eye there, in the game camera's
    /// frame), with nothing to blend from.
    #[test]
    fn a_new_camera_shot_starts_at_the_camera_on_screen() {
        let on_screen = Transform::from_xyz(4., 2., -3.).looking_at(Vec3::Y, Vec3::Y);
        let skater = Subject { position: Vec3::ZERO, game: Transform::from_xyz(-2., 1.8, -2.5).looking_at(Vec3::Y, Vec3::Y) };
        for kind in [CameraKind::Tripod, CameraKind::AdvTripod, CameraKind::Free, CameraKind::AdvFree, CameraKind::Follow, CameraKind::AdvFollow] {
            let mut editor = Editor::new(false);
            (editor.state, editor.shot, editor.unplaced) = (State::Camera(kind), Shot::new(kind), true);
            editor.before = Some(Shot::new(kind));
            start_edit(&mut editor, on_screen, skater);
            assert!(!editor.unplaced && editor.before.is_none(), "{kind:?}");
            let (camera, _) = evaluate(&editor, 0.5, 0., skater, 1., 0.).unwrap();
            assert!(camera.translation.distance(on_screen.translation) < 1e-3, "{kind:?} starts on screen, not at the origin: {:?}", camera.translation);
            // Placed once: a later frame leaves the edit alone.
            editor.shot.eye += Vec3::X;
            let shot = editor.shot;
            start_edit(&mut editor, on_screen, skater);
            assert_eq!(editor.shot, shot, "{kind:?}");
        }
    }

    /// E / 1b: Instant Replay's stick orbit (0x8261135c): sticks at rest inside the margins (LY / RX / RY / D-pad; LX
    /// is not read) first enter Follow from the view on screen and move nothing (0x82611494), then steer it; playing
    /// holds it; scrubbing commits it, the clip under the playhead becoming that Follow camera (0x8261141c), and the
    /// next stick input carries on from that clip. Outside the margins nothing starts. 1f: the editor opens inside
    /// them (0.98), so the sticks work at once.
    #[test]
    fn instant_replay_stick_orbit() {
        let mut editor = Editor::new(false);
        editor.playhead = 1.;
        orbit_update(&mut editor, Sticks { right: Vec2::X, ..default() }, subject(), on_game());
        assert_eq!(editor.orbit, None, "outside the margins");
        editor.playhead = OPEN_PLAYHEAD;
        orbit_update(&mut editor, Sticks { left: Vec2::X, ..default() }, subject(), on_game());
        assert_eq!(editor.orbit, None, "LX is not read");
        let sticks = Sticks { left: Vec2::new(1., 1.), right: Vec2::X, ..default() };
        orbit_update(&mut editor, sticks, subject(), on_game());
        let entered = editor.orbit.expect("started where the editor opens");
        assert_eq!(entered.kind, CameraKind::Follow);
        let eye = |editor: &Editor| view(&editor.orbit.expect("orbiting"), subject()).0;
        assert!(eye(&editor).distance(on_game().translation) < 1e-5, "the first update enters and moves nothing");
        orbit_update(&mut editor, sticks, subject(), on_game());
        assert!((turned(eye(&editor)) - 2.5).abs() < 1e-3, "then RX turns it");
        let start = (on_game().translation - subject().target()).length();
        assert!(((eye(&editor) - subject().target()).length() - (start - 0.2)).abs() < 1e-4, "and LY dollies");
        editor.playing = true;
        let held = editor.orbit;
        orbit_update(&mut editor, sticks, subject(), on_game());
        assert_eq!(editor.orbit, held, "playing: held");
        editor.playing = false;
        editor.scrub(1., 0.);
        orbit_update(&mut editor, sticks, subject(), on_game());
        assert_eq!(editor.orbit, None, "scrubbing ends the orbit");
        assert_eq!(Some(editor.camera_at(editor.playhead)), held, "and commits it: the clip is that Follow camera");
        editor.scrub_velocity = 0.;
        orbit_update(&mut editor, sticks, subject(), on_game());
        assert_eq!(editor.orbit, held, "the next input enters from the committed clip as it is");
        // 1b: from another camera the orbit starts at the view on screen, wherever that is, looking at the skater.
        let mut editor = Editor::new(false);
        let on_screen = Transform::from_xyz(3., 2., 3.).looking_at(Vec3::Y, Vec3::Y);
        orbit_update(&mut editor, Sticks { right: Vec2::new(0.01, 0.), ..default() }, subject(), on_screen);
        let (eye, look) = view(&editor.orbit.expect("started"), subject());
        assert!(eye.distance(on_screen.translation) < 1e-4 && look == subject().target(), "{eye:?} {look:?}");
    }

    /// 1c (0x82c80ea0): D-pad up / down steps the aim on the skater 0.01 an update, +-1.0 in all per edit (the step
    /// that reaches 1.0 is dropped: 100 steps apply); the eye stays put in Follow and Tripod. Each edit's total starts
    /// from 0 (every camera entry zeroes cam+0x38), so a re-entered camera gets another metre.
    #[test]
    fn dpad_height_moves_the_aim_within_one_metre() {
        let (up, down) = (Sticks { dpad: Vec2::new(0., 1.), ..default() }, Sticks { dpad: Vec2::new(0., -1.), ..default() });
        let mut tripod = Shot::new(CameraKind::Tripod);
        tripod.eye = Vec3::new(0., 2., 5.);
        let total = &mut 0.;
        for _ in 0..300 {
            steer(&mut tripod, up, subject(), &mut 0., total);
        }
        assert!(tripod.height < AIM_MAX && tripod.height > AIM_MAX - 1e-4, "100 steps, the 101st dropped: {}", tripod.height);
        assert_eq!(*total, AIM_MAX);
        let (eye, look) = view(&tripod, subject());
        assert_eq!(eye, Vec3::new(0., 2., 5.), "a Tripod eye stays put");
        assert!((look - (subject().target() + Vec3::Y * tripod.height)).length() < 1e-6, "it aims about 1 m higher");
        let mut follow = Shot::new(CameraKind::Follow);
        start_at(&mut follow, &on_game(), subject());
        let (eye0, look0) = view(&follow, subject());
        let total = &mut 0.;
        for _ in 0..300 {
            steer(&mut follow, down, subject(), &mut 0., total);
        }
        let (eye1, look1) = view(&follow, subject());
        assert!(eye1.distance(eye0) < 1e-5, "a Follow eye stays put too");
        assert!((look1 - (look0 - Vec3::Y)).length() < 1e-4, "the aim drops about 1 m");
        let total = &mut 0.;
        for _ in 0..300 {
            steer(&mut follow, down, subject(), &mut 0., total);
        }
        assert!((follow.height + 2.).abs() < 1e-4, "a new edit, another metre: {}", follow.height);
    }

    /// 1d (0x82c80de0): the Free camera rises at most 10 m in an edit, and sinks at most 10 m back (the step reaching
    /// the limit dropped); the look comes along.
    #[test]
    fn the_free_camera_rises_ten_metres_at_most() {
        let mut free = Shot::new(CameraKind::Free);
        free.look = Vec3::NEG_Z;
        let (rise, total) = (&mut 0., &mut 0.);
        for _ in 0..2000 {
            steer(&mut free, Sticks { dpad: Vec2::new(0., 1.), ..default() }, subject(), rise, total);
        }
        assert!(free.eye.y < FREE_HEIGHT_MAX && free.eye.y > FREE_HEIGHT_MAX - 0.05, "{}", free.eye.y);
        assert_eq!(*total, FREE_HEIGHT_MAX);
        for _ in 0..4000 {
            steer(&mut free, Sticks { dpad: Vec2::new(0., -1.), ..default() }, subject(), rise, total);
        }
        assert!((free.eye.y + FREE_HEIGHT_MAX).abs() < 0.1, "{}", free.eye.y);
        assert!((free.look.y - free.eye.y).abs() < 1e-5, "the look comes along");
    }

    /// Report Q3 (0x82683680): while a camera is edited its eye stays within 40 m of the skater. A Free camera comes
    /// in onto the sphere keeping its view direction; a Follow / Tripod eye comes in along its line to the aim; inside
    /// 40 m, and for the Game camera, nothing changes. Instant Replay's orbit, dollied out, stops there too.
    #[test]
    fn an_edited_camera_stays_within_40_metres() {
        let skater = subject();
        let far = Vec3::new(0., 1., 60.);
        for kind in [CameraKind::Free, CameraKind::AdvFree, CameraKind::Tripod, CameraKind::AdvTripod, CameraKind::Follow, CameraKind::AdvFollow, CameraKind::Game] {
            let mut editor = Editor::new(false);
            (editor.state, editor.shot) = (State::Camera(kind), Shot::new(kind));
            editor.shot.eye = if matches!(kind, CameraKind::Follow | CameraKind::AdvFollow) { skater.to_game(far) } else { far };
            editor.shot.look = far + Vec3::NEG_Z;
            edit_update(&mut editor, Sticks::default(), 0., skater);
            let (eye, look) = view(&editor.shot, skater);
            match kind {
                CameraKind::Free | CameraKind::AdvFree => {
                    assert!((eye.length() - TETHER).abs() < 1e-3, "{kind:?}: on the sphere: {eye:?}");
                    assert!((look - eye - Vec3::NEG_Z).length() < 1e-5, "{kind:?}: same view direction");
                }
                CameraKind::Game => assert_eq!(editor.shot.eye, far, "the Game camera is never held"),
                _ => {
                    assert!(((eye - look).length() - TETHER).abs() < 1e-3, "{kind:?}: 40 m from the aim: {eye:?}");
                    assert!((eye - look).normalize().dot(Vec3::Z) > 0.9999, "{kind:?}: along its line");
                }
            }
        }
        let mut editor = Editor::new(false);
        (editor.state, editor.shot) = (State::Camera(CameraKind::Tripod), Shot::new(CameraKind::Tripod));
        editor.shot.eye = Vec3::new(0., 1., 10.);
        edit_update(&mut editor, Sticks::default(), 0., skater);
        assert_eq!(editor.shot.eye, Vec3::new(0., 1., 10.), "inside 40 m");
        let mut editor = Editor::new(false);
        let start = Transform::from_xyz(0., 1., -39.9).looking_at(Vec3::Y, Vec3::Y);
        for _ in 0..20 {
            orbit_update(&mut editor, Sticks { left: Vec2::new(0., -1.), ..default() }, skater, start);
        }
        let (eye, look) = view(&editor.orbit.expect("orbiting"), skater);
        assert!(((eye - look).length() - TETHER).abs() < 1e-3, "{eye:?}");
    }

    /// User 10-27: dollied out to the 40 m tether from above the skater (the aim 1 m over its feet), holding the stick
    /// up dollied nothing: each 0.2 m in left the eye over 40 m from the feet, and the tether pulled it back out.
    #[test]
    fn a_camera_at_the_tether_dollies_back_in() {
        let skater = subject();
        let mut editor = Editor::new(false);
        let start = Transform::from_xyz(0., 20., -20.).looking_at(Vec3::Y, Vec3::Y);
        for _ in 0..400 {
            orbit_update(&mut editor, Sticks { left: Vec2::new(0., -1.), ..default() }, skater, start);
        }
        let at = |editor: &Editor| {
            let (eye, look) = view(editor.orbit.as_ref().expect("orbiting"), skater);
            (eye - look).length()
        };
        let out = at(&editor);
        assert!(out > TETHER - 0.5, "out at the tether: {out}");
        for _ in 0..10 {
            orbit_update(&mut editor, Sticks { left: Vec2::new(0., 1.), ..default() }, skater, start);
        }
        assert!(at(&editor) < out - 1.5, "ten updates in, about 2 m: {} from {out}", at(&editor));
    }

    /// Report Q1 (0x82c7ffb8): a Follow eye rides the recorded game camera in its yaw frame (the game camera's pitch
    /// doesn't tilt it, the skater's own turns don't enter it) and looks at the skater with its aim offset.
    #[test]
    fn a_follow_eye_rides_the_game_camera() {
        let mut shot = Shot::new(CameraKind::Follow);
        (shot.eye, shot.height) = (Vec3::new(1., 0.5, -2.), 0.3);
        let at = |x: f32, turn: f32| Subject {
            position: Vec3::new(x, 0., 0.),
            game: Transform::from_xyz(x, 1.5, -3.).with_rotation(Quat::from_rotation_y(turn) * Quat::from_rotation_x(-0.2)),
        };
        let (eye, look) = view(&shot, at(0., 0.));
        assert!(eye.distance(Vec3::new(1., 2., -5.)) < 1e-5, "{eye:?}");
        assert!((look - Vec3::new(0., LOOK_HEIGHT + 0.3, 0.)).length() < 1e-6);
        let turn = std::f32::consts::FRAC_PI_2;
        let (eye, look) = view(&shot, at(5., turn));
        let expected = Vec3::new(5., 1.5, -3.) + Quat::from_rotation_y(turn) * Vec3::new(1., 0.5, -2.);
        assert!(eye.distance(expected) < 1e-4, "5 m on and turned: {eye:?} vs {expected:?}");
        assert!((look - Vec3::new(5., LOOK_HEIGHT + 0.3, 0.)).length() < 1e-6);
    }

    /// A save from before 10-27: migrate_follow turns each Follow shot (the skater's frame, the eye raised by
    /// `height`) into the eye it drew at its key, in the game camera's frame; the view at the key is unchanged.
    #[test]
    fn an_old_saves_follow_shot_keeps_its_view_at_its_key() {
        let root = Transform::from_xyz(2., 0.5, 1.).with_rotation(Quat::from_rotation_y(0.7));
        let game = Transform::from_xyz(-1., 1.8, -2.).looking_at(Vec3::new(2., 1.5, 1.), Vec3::Y);
        let mut old = Shot::new(CameraKind::Follow);
        (old.yaw, old.pitch, old.distance, old.height) = (30., -10., 0.5, 0.2);
        // The view the earlier model drew: an orbit about the skater's heading, the eye raised by its height.
        let target = root.translation + Vec3::Y * LOOK_HEIGHT;
        let back = Quat::from_rotation_y(0.7 + std::f32::consts::PI + 30f32.to_radians()) * Quat::from_rotation_x(10f32.to_radians()) * Vec3::Z;
        let drawn = target + back * 2.5 + Vec3::Y * 0.2;
        let mut editor = Editor::new(false);
        editor.keys[0].push(Key { t: 0.4, soft: false, cut: false, value: Value::Camera(old), ease: Ease::Smooth });
        editor.legacy_follow = true;
        migrate_follow(&mut editor, |t| (t == 0.4).then_some((root, game)));
        let Value::Camera(shot) = editor.keys[0][0].value else { panic!("a camera key") };
        let (eye, look) = view(&shot, Subject::new(&root, game));
        assert!(eye.distance(drawn) < 1e-4 && look.distance(target) < 1e-6, "{eye:?} vs {drawn:?}");
        assert!(!editor.legacy_follow);
    }

    /// Stage 0 §2: replay01-04 are never loaded in S3; the Game camera is the recorded game camera.
    #[test]
    fn game_camera_is_recorded_camera() {
        let editor = Editor::new(false);
        let recorded = Transform::from_xyz(3., 2., 1.);
        let at = Subject { position: Vec3::ZERO, game: recorded };
        assert_eq!(evaluate(&editor, 0.5, 0., at, 1., 0.), None);
        // A Game camera zoomed in its edit (0x826116a0 steps the lens) narrows the recorded field of view.
        let mut editor = Editor::new(false);
        editor.state = State::Camera(CameraKind::Game);
        editor.shot = Shot::new(CameraKind::Game);
        editor.shot.lens = 48.;
        let (t, fov) = evaluate(&editor, 0.5, 0., at, 1., 0.).unwrap();
        assert_eq!(t, recorded);
        assert!((fov.unwrap() - 2. * (0.5_f32.tan() * 0.5).atan()).abs() < 1e-6);
    }

    #[test]
    fn adv_keys_interpolate() {
        let key = |t: f32, x: f32, lens: f32, cut: bool| {
            let mut s = Shot::new(CameraKind::AdvFree);
            s.eye = Vec3::new(x, 0., 0.);
            s.lens = lens;
            (t, s, cut, Ease::Smooth)
        };
        let keys = [key(0.2, 0., 24., false), key(0.4, 1., 50., false), key(0.6, 2., 50., false), key(0.8, 3., 75., false)];
        // Catmull-Rom through evenly spaced points on a line is the line (inner segments).
        assert!((interpolate(&keys, 0.5).unwrap().eye.x - 1.5).abs() < 1e-5);
        assert!((interpolate(&keys, 0.3).unwrap().lens - 37.).abs() < 1e-4, "linear lens");
        assert_eq!(interpolate(&keys, 0.1).unwrap().eye.x, 0.);
        assert_eq!(interpolate(&keys, 0.9).unwrap().eye.x, 3.);
        // A cut key holds its shot for 0.02.
        let cut = [key(0.2, 0., 24., false), key(0.4, 1., 24., true), key(0.6, 2., 24., false)];
        assert!((interpolate(&cut, 0.41).unwrap().eye.x - 1.).abs() < 1e-5);
        assert!(interpolate(&cut, 0.5).unwrap().eye.x > 1.);
    }

    /// Item 16: each Adv key's ease shapes its segment: Hold steps, Linear lerps, Smooth is retail's Catmull-Rom;
    /// the ends hold whatever the ease; a cut holds CUT_HOLD and then eases from its shot.
    #[test]
    fn camera_eases_per_key() {
        let shot = |x: f32| {
            let mut s = Shot::new(CameraKind::AdvFree);
            s.eye = Vec3::new(x, 0., 0.);
            s.look = Vec3::new(x, 0., -1.);
            s
        };
        let keys = |ease| vec![(0.2, shot(0.), false, ease), (0.4, shot(10.), false, ease), (0.6, shot(30.), false, ease)];
        let x = |ease, t| interpolate(&keys(ease), t).unwrap().eye.x;
        assert_eq!((x(Ease::Hold, 0.3), x(Ease::Hold, 0.5)), (0., 10.));
        assert!((x(Ease::Linear, 0.3) - 5.).abs() < 1e-4);
        let smooth = crate::replay::editor::catmull(0., 0., 10., 30., 0.5);
        assert!((x(Ease::Smooth, 0.3) - smooth).abs() < 1e-4 && (smooth - 5.).abs() > 0.1, "{}", x(Ease::Smooth, 0.3));
        for ease in Ease::ALL {
            assert_eq!((x(ease, 0.1), x(ease, 0.2), x(ease, 0.6), x(ease, 0.9)), (0., 0., 30., 30.), "{ease:?}");
        }
        let mut cut = keys(Ease::Linear);
        cut[1].2 = true;
        assert_eq!(interpolate(&cut, 0.41).unwrap().eye.x, 10.);
        assert!((interpolate(&cut, 0.51).unwrap().eye.x - 20.).abs() < 1e-3, "from the hold's end: u = 0.5");
    }

    /// Item 16: roll eases the short way through +-180 and stays in -180..180; equal rolls stay exact.
    #[test]
    fn roll_turns_the_short_way() {
        let mut a = Shot::new(CameraKind::AdvFree);
        a.roll = 170.;
        let mut b = a;
        b.roll = -170.;
        b.eye = Vec3::X;
        let keys = [(0.2, a, false, Ease::Linear), (0.4, b, false, Ease::Hold)];
        let roll = |t| interpolate(&keys, t).unwrap().roll;
        assert!((roll(0.3).abs() - 180.).abs() < 1e-3, "through 180, not 0: {}", roll(0.3));
        assert!((roll(0.25) - 175.).abs() < 1e-3);
        assert!((roll(0.35) + 175.).abs() < 1e-3);
        assert_eq!(roll(0.5), -170.);
        assert_eq!((wrap_degrees(190.), wrap_degrees(180.), wrap_degrees(-180.), wrap_degrees(-540.)), (-170., -180., -180., -180.));
        b.roll = 170.;
        let flat = [(0.2, a, false, Ease::Smooth), (0.4, b, false, Ease::Smooth)];
        assert_eq!(interpolate(&flat, 0.3).unwrap().roll, 170.);
        // The evaluated camera turns about its view axis.
        let mut editor = Editor::new(false);
        editor.state = State::Camera(CameraKind::Free);
        editor.shot = Shot::new(CameraKind::Free);
        editor.shot.eye = Vec3::new(0., 1., 5.);
        editor.shot.look = Vec3::new(0., 1., 0.);
        let level = evaluate(&editor, 0.5, 0., subject(), 1., 0.).unwrap().0;
        editor.shot.roll = 90.;
        let rolled = evaluate(&editor, 0.5, 0., subject(), 1., 0.).unwrap().0;
        assert!(level.forward().dot(*rolled.forward()) > 0.9999, "same view direction");
        assert!((rolled.up().dot(*level.right()) + 1.).abs() < 1e-4 || (rolled.up().dot(*level.right()) - 1.).abs() < 1e-4, "up turned 90 degrees");
    }

    #[test]
    fn shake_levels_grow() {
        let at = |level| shake(level, 0.37).angle_between(Quat::IDENTITY);
        assert_eq!(at(0), 0.);
        assert!(at(1) < at(2) && at(2) < at(3));
    }
}
