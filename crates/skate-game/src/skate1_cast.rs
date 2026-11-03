//! Skate 1's tutorial people (P4): Shingo and Seb at their GlobalSpawn rows (research.md 4.1 / 4.2 / 4.2a,
//! research7.md 2), the AI riders' and the running scene's cast keys, all drawn by skate2_cast's Cast from P1's
//! `private/challenges-skate1/cast/<model>.glb` (old_challenges_cast.py). skate1_challenge.rs sets [`People`]'s pub
//! fields; this module owns the Cast keys it writes and nothing else.
use crate::{
    app::{FrameSet, SimulationSet},
    challenge::cutscene::{Clip1, Clip1Meta, Cutscene},
    config::Config,
    physics::{SkaterRuntime, riders::Riders},
    skate1_rider::{Rng, Skate1Ai},
    skate2_cast::Cast,
};
use bevy::prelude::*;
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub(crate) const FOLDER: &str = "private/challenges-skate1";
/// Entity keys (class 60D703C77CAB1631, research 4.2): "IP Shingo" and "Photographer" (Seb).
const SHINGO: &str = "0D0BDC7925D47161";
const SEB: &str = "7E5E359D6CE96AEC";
pub(crate) type M4 = [[f32; 4]; 4];

/// tGlobalSpawnCharacterInfo (research 4.1): entity key, locator, start mode `a`, second locator `b`.
#[derive(Deserialize, Clone)]
struct Spawn {
    key: String,
    locator: String,
    mode: u8,
    second: Option<String>,
}
#[derive(Deserialize)]
struct Mission {
    #[serde(rename = "GlobalSpawn")]
    spawn: Option<Vec<Spawn>>,
    /// Hash_16E7E79B28332502: a start removes every other challenge's NPCs.
    #[serde(rename = "ClearOtherNPCs", default)]
    clear: bool,
}
#[derive(Deserialize)]
struct MissionRow {
    #[serde(rename = "self")]
    row: Mission,
}
#[derive(Deserialize)]
struct Locator {
    matrix: M4,
}
#[derive(Deserialize)]
struct Places {
    locators: BTreeMap<String, Locator>,
}
#[derive(Deserialize)]
struct Tutorial {
    missions: BTreeMap<String, MissionRow>,
    /// P6 R1: the four pro tutorials' rows (RETAIL P1 export, same `self` shape); absent in pre-P6 exports.
    #[serde(default)]
    pro_tutorials: BTreeMap<String, MissionRow>,
    places: Places,
}
/// A living-world entity (cast.json `entities`): the model drawn and its remap rows (class 53E332F6).
#[derive(Deserialize)]
struct Npc {
    draw: String,
    #[serde(default)]
    remap: BTreeMap<String, Vec<String>>,
}
#[derive(Deserialize)]
struct CastFile {
    entities: BTreeMap<String, Npc>,
    clips: BTreeMap<String, Value>,
}

/// P1's mission spawn rows and locators (tutorial.json), entities and clip metas (cast.json).
struct Data {
    missions: BTreeMap<String, Mission>,
    locators: BTreeMap<String, M4>,
    npcs: BTreeMap<String, Npc>,
    /// Clip metas, read as cutscene.rs's `Clip1Meta` when a member first plays one.
    clips: BTreeMap<String, Value>,
}

impl Data {
    fn load(root: &Path) -> Result<Self, String> {
        let read = |file: &str| -> Result<Value, String> {
            let bytes = std::fs::read(root.join(FOLDER).join(file)).map_err(|e| format!("{file}: {e}"))?;
            serde_json::from_slice(&bytes).map_err(|e| format!("{file}: {e}"))
        };
        Self::parse(read("tutorial.json")?, read("cast.json")?)
    }
    fn parse(tutorial: Value, cast: Value) -> Result<Self, String> {
        let t: Tutorial = serde_json::from_value(tutorial).map_err(|e| format!("tutorial.json: {e}"))?;
        let c: CastFile = serde_json::from_value(cast).map_err(|e| format!("cast.json: {e}"))?;
        Ok(Self {
            missions: t.missions.into_iter().chain(t.pro_tutorials).map(|(name, m)| (name, m.row)).collect(),
            locators: t.places.locators.into_iter().map(|(name, l)| (name, l.matrix)).collect(),
            npcs: c.entities,
            clips: c.clips,
        })
    }

    /// The rows standing now (research 4.1), as (entity, locator), in mission order and once each (C3 INFERRED:
    /// one entity on one locator from two challenges is two records drawn on one spot; drawn once here):
    /// - a row is live while its challenge is available (0x823d0d20: unlocked and not completed);
    /// - while `running` runs (its start cleanup done, 0x823d82b0), its mode-0 rows leave, mode 1 stays, and mode 2
    ///   moves to `second` when that locator is loaded, else keeps its own (RETAIL research7 2, 0x823d7158);
    /// - if `running` sets ClearOtherNPCs, every other challenge's rows leave.
    ///
    /// UNTRACED C1: the spawn range (research7 2: the entity row's float + 10 m for marquee pros; which float is
    /// untraced). Fallback: every live row stands.
    fn live(&self, available: &BTreeSet<String>, running: Option<&str>) -> Vec<(String, String)> {
        let clear = running.and_then(|r| self.missions.get(r)).is_some_and(|m| m.clear);
        let mut out: Vec<(String, String)> = Vec::new();
        for (name, m) in self.missions.iter().filter(|(n, _)| available.contains(*n)) {
            let own = running == Some(name.as_str());
            for s in m.spawn.iter().flatten() {
                let at = match (own, s.mode) {
                    (false, _) if clear => continue,
                    (true, 0) => continue,
                    (true, 2) => s.second.as_ref().filter(|l| self.locators.contains_key(*l)).unwrap_or(&s.locator),
                    _ => &s.locator,
                };
                let row = (s.key.clone(), at.clone());
                if !out.contains(&row) {
                    out.push(row);
                }
            }
        }
        out
    }

    /// Each row's cast key (its entity's `draw` model, `model#2` for a second one), entity and locator matrix;
    /// rows whose entity or locator P1 didn't export are dropped.
    fn cast_keys(&self, rows: &[(String, String)]) -> Vec<(String, String, M4)> {
        let mut out: Vec<(String, String, M4)> = Vec::new();
        for (entity, locator) in rows {
            let (Some(npc), Some(at)) = (self.npcs.get(entity), self.locators.get(locator)) else { continue };
            let n = out.iter().filter(|(k, ..)| k.split('#').next() == Some(npc.draw.as_str())).count();
            let key = if n == 0 { npc.draw.clone() } else { format!("{}#{}", npc.draw, n + 1) };
            out.push((key, entity.clone(), *at));
        }
        out
    }
}

const PROP_ROOT: &str = "PROP_ROOT";
/// The entities that carry a board in their idle: Shingo's marquee recipe has `shingo_skateboard`, Seb's has none
/// (research 4.2). P6 R2 RETAIL: each pro's marquee recipe lists his board too (Paul_rodriguez.xml
/// paul_rodriquez_skateboard_LOD, jason_dill.xml, mike_carroll.xml, Chris_haslam.xml) on Shingo's idle remap 66F08967.
/// User 10-15: Danny Way's and Cuz's exported models carry the same board part (Retail_Equipment, 0.9 m long).
const BOARD: [&str; 7] = [SHINGO, PROD, DILL, CARROLL, HASLAM, DANNY, CUZ];
/// The pros' GlobalSpawn entity keys (RETAIL, P1 export of fp_u3_tutorial02 / 01 / 05 / 04).
const PROD: &str = "B033A0ADE0933A58";
const DILL: &str = "0DED833242F2F857";
const CARROLL: &str = "146D3B7D88385267";
const HASLAM: &str = "5B87C6981CA2AE86";
/// fp_u3_tutorial03's pro and he_u4_besttrick01's host (P1 export, cast.json "Danny Way" / "cuz").
const DANNY: &str = "42545B0938E341C9";
const CUZ: &str = "8C0D2AE0C53B259C";

/// The remap pick (research 4.2, 0x8281ca60): entry `r % n` of an array, entry 0 for n <= 1.
/// UNTRACED C4: re-picked per loop or per graph entry. Fallback: the caller picks once per spawn.
fn variant(n: usize, r: u32) -> usize {
    if n <= 1 { 0 } else { r as usize % n }
}

/// `meta`'s clip (a cast.json `clips` row, its file under `dir`) posed onto a member's bones by name, PROP_ROOT last.
fn member_clip(dir: &Path, meta: &Value, names: &[String]) -> Result<Clip1, String> {
    let meta: Clip1Meta = serde_json::from_value(meta.clone()).map_err(|e| e.to_string())?;
    let mut clip = Clip1::load(dir, meta)?;
    let mut bones = names.to_vec();
    bones.push(PROP_ROOT.into());
    clip.remap(&bones);
    Ok(clip)
}

/// An idle frame on a member's bones. UNTRACED C12 (spec: a 48-bone pedestrian clip on the 125-joint model):
/// fallback by name in model space, a joint the clip lacks keeps its transform (NaN). For a board carrier,
/// SKATEBOARD_ROOT sits on the clip's PROP_ROOT at identity: C5 INFERRED (the remap rows carry no prop offset),
/// checked by `the_idles_stand_on_the_ground_with_the_board_in_hand`.
fn stand_pose(clip: &Clip1, time: f32, names: &[String], board: bool) -> Vec<Mat4> {
    let mut pose = clip.pose(time, &[]);
    let prop = pose.pop().unwrap_or(Mat4::NAN);
    if board && let Some(b) = names.iter().position(|n| n.eq_ignore_ascii_case("SKATEBOARD_ROOT")) {
        pose[b] = prop;
    }
    pose
}

/// IsFacingSkater's gate (research 4.2): (FOV rad, range m) to start tracking, and to keep tracking.
#[derive(Clone, Copy)]
struct Gate {
    into: (f32, f32),
    out: (f32, f32),
}
/// marquee.xml: TrackingSkater at FOV 1.4 within 6 m, back to HoldAtSpawn outside 1.5 / 7 m.
const SHINGO_GATE: Gate = Gate { into: (1.4, 6.), out: (1.5, 7.) };
/// Photographer.xml: 1.4 / 12 m in, 1.5 / 13 m out.
const SEB_GATE: Gate = Gate { into: (1.4, 12.), out: (1.5, 13.) };
/// UNTRACED C7: ProTrackSkater's turn rate. Fallback: pi rad/s.
const TURN_RATE: f32 = std::f32::consts::PI;

/// One frame of TrackingSkater: whether he tracks after it, and his yaw (forward = (sin yaw, 0, cos yaw)).
/// `always` = TrackLocalSkater (Seb shooting). RETAIL (research7 2): he turns only with the player inside his
/// +-FOV/2 cone and range; otherwise he keeps his heading. UNTRACED C6: the range in 3D or flat; fallback 3D.
/// C8 INFERRED (research7 2): HoldAtSpawn does not turn him back to the locator heading.
fn track(tracking: bool, yaw: f32, at: Vec3, player: Vec3, gate: Gate, always: bool, dt: f32) -> (bool, f32) {
    let d = player - at;
    let off = wrap(d.x.atan2(d.z) - yaw);
    let (fov, range) = if tracking { gate.out } else { gate.into };
    let tracking = always || (off.abs() <= fov * 0.5 && d.length() <= range);
    if !tracking || Vec2::new(d.x, d.z).length() < 1e-3 {
        return (tracking, yaw);
    }
    (true, wrap(yaw + off.clamp(-TURN_RATE * dt, TURN_RATE * dt)))
}

fn wrap(a: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    (a + PI).rem_euclid(TAU) - PI
}

/// Seb's PictureTaking / CheckOutPictures (Photographer.xml, MotionGraph_Photographer.xml; research 4.2). Which
/// clip plays on which edge is INFERRED from the clip names: stand -> shoot SHOOT_FROMSTAND_INTO, shoot -> review
/// SHOOT_FROMREVIEW_OUT, review -> shoot SHOOT_FROMREVIEW_INTO, review -> stand STAND_REVIEW_OUT.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Photo {
    #[default]
    Idle,
    Into,
    Shoot,
    ToReview,
    Review,
    BackToShoot,
    FromReview,
}

impl Photo {
    /// The state's clip; Idle plays the IdleBasicCyc variant. UNTRACED C9: PHOTO_SHOOT_BLEND (type 6) is not
    /// decoded. Fallback: its neutral PHOT_SHOOT_IDLE_N_0_CYC.
    fn clip(self) -> Option<&'static str> {
        Some(match self {
            Photo::Idle => return None,
            Photo::Into => "PHOT_SHOOT_FROMSTAND_N_0_INTO",
            Photo::Shoot => "PHOT_SHOOT_IDLE_N_0_CYC",
            Photo::ToReview => "PHOT_SHOOT_FROMREVIEW_N_0_OUT",
            Photo::Review => "PHOT_STAND_REVIEW_N_0_CYC",
            Photo::BackToShoot => "PHOT_SHOOT_FROMREVIEW_N_0_INTO",
            Photo::FromReview => "PHOT_STAND_REVIEW_N_0_OUT",
        })
    }
    /// The next state: `shooting` = IsPhotographerTakingPictures, `done` = this state's clip has played once.
    /// UNTRACED C10: how long he reviews. Fallback: one cycle.
    fn next(self, shooting: bool, done: bool) -> Photo {
        match (self, shooting, done) {
            (Photo::Idle, true, _) => Photo::Into,
            (Photo::Into | Photo::BackToShoot, _, true) => Photo::Shoot,
            (Photo::Shoot, false, _) => Photo::ToReview,
            (Photo::ToReview, _, true) => Photo::Review,
            (Photo::Review, true, _) => Photo::BackToShoot,
            (Photo::Review, false, true) => Photo::FromReview,
            (Photo::FromReview, _, true) => Photo::Idle,
            (state, ..) => state,
        }
    }
}

/// UNTRACED C11: the graph's blend times are 1.0 and its cycles 0.1 (research 4.2), per edge. Fallback: 0.1 s on
/// every clip change.
const BLEND: f32 = 0.1;

/// Bone by bone in model space: scale and translation lerped, rotation slerped; a NaN bone on either side takes `to`.
fn blend(from: &[Mat4], to: &[Mat4], w: f32) -> Vec<Mat4> {
    from.iter()
        .zip(to)
        .map(|(a, b)| {
            if a.is_nan() || b.is_nan() {
                return *b;
            }
            let (sa, ra, ta) = a.to_scale_rotation_translation();
            let (sb, rb, tb) = b.to_scale_rotation_translation();
            Mat4::from_scale_rotation_translation(sa.lerp(sb, w), ra.slerp(rb, w), ta.lerp(tb, w))
        })
        .collect()
}

/// The lead's state in (skate1_challenge.rs sets the pub fields each frame), the people's own state.
#[derive(Resource, Default)]
pub(crate) struct People {
    /// The Skate 1 tutorial owns the cast: Skate 1 map, offline, its data loaded. false releases it.
    pub on: bool,
    /// Challenge names unlocked and not completed (a running one included).
    pub available: BTreeSet<String>,
    /// The challenge whose start cleanup ran (state 3, research 4.1), until it ends.
    pub running: Option<String>,
    /// LivingWorld.StartTakingPhotos .. StopTakingPhotos (IsPhotographerTakingPictures).
    pub photographing: bool,
    data: Option<Data>,
    failed: bool,
    /// Cast keys this module put into Cast.
    mine: BTreeSet<String>,
    stands: BTreeMap<String, Stand>,
    /// (cast key, clip) -> the clip on that member's bones; None = failed (warned once).
    clips: BTreeMap<(String, String), Option<Clip1>>,
    rng: Rng,
}

/// A standing member's state.
struct Stand {
    at: M4,
    yaw: f32,
    tracking: bool,
    idle: usize,
    photo: Photo,
    time: f32,
    /// The pose faded out after a clip change, and the fade so far (0..1).
    from: Option<(Vec<Mat4>, f32)>,
    last: Vec<Mat4>,
}
impl Stand {
    /// RETAIL (research7 2, 0x823d7158): a spawned NPC faces its locator's own rotation; the At row (row 2), the
    /// model facing +Z.
    fn new(at: M4, idle: usize) -> Self {
        let yaw = at[2][0].atan2(at[2][2]);
        Self { at, yaw, tracking: false, idle, photo: Photo::Idle, time: 0., from: None, last: Vec::new() }
    }
}

/// The S1 cast this frame: the stands, the riders (their own `#s1ai` copy of the model), then the running scene's
/// actors not already placed, at its base (as skate2_challenge place_cast).
fn wanted(stands: &[(String, String, M4)], riders: &[(String, M4)], actors: Vec<(String, M4)>) -> BTreeMap<String, M4> {
    let mut wanted: BTreeMap<String, M4> = stands.iter().map(|(key, _, at)| (key.clone(), *at)).collect();
    wanted.extend(riders.iter().cloned());
    for (key, at) in actors {
        wanted.entry(key).or_insert(at);
    }
    wanted
}

/// Writes the S1 cast into skate2_cast's Cast, touching only the keys it owns; the GLBs come from FOLDER/cast.
fn apply(mine: &mut BTreeSet<String>, cast: &mut Cast, wanted: BTreeMap<String, M4>, riding: BTreeSet<String>) {
    for key in mine.iter().filter(|k| !wanted.contains_key(*k)) {
        cast.wanted.remove(key);
        cast.poses.remove(key);
    }
    cast.riding.retain(|k| !mine.contains(k));
    cast.riding.extend(riding);
    for (key, at) in &wanted {
        if cast.wanted.get(key) != Some(at) {
            cast.wanted.insert(key.clone(), *at);
        }
    }
    cast.folder = FOLDER;
    *mine = wanted.into_keys().collect();
}

/// The S1 tutorial gave the cast up (map change, session, data gone): its keys leave, the folder goes back.
fn release(mine: &mut BTreeSet<String>, cast: &mut Cast) {
    apply(mine, cast, BTreeMap::new(), BTreeSet::new());
    cast.folder = "";
}

/// Whether the stand writes `key`'s pose and facing: not while a cutscene holds it (its stale pose is dropped).
fn posable(cast: &mut Cast, key: &str) -> bool {
    let free = !cast.held.contains(key);
    if !free {
        cast.poses.remove(key);
    }
    free
}

pub(crate) struct Skate1PeoplePlugin;
impl Plugin for Skate1PeoplePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<People>()
            .init_resource::<Skate1Ai>()
            .add_systems(Update, people.after(FrameSet::Physics).before(FrameSet::Animation))
            .add_systems(
                FixedUpdate,
                crate::skate1_rider::drive.after(SimulationSet::Physics).before(crate::physics::riders::advance),
            );
    }
}

fn people(
    mut people: ResMut<People>,
    mut cast: ResMut<Cast>,
    ai: Res<Skate1Ai>,
    cutscene: Res<Cutscene>,
    riders: Option<Res<Riders>>,
    (config, skater, real): (Res<Config>, Res<SkaterRuntime>, Res<Time<Real>>),
    mut transforms: Query<&mut Transform>,
    mut logged: Local<f32>,
) {
    let cast = &mut *cast;
    let People { on, available, running, photographing, data, failed, mine, stands, clips, rng } = &mut *people;
    if !*on {
        if !mine.is_empty() {
            release(mine, cast);
        }
        stands.clear();
        clips.clear();
        return;
    }
    if data.is_none() && !*failed {
        match Data::load(&config.asset_root) {
            Ok(d) => *data = Some(d),
            Err(error) => {
                warn!("Skate 1 people unavailable ({error}). Prepare them with skate2port/old_challenges_cast.py");
                *failed = true;
            }
        }
    }
    let Some(data) = data.as_ref() else { return };
    let standing = data.cast_keys(&data.live(available, running.as_deref()));
    let rider_keys = ai.riders();
    let riding: BTreeSet<String> = rider_keys.iter().map(|(k, _)| k.clone()).collect();
    apply(mine, cast, wanted(&standing, &rider_keys, cutscene.actors()), riding.clone());
    // A full-skater rider is posed in world space (skate2_cast pose): its root at the origin once it rides.
    for key in riding.iter().filter(|k| riders.as_deref().is_some_and(|r| r.get(k).is_some())) {
        if let Some(root) = cast.entity(key)
            && let Ok(mut t) = transforms.get_mut(root)
        {
            *t = Transform::IDENTITY;
        }
    }
    // Playtest 10-15 diagnostic (the riding AI is invisible): his root against where his posed bones are, every 2 s.
    // Bones near the origin while the root is far mean the pose isn't world space under the identity root.
    // ponytail: remove once a preview log has settled it.
    *logged -= real.delta_secs();
    if *logged <= 0. {
        for key in &riding {
            let Some(rider) = riders.as_deref().and_then(|r| r.get(key)) else { continue };
            let root = rider.root();
            let pose = rider.pose();
            let far = pose.iter().map(|m| m.w_axis.truncate().length()).fold(0_f32, f32::max);
            info!(
                "Skate 1 rider diag: {key} root=({:.1},{:.1},{:.1}) bones={} first_bone={:?} farthest_bone_from_origin={far:.1}",
                root[3][0],
                root[3][1],
                root[3][2],
                pose.len(),
                pose.first().map(|m| m.w_axis.truncate())
            );
            *logged = 2.;
        }
    }
    let m = &skater.animated_skeleton.roots.animation_to_world;
    let player = Vec3::new(m[3][0], m[3][1], m[3][2]);
    let dt = real.delta_secs().min(0.1);
    stands.retain(|key, s| standing.iter().any(|(k, _, at)| k == key && *at == s.at));
    for (key, entity, at) in &standing {
        if !posable(cast, key) {
            continue;
        }
        let Some(npc) = data.npcs.get(entity) else { continue };
        let idles = npc.remap.get("IdleBasicCyc").map_or(&[][..], Vec::as_slice);
        let s = stands.entry(key.clone()).or_insert_with(|| Stand::new(*at, variant(idles.len(), rng.next())));
        let seb = entity == SEB;
        let shooting = *photographing && seb;
        let gate = if seb { SEB_GATE } else { SHINGO_GATE };
        (s.tracking, s.yaw) = track(s.tracking, s.yaw, Vec3::from_slice(&at[3][..3]), player, gate, shooting, dt);
        let (Some(root), Some(names)) = (cast.entity(key), cast.bones(key).map(<[String]>::to_vec)) else { continue };
        if let Ok(mut t) = transforms.get_mut(root) {
            t.rotation = Quat::from_rotation_y(s.yaw);
        }
        let Some(name) = s.photo.clip().map(str::to_owned).or_else(|| idles.get(s.idle).cloned()) else { continue };
        let clip = clips.entry((key.clone(), name.clone())).or_insert_with(|| {
            let meta = data.clips.get(&name).ok_or_else(|| format!("no clip {name}"));
            meta.and_then(|meta| member_clip(&config.asset_root.join(FOLDER), meta, &names))
                .map_err(|error| warn!("Skate 1 people: {key} can't play {name} ({error})"))
                .ok()
        });
        let Some(clip) = clip.as_ref() else { continue };
        s.time += dt;
        let length = clip.length();
        if seb {
            let next = s.photo.next(shooting, s.time >= length);
            if next != s.photo {
                (s.photo, s.time, s.from) = (next, 0., Some((s.last.clone(), 0.)));
                continue;
            }
        }
        let looping = matches!(s.photo, Photo::Idle | Photo::Shoot | Photo::Review);
        let t = if looping && length > 0. { s.time % length } else { s.time.min(length) };
        let mut pose = stand_pose(clip, t, &names, BOARD.contains(&entity.as_str()));
        if let Some((from, w)) = s.from.as_mut() {
            *w += dt / BLEND;
            if *w < 1. && from.len() == pose.len() {
                pose = blend(from, &pose, *w);
            }
        }
        if s.from.as_ref().is_some_and(|(from, w)| *w >= 1. || from.len() != pose.len()) {
            s.from = None;
        }
        s.last.clone_from(&pose);
        cast.poses.insert(key.clone(), pose);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn m(x: f32) -> M4 {
        [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [x, 0., 0., 1.]]
    }
    fn names(list: &[&str]) -> BTreeSet<String> {
        list.iter().map(|s| s.to_string()).collect()
    }
    fn row(entity: &str, locator: &str) -> (String, String) {
        (entity.into(), locator.into())
    }

    /// The five missions' `self` rows as P1 exports them (tutorial.json, research 4.1 table), on stand-in locators.
    fn data() -> Data {
        let spawn = |key: &str, locator: &str, mode: u8, second: Option<&str>| {
            json!({"class": "60D703C77CAB1631", "key": key, "locator": locator, "mode": mode, "second": second})
        };
        let tutorial = json!({
            "missions": {
                "he_u1_film01": {"self": {"GlobalSpawn": null, "ClearOtherNPCs": false}},
                "he_u1_skate01": {"self": {"GlobalSpawn": [spawn(SHINGO, "skate_loc", 0, None)], "ClearOtherNPCs": true}},
                "he_u2_follow01": {"self": {"GlobalSpawn": [spawn(SHINGO, "follow_loc", 0, None), spawn(SEB, "seb_loc", 1, None)], "ClearOtherNPCs": false}},
                "he_u2_photo01": {"self": {"GlobalSpawn": [spawn(SEB, "seb_loc", 2, Some("photo_loc"))], "ClearOtherNPCs": true}},
                "he_u3_film01": {"self": {"GlobalSpawn": null, "ClearOtherNPCs": false}},
            },
            "places": {"locators": {
                "skate_loc": {"matrix": m(0.)}, "follow_loc": {"matrix": m(1.)},
                "seb_loc": {"matrix": m(2.)}, "photo_loc": {"matrix": m(3.)},
            }},
        });
        let cast = json!({
            "entities": {
                SHINGO: {"draw": "IP_shingo", "remap": {"IdleBasicCyc": ["PRO_WNDR_STND_IDLE_0_CYC", "PRO_WNDR_STND_IDLE_3_CYC"]}},
                SEB: {"draw": "Deran_Chung", "remap": {"IdleBasicCyc": ["PRO_WNDR_STND_IDLE_0_CYC"]}},
            },
            "clips": {},
        });
        Data::parse(tutorial, cast).unwrap()
    }

    /// P6 R1: the pros' rows come from `pro_tutorials` and follow the same modes: P-Rod's mode 0 leaves at his start,
    /// Dill's mode 1 stays, Carroll's mode 2 moves to NPCAlt01. All four carry a board (R2).
    #[test]
    fn pro_tutorial_rows_stand_and_move_by_mode() {
        let spawn = |key: &str, locator: &str, mode: u8, second: Option<&str>| {
            json!({"self": {"GlobalSpawn": [{"class": "60D703C77CAB1631", "key": key, "locator": locator, "mode": mode, "second": second}], "ClearOtherNPCs": false}})
        };
        let tutorial = json!({
            "missions": {},
            "pro_tutorials": {
                "fp_u3_tutorial02": spawn(PROD, "prod_loc", 0, None),
                "fp_u3_tutorial01": spawn(DILL, "dill_loc", 1, None),
                "fp_u3_tutorial05": spawn(CARROLL, "carroll_loc", 2, Some("carroll_alt")),
            },
            "places": {"locators": {"prod_loc": {"matrix": m(0.)}, "dill_loc": {"matrix": m(1.)}, "carroll_loc": {"matrix": m(2.)}, "carroll_alt": {"matrix": m(3.)}}},
        });
        let d = Data::parse(tutorial, json!({"entities": {}, "clips": {}})).unwrap();
        assert_eq!(d.live(&names(&["fp_u3_tutorial02"]), None), [row(PROD, "prod_loc")]);
        assert!(d.live(&names(&["fp_u3_tutorial02"]), Some("fp_u3_tutorial02")).is_empty());
        assert_eq!(d.live(&names(&["fp_u3_tutorial01"]), Some("fp_u3_tutorial01")), [row(DILL, "dill_loc")]);
        assert_eq!(d.live(&names(&["fp_u3_tutorial05"]), Some("fp_u3_tutorial05")), [row(CARROLL, "carroll_alt")]);
        for pro in [PROD, DILL, CARROLL, HASLAM, DANNY, CUZ] {
            assert!(BOARD.contains(&pro), "{pro} carries a board");
        }
    }

    /// research 4.1 "When a row is live": a challenge's rows stand while it is unlocked and not completed.
    #[test]
    fn a_spawn_row_stands_while_its_challenge_is_available() {
        let d = data();
        assert!(d.live(&names(&[]), None).is_empty());
        assert_eq!(d.live(&names(&["he_u1_skate01"]), None), [row(SHINGO, "skate_loc")]);
        assert_eq!(d.live(&names(&["he_u2_follow01"]), None), [row(SHINGO, "follow_loc"), row(SEB, "seb_loc")]);
        assert_eq!(d.live(&names(&["he_u2_photo01"]), None), [row(SEB, "seb_loc")]);
    }

    /// research 4.1 "a": at its own start a mode-0 NPC leaves, mode 1 stays, mode 2 moves to `second`; a starting
    /// challenge with ClearOtherNPCs clears everyone else's; a challenge with no rows (film) changes nothing.
    #[test]
    fn a_running_challenge_moves_and_clears_npcs_by_mode() {
        let d = data();
        let all = names(&["he_u1_film01", "he_u1_skate01", "he_u2_follow01", "he_u2_photo01"]);
        assert!(d.live(&all, Some("he_u1_skate01")).is_empty(), "Shingo leaves; ClearOtherNPCs takes the rest");
        assert_eq!(d.live(&names(&["he_u2_follow01"]), Some("he_u2_follow01")), [row(SEB, "seb_loc")]);
        assert_eq!(d.live(&names(&["he_u2_photo01"]), Some("he_u2_photo01")), [row(SEB, "photo_loc")]);
        assert_eq!(d.live(&all, Some("he_u1_film01")), d.live(&all, None));
    }

    /// C3 (INFERRED two records): Seb's Follow and Photo rows on one locator draw one Seb.
    #[test]
    fn one_entity_on_one_locator_stands_once() {
        let both = names(&["he_u2_follow01", "he_u2_photo01"]);
        assert_eq!(data().live(&both, None), [row(SHINGO, "follow_loc"), row(SEB, "seb_loc")]);
    }

    /// Cast keys are the model to draw (cast.json `draw`; skate2_cast loads `cast/<key before #>.glb`), `#2` on.
    #[test]
    fn each_standing_npc_gets_its_model_key() {
        let keys = data().cast_keys(&[row(SHINGO, "skate_loc"), row(SHINGO, "follow_loc"), row(SEB, "seb_loc"), row(SEB, "nowhere")]);
        let short: Vec<_> = keys.iter().map(|(k, e, _)| (k.as_str(), e.as_str())).collect();
        assert_eq!(short, [("IP_shingo", SHINGO), ("IP_shingo#2", SHINGO), ("Deran_Chung", SEB)]);
        assert_eq!(keys[1].2, m(1.));
    }

    /// A clip file as P1's s1abin.write_clip writes it: frames x bones x (rotation xyzw, translation xyz, scale xyz).
    fn write_clip(dir: &Path, name: &str, frames: &[Vec<(Quat, Vec3)>]) -> String {
        let bytes: Vec<u8> = frames
            .iter()
            .flatten()
            .flat_map(|(q, t)| [q.x, q.y, q.z, q.w, t.x, t.y, t.z, 1., 1., 1.])
            .flat_map(f32::to_le_bytes)
            .collect();
        std::fs::create_dir_all(dir.join("anim1")).unwrap();
        std::fs::write(dir.join(format!("anim1/{name}.bin")), bytes).unwrap();
        format!("anim1/{name}.bin")
    }

    /// research 4.2 "How one variant is picked": entry rng % n of an array, entry 0 for n <= 1.
    #[test]
    fn the_idle_variant_is_rng_mod_n() {
        assert_eq!((variant(4, 7), variant(4, 8), variant(1, 9), variant(0, 5)), (3, 0, 0, 0));
    }

    /// C5 / C12: a member's bones take the clip by name (one it lacks keeps its transform: NaN), and Shingo's
    /// SKATEBOARD_ROOT rides the clip's PROP_ROOT.
    #[test]
    fn a_board_carrier_holds_his_board_on_the_prop() {
        let dir = std::env::temp_dir().join(format!("skate1-cast-{}", std::process::id()));
        let prop = (Quat::from_rotation_x(1.), Vec3::new(0.2, 0.4, 0.1));
        let file = write_clip(&dir, "IDLE", &[vec![(Quat::IDENTITY, Vec3::ZERO), (Quat::IDENTITY, Vec3::Y * 0.9), prop]]);
        let meta = json!({"file": file, "fps": 30., "frames": 1, "bones": ["TRAJECTORY", "HIPS", "PROP_ROOT"], "parents": [-1, 0, 0]});
        let names: Vec<String> = ["HIPS", "SKATEBOARD_ROOT", "JAW"].map(String::from).to_vec();
        let clip = member_clip(&dir, &meta, &names).unwrap();
        let pose = stand_pose(&clip, 0., &names, true);
        assert_eq!(pose.len(), 3, "PROP_ROOT is not a member bone");
        assert!(pose[0].w_axis.truncate().abs_diff_eq(Vec3::Y * 0.9, 1e-6));
        assert!(pose[1].abs_diff_eq(Mat4::from_rotation_translation(prop.0, prop.1), 1e-6), "the board on the prop");
        assert!(pose[2].is_nan(), "JAW: not in the clip, kept at bind");
        assert!(stand_pose(&clip, 0., &names, false)[1].is_nan(), "no board: SKATEBOARD_ROOT untouched");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// research 4.2 / 4.2a on P1's data: every IdleBasicCyc clip stands the hips 0.85-0.95 m up with the toes on the
    /// ground (measured 0.885-0.923 / 0.029-0.038); Shingo's board on PROP_ROOT stands upright, tail down, nose by his
    /// right hand (measured: 70-88 deg, tail 0.15-0.18 m, nose 0.25-0.27 m from RIGHTHAND).
    #[test]
    #[ignore = "needs SKATE3_ASSET_ROOT with private/challenges-skate1"]
    fn the_idles_stand_on_the_ground_with_the_board_in_hand() {
        let root = std::path::PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap());
        let data = Data::load(&root).unwrap();
        let glb = std::fs::read(root.join(FOLDER).join("cast/IP_shingo.glb")).unwrap();
        let len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        let json: Value = serde_json::from_slice(&glb[20..20 + len]).unwrap();
        let local = |name: &str| {
            let node = json["nodes"].as_array().unwrap().iter().find(|n| n["name"] == name).unwrap();
            Mat4::from_cols_array(&std::array::from_fn(|i| node["matrix"][i].as_f64().unwrap() as f32))
        };
        // animation.rs render_basis: the joint frame a native model matrix becomes on the GLB.
        let basis = Mat4::from_cols(Vec4::X, -Vec4::Z, Vec4::Y, Vec4::W);
        let names: Vec<String> = ["HIPS", "RIGHTTOEBASE", "LEFTTOEBASE", "RIGHTHAND", "SKATEBOARD_ROOT"].map(String::from).to_vec();
        for (entity, board) in [(SHINGO, true), (SEB, false)] {
            for clip in &data.npcs[entity].remap["IdleBasicCyc"] {
                let c = member_clip(&root.join(FOLDER), &data.clips[clip], &names).unwrap();
                for t in [0., c.length() * 0.5] {
                    let pose = stand_pose(&c, t, &names, board);
                    let y = |i: usize| pose[i].w_axis.y;
                    assert!((0.85..0.95).contains(&y(0)), "{clip} hips {}", y(0));
                    assert!((0. ..0.06).contains(&y(1)) && (0. ..0.06).contains(&y(2)), "{clip} toes {} {}", y(1), y(2));
                    if board {
                        let deck = pose[4] * basis;
                        let front = (deck * local("TRUCK_FRONT")).w_axis.truncate();
                        let back = (deck * local("TRUCK_BACK")).w_axis.truncate();
                        let upright = (front - back).normalize().y;
                        assert!(upright > 0.9 && back.y < 0.25, "{clip}: upright {upright}, tail {}", back.y);
                        assert!(front.distance(pose[3].w_axis.truncate()) < 0.3, "{clip}: nose by the right hand");
                    }
                }
            }
        }
    }

    /// research 4.2 / research7 2: Shingo tracks a player inside his +-0.7 rad cone within 6 m and keeps him to 7 m
    /// (1.5 rad); Seb's reach is 12 / 13 m, and shooting (TrackLocalSkater) he follows the player anywhere. C7: pi rad/s.
    #[test]
    fn an_npc_tracks_a_player_in_front_of_him() {
        use std::f32::consts::PI;
        let o = Vec3::ZERO;
        let (on, yaw) = track(false, 0., o, Vec3::new(1.5, 0., 2.598), SHINGO_GATE, false, 0.1);
        assert!(on && (yaw - PI * 0.1).abs() < 1e-5, "30 deg left, 3 m: turns a tenth of pi: {yaw}");
        assert_eq!(track(false, 0., o, Vec3::new(0., 0., -3.), SHINGO_GATE, false, 0.1), (false, 0.), "behind him");
        assert!(!track(false, 0., o, Vec3::Z * 6.5, SHINGO_GATE, false, 0.1).0, "6.5 m: too far to start");
        assert!(track(true, 0., o, Vec3::Z * 6.5, SHINGO_GATE, false, 0.1).0, "but near enough to keep");
        assert!(!track(true, 0., o, Vec3::Z * 7.5, SHINGO_GATE, false, 0.1).0);
        assert!(track(false, 0., o, Vec3::Z * 11., SEB_GATE, false, 0.1).0);
        let (on, yaw) = track(false, 0., o, Vec3::new(0., 0., -20.), SEB_GATE, true, 0.1);
        assert!(on && (yaw.abs() - PI * 0.1).abs() < 1e-5, "shooting: any distance, any side");
        let (_, yaw) = track(true, 0., o, Vec3::new(0.1, 0., 1.), SHINGO_GATE, false, 1.);
        assert!((yaw - 0.1f32.atan2(1.)).abs() < 1e-6, "a short turn ends on the player");
    }

    /// research7 2 (0x823d7158): a spawned NPC starts facing his locator's rotation (At row).
    #[test]
    fn a_stand_starts_facing_its_locator() {
        let at = [[0., 0., 1., 0.], [0., 1., 0., 0.], [-1., 0., 0., 0.], [3., 0., 4., 1.]];
        let yaw = Stand::new(at, 0).yaw;
        assert!((Quat::from_rotation_y(yaw) * Vec3::Z).abs_diff_eq(-Vec3::X, 1e-6), "{yaw}");
    }

    /// Photographer.xml (research 4.2): PictureTaking while taking pictures, then CheckOutPictures; the edges' clips
    /// INFERRED from their names. C10: one review cycle.
    #[test]
    fn seb_shoots_then_reviews_his_pictures() {
        let mut s = Photo::Idle;
        let mut seen = Vec::new();
        for (shooting, done) in [(true, false), (true, true), (true, true), (false, false), (false, true), (false, false), (false, true), (false, true)] {
            s = s.next(shooting, done);
            seen.push(s);
        }
        use Photo::*;
        assert_eq!(seen, [Into, Shoot, Shoot, ToReview, Review, Review, FromReview, Idle]);
        assert_eq!((Review.next(true, false), BackToShoot.next(true, true)), (BackToShoot, Shoot));
        assert_eq!((Idle.clip(), Shoot.clip()), (None, Some("PHOT_SHOOT_IDLE_N_0_CYC")));
    }

    /// C11: a clip change crossfades bone by bone (slerp / lerp in model space); a bone NaN on either side takes the new.
    #[test]
    fn a_change_of_clip_crossfades() {
        let a = [Mat4::from_translation(Vec3::X), Mat4::IDENTITY, Mat4::NAN];
        let b = [Mat4::from_rotation_translation(Quat::from_rotation_y(1.), Vec3::Z), Mat4::NAN, Mat4::IDENTITY];
        let half = blend(&a, &b, 0.5);
        let (_, r, t) = half[0].to_scale_rotation_translation();
        assert!(t.abs_diff_eq(Vec3::new(0.5, 0., 0.5), 1e-6) && r.abs_diff_eq(Quat::from_rotation_y(0.5), 1e-5));
        assert!(half[1].is_nan() && half[2] == Mat4::IDENTITY);
    }

    /// The photo states' clips are all in P1's export (a typo would silently idle him).
    #[test]
    #[ignore = "needs SKATE3_ASSET_ROOT with private/challenges-skate1"]
    fn seb_photo_clips_are_exported() {
        let data = Data::load(Path::new(&std::env::var("SKATE3_ASSET_ROOT").unwrap())).unwrap();
        use Photo::*;
        for s in [Into, Shoot, ToReview, Review, BackToShoot, FromReview] {
            assert!(data.clips.contains_key(s.clip().unwrap()), "{s:?}");
        }
    }

    /// Review Focus 2: the S1 cast writes only its own keys into the shared Cast (a standing actor stays where he
    /// stands; the scene binds him there), and releasing it leaves Skate 2's keys, riders and folder as they were.
    #[test]
    fn turning_off_removes_only_skate1_keys() {
        let mut cast = Cast::default();
        cast.wanted.insert("slappy".into(), m(9.));
        cast.riding.insert("slappy#ai1".into());
        let mut mine = BTreeSet::new();
        let stands = [("IP_shingo".to_string(), SHINGO.to_string(), m(0.))];
        let riders = [("IP_shingo#s1ai2".to_string(), m(1.))];
        let w = wanted(&stands, &riders, vec![("IP_shingo".into(), m(5.)), ("tutorial_skater".into(), m(6.))]);
        apply(&mut mine, &mut cast, w, ["IP_shingo#s1ai2".to_string()].into());
        assert_eq!(cast.wanted.keys().collect::<Vec<_>>(), ["IP_shingo", "IP_shingo#s1ai2", "slappy", "tutorial_skater"]);
        assert_eq!(cast.wanted["IP_shingo"], m(0.));
        assert_eq!(cast.folder, FOLDER);
        assert!(cast.riding.contains("IP_shingo#s1ai2") && cast.riding.contains("slappy#ai1"));
        cast.poses.insert("IP_shingo".into(), vec![Mat4::IDENTITY]);
        release(&mut mine, &mut cast);
        assert_eq!(cast.wanted.keys().collect::<Vec<_>>(), ["slappy"]);
        assert_eq!(cast.riding.iter().collect::<Vec<_>>(), ["slappy#ai1"]);
        assert!(cast.poses.is_empty() && cast.folder.is_empty() && mine.is_empty());
    }

    /// Review Focus 3: a member a cutscene holds gets no idle pose (its old one is dropped) and no facing.
    #[test]
    fn a_held_member_is_left_to_the_cutscene() {
        let mut cast = Cast::default();
        cast.held.insert("IP_shingo".into());
        cast.poses.insert("IP_shingo".into(), vec![Mat4::IDENTITY]);
        assert!(!posable(&mut cast, "IP_shingo") && cast.poses.is_empty());
        assert!(posable(&mut cast, "Deran_Chung"));
    }
}
