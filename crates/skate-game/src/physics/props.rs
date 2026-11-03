//! Movable props (G7). Skate 3 districts ship their DMO instances merged into
//! one render-only `native-props/<map>.skate` plus a report JSON of authored
//! matrices. Each movable instance becomes its own render entities, its own
//! solid query mesh and one grab registry object; holding it (OffBoardPushing
//! 502) drives it with the retail mover 82d45318. Nothing new is installed:
//! instances are split at load by the package's `{instance_id}_{mesh}` names.
//! ponytail: Skate 1/2 maps have no native-props package and keep no props.
use super::{GamePhysics, SkaterRuntime, offboard::grab_scene::MeshAssembly};
use bevy::prelude::*;
use skate_core::{
    input::{controller::DerivedControllerInput, offboard_intentions::produce_object_move},
    math::Vector3,
    physics::{
        board::BodyId,
        board_world::{
            BoardWorld, WorldTriangle,
            query_metadata::{Bounds, QueryMesh, QueryPool},
        },
        contact::RetailContactMaterial,
        drive_frames::RetailAffineTransform,
        skeleton_animation_record::compose_affine,
        triangle_query::{TriangleLineHit, triangle_segment},
    },
    player::{
        offboard::{
            grab_scene as native,
            ground_sync::{BoardLimits, board_bounds},
        },
        state::PhysicalStateId,
    },
    point_graph::PointGraph,
    scoring::air::{self, ContextObject, ContextSlot},
};
use skate_data::skate_map::SkateMap;
use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
};

/// Grab spline: inset behind its face, so validation lines hit the prop itself.
const INSET: f32 = 0.1;
/// Query mesh support identities and physical assemblies for props only.
const GEOMETRY_BASE: u32 = 0x5000_0000;
const ASSEMBLY_BASE: u32 = 0x4000_0000;
/// Packed surface codes (audio | physics << 7 | pattern << 12, physics 0 =
/// normal) per template class, not copied from the ground below (review I6).
/// AudioSurfaceMap grain: audio 8 = metal_smooth, audio 5 = wood_ramp.
/// ponytail: one code per class; concrete benches and barriers sound metal.
const METAL: u32 = 8;
const WOOD: u32 = 5;
/// physics_state_offboard_pushing (3EDA5B140604613D) with the Record default
/// mass 1 / yaw inertia 1: S(1) = 1 and R(1) = 2 (EABFCC79873A2859).
/// ponytail: no per-prop mass; heavy dumpsters move like light props.
const MAX_PUSH: f32 = 3.0;
const MAX_PULL: f32 = 2.0;
const MAX_STRAFE: f32 = 2.5;
const MAX_FORCE: f32 = 20.0;
const MAX_TORQUE: f32 = 6.0;
const ROTATION_SCALE: f32 = 2.0;
const RESYNC: f32 = 0.1;
/// Hash_96ECC98838ECCC11: v624's point sits this far behind the grip (82d46610).
const TRACK_BACK: f32 = f32::from_bits(0x3f33_3333);
/// Hash_02F03DA51050C8B3 and Hash_5E35DB02BE697A58 ([+1184], [+1180]): the crouch's
/// reach point on the anim frame, up and along Z.
const REACH_UP: f32 = f32::from_bits(0x3f99_999a);
const REACH_BACK: f32 = f32::from_bits(0x3f38_b978);
/// 82D44A10: body target 0.65 m behind the grip, limit_delta max 0.1 m.
const BODY_OFFSET: f32 = 0.65;
const BODY_STEP: f32 = 0.1;
/// Largest ground change per tick a held prop follows; more is a wall (N2).
const STEP_UP: f32 = 0.3;
/// Release overlap margin around the hull (N3).
const CLEARANCE: f32 = 0.1;

#[derive(serde::Deserialize)]
struct Report {
    instances: Vec<Instance>,
}
#[derive(serde::Deserialize)]
struct Instance {
    instance_id: String,
    #[serde(default)]
    template_id: String,
    /// Row vectors: rows 0..2 axes, row 3 translation.
    matrix: [[f32; 4]; 4],
    name: String,
    /// Skate 2 packages (old_tutorial_dmos.py, round2 T4): the template's retail collision, world triangles.
    #[serde(default)]
    collision: Vec<[[f32; 3]; 3]>,
    /// Skate 2 packages (10-29): each collision triangle's retail ClusteredMesh edge codes, in order.
    #[serde(default)]
    collision_edges: Vec<[u8; 3]>,
    /// Skate 2 packages: false for a template without GRABDATA (EB001F).
    #[serde(default = "grabbable")]
    grab: bool,
    /// Skate 2 packages: the retail collision's packed surface where the exporter keeps it (old_tutorial_gate.py:
    /// Slappy's gate, 929, the RWCM surface the map's own triangles carry); else the class's.
    #[serde(default)]
    surface: Option<u32>,
}

fn grabbable() -> bool {
    true
}

/// Solid and movable: retail collision in the package, or Skate 3's allowlist.
fn movable(instance: &Instance) -> bool {
    !instance.collision.is_empty() || class(&instance.name).is_some()
}

/// A props package file: a `name` with a '/' is a path under private/ (the Skate 2 tutorial's), else a district's
/// native-props.
fn package(asset_root: &Path, name: &str, extension: &str) -> std::path::PathBuf {
    let (private, file) = (asset_root.join("private"), format!("{name}.{extension}"));
    if name.contains('/') { private.join(file) } else { private.join("native-props").join(file) }
}

/// skate2port/old_tutorial_dmos.py's `private/skate2-tutorial/dmos.json` (G8): the DMOs each Skate 2 lesson
/// places or tests, drawn from the `source` package (round2 T4: Skate 2's own models and retail collision).
#[derive(serde::Deserialize)]
struct Dmos {
    map: String,
    source: String,
    challenges: std::collections::BTreeMap<String, Vec<Dmo>>,
    /// skate2port/old_tutorial_gate.py (round2 10-14): DynamicHull name -> its vault default state and, per state, the
    /// rows retail loads in it (slappys: Locked -> PJ03_SlappysGate's door, 0x823a8be8; Unlock -> none).
    #[serde(default)]
    hulls: BTreeMap<String, HullData>,
}
#[derive(serde::Deserialize)]
struct HullData {
    default: String,
    states: BTreeMap<String, Vec<Dmo>>,
}
#[derive(serde::Deserialize)]
struct Dmo {
    name: String,
    instance_id: String,
    template_id: String,
    matrix: [[f32; 4]; 4],
}

/// A map's tutorial DMOs: `source` report instance -> (lesson, DMO instance id, retail matrix). Each DMO borrows
/// the next unused movable instance of its template; the same files give the same picks to physics and render.
struct Staged {
    source: String,
    report: Report,
    picks: HashMap<usize, (String, u64, [[f32; 4]; 4])>,
    /// DynamicHull name -> default state (`Dmos::hulls`).
    defaults: BTreeMap<String, String>,
}

fn staged(asset_root: &Path, map: &str) -> Option<Staged> {
    let path = asset_root.join("private").join("skate2-tutorial").join("dmos.json");
    let dmos: Dmos = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    if dmos.map != map {
        return None;
    }
    let report = report(asset_root, &dmos.source)?;
    let mut picks = HashMap::new();
    // Round2 10-14: a DynamicHull state's rows are owned by "hull:state" (no lesson name has a ':'); `Props::stage`
    // shows them while the hull is in that state.
    let hulls = dmos.hulls.iter().flat_map(|(hull, data)| {
        data.states.iter().map(move |(state, list)| (format!("{hull}:{state}"), list))
    });
    for (lesson, list) in dmos.challenges.iter().map(|(lesson, list)| (lesson.clone(), list)).chain(hulls) {
        for dmo in list {
            let free = report.instances.iter().enumerate().position(|(i, instance)| {
                instance.template_id.eq_ignore_ascii_case(&dmo.template_id)
                    && movable(instance)
                    && !picks.contains_key(&i)
            });
            match (free, u64::from_str_radix(&dmo.instance_id, 16)) {
                (Some(i), Ok(id)) => {
                    picks.insert(i, (lesson.clone(), id, dmo.matrix));
                }
                // A template the source package lacks stays absent.
                _ => debug!("SKATE_PROPS: {lesson} {} has no {} model; skipped", dmo.name, dmos.source),
            }
        }
    }
    Some(Staged {
        defaults: dmos.hulls.iter().map(|(hull, data)| (hull.clone(), data.default.clone())).collect(),
        source: dmos.source,
        report,
        picks,
    })
}

/// `pose` drawn at `from` (row-vector matrix), placed as if drawn at `to`.
fn relocate(pose: Pose, from: [[f32; 4]; 4], to: [[f32; 4]; 4]) -> Pose {
    let affine = |m: [[f32; 4]; 4]| bevy::math::Affine3A::from_mat4(Mat4::from_cols_array_2d(&m));
    let t = affine(to) * affine(from).inverse();
    Pose {
        rotation: Mat3::from_quat(Quat::from_mat3(&(Mat3::from(t.matrix3) * pose.rotation)).normalize()),
        centre: t.transform_point3(pose.centre),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hull {
    /// Heavy compact street furniture: an oriented box.
    Box,
    /// Rideables: their own welded render triangles.
    Render,
}

/// Template allowlist checked against every installed native-props report
/// (G7 review C2/C3). Everything else (patio tables, bins, bags, chairs,
/// balls, lids, nets, stands, kegs...) is not solid and stays merged and
/// static until knock-over physics exists.
fn class(name: &str) -> Option<(Hull, u32)> {
    let template = name.split("_0x").next().unwrap_or(name).to_ascii_lowercase();
    let has = |keys: &[&str]| keys.iter().any(|key| template.contains(key));
    if has(&["rail", "flatbar", "tbar"]) {
        Some((Hull::Render, METAL))
    } else if has(&["quarterpipe", "ramp", "jump", "plywood", "staircase"]) {
        Some((Hull::Render, WOOD))
    } else if has(&[
        "dumpster",
        "vendingmachine",
        "newspaper",
        "mailbox",
        "potty",
        "picnictable",
        "pinnictable",
        "_ind_table",
        "bench",
        "barrier",
        "container",
    ]) {
        Some((Hull::Box, METAL))
    } else {
        None
    }
}

/// The game thread lists DMOs this near the skater for the audio (`sub_827A43C8`: 30 m).
const DMO_RANGE: f32 = 30.0;

/// A DMO's `livingworld_dynamicobject_characteristics` row (its audio list entry's `+40`: RETAIL, the row of its
/// attribute instance, DMO `[[+4380]+0]+24`). INFERRED: the row named as its template, the instance name without its
/// `_DMO_<n>` / `_<n>` / `_0x..` suffix (`WedgeRamp_large_DMO_1000` -> wedgeramp_large, round2-research2's row for the
/// wedge; picnictable_wood, picnictable_broken, oildrum, dumpstergrunge, woodpanel are all rows of the class). A name the
/// vault lacks finds no row: the title then plays nothing for it.
fn characteristics(name: &str) -> u64 {
    let name = name.split("_0x").next().unwrap_or(name).to_ascii_lowercase();
    let mut parts: Vec<&str> = name.split('_').collect();
    while parts.len() > 1 && parts.last().is_some_and(|p| p.bytes().all(|b| b.is_ascii_digit())) {
        parts.pop();
    }
    if parts.len() > 1 && parts.last() == Some(&"dmo") {
        parts.pop();
    }
    skate_data::audio::world::hash64(&parts.join("_"))
}

fn report(asset_root: &Path, map: &str) -> Option<Report> {
    serde_json::from_slice(&std::fs::read(package(asset_root, map, "json")).ok()?).ok()
}

/// Per package material (zero-based): the movable report instance it draws.
fn material_instances(map: &SkateMap, report: &Report, keep: &dyn Fn(usize) -> bool) -> Vec<Option<u32>> {
    let ids: HashMap<&str, u32> = report
        .instances
        .iter()
        .enumerate()
        .filter(|&(n, i)| movable(i) && keep(n))
        .map(|(n, i)| (i.instance_id.as_str(), n as u32))
        .collect();
    map.materials
        .iter()
        .map(|m| {
            m.name
                .rsplit_once('_')
                .and_then(|(id, _)| ids.get(id).copied())
        })
        .collect()
}

/// Solid props stay experimental until a play test: on only with SKATE_PROPS=1.
pub(crate) fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("SKATE_PROPS").is_ok_and(|v| v == "1"))
}

/// Render side of the split; empty (no instances) without a report.
pub(crate) fn render_instances(asset_root: &Path, map: &SkateMap) -> Vec<Option<u32>> {
    report(asset_root, &map.name).map_or_else(Vec::new, |r| material_instances(map, &r, &|_| true))
}

/// The tutorial DMOs' render side: the source package cut down to the picked instances, and their split.
pub(crate) fn staged_package(asset_root: &Path, map: &str) -> Option<(SkateMap, Vec<Option<u32>>)> {
    let staged = staged(asset_root, map)?;
    let path = package(asset_root, &staged.source, "skate");
    let mut package = match std::fs::read(&path).map_err(|e| e.to_string()).and_then(|d| SkateMap::parse_render_only(&d)) {
        Ok(package) => package,
        Err(error) => {
            error!("SKATE_PROPS: {}: {error}", path.display());
            return None;
        }
    };
    let instances = material_instances(&package, &staged.report, &|i| staged.picks.contains_key(&i));
    let vertices = &package.geometry.vertices;
    let drawn = |v: u32| {
        (vertices[v as usize].material as usize).checked_sub(1).and_then(|m| instances.get(m).copied().flatten()).is_some()
    };
    // Only the picked props' vertices too (I5): the lightmap atlas packs a tile for every vertex's material.
    let (mut remap, mut kept) = (HashMap::new(), Vec::new());
    let indices: Vec<u32> = package.geometry.indices.chunks_exact(3).filter(|t| drawn(t[0])).flatten().map(|&v| {
        *remap.entry(v).or_insert_with(|| {
            kept.push(vertices[v as usize].clone());
            kept.len() as u32 - 1
        })
    }).collect();
    (package.geometry.vertices, package.geometry.indices) = (kept, indices);
    Some((package, instances))
}

/// One render batch of a movable prop; its Transform follows the prop.
#[derive(Component)]
pub(crate) struct PropPiece(pub u32);

/// P3 HUD hook: what the off-board prompt should show this tick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Prompt {
    #[default]
    None,
    /// Off board, standing at a grabbable face: "<RB> Grab".
    Grab,
    /// Holding a prop (state 502): move/rotate tip.
    Holding,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Pose {
    /// Columns are the prop's local axes in world space.
    rotation: Mat3,
    centre: Vec3,
}
impl Pose {
    fn apply(self, p: Vec3) -> Vec3 {
        self.rotation * p + self.centre
    }
    /// Native grab frame rows: right, up, forward, position.
    fn frame(self) -> native::Frame {
        let r = self.rotation;
        [
            r.x_axis.extend(0.).to_array(),
            r.y_axis.extend(0.).to_array(),
            r.z_axis.extend(0.).to_array(),
            self.centre.extend(1.).to_array(),
        ]
    }
}

/// One tick of OB_ObjectMvZ / OB_ObjectMvX / OB_ObjectMvRot.
#[derive(Clone, Copy, Debug, Default)]
struct Command {
    push: f32,
    strafe: f32,
    rotate: f32,
}

/// Retail OffBoardPushing hold state (g7-mover-spec.md sections 4-5).
#[derive(Clone, Copy, Debug)]
struct Held {
    prop: usize,
    /// Grip point P on the grabbed face spline and the grip axis Z (into the
    /// prop), both in prop space so they turn with it.
    grip: Vec3,
    inward: Vec3,
    /// Command heading H (+368, -Z at resync) and L (+400, -X), world space.
    heading: Vec3,
    lateral: Vec3,
    velocity: Vec3,
    /// Engine yaw rate: positive turns +Z toward +X, as retail's 8296ec98 measures it; driven toward +w (82d45bc4).
    yaw_rate: f32,
    /// 82D44A10 flag 0x02: pose-error break, sticky until 502 is left.
    broken: bool,
    /// How far the grip sits behind the real face (INSET, less on thin props).
    inset: f32,
    /// Centre height over the ground at bind; None (no ground under it) keeps
    /// y fixed (N2).
    lift: Option<f32>,
    /// Last held pose clear of the board and the skater COM (N3).
    clear: Pose,
    /// 82d46610 p608 and v624: the point TRACK_BACK behind the grip, and its smoothed velocity.
    point: Vec3,
    v624: Vec3,
    /// 82d45318 F672 (the force, rate-limited) and v640 (its filter).
    force: Vec3,
    v640: Vec3,
    /// [+1136], 0.5 at bind (82d44234).
    crouch: f32,
    /// This tick's FillPhysOut 82d46fd0 +0..+28.
    out: [f32; 8],
}

fn flat(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0., v.z).normalize_or_zero()
}

/// Retail H = -Z and L = -X for a grip axis Z, kept horizontal: X = Y x Z is the skater's left, so L = Z x Y is the
/// right (82d45610).
fn axes(z: Vec3) -> (Vec3, Vec3) {
    let z = flat(z);
    (-z, z.cross(Vec3::Y))
}

/// E4FF0185DA44CDBD: off-centre-induced rotation over |d|.
fn curve_a(d: f32) -> f32 {
    PointGraph {
        x: [
            0x0000_0000, 0x3e73_7de9, 0x3f0c_ecd2, 0x3f52_2306, 0x3f92_c322, 0x3fcb_0c80,
            0x3fe5_50e3, 0x4000_0000,
        ]
        .map(f32::from_bits),
        y: [
            0x0000_0000, 0x0000_0000, 0x3d92_4924, 0x3e36_db6c, 0x3ec7_5075, 0x3f68_3a84,
            0x3f80_0000, 0x3f80_0000,
        ]
        .map(f32::from_bits),
    }
    .evaluate(d)
}

/// BFB3BEF0BB2661C0: rotation attenuation over |d|.
fn curve_b(d: f32) -> f32 {
    PointGraph {
        x: [
            0x0000_0000, 0x3e91_eda8, 0x3ed2_f87e, 0x3f08_c176, 0x3f1f_aff3, 0x3f31_32de,
            0x3f57_23dc, 0x3f80_0000,
        ]
        .map(f32::from_bits),
        y: [
            0x3f80_0000, 0x3f80_0000, 0x3f75_f15f, 0x3f6a_0ea1, 0x3f55_074f, 0x3f45_7c58,
            0x3f26_6666, 0x3f08_3a84,
        ]
        .map(f32::from_bits),
    }
    .evaluate(d)
}

/// 82d46610 crouch rate: -0.1 * (grip.y - hands.y). For the first second ([+1180] > 0) curve
/// 702F25BA3A5AAA56 blends it toward the reach point.
fn crouch_rate(grip_y: f32, hands_y: f32, timer: f32, reach_y: f32) -> f32 {
    let rate = -0.1 * (grip_y - hands_y);
    if timer > 1.0 {
        return rate;
    }
    let c2 = PointGraph {
        x: [
            0x0000_0000, 0x3ca8_1c13, 0x3dc6_7660, 0x3e1f_f002, 0x3e4e_a26e, 0x3e8c_175b,
            0x3ea2_45af, 0x3eb3_3333,
        ]
        .map(f32::from_bits),
        y: [
            0x3f80_0000, 0x3f80_0000, 0x3f6b_e2bf, 0x3f35_074f, 0x3f01_d41c, 0x3e36_db6c,
            0x3d20_ea10, 0x0000_0000,
        ]
        .map(f32::from_bits),
    }
    .evaluate(timer);
    -0.5 * c2 * (reach_y - hands_y) + (1. - c2) * rate
}

impl Held {
    /// 82d444a0/82d45d30: grip at the face-spline point closest to the hand,
    /// kept min(margin, len / 2) from either end (settings +444).
    fn bind(prop: usize, pose: Pose, half: Vec3, face: usize, hand: Vec3, margin: f32) -> Self {
        let (normal, [a, b]) = face_spline(half, face);
        let hand = pose.rotation.transpose() * (hand - pose.centre);
        let along = b - a;
        let length = along.length().max(1e-6);
        let end = margin.clamp(0., length * 0.5) / length;
        let grip = a + along * ((hand - a).dot(along) / (length * length)).clamp(end, 1. - end);
        let (heading, lateral) = axes(pose.rotation * -normal);
        Self {
            prop,
            grip,
            inward: -normal,
            heading,
            lateral,
            velocity: Vec3::ZERO,
            yaw_rate: 0.,
            broken: false,
            inset: normal.abs().dot(half) - grip.dot(normal),
            lift: None,
            clear: pose,
            point: pose.apply(grip) + heading * TRACK_BACK,
            v624: Vec3::ZERO,
            force: Vec3::ZERO,
            v640: Vec3::ZERO,
            crouch: 0.5,
            out: [0.; 8],
        }
    }

    /// 82d46610 v624: the point TRACK_BACK behind the grip (along H), its velocity smoothed
    /// 0.85/0.15 at the fixed 1/60 s, zero for the first 0.1 s of the hold.
    fn track(&mut self, pose: Pose, timer: f32) {
        let point = pose.apply(self.grip) + self.heading * TRACK_BACK;
        let rate = 1. / f32::from_bits(0x3c88_8889);
        self.v624 = self.v624 * 0.85 + (point - self.point) * rate * 0.15;
        self.point = point;
        if timer < 0.1 {
            self.v624 = Vec3::ZERO;
        }
    }

    /// FillPhysOut 82d46fd0 on the hold frame: Z240 into the prop, -X208 the skater's right.
    fn phys_out(&self, pose: Pose) -> [f32; 8] {
        let (back, right) = axes(pose.rotation * self.inward);
        let z = -back;
        [
            self.v624.dot(z),
            self.v624.dot(right),
            self.v640.dot(z),
            self.v640.dot(right),
            // [+1192]: |measured yaw rate|; the kinematic prop turns at exactly this rate.
            self.yaw_rate.abs(),
            // UNTRACED: [+1196] is |2 v_cmd| only while [+992] != 0, and no writer of [+992] is known.
            0.,
            // [+1144] = 1/[+960]; INFERRED: the record default mass 1, as the mover assumes.
            1.,
            self.crouch,
        ]
    }

    /// 82D44A10 body target: 0.65 m behind the real face along Z (N3), and Z.
    fn body_target(&self, pose: Pose) -> (Vec3, Vec3) {
        let z = flat(pose.rotation * self.inward);
        (pose.apply(self.grip) - z * (BODY_OFFSET + self.inset), z)
    }

    /// Mover 82d45318 for one tick; returns the unswept next pose.
    /// ponytail: the Kp 20 / Kd 40 drive is replaced by its force and torque
    /// clamps acting on a kinematic body; y is left to `ground` (N2).
    fn step(&mut self, pose: Pose, command: Command, dt: f32) -> Pose {
        let (back, right) = axes(pose.rotation * self.inward);
        let d = right.dot(pose.centre - pose.apply(self.grip));
        let rotate = (d.signum() * curve_a(d.abs()) * command.push + command.rotate).min(1.);
        let w = -(curve_b(d.abs()) * rotate * ROTATION_SCALE);
        let speed = if command.push > 0. { MAX_PUSH } else { MAX_PULL };
        let forward = command.push * speed + 0.25 * d * w;
        if w.abs() >= RESYNC || self.heading.angle_between(back) > RESYNC {
            self.heading = back;
            self.lateral = right;
        }
        let mut target = -self.heading * forward + self.lateral * command.strafe * MAX_STRAFE;
        target.y = 0.;
        let before = self.velocity;
        self.velocity += (target - self.velocity).clamp_length_max(MAX_FORCE * dt);
        // 82d45318: F672 follows the force at most 4 a tick, v640 filters it 0.9/0.1.
        // ponytail: the kinematic clamp's dv/dt (mass 1) stands in for the PID's u
        // (UNTRACED: the +1008 block and [+848]); trace those to drive a real body.
        if dt > 0. {
            self.force += ((self.velocity - before) / dt - self.force).clamp_length_max(4.);
        }
        self.v640 = self.v640 * 0.9 + self.force * 0.1;
        self.yaw_rate += (w - self.yaw_rate).clamp(-MAX_TORQUE * dt, MAX_TORQUE * dt);
        Pose {
            rotation: Mat3::from_rotation_y(self.yaw_rate * dt) * pose.rotation,
            centre: pose.centre + self.velocity * dt,
        }
    }
}

struct Prop {
    instance: u32,
    /// DMO instance id (Skate 2 tutorial testers name props by it).
    id: u64,
    /// The authored matrix's origin in prop space (M1: volume testers test it, not the hull centre).
    origin: Vec3,
    /// Tutorial DMOs: the lesson that places it; solid and drawn only while
    /// it runs (`active`). None = a district prop, always active.
    owner: Option<String>,
    active: bool,
    /// Query mesh index in the BoardWorld.
    mesh: usize,
    authored: Pose,
    /// Where the render package draws it (= authored unless relocated).
    drawn: Pose,
    pose: Pose,
    half: Vec3,
    /// Collision hull relative to the pose, with its edge flags and cosines.
    hull: Vec<[Vec3; 3]>,
    features: Vec<(u32, [f32; 3])>,
    tag: u32,
    /// Skate 2 packages: false = no GRABDATA, solid but never grabbed.
    grab: bool,
    /// Its `livingworld_dynamicobject_characteristics` row ([`characteristics`]), which picks its drag sound.
    row: u64,
}

pub(crate) struct Props {
    props: Vec<Prop>,
    /// Report instance -> prop index.
    by_instance: Vec<Option<usize>>,
    material: RetailContactMaterial,
    held: Option<Held>,
    revision: u64,
    pub(super) prompt: Prompt,
    /// Loaded by `load_staged`: render pieces physics doesn't know stay hidden.
    staged: bool,
    /// The prop last bound (held now, or released since): the one the audio hears ([`Props::audio_dmo`]).
    heard: Option<usize>,
    /// DynamicHull defaults (dmos.json `hulls`) and each hull's state (`set_hulls`; None = no career, no hull).
    hull_defaults: BTreeMap<String, String>,
    hull_states: Option<BTreeMap<String, String>>,
}

/// Each load starts a distinct revision range, so a reload re-presents.
static LOADS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl Props {
    pub(super) fn empty(material: RetailContactMaterial) -> Self {
        Self {
            props: Vec::new(),
            by_instance: Vec::new(),
            material,
            held: None,
            revision: 0,
            prompt: Prompt::None,
            staged: false,
            heard: None,
            hull_defaults: BTreeMap::new(),
            hull_states: None,
        }
    }

    /// Append every movable prop of `map` to `world` (one query mesh each) and
    /// return its grab objects and mesh bindings. Retail Skate 3 maps only.
    /// Any prop failure degrades to no props, never to a failed map (review I4).
    pub(super) fn load(
        asset_root: &Path,
        map: &SkateMap,
        world: BoardWorld,
        material: RetailContactMaterial,
    ) -> Result<(BoardWorld, Self, Vec<native::Object>, Vec<MeshAssembly>), String> {
        Self::build(asset_root, &map.name, &map.name, report(asset_root, &map.name), None, world, material)
    }

    /// G8: the Skate 2 tutorial's DMOs on `map` (dmos.json), placed at their
    /// retail matrices and inactive until `stage` names their lesson. Loaded
    /// with SKATE_PROPS unset too: the lessons need them.
    pub(super) fn load_staged(
        asset_root: &Path,
        map: &SkateMap,
        world: BoardWorld,
        material: RetailContactMaterial,
    ) -> Result<(BoardWorld, Self, Vec<native::Object>, Vec<MeshAssembly>), String> {
        let Some(staged) = staged(asset_root, &map.name) else {
            return Ok((world, Self::empty(material), Vec::new(), Vec::new()));
        };
        let (source, report, picks) = (staged.source, staged.report, staged.picks);
        let mut built = Self::build(asset_root, &map.name, &source, Some(report), Some(picks), world, material)?;
        built.1.staged = true;
        built.1.hull_defaults = staged.defaults;
        Ok(built)
    }

    /// `map`'s props from the `name` district's package (`map` itself, or a staged source).
    fn build(
        asset_root: &Path,
        map: &str,
        name: &str,
        report: Option<Report>,
        picks: Option<HashMap<usize, (String, u64, [[f32; 4]; 4])>>,
        world: BoardWorld,
        material: RetailContactMaterial,
    ) -> Result<(BoardWorld, Self, Vec<native::Object>, Vec<MeshAssembly>), String> {
        let mut props = Self::empty(material);
        let path = package(asset_root, name, "skate");
        let (Some(report), true, true) = (report, path.is_file(), world.query_metadata().is_ok()) else {
            return Ok((world, props, Vec::new(), Vec::new()));
        };
        let package = match std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|data| SkateMap::parse_geometry_only(&data))
        {
            Ok(package) => package,
            Err(error) => {
                // Static, non-solid props are better than no map.
                error!("SKATE_PROPS: {}: {error}", path.display());
                return Ok((world, props, Vec::new(), Vec::new()));
            }
        };
        let instances = material_instances(&package, &report, &|i| picks.as_ref().is_none_or(|p| p.contains_key(&i)));
        let mut triangles: Vec<Vec<[Vec3; 3]>> = vec![Vec::new(); report.instances.len()];
        for tri in package.geometry.indices.chunks_exact(3) {
            let vertex = |i: u32| &package.geometry.vertices[i as usize];
            let instance = (vertex(tri[0]).material as usize)
                .checked_sub(1)
                .and_then(|m| instances.get(m).copied().flatten());
            if let Some(instance) = instance {
                triangles[instance as usize]
                    .push([0, 1, 2].map(|k| Vec3::from_array(vertex(tri[k]).position)));
            }
        }
        props.by_instance = vec![None; report.instances.len()];
        for (index, (instance, tris)) in report.instances.iter().zip(&triangles).enumerate() {
            // Drawn props only: physics never adds one the render split cannot show.
            let Some((pose, half, local, tag)) = shape(instance, tris).filter(|_| !tris.is_empty()) else {
                continue;
            };
            // Retail's own edge codes where the package has one per triangle (the welded recompute pairs a crease
            // three triangles share as a convex knife edge: the tutorial wedge's toe, 10-29 bug 7).
            let native: Option<Vec<_>> = (instance.collision_edges.len() == local.len())
                .then(|| instance.collision_edges.iter().map(|&e| crate::skate_world::decode_native_edges(e).ok()).collect())
                .flatten();
            let (hull, features): (Vec<_>, Vec<_>) = match native {
                Some(features) => local
                    .iter()
                    .copied()
                    .zip(features)
                    .filter(|([a, b, c], _)| (*b - *a).cross(*c - *a).length_squared() > 1e-6)
                    .unzip(),
                None => welded_hull(&local),
            };
            if hull.is_empty() {
                continue;
            }
            let (owner, id, authored, to) = match picks.as_ref().and_then(|p| p.get(&index)) {
                Some((lesson, id, to)) => (Some(lesson.clone()), *id, relocate(pose, instance.matrix, *to), *to),
                None => (None, u64::from_str_radix(&instance.instance_id, 16).unwrap_or(0), pose, instance.matrix),
            };
            props.by_instance[index] = Some(props.props.len());
            props.props.push(Prop {
                instance: index as u32,
                id,
                origin: authored.rotation.transpose() * (Vec3::from_slice(&to[3][..3]) - authored.centre),
                active: owner.is_none(),
                owner,
                mesh: 0,
                authored,
                drawn: pose,
                pose: authored,
                half,
                hull,
                features,
                tag,
                grab: instance.grab,
                row: characteristics(&instance.name),
            });
        }
        // Everything fallible runs before the world is taken apart.
        let built: Option<Vec<_>> = props
            .props
            .iter()
            .map(|prop| {
                let tris = prop_triangles(prop, prop.pose, material)?;
                let bounds = Bounds::from_points(tris.iter().flat_map(|t| t.vertices))?;
                Some((tris, bounds))
            })
            .collect();
        let Some(built) = built else {
            error!("SKATE_PROPS: {}: invalid prop hull; props stay static", name);
            return Ok((world, Self::empty(material), Vec::new(), Vec::new()));
        };
        let (mut world_triangles, metadata) = world.into_parts();
        let mut metadata = metadata.ok_or("Prop world lost its query metadata")?;
        // One exact growth instead of doubling the city's triangles (BoardWorld::new would shrink it back after).
        let added: usize = built.iter().map(|(tris, _)| tris.len()).sum();
        world_triangles.reserve_exact(added);
        metadata.packed_surfaces.reserve_exact(added);
        let mut objects = Vec::with_capacity(props.props.len());
        let mut bindings = Vec::with_capacity(props.props.len());
        for (i, (prop, (tris, bounds))) in props.props.iter_mut().zip(built).enumerate() {
            let start = world_triangles.len();
            metadata
                .packed_surfaces
                .extend(std::iter::repeat_n(prop.tag as u16, tris.len()));
            world_triangles.extend(tris);
            prop.mesh = metadata.meshes.len();
            let geometry = GEOMETRY_BASE + i as u32;
            metadata.meshes.push(QueryMesh {
                triangle_range: start..world_triangles.len(),
                local_to_world: RetailAffineTransform::IDENTITY,
                world_to_local: RetailAffineTransform::IDENTITY,
                local_bounds: bounds,
                matching_group: -1,
                rejection_flags: 0,
                geometry,
                pool: QueryPool::Ground,
            });
            let assembly = ASSEMBLY_BASE + i as u32 + 1;
            bindings.push(MeshAssembly {
                mesh: geometry,
                assembly,
            });
            let mut object = grab_object(i, assembly, prop);
            object.disabled = !prop.active || !prop.grab;
            objects.push(object);
        }
        // Cannot fail: appended meshes are contiguous, identity and bounded.
        let mut world = BoardWorld::with_query_metadata(world_triangles, metadata)
            .map_err(str::to_owned)?;
        for prop in props.props.iter().filter(|p| !p.active) {
            if let Err(error) = world.hide_mesh(prop.mesh) {
                warn!("SKATE_PROPS: hide {}: {error}", prop.instance);
            }
        }
        props.revision = LOADS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) << 32;
        eprintln!(
            "SKATE_PROPS_READY map={map} source={name} movable={} instances={} staged={}",
            props.props.len(),
            report.instances.len(),
            props.props.iter().filter(|p| p.owner.is_some()).count()
        );
        Ok((world, props, objects, bindings))
    }

    /// Test hook: where the held prop is now.
    #[cfg(test)]
    pub(super) fn held_centre(&self) -> Option<Vec3> {
        self.held.map(|held| self.props[held.prop].pose.centre)
    }

    /// This tick's OffBoard +0..+28 for the held prop (FillPhysOut 82d46fd0).
    pub(super) fn held_out(&self) -> Option<[f32; 8]> {
        self.held.map(|held| held.out)
    }

    /// The prop held now, else the one last held while it is within [`DMO_RANGE`] of `skater`, as an entry of retail's
    /// audio DMO list (round2 bug 7): SFXObj_Moving can only start for the held DMO (research5 Q4) and runs on after a
    /// release until the prop slows to 0.2 m/s or loses contact, and a released prop here stops where it is put down.
    /// RETAIL fields: `+16` its velocity (the mover's), `+48` its angular velocity (the mover turns it about Y at
    /// `yaw_rate`), `+68` contact (INFERRED: carried on the ground, which `lift` follows; released, resting where put),
    /// `+76` / `+79` side / upright from its up axis. Impulse, wheels and the rest are 0 (UNTRACED / UNKNOWN).
    /// ponytail: one entry; loose DMOs (SFXObj_Dynamic's c_dynamic_*) need prop physics the port does not have.
    pub(super) fn audio_dmo(&self, skater: [f32; 3]) -> Option<skate_audio_core::game::DmoEntry> {
        let held = self.held;
        let i = held.map(|h| h.prop).or(self.heard)?;
        let prop = self.props.get(i).filter(|p| p.active)?;
        if held.is_none() && prop.pose.centre.distance(Vec3::from_array(skater)) > DMO_RANGE {
            return None;
        }
        let up = prop.pose.rotation.y_axis;
        Some(skate_audio_core::game::DmoEntry {
            id: i as u32 + 1,
            velocity: held.map_or([0.; 3], |h| h.velocity.to_array()),
            angular: held.map_or([0.; 3], |h| [0., h.yaw_rate, 0.]),
            held: held.is_some(),
            row: prop.row,
            contact: held.is_none_or(|h| h.lift.is_some()),
            on_side: up.y.abs() < 0.3,
            upright: up.y > 0.65,
            position: prop.pose.centre.to_array(),
            ..Default::default()
        })
    }

    /// Render delta for one report instance: current pose * drawn^-1, and
    /// whether it is shown (tutorial DMOs outside their lesson are not).
    fn delta(&self, instance: u32) -> (Transform, bool) {
        let Some(prop) = self
            .by_instance
            .get(instance as usize)
            .copied()
            .flatten()
            .map(|i| &self.props[i])
        else {
            // M2: a borrowed tutorial model physics dropped would sit at its source district position.
            return (Transform::IDENTITY, !self.staged);
        };
        let rotation = prop.pose.rotation * prop.drawn.rotation.transpose();
        let transform = Transform {
            translation: prop.pose.centre - rotation * prop.drawn.centre,
            rotation: Quat::from_mat3(&rotation),
            scale: Vec3::ONE,
        };
        (transform, prop.active)
    }

    /// Active tutorial DMOs: (instance id, retail origin), for the lesson's testers.
    pub(crate) fn dmos(&self) -> impl Iterator<Item = (u64, Vec3)> + '_ {
        self.props
            .iter()
            .filter(|p| p.active && p.owner.is_some())
            .map(|p| (p.id, p.pose.apply(p.origin)))
    }

    /// 82761100 + 82DA6F58: slot 6 of output +13648 for one player -- the props near `player` (PhysOut_Skeleton +0x1A0)
    /// that `probe` (the hips probe's origin and hit) is over. Online the list comes from the DMO manager
    /// `[TheSimulation+0x104]` (82587F88); offline from DB 0x830854A8, whose entries are the same DMOs (SUSPECTED).
    /// Every active prop is listed, in load order, at its live pose; the key is index + 1, as [`Props::audio_dmo`].
    /// shortcut: every prop counts as Score entity 0. Retail skips a DMO whose characteristics row has no Score row or
    /// EEntityType -1 (0x82DA7084..0x82DA717C); that needs the exporter's per-instance characteristics key (`row` is
    /// name-inferred and resolves for few Skate 3 props). Upgrade when the props report carries it.
    /// shortcut: no DMO ownership (82587F88's owner filter) and no "grinding this DMO" byte +0x6D (props have no grind
    /// splines), so a prop counts only when the probe is over it.
    pub(super) fn context_slot(&self, player: [f32; 4], probe: Option<([f32; 4], [f32; 4])>) -> ContextSlot {
        let objects = self.props.iter().enumerate().filter(|(_, p)| p.active).map(|(i, p)| {
            let r = p.pose.rotation;
            ContextObject {
                matrix: [
                    r.x_axis.extend(0.).to_array(),
                    r.y_axis.extend(0.).to_array(),
                    r.z_axis.extend(0.).to_array(),
                    p.pose.apply(p.origin).extend(1.).to_array(),
                ],
                // shortcut: the collision box stands in for the physics model's bounds (82760ED0's +0x40/+0x50).
                min: (-p.origin - p.half).extend(0.).to_array(),
                max: (p.half - p.origin).extend(0.).to_array(),
                id: i as u32 + 1,
                entity: 0,
            }
        });
        air::object_slot(air::near_objects(player, objects), probe)
    }

    /// Put prop `i` back at its authored pose (solid if active).
    fn restore(&mut self, i: usize, world: &mut BoardWorld, objects: &mut [native::Object]) {
        let prop = &mut self.props[i];
        prop.pose = prop.authored;
        // A rejected registry has no objects (physics.rs); the mesh still moves.
        if let Some(object) = objects.get_mut(i) {
            object.frame = prop.pose.frame();
        }
        if prop.active
            && let Some(Err(error)) = prop_triangles(prop, prop.pose, self.material).map(|t| world.set_mesh_triangles(prop.mesh, &t))
        {
            warn!("SKATE_PROPS: restore {}: {error}", prop.instance);
        }
        self.revision += 1;
    }

    /// Round2 10-14: each DynamicHull's state for `stage` (None = no career: no hull, as retail loads one only in
    /// career mode); a hull the career never set is in its vault default. Cheap when nothing changes.
    pub(super) fn set_hulls(&mut self, states: Option<&BTreeMap<String, String>>) {
        if self.hull_states.as_ref() != states {
            self.hull_states = states.cloned();
        }
    }

    /// G8: make the DMOs of `lesson` (and only those) solid, grabbable and
    /// drawn, each starting at its authored pose. Cheap when nothing changes;
    /// a held prop keeps its state until released.
    pub(super) fn stage(&mut self, world: &mut BoardWorld, objects: &mut [native::Object], lesson: Option<&str>) {
        let held = self.held.map(|h| h.prop);
        for i in 0..self.props.len() {
            let prop = &self.props[i];
            let Some(owner) = &prop.owner else {
                continue;
            };
            // I1 soft-lock fail-safe: no grab object (rejected registry) = never solid, so the gap stays open and the
            // exit volume completes the objective.
            let wanted = match owner.split_once(':') {
                // A DynamicHull state's row: there while its hull is in that state (round2 10-14).
                Some((hull, state)) => self
                    .hull_states
                    .as_ref()
                    .and_then(|states| states.get(hull).or(self.hull_defaults.get(hull)))
                    .is_some_and(|s| s == state),
                None => lesson == Some(owner.as_str()),
            };
            let active = wanted && objects.get(i).is_some();
            if active == prop.active || held == Some(i) {
                continue;
            }
            self.props[i].active = active;
            if let Some(object) = objects.get_mut(i) {
                object.disabled = !active || !self.props[i].grab;
            }
            if active {
                self.restore(i, world, objects);
            } else {
                if let Err(error) = world.hide_mesh(self.props[i].mesh) {
                    warn!("SKATE_PROPS: stage {}: {error}", self.props[i].instance);
                }
                self.revision += 1;
            }
        }
    }

    /// DMOResetInfo (lesson start, retry): active, unheld props authored or now inside the volume go back to their
    /// authored pose.
    /// ponytail: "authored or now inside" so one pushed out still comes back.
    pub(super) fn reset(&mut self, world: &mut BoardWorld, objects: &mut [native::Object], inside: impl Fn(Vec3) -> bool) {
        let held = self.held.map(|h| h.prop);
        for i in 0..self.props.len() {
            let prop = &self.props[i];
            if prop.active && held != Some(i) && (inside(prop.authored.centre) || inside(prop.pose.centre)) && prop.pose != prop.authored {
                self.restore(i, world, objects);
            }
        }
    }
}

/// A movable instance's pose, half extents, prop-space hull (unwelded) and surface: from its retail collision when the
/// package has it (Skate 2), else from its render triangles by the Skate 3 allowlist.
/// ponytail: a Skate 2 template with no Skate 3 class sounds wood when its name says so, else metal.
fn shape(instance: &Instance, render: &[[Vec3; 3]]) -> Option<(Pose, Vec3, Vec<[Vec3; 3]>, u32)> {
    let collision: Vec<[Vec3; 3]> = instance.collision.iter().map(|t| t.map(Vec3::from_array)).collect();
    let (kind, tag) = match class(&instance.name) {
        Some(class) => class,
        None if !collision.is_empty() => {
            let fallback = if instance.name.to_ascii_lowercase().contains("wood") { WOOD } else { METAL };
            (Hull::Render, instance.surface.unwrap_or(fallback))
        }
        None => return None,
    };
    let tris = if collision.is_empty() { render } else { &collision[..] };
    if tris.is_empty() {
        return None;
    }
    let (pose, half) = oriented_box(instance.matrix, tris);
    let inverse = pose.rotation.transpose();
    let local = match kind {
        Hull::Box if collision.is_empty() => box_hull(half),
        _ => tris.iter().map(|t| t.map(|p| inverse * (p - pose.centre))).collect(),
    };
    Some((pose, half, local, tag))
}

/// Orthonormal box around `tris` on the authored matrix axes.
fn oriented_box(matrix: [[f32; 4]; 4], tris: &[[Vec3; 3]]) -> (Pose, Vec3) {
    let row = |i: usize| Vec3::new(matrix[i][0], matrix[i][1], matrix[i][2]);
    let x = row(0).normalize_or_zero();
    let z = x.cross(row(1)).normalize_or_zero();
    let rotation = if z == Vec3::ZERO {
        Mat3::IDENTITY
    } else {
        Mat3::from_cols(x, z.cross(x), z)
    };
    let (min, max) = tris
        .iter()
        .flatten()
        .map(|&p| rotation.transpose() * p)
        .fold((Vec3::INFINITY, Vec3::NEG_INFINITY), |(lo, hi), p| {
            (lo.min(p), hi.max(p))
        });
    let pose = Pose {
        rotation,
        centre: rotation * ((min + max) * 0.5),
    };
    (pose, ((max - min) * 0.5).max(Vec3::splat(0.02)))
}

/// Twelve outward-wound triangles of a centred box.
fn box_hull(half: Vec3) -> Vec<[Vec3; 3]> {
    let mut out = Vec::with_capacity(12);
    for a in 0..3 {
        let (u, v) = ((a + 1) % 3, (a + 2) % 3);
        for s in [1f32, -1.] {
            // e_u x e_v = e_a, so this uv order faces +a; reversed it faces -a.
            let mut quad = [(-1., -1.), (1., -1.), (1., 1.), (-1., 1.)].map(|(p, q)| {
                let mut c = Vec3::ZERO;
                c[a] = s * half[a];
                c[u] = p * half[u];
                c[v] = q * half[v];
                c
            });
            if s < 0. {
                quad.reverse();
            }
            out.push([quad[0], quad[1], quad[2]]);
            out.push([quad[0], quad[2], quad[3]]);
        }
    }
    out
}

/// Welded, sliver-free hull with the RW compiler's edge data (review I1/I2):
/// diagonals and coplanar seams are flat, real edges keep their cosines.
fn welded_hull(tris: &[[Vec3; 3]]) -> (Vec<[Vec3; 3]>, Vec<(u32, [f32; 3])>) {
    let (positions, welded) = crate::skate_world::weld(tris.iter().copied());
    let (mut vertices, mut normals) = (Vec::new(), Vec::new());
    for ids in welded {
        let [a, b, c] = ids.map(|i| positions[i]);
        let cross = (b - a).cross(c - a);
        // triangle_from_volume keeps an unnormalised normal at |cross|^2 <= 2^-23.
        if cross.length_squared() > 1e-6 {
            vertices.push(ids);
            normals.push(cross.normalize());
        }
    }
    let (flags, cosines) = crate::skate_world::edge_features(&positions, &vertices, &normals);
    (
        vertices.iter().map(|ids| ids.map(|i| positions[i])).collect(),
        flags.into_iter().zip(cosines).collect(),
    )
}

fn vector(p: Vec3) -> Vector3 {
    Vector3::new(p.x, p.y, p.z)
}

fn prop_triangles(prop: &Prop, pose: Pose, material: RetailContactMaterial) -> Option<Vec<WorldTriangle>> {
    prop.hull
        .iter()
        .zip(&prop.features)
        .map(|(t, &(flags, cosines))| {
            WorldTriangle::from_vertices(t.map(|p| vector(pose.apply(p))), material, prop.tag, flags, cosines, 0.)
        })
        .collect()
}

/// One horizontal grab spline on vertical face `face` ([+X, -X, +Z, -Z]) at
/// hand height: its outward normal and endpoints, prop space.
fn face_spline(half: Vec3, face: usize) -> (Vec3, [Vec3; 2]) {
    let normal = [Vec3::X, Vec3::NEG_X, Vec3::Z, Vec3::NEG_Z][face];
    let height = (half.y - 0.05).clamp(-half.y + 0.3, -half.y + 1.2);
    let along = Vec3::new(normal.z, 0., normal.x).abs();
    let depth = normal.abs().dot(half);
    let middle = normal * (depth - INSET.min(depth * 0.5)) + Vec3::Y * height;
    let reach = along * along.dot(half);
    (normal, [middle - reach, middle + reach])
}

/// Four grab splines, one per vertical face; descriptor id = index * 4 + face + 1.
fn grab_object(index: usize, assembly: u32, prop: &Prop) -> native::Object {
    let splines = (0..4)
        .map(|face| {
            let (normal, ends) = face_spline(prop.half, face);
            let id = (index * 4 + face) as u32 + 1;
            native::Spline {
                descriptor: native::Descriptor { kind: 2, id },
                geometry: std::sync::Arc::new(native::Geometry {
                    id,
                    points: ends.map(|p| p.extend(1.).to_array()).to_vec(),
                    approach_vectors: vec![normal.extend(0.).to_array()],
                    word_60: 0,
                }),
                word_272: 0,
            }
        })
        .collect();
    native::Object {
        id: index as u32 + 1,
        provider: native::Provider::Dmo {
            selection_variant: 0,
            matching_group: -1,
            record_enabled: true,
        },
        disabled: false,
        assembly_ready: true,
        assembly: Some(native::AssemblyData {
            identity: assembly,
            first_part: None,
        }),
        frame: prop.pose.frame(),
        object_vector_128: [0.; 4],
        splines,
    }
}

/// Nearest hit on from -> to among triangles whose normal passes `keep`,
/// tested one by one so a floor touch cannot hide a wall behind it (N2).
/// The normal is the hit triangle's face normal, turned to the contact side: an edge contact (a sphere sunk into a
/// floor meeting its diagonal, or grazing a post's corner) reports the face it belongs to, not a slant between faces.
fn nearest(world: &BoardWorld, from: Vec3, to: Vec3, radius: f32, keep: impl Fn(Vec3) -> bool) -> Option<(f32, Vec3)> {
    let (start, direction) = (vector(from), vector(to - from));
    world
        .line_candidates(start, vector(to), radius)
        .filter_map(|(_, entry)| {
            let mut hit = TriangleLineHit {
                position: Vector3::ZERO,
                normal: Vector3::ZERO,
                fraction: 0.,
                volume_parameter: [0.; 3],
            };
            let t = &entry.triangle();
            triangle_segment(&mut hit, start, direction, t.vertices, radius, t.fatness).then(|| {
                let contact = Vec3::new(hit.normal.x, hit.normal.y, hit.normal.z);
                let [a, b, c] = t.vertices.map(|v| Vec3::new(v.x, v.y, v.z));
                let face = (b - a).cross(c - a).normalize_or(contact);
                (hit.fraction, if face.dot(contact) < 0. { -face } else { face })
            })
        })
        .filter(|&(_, n)| keep(n))
        .min_by(|a, b| a.0.total_cmp(&b.0))
}

/// Stops the prop where a layer of spheres filling its footprint at centre height (each on its own line from `from`
/// to `to`, so turning sweeps the ends too) meets a wall, and returns the hit fraction and horizontal wall normal.
/// Floors and anything a sphere is moving away from never stop it; the radius keeps the spheres above kerbs the
/// ground follow climbs (N2).
/// UNTRACED: retail's held-prop stop rule (round2-research2 Q3); round2 bug 7 widened the port's one centre sphere,
/// which let a long prop's ends through walls.
/// ponytail: straight lines, not arcs, and every layer point (~50 for the wedge). Upgrade path: a swept OBB.
fn sweep(world: &BoardWorld, from: Pose, to: Pose, half: Vec3) -> Option<(f32, Vec3)> {
    let radius = half.x.min(half.z).min(half.y - 0.05).max(0.02);
    let inner = Vec3::new(half.x - radius, 0., half.z - radius).max(Vec3::ZERO);
    let count = |i: usize| (inner[i] * 2. / radius).ceil() as usize + 1;
    let along = |i: usize, k: usize, n: usize| {
        if n == 1 { 0. } else { -inner[i] + inner[i] * 2. * k as f32 / (n - 1) as f32 }
    };
    let (nx, nz) = (count(0), count(2));
    let mut best: Option<(f32, Vec3)> = None;
    for i in 0..nx {
        for k in 0..nz {
            let local = Vec3::new(along(0, i, nx), 0., along(2, k, nz));
            let (a, b) = (from.apply(local), to.apply(local));
            let motion = b - a;
            if motion.length_squared() < 1e-12 {
                continue;
            }
            if let Some(hit) = nearest(world, a, b, radius, |n| n.y < 0.7 && motion.dot(flat(n)) < 0.)
                && best.is_none_or(|b| hit.0 < b.0)
            {
                best = Some(hit);
            }
        }
    }
    best.map(|(fraction, n)| (fraction.clamp(0., 1.), flat(n)))
}

/// Ground height under `centre`: a thin line from above the prop's top down
/// to `floor - STEP_UP`, floor-like triangles only (N2).
/// ponytail: one probe under the centre, so a box half over a ledge still
/// hangs there. Upgrade path: probe the four bottom corners.
fn ground(world: &BoardWorld, centre: Vec3, half: Vec3, floor: f32) -> Option<f32> {
    let top = centre + Vec3::Y * half.y.max(STEP_UP);
    let bottom = Vec3::new(centre.x, floor - STEP_UP, centre.z);
    nearest(world, top, bottom, 0., |n| n.y >= 0.7).map(|(f, _)| top.y + (bottom.y - top.y) * f)
}

/// Bind-time centre height over the ground; probe with the prop's mesh hidden.
fn lift(world: &BoardWorld, pose: Pose, half: Vec3) -> Option<f32> {
    ground(world, pose.centre, half, pose.centre.y - half.y).map(|y| pose.centre.y - y)
}

/// One held tick against the world: mover, wall sweep, terrain follow.
fn carry(world: &BoardWorld, held: &mut Held, pose: Pose, half: Vec3, command: Command, dt: f32) -> Pose {
    let mut next = held.step(pose, command, dt);
    if let Some((fraction, normal)) = sweep(world, pose, next, half) {
        next.centre = pose.centre.lerp(next.centre, fraction);
        next.rotation =
            Mat3::from_quat(Quat::from_mat3(&pose.rotation).slerp(Quat::from_mat3(&next.rotation), fraction));
        held.velocity -= normal * held.velocity.dot(normal).min(0.);
    }
    follow_ground(world, held, pose, &mut next, half);
    next
}

/// Terrain follow (N2): sit `next` on the ground at `lift`; a step or drop
/// over STEP_UP, or no ground, is a wall and keeps the old centre.
fn follow_ground(world: &BoardWorld, held: &mut Held, from: Pose, next: &mut Pose, half: Vec3) {
    let Some(lift) = held.lift else {
        return;
    };
    let floor = from.centre.y - lift;
    match ground(world, next.centre, half, floor) {
        Some(y) if y - floor <= STEP_UP => next.centre.y = y + lift,
        _ => {
            next.centre = from.centre;
            held.velocity = Vec3::ZERO;
        }
    }
}

/// Whether any of `solids` is inside the hull grown by CLEARANCE.
fn overlaps(pose: Pose, half: Vec3, solids: &[Vec3]) -> bool {
    solids
        .iter()
        .any(|&p| ((pose.rotation.transpose() * (p - pose.centre)).abs() - half).max_element() < CLEARANCE)
}

/// Release pose (N3): with the board or the skater COM inside the hull, push
/// the prop along its grip axis until they leave through the held face; if a
/// wall or the ground blocks that, go back to the last clear held pose. Then
/// settle on the ground and re-orthonormalise (M3).
/// ponytail: the clear pose can overlap again if the board moved under it
/// since. Upgrade path: keep the mesh hidden until the overlap clears.
fn release_pose(world: &BoardWorld, held: &Held, pose: Pose, half: Vec3, solids: &[Vec3]) -> Pose {
    let mut pose = pose;
    if overlaps(pose, half, solids) {
        let depth = held.inward.abs().dot(half) + CLEARANCE + 0.01;
        let push = solids
            .iter()
            .map(|&p| pose.rotation.transpose() * (p - pose.centre))
            .filter(|l| (l.abs() - half).max_element() < CLEARANCE)
            .map(|l| l.dot(held.inward) + depth)
            .fold(0., f32::max);
        let mut pushed = Pose {
            centre: pose.centre + flat(pose.rotation * held.inward) * push,
            ..pose
        };
        let mut probe = *held;
        follow_ground(world, &mut probe, pose, &mut pushed, half);
        pose = if sweep(world, pose, pushed, half).is_some()
            || pushed.centre == pose.centre
            || overlaps(pushed, half, solids)
        {
            held.clear
        } else {
            pushed
        };
    }
    if let Some(lift) = held.lift
        && let Some(y) = ground(world, pose.centre, half, pose.centre.y - lift)
    {
        pose.centre.y = y + lift;
    }
    pose.rotation = Mat3::from_quat(Quat::from_mat3(&pose.rotation).normalize());
    pose
}

/// The skater root walks toward the body target facing Z, at most 0.1 m per
/// tick (82BD41B0). ponytail: overrides the Ground controller's own motion
/// in 502 (82bdf268 itself is unported); the controller integrates from this
/// frame next tick.
fn follow(skater: &mut SkaterRuntime, target: Vec3, z: Vec3) {
    let state = &mut skater.biped_ground.controller.state;
    let frame = &mut state.motion.frame_0;
    let up = Vec3::from_slice(&frame[1][..3]);
    let forward = (z - up * z.dot(up)).normalize_or_zero();
    if forward == Vec3::ZERO {
        return;
    }
    let position = Vec3::from_slice(&frame[3][..3]);
    let mut step = (Vec3::new(target.x, position.y, target.z) - position).clamp_length_max(BODY_STEP);
    // M1: never step into what contact correction is pushing the root out of.
    if state.contact.active {
        let away = Vec3::from_slice(&state.contact.direction[..3]).normalize_or_zero();
        step -= away * step.dot(away).min(0.);
    }
    frame[0] = up.cross(forward).extend(frame[0][3]).to_array();
    frame[2] = forward.extend(frame[2][3]).to_array();
    for axis in 0..3 {
        frame[3][axis] += step[axis];
        state.position_368[axis] += step[axis];
    }
}

/// After state selection and the state update: bind on entering 502, run the
/// mover while in it, rewrite the collision once on release (the only index
/// rebuild).
pub(super) fn advance(
    physics: &mut GamePhysics,
    skater: &mut SkaterRuntime,
    controller: &DerivedControllerInput,
) {
    let GamePhysics {
        props,
        world,
        offboard_grab_scene,
        board,
        ..
    } = physics;
    if props.props.is_empty() {
        return;
    }
    let p = &skater.player_input.processed;
    let frame = p.effective_anim_transform_192.map(|v| v.map(f32::from_bits));
    // Ground's grab sync latches this before 502 and path B may re-latch
    // another prop during it (grab.rs); transition.rs keeps it across entry.
    let latch = &skater.biped_ground.ground;
    let latched = (latch.counter_156 as usize).wrapping_sub(1);
    let latched = (latch.counter_152 == 2 && latched / 4 < props.props.len()).then_some(latched);
    // M9: a re-latch onto another prop releases this one; the next tick binds it.
    // ponytail: a face change on the same prop keeps the old grip.
    let holding = skater.player_state.current() == PhysicalStateId::OffBoardPushing
        && props.held.is_none_or(|h| latched.is_none_or(|id| id / 4 == h.prop));
    // N3: what a re-solidified hull must not close around. A carried board
    // rides on the skater, so only a dropped one counts.
    let position = |id: BodyId| {
        let p = board.bodies()[id.index()].rates.position;
        Vec3::new(p.x, p.y, p.z)
    };
    let com = skater.animated_skeleton.board_frames.com_frame[3];
    let mut solids = vec![Vec3::from_slice(&com[..3])];
    if skater.skateboard_controller.fields.state_448 != 1 {
        solids.extend([BodyId::Deck, BodyId::FrontTruck, BodyId::BackTruck].map(position));
    }
    match (props.held, holding) {
        (None, true) => {
            let Some(id) = latched.filter(|id| offboard_grab_scene.objects.objects.get(id / 4).is_some_and(|o| !o.disabled)) else {
                return;
            };
            let hand = compose_affine(
                &skater.animated_skeleton.roots.animation_to_world,
                &skater.animated_skeleton.record.pose[23],
            )[3];
            let prop = &props.props[id / 4];
            let hand = Vec3::from_slice(&hand[..3]);
            let margin = skater.biped_ground.grab_settings.margin_444;
            let mut held = Held::bind(id / 4, prop.pose, prop.half, id % 4, hand, margin);
            if let Err(error) = world.hide_mesh(prop.mesh) {
                warn!("SKATE_PROPS: grab {}: {error}", prop.instance);
            }
            // Probed after hiding, so the prop's own top is not its ground.
            held.lift = lift(world, prop.pose, prop.half);
            held.out = held.phys_out(prop.pose);
            props.held = Some(held);
            props.heard = Some(id / 4);
            props.prompt = Prompt::Holding;
        }
        (Some(mut held), true) => {
            // Release (c): a skeleton pose error over 0.2 m breaks the hold;
            // clearing the latch drops flag_304, so the selector leaves 502.
            let error = p.collision_pose_error_736.map(f32::from_bits);
            held.broken |= Vec3::from_slice(&error[..3]).length_squared() > 0.04;
            if held.broken {
                skater.biped_ground.ground.flags_144_to_150[0] = false;
                props.held = Some(held);
                return;
            }
            // Retail reads processed2784..2792, filled from MG attributes once
            // MovingObjectNew attaches them; nothing in the port proves the stock
            // graph reaches that, so read the same producer (8259C4B0) directly.
            let [push, strafe, rotate] = produce_object_move(controller).map(|i| i.value);
            let command = Command {
                push,
                strafe,
                rotate,
            };
            let prop = &mut props.props[held.prop];
            // 82d46610, before the mover: v624, and the crouch from the hands (physics parts 3
            // LEFTHAND, 7 RIGHTHAND) and the reach point on the anim frame (processed+192).
            held.track(prop.pose, p.state_timer_2664);
            let hand = |part: usize| {
                compose_affine(
                    &skater.animated_skeleton.roots.animation_to_world,
                    &skater.animated_skeleton.record.pose[part],
                )[3][1]
            };
            let hands = (hand(3) + hand(7)) * 0.5;
            let reach = frame[3][1] + frame[1][1] * REACH_UP + frame[2][1] * (REACH_BACK + 0.125);
            let rate = crouch_rate(prop.pose.apply(held.grip).y, hands, p.state_timer_2664, reach);
            held.crouch = (held.crouch + rate).clamp(0., 1.);
            let tracked = prop.pose;
            let next = carry(world, &mut held, prop.pose, prop.half, command, p.timestep_2604);
            held.out = held.phys_out(tracked);
            if !overlaps(next, prop.half, &solids) {
                held.clear = next;
            }
            if next != prop.pose {
                prop.pose = next;
                offboard_grab_scene.objects.objects[held.prop].frame = next.frame();
                props.revision += 1;
            }
            props.held = Some(held);
            let (target, z) = held.body_target(next);
            follow(skater, target, z);
        }
        (Some(held), false) => {
            props.held = None;
            let prop = &mut props.props[held.prop];
            let pose = release_pose(world, &held, prop.pose, prop.half, &solids);
            if pose != prop.pose {
                prop.pose = pose;
                offboard_grab_scene.objects.objects[held.prop].frame = pose.frame();
                props.revision += 1;
            }
            let tris = prop_triangles(prop, prop.pose, props.material).or_else(|| {
                // Keep the world consistent: fall back to where it was authored.
                prop.pose = prop.authored;
                offboard_grab_scene.objects.objects[held.prop].frame = prop.pose.frame();
                props.revision += 1;
                prop_triangles(prop, prop.pose, props.material)
            });
            if let Some(Err(error)) = tris.map(|t| world.set_mesh_triangles(prop.mesh, &t)) {
                warn!("SKATE_PROPS: release {}: {error}", prop.instance);
            }
            props.prompt = Prompt::None;
        }
        (None, false) => {
            props.prompt = Prompt::None;
            if skater.player_state.current() != PhysicalStateId::BipedGround {
                return;
            }
            // Same query grab.rs submits while RB is held, run for the prompt.
            let settings = skater.biped_ground.grab_settings;
            let radians = f32::from_bits(0x3c8efa35);
            let position = p.vectors_544_560_592_608[2].map(f32::from_bits);
            let query = native::Query {
                position,
                sort_position: position,
                bounds: board_bounds(frame, settings.offset_32, settings.extent_16),
                limits: BoardLimits {
                    margin: settings.margin_444,
                    angle_a: settings.angle_456 * radians,
                    angle_b: settings.angle_440 * radians,
                },
                mode: 4,
                capacity: 5,
                context: skate_core::player::offboard::ground_query::QueryContext {
                    selection_flags_2948: p.actor_query_2948,
                    matching_id_2952: p.actor_query_2952 as i32,
                },
            };
            if native::query(&offboard_grab_scene.objects, &query).is_ok_and(|r| !r.is_empty()) {
                props.prompt = Prompt::Grab;
            }
        }
    }
}

/// Moves (and shows or hides) prop render pieces only when a prop changed, a
/// new map loaded or new pieces spawned.
pub(super) fn present(
    physics: Res<GamePhysics>,
    mut last: Local<u64>,
    added: Query<(), Added<PropPiece>>,
    mut pieces: Query<(&PropPiece, &mut Transform, Option<&mut Visibility>)>,
) {
    let props = &physics.props;
    if props.revision == *last && added.is_empty() {
        return;
    }
    *last = props.revision;
    for (piece, mut transform, visibility) in &mut pieces {
        let (next, shown) = props.delta(piece.0);
        if *transform != next {
            *transform = next;
        }
        let shown = if shown { Visibility::Inherited } else { Visibility::Hidden };
        if let Some(mut visibility) = visibility
            && *visibility != shown
        {
            *visibility = shown;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn material() -> RetailContactMaterial {
        RetailContactMaterial {
            static_friction: 0.7,
            dynamic_friction: 0.4,
            restitution: 0.2,
        }
    }

    fn box_prop(half: Vec3, centre: Vec3) -> Prop {
        let (hull, features) = welded_hull(&box_hull(half));
        let pose = Pose {
            rotation: Mat3::IDENTITY,
            centre,
        };
        Prop {
            instance: 0,
            id: 0,
            origin: Vec3::ZERO,
            owner: None,
            active: true,
            mesh: 0,
            authored: pose,
            drawn: pose,
            pose,
            half,
            hull,
            features,
            tag: METAL,
            grab: true,
            row: 0,
        }
    }

    /// 82761100 / 82DA6F58 over a prop: its bounds sit on its matrix origin (here its base), so a probe that went
    /// through it to the ground does not count it, one that hit its top does.
    #[test]
    fn context_slot_lists_the_prop_under_the_probe() {
        let mut props = Props::empty(material());
        let mut prop = box_prop(Vec3::new(1., 0.5, 1.), Vec3::new(4., 0.5, 0.));
        prop.origin = Vec3::new(0., -0.5, 0.);
        props.props.push(prop);
        let probe = |y| Some(([4., 3., 0., 1.], [4., y, 0., 1.]));
        let slot = props.context_slot([3., 0., 0., 1.], probe(1.));
        assert_eq!((slot.count, slot.entries[0]), (1, (1, 0)));
        assert_eq!(props.context_slot([3., 0., 0., 1.], probe(0.)).count, 0, "through it to the ground");
        props.props[0].active = false;
        assert_eq!(props.context_slot([3., 0., 0., 1.], probe(1.)).count, 0);
    }

    fn at(centre: Vec3) -> Pose {
        Pose {
            rotation: Mat3::IDENTITY,
            centre,
        }
    }

    /// A report instance at x = 10; Skate 2 package rows (round2 T4) carry `collision` and `grab`.
    fn instance(name: &str, collision: Vec<[Vec3; 3]>) -> Instance {
        serde_json::from_value(serde_json::json!({
            "instance_id": "00000000000000AA",
            "matrix": [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [10., 0., 0., 1.]],
            "name": name,
            "collision": collision.iter().map(|t| t.map(|p| p.to_array())).collect::<Vec<_>>(),
        }))
        .unwrap()
    }

    fn moved_box(half: Vec3, centre: Vec3) -> Vec<[Vec3; 3]> {
        box_hull(half).into_iter().map(|t| t.map(|p| p + centre)).collect()
    }

    #[test]
    fn a_source_with_a_slash_is_a_package_under_private() {
        let root = Path::new("assets");
        assert_eq!(package(root, "DownTown", "skate"), root.join("private").join("native-props").join("DownTown.skate"));
        assert_eq!(
            package(root, "skate2-tutorial/props/Skate2Tutorial", "json"),
            root.join("private").join("skate2-tutorial").join("props").join("Skate2Tutorial.json")
        );
    }

    #[test]
    fn retail_collision_shapes_the_prop_and_makes_any_template_solid() {
        // Skate 2's oil drum has no Skate 3 class: its retail collision makes it solid, and its box and hull come
        // from that collision, not from the (here larger) render triangles.
        let render = moved_box(Vec3::splat(2.), Vec3::new(10., 2., 0.));
        let collision = moved_box(Vec3::new(0.3, 0.5, 0.3), Vec3::new(10., 0.5, 0.));
        let drum = instance("OilDrum_DMO_1001", collision.clone());
        assert!(drum.grab, "a report without `grab` keeps every prop grabbable");
        let (pose, half, hull, tag) = shape(&drum, &render).expect("solid");
        assert!((pose.centre - Vec3::new(10., 0.5, 0.)).length() < 1e-5, "{pose:?}");
        assert!((half - Vec3::new(0.3, 0.5, 0.3)).length() < 1e-5, "{half}");
        assert_eq!(hull.len(), 12);
        assert!(hull.iter().flatten().all(|p| (p.abs() - half).max_element() < 1e-5));
        assert_eq!(tag, METAL);
        assert_eq!(shape(&instance("WoodPanel_DMO_1000", collision), &render).unwrap().3, WOOD);
        // Without collision Skate 3's allowlist decides, from the render triangles.
        assert!(shape(&instance("OilDrum_DMO_1001", vec![]), &render).is_none());
        let (pose, half, _, tag) = shape(&instance("DumpsterGrunge_DMO_1000", vec![]), &render).unwrap();
        assert!((pose.centre - Vec3::new(10., 2., 0.)).length() < 1e-5 && (half - Vec3::splat(2.)).length() < 1e-5);
        assert_eq!(tag, METAL);
    }

    #[test]
    fn a_prop_without_grab_data_is_solid_but_never_grabbable() {
        // Skate 2's wood panels have no GRABDATA (EB001F): solid in their lesson, never offered to the hands.
        let (half, centre) = (Vec3::new(1., 0.5, 0.5), Vec3::new(0., 0.5, 0.));
        let (mut world, mut props, mut objects) = staged_world(half, centre);
        props.props[0].grab = false;
        props.stage(&mut world, &mut objects, Some("lesson"));
        assert!(solid(&world, centre) && props.props[0].active && objects[0].disabled);
    }

    #[test]
    #[ignore = "requires SKATE_ASSET_ROOT and SKATE_PROPS_MAP (an installed Skate 3 district .skate)"]
    fn installed_district_props_load_solid_and_move() {
        let root = std::path::PathBuf::from(std::env::var_os("SKATE_ASSET_ROOT").unwrap());
        let map = SkateMap::load(Path::new(&std::env::var_os("SKATE_PROPS_MAP").unwrap())).unwrap();
        let world = crate::skate_world::collision_world(&map, material()).unwrap();
        let before = world.triangles().len();
        let (mut world, mut props, objects, bindings) =
            Props::load(&root, &map, world, material()).unwrap();
        assert!(!props.props.is_empty());
        let added = world.triangles().len() - before;
        println!(
            "props={} added_triangles={added} render_instances={}",
            props.props.len(),
            {
                let package = root.join(format!("private/native-props/{}.skate", map.name));
                let package = SkateMap::parse_geometry_only(&std::fs::read(package).unwrap()).unwrap();
                let pieces: std::collections::HashSet<_> =
                    render_instances(&root, &package).into_iter().flatten().collect();
                assert_eq!(pieces.len(), props.props.len());
                pieces.len()
            }
        );
        let registry = super::super::offboard::grab_scene::Registry::new(&world, objects, bindings).unwrap();
        assert_eq!(registry.objects.objects.len(), props.props.len());
        // The first prop is solid where authored, gone while held, solid where dropped.
        let prop = &mut props.props[0];
        let probe = |world: &BoardWorld, centre: Vec3| {
            world
                .query_thin_line(vector(centre + Vec3::Y * 10.), vector(centre))
                .unwrap()
                .map(|hit| hit.tag)
        };
        assert!(probe(&world, prop.pose.centre).is_some());
        world.hide_mesh(prop.mesh).unwrap();
        // N2: the authored prop rests on ground the terrain follow finds.
        let rest = lift(&world, prop.pose, prop.half);
        println!("lift={rest:?} half_y={}", prop.half.y);
        assert!(rest.is_some_and(|l| (l - prop.half.y).abs() < STEP_UP));
        let moved = Pose {
            centre: prop.pose.centre + Vec3::new(0., 50., 0.),
            ..prop.pose
        };
        prop.pose = moved;
        let tris = prop_triangles(prop, moved, material()).unwrap();
        world.set_mesh_triangles(prop.mesh, &tris).unwrap();
        assert_eq!(probe(&world, moved.centre), Some(prop.tag));
    }

    #[test]
    fn templates_classify_by_allowlist() {
        let name = |t: &str| format!("{t}_0x0000042103e38705:0x2c70170600060098_HighLOD");
        assert_eq!(class(&name("DMO_DT_DumpsterLargeBlue_1001")), Some((Hull::Box, METAL)));
        assert_eq!(class(&name("AD_Bench_DMO_1002")), Some((Hull::Box, METAL)));
        assert_eq!(class(&name("DMO_UN_largeQuarterPipeDeck_1000")), Some((Hull::Render, WOOD)));
        assert_eq!(class(&name("DMO_Rail_RainbowUp_1000")), Some((Hull::Render, METAL)));
        assert_eq!(class(&name("AK_UN_GuardRailGrind_1000")), Some((Hull::Render, METAL)));
        for light in ["CB_GLBL_patioTable_1000", "garbageBag_1001", "DD_GLBL_TinTrashLid_1000", "DMO_UN_SoccerNet_1000", "ED_GLBL_basketball_1000"] {
            assert_eq!(class(&name(light)), None, "{light}");
        }
    }

    #[test]
    fn box_hull_is_twelve_outward_triangles_on_the_box() {
        let half = Vec3::new(1., 0.5, 2.);
        let hull = box_hull(half);
        assert_eq!(hull.len(), 12);
        for t in &hull {
            assert!(t.iter().all(|p| p.abs() == half), "{t:?} is not on the box corners");
            let world = WorldTriangle::from_vertices(t.map(vector), material(), 3, 0, [1.; 3], 0.).expect("valid triangle");
            let n = world.triangle().feature.normal;
            let centroid = (t[0] + t[1] + t[2]) / 3.;
            assert!(Vec3::new(n.x, n.y, n.z).dot(centroid) > 0., "{t:?} faces inward");
        }
        // Every face of the box is covered exactly twice (two triangles).
        for axis in 0..3 {
            for s in [1., -1.] {
                let count = hull
                    .iter()
                    .filter(|t| t.iter().all(|p| p[axis] == s * half[axis]))
                    .count();
                assert_eq!(count, 2);
            }
        }
    }

    #[test]
    fn box_diagonals_are_flat_and_box_edges_convex() {
        let (hull, features) = welded_hull(&box_hull(Vec3::new(1., 0.5, 2.)));
        assert_eq!(hull.len(), 12);
        for (flags, cosines) in features {
            let flat: Vec<_> = (0..3).filter(|&e| cosines[e] == 1.).collect();
            assert_eq!(flat.len(), 1, "one diagonal per triangle: {cosines:?}");
            for e in 0..3 {
                let convex = flags & (0x20 << e) != 0;
                if cosines[e] == 1. {
                    assert!(!convex);
                } else {
                    assert!(cosines[e].abs() < 1e-6 && convex, "{flags:#x} {cosines:?}");
                }
            }
        }
    }

    #[test]
    fn render_hull_drops_slivers_and_welded_degenerates() {
        let quad = [
            [Vec3::ZERO, Vec3::new(1., 0., 0.), Vec3::new(1., 0., 1.)],
            [Vec3::ZERO, Vec3::new(1., 0., 1.), Vec3::new(0., 0., 1.)],
        ];
        let sliver = [Vec3::ZERO, Vec3::new(1., 0., 0.), Vec3::new(0.5, 0.0004, 0.)];
        let welded = [Vec3::ZERO, Vec3::new(0.0002, 0., 0.), Vec3::new(0., 0., 1.)];
        let (hull, features) = welded_hull(&[quad[0], sliver, quad[1], welded]);
        assert_eq!(hull.len(), 2);
        // The shared diagonal of the coplanar pair is flat on both sides;
        // open outline edges keep the convex default.
        assert_eq!(features[0].0 & (0x20 << 2), 0);
        assert_eq!(features[1].0 & 0x20, 0);
        assert_ne!(features[0].0 & 0x20, 0);
    }

    #[test]
    fn oriented_box_follows_the_authored_yaw() {
        let yaw = Mat3::from_rotation_y(0.7);
        let half = Vec3::new(1., 0.5, 2.);
        let centre = Vec3::new(10., 1., -4.);
        let tris: Vec<_> = box_hull(half)
            .into_iter()
            .map(|t| t.map(|p| yaw * p + centre))
            .collect();
        let mut matrix = [[0.; 4]; 4];
        for i in 0..3 {
            matrix[i] = yaw.col(i).extend(0.).to_array();
        }
        let (pose, found) = oriented_box(matrix, &tris);
        assert!((found - half).abs().max_element() < 1e-4);
        assert!((pose.centre - centre).length() < 1e-4);
    }

    /// Holds the -Z face (skater on the -Z side, pushing toward +Z) for `ticks`.
    fn drive(hand_x: f32, command: Command, ticks: usize) -> (Held, Pose) {
        let prop = box_prop(Vec3::new(1., 0.5, 0.5), Vec3::new(0., 0.5, 0.));
        let mut held = Held::bind(0, prop.pose, prop.half, 3, Vec3::new(hand_x, 1., -1.), 0.);
        let mut pose = prop.pose;
        for tick in 0..ticks {
            held.track(pose, tick as f32 / 60.);
            pose = held.step(pose, command, 1. / 60.);
        }
        (held, pose)
    }

    #[test]
    fn phys_out_signs_follow_retail_82d46fd0() {
        // F6's four cases: [v624.Z, v624.R, v640.Z, v640.R, |yaw rate|, +1196, 1/mass, crouch].
        let out = |command: Command| {
            let (held, pose) = drive(0., command, 60);
            held.phys_out(pose)
        };
        let push = out(Command {
            push: 1.,
            ..Default::default()
        });
        assert!(
            (push[0] - MAX_PUSH).abs() < 1e-2
                && push[1].abs() < 1e-4
                && push[2] > 0.
                && push[3].abs() < 1e-4,
            "{push:?}"
        );
        // Crouch starts at 0.5 at bind (82d44234); `step` alone never moves it.
        assert_eq!(&push[4..], &[0., 0., 1., 0.5]);
        let pull = out(Command {
            push: -1.,
            ..Default::default()
        });
        assert!((pull[0] + MAX_PULL).abs() < 1e-2 && pull[2] < 0., "{pull:?}");
        let strafe = out(Command {
            strafe: 1.,
            ..Default::default()
        });
        assert!(
            (strafe[1] - MAX_STRAFE).abs() < 1e-2
                && strafe[3] > 0.
                && strafe[0].abs() < 1e-4
                && strafe[2].abs() < 1e-4,
            "{strafe:?}"
        );
        // Turning in place: the point 0.7 m behind the grip swings to the skater's left
        // (b < 0) and the filter lags the turn (a < 0).
        let rotate = out(Command {
            rotate: 1.,
            ..Default::default()
        });
        assert!(
            rotate[0] < 0. && rotate[1] < 0. && rotate[2].abs() < 1e-3 && rotate[3].abs() < 1e-3,
            "{rotate:?}"
        );
        assert!((rotate[4] - ROTATION_SCALE).abs() < 1e-3 && rotate[5..7] == [0., 1.], "{rotate:?}");
    }

    #[test]
    fn crouch_follows_the_grip_height_and_the_reach_in_the_first_second() {
        // 82d46610: -0.1 * (grip.y - hands.y); while the timer <= 1, curve 702F blends in the reach point.
        assert!((crouch_rate(0.5, 1.0, 2.0, 9.) - 0.05).abs() < 1e-7);
        assert!((crouch_rate(0.5, 1.0, 0.0, 1.2) + 0.1).abs() < 1e-7);
        assert_eq!(crouch_rate(0.5, 1.0, 0.5, 9.), crouch_rate(0.5, 1.0, 2.0, 9.));
        let c2 = f32::from_bits(0x3f01_d41c);
        let want = -0.5 * c2 * (1.2 - 1.0) + (1. - c2) * (-0.1 * (0.5 - 1.0));
        assert!((crouch_rate(0.5, 1.0, f32::from_bits(0x3e4e_a26e), 1.2) - want).abs() < 1e-6);
    }

    #[test]
    fn mover_reaches_retail_push_pull_and_strafe_speeds() {
        let push = Command {
            push: 1.,
            ..Default::default()
        };
        let (held, pose) = drive(0., push, 60);
        assert!((held.velocity - Vec3::Z * MAX_PUSH).length() < 1e-3, "{:?}", held.velocity);
        assert!(pose.centre.z > 1.5 && pose.centre.x.abs() < 1e-4 && pose.centre.y == 0.5);
        // 20 m/s^2 force clamp: after one tick the prop has only 1/3 m/s.
        assert!((drive(0., push, 1).0.velocity.z - MAX_FORCE / 60.).abs() < 1e-4);
        let pull = Command {
            push: -1.,
            ..Default::default()
        };
        assert!((drive(0., pull, 60).0.velocity - Vec3::NEG_Z * MAX_PULL).length() < 1e-3);
        // Stick right strafes toward the skater's right: facing +Z in Y-up right-handed space that is -X
        // (82898D20 builds local = [-stickLR, 0, stickUD]; L = -X at 82d45610).
        let strafe = Command {
            strafe: 1.,
            ..Default::default()
        };
        assert!((drive(0., strafe, 60).0.velocity - Vec3::NEG_X * MAX_STRAFE).length() < 1e-3);
    }

    #[test]
    fn off_centre_push_turns_the_far_side_forward() {
        let push = Command {
            push: 1.,
            ..Default::default()
        };
        // Grip 0.8 m to the skater's right of centre (-X is the right when facing +Z): the prop yaws its +Z face
        // toward +X, the skater's left, and translates slower than centred.
        // curveA(0.8) = 0.170 and curveB(0.8) = 0.683: w = 0.683 * 0.170 * 2.
        let (held, pose) = drive(-0.8, push, 30);
        assert!((held.yaw_rate - 0.2326).abs() < 1e-3, "{}", held.yaw_rate);
        assert!((pose.rotation * Vec3::Z).x > 0.);
        assert!(held.velocity.length() < MAX_PUSH);
        let (mirrored, _) = drive(0.8, push, 30);
        assert!((mirrored.yaw_rate + held.yaw_rate).abs() < 1e-4);
        // Centred: no turn. Right stick alone turns in place.
        assert_eq!(drive(0., push, 30).0.yaw_rate, 0.);
        let rotate = Command {
            rotate: 1.,
            ..Default::default()
        };
        let (spin, pose) = drive(0., rotate, 120);
        // Right stick right turns the front face toward the skater's right (-X): yaw rate negative in the engine's
        // +Z-toward-+X sense (82d45bc4 drives the measured rate toward +w; rotate feeds w with the retail sign).
        assert!((spin.yaw_rate + ROTATION_SCALE).abs() < 1e-3 && spin.velocity.length() < 1e-3);
        assert!((pose.centre - Vec3::new(0., 0.5, 0.)).length() < 1e-4);
    }

    #[test]
    fn mover_axes_are_retail_heading_and_right() {
        // 82d45610: H = -Z, L = -X, where X = Y x Z is the skater's left.
        assert_eq!(axes(Vec3::Z), (Vec3::NEG_Z, Vec3::NEG_X));
    }

    #[test]
    fn sweep_stops_the_prop_at_a_wall_but_not_when_leaving_it() {
        let wall = box_prop(Vec3::new(5., 2., 0.1), Vec3::new(0., 2., 2.));
        let world = BoardWorld::new(prop_triangles(&wall, wall.pose, material()).unwrap());
        let half = Vec3::new(1., 0.5, 0.5);
        let (fraction, normal) =
            sweep(&world, at(Vec3::new(0., 0.5, 0.)), at(Vec3::new(0., 0.5, 3.)), half).expect("wall hit");
        assert!(fraction > 0.2 && fraction < 0.6, "{fraction}");
        assert!((normal - Vec3::NEG_Z).length() < 1e-4);
        assert!(sweep(&world, at(Vec3::new(0., 0.5, 0.)), at(Vec3::new(0., 0.5, -3.)), half).is_none());
    }

    /// A 0.2 m post 4 m off the line of a 10 m prop, and the prop's half extents.
    fn post_world() -> (BoardWorld, Vec3) {
        let post = box_prop(Vec3::new(0.1, 2., 0.1), Vec3::new(4., 2., 2.));
        (BoardWorld::new(prop_triangles(&post, post.pose, material()).unwrap()), Vec3::new(5., 0.5, 0.5))
    }

    #[test]
    fn a_long_prop_stops_where_its_end_meets_a_post() {
        // Round2 bug 7 ("drags through objects"): one sphere at the centre let a long prop's ends pass through.
        let (world, half) = post_world();
        let (fraction, normal) =
            sweep(&world, at(Vec3::new(0., 0.5, 0.)), at(Vec3::new(0., 0.5, 3.)), half).expect("post hit");
        // Its +Z face (z = 0.5) meets the post's -Z face (z = 1.9) after 1.4 m of 3.
        assert!(fraction > 0.4 && fraction < 0.55, "{fraction}");
        assert!((normal - Vec3::NEG_Z).length() < 1e-4);
        // Turning in place sweeps the ends too.
        let turned = Pose {
            rotation: Mat3::from_rotation_y(-0.6),
            centre: Vec3::new(0., 0.5, 0.),
        };
        assert!(sweep(&world, at(Vec3::new(0., 0.5, 0.)), turned, half).is_some());
    }

    #[test]
    fn turning_a_long_prop_into_a_post_stops_the_turn() {
        let (world, half) = post_world();
        let mut pose = at(Vec3::new(0., 0.5, 0.));
        let mut held = Held::bind(0, pose, half, 3, Vec3::new(0., 1., -1.), 0.);
        // Right stick right swings the +X end toward +Z, into the post (centre bearing atan2(2, 4) = 0.464 rad).
        let rotate = Command {
            rotate: 1.,
            ..Default::default()
        };
        for _ in 0..120 {
            pose = carry(&world, &mut held, pose, half, rotate, 1. / 60.);
        }
        let x = pose.rotation * Vec3::X;
        let turned = x.z.atan2(x.x);
        assert!(turned > 0.2 && turned < 0.464, "{turned}");
    }

    /// Boxes (half, centre, pitch about X) as one static world.
    fn slabs(parts: &[(Vec3, Vec3, f32)]) -> BoardWorld {
        BoardWorld::new(
            parts
                .iter()
                .flat_map(|&(half, centre, pitch)| {
                    let rotation = Mat3::from_rotation_x(pitch);
                    prop_triangles(&box_prop(half, centre), Pose { rotation, centre }, material()).unwrap()
                })
                .collect(),
        )
    }

    /// Binds the -Z face of a box resting on y = 0 and pushes it +Z.
    fn carry_for(world: &BoardWorld, half: Vec3, ticks: usize) -> Pose {
        let mut pose = box_prop(half, Vec3::new(0., half.y, 0.)).pose;
        let mut held = Held::bind(0, pose, half, 3, Vec3::new(0., 1., -1.), 0.);
        held.lift = lift(world, pose, half);
        assert!((held.lift.unwrap() - half.y).abs() < 1e-5);
        let push = Command {
            push: 1.,
            ..Default::default()
        };
        for _ in 0..ticks {
            pose = carry(world, &mut held, pose, half, push, 1. / 60.);
        }
        pose
    }

    #[test]
    fn held_prop_follows_the_ground_and_steps_or_drops_stop_it() {
        let floor = (Vec3::new(10., 0.1, 10.), Vec3::new(0., -0.1, 0.), 0.);
        // A 5 degree street rising toward +Z through the origin: no sinking.
        let pitch = -5f32.to_radians();
        let slope = (floor.0, Mat3::from_rotation_x(pitch) * floor.1, pitch);
        let half = Vec3::new(1., 0.5, 0.5);
        let pose = carry_for(&slabs(&[slope]), half, 60);
        assert!(pose.centre.z > 1.);
        assert!((pose.centre.y - (0.5 + pose.centre.z * 5f32.to_radians().tan())).abs() < 2e-3, "{pose:?}");
        // A tall box's sweep sphere clears a kerb face, so the ground probe
        // decides: it climbs 0.2 m but a 0.4 m step is a wall.
        let tall = Vec3::new(1., 1., 0.5);
        let step = |h: f32| (Vec3::new(5., h * 0.5, 1.75), Vec3::new(0., h * 0.5, 3.25), 0.);
        let pose = carry_for(&slabs(&[floor, step(0.2)]), tall, 90);
        assert!(pose.centre.z > 1.5 && (pose.centre.y - 1.2).abs() < 1e-4, "{pose:?}");
        let pose = carry_for(&slabs(&[floor, step(0.4)]), tall, 90);
        assert!(pose.centre.z < 1.5 && pose.centre.z > 1.4 && (pose.centre.y - 1.).abs() < 1e-5, "{pose:?}");
        // A floor ending at z = 1 over a void: it stops at the edge, no hover.
        let ledge = (Vec3::new(10., 0.1, 5.5), Vec3::new(0., -0.1, -4.5), 0.);
        let pose = carry_for(&slabs(&[ledge]), half, 90);
        assert!(pose.centre.z <= 1. + 1e-4 && pose.centre.z > 0.9 && (pose.centre.y - 0.5).abs() < 1e-5, "{pose:?}");
    }

    #[test]
    fn a_floor_touch_no_longer_hides_the_wall_behind_it() {
        let world = slabs(&[
            (Vec3::new(10., 0.1, 10.), Vec3::new(0., -0.1, 0.), 0.),
            (Vec3::new(5., 2., 0.1), Vec3::new(0., 2., 2.1), 0.),
        ]);
        // Radius 0.45 at y = 0.3: the floor is the nearest hit at fraction 0.
        let half = Vec3::new(1., 0.5, 0.5);
        let (fraction, normal) =
            sweep(&world, at(Vec3::new(0., 0.3, 0.)), at(Vec3::new(0., 0.3, 3.)), half).expect("wall behind the floor");
        assert!(fraction > 0.4 && fraction < 0.6, "{fraction}");
        assert!((normal - Vec3::NEG_Z).length() < 1e-4);
    }

    #[test]
    fn release_pushes_the_prop_off_the_board_or_back_to_its_clear_pose() {
        let floor = (Vec3::new(10., 0.1, 10.), Vec3::new(0., -0.1, 0.), 0.);
        let half = Vec3::new(1., 0.5, 0.5);
        let pose = box_prop(half, Vec3::new(0., 0.5, 0.)).pose;
        let mut held = Held::bind(0, pose, half, 3, Vec3::new(0., 1., -1.), 0.);
        held.lift = Some(0.5);
        held.clear = Pose {
            centre: Vec3::new(0., 0.5, -1.),
            ..pose
        };
        // Deck under the far half: pushed on along +Z until it leaves the held face.
        let deck = [Vec3::new(0., 0.05, 0.3)];
        let open = slabs(&[floor]);
        let out = release_pose(&open, &held, pose, half, &deck);
        assert!(!overlaps(out, half, &deck));
        assert!((out.centre - Vec3::new(0., 0.5, 0.91)).length() < 1e-4, "{out:?}");
        // A wall in the way: back to the last clear pose instead.
        let walled = slabs(&[floor, (Vec3::new(5., 2., 0.1), Vec3::new(0., 2., 1.3), 0.)]);
        let back = release_pose(&walled, &held, pose, half, &deck);
        assert!((back.centre - held.clear.centre).length() < 1e-5, "{back:?}");
        // Nothing inside: released where it is.
        let kept = release_pose(&open, &held, pose, half, &[Vec3::new(0., 0.9, -1.2)]);
        assert!((kept.centre - pose.centre).length() < 1e-5);
    }

    #[test]
    fn grip_keeps_the_margin_and_the_body_stands_off_the_real_face() {
        let half = Vec3::new(1., 0.5, 0.5);
        let pose = box_prop(half, Vec3::new(0., 0.5, 0.)).pose;
        let held = Held::bind(0, pose, half, 3, Vec3::new(0.95, 1., -1.), 0.25);
        assert!((pose.apply(held.grip).x - 0.75).abs() < 1e-5);
        // The -Z face is at z = -0.5; the root stands 0.65 m off it, facing +Z.
        let (target, z) = held.body_target(pose);
        assert!((target.z + 1.15).abs() < 1e-5 && (z - Vec3::Z).length() < 1e-6, "{target}");
        // A thin board keeps half its depth as inset: still 0.65 m off.
        let thin = Vec3::new(1., 0.5, 0.06);
        let held = Held::bind(0, pose, thin, 3, Vec3::new(0., 1., -1.), 0.);
        assert!((held.body_target(pose).0.z + 0.71).abs() < 1e-5);
    }

    #[test]
    fn face_splines_qualify_only_for_a_skater_facing_the_face() {
        let prop = box_prop(Vec3::new(1., 0.5, 0.5), Vec3::new(0., 0.5, 0.));
        let registry = native::Registry::new(vec![grab_object(0, ASSEMBLY_BASE + 1, &prop)]).unwrap();
        // Skater 0.6 m in front of the +Z face, facing -Z, feet on the ground.
        let frame = [
            [-1., 0., 0., 0.],
            [0., 1., 0., 0.],
            [0., 0., -1., 0.],
            [0., 0., 1.1, 1.],
        ];
        let query = |frame: native::Frame| native::Query {
            position: [frame[3][0], 1., frame[3][2], 1.],
            sort_position: [frame[3][0], 1., frame[3][2], 1.],
            bounds: board_bounds(frame, [0., 1., 0.1, 0.], [0.8, 0.8, 1.01, 0.]),
            limits: BoardLimits {
                margin: 0.25,
                angle_a: 80f32.to_radians(),
                angle_b: 50f32.to_radians(),
            },
            mode: 4,
            capacity: 5,
            context: skate_core::player::offboard::ground_query::QueryContext {
                selection_flags_2948: 0,
                matching_id_2952: -1,
            },
        };
        let found = native::query(&registry, &query(frame)).unwrap();
        assert_eq!(found.len(), 1);
        // +Z is face 2: descriptor id 3 maps back to prop 0.
        assert_eq!(found[0].descriptor().id, 3);
        assert_eq!((found[0].descriptor().id as usize - 1) / 4, 0);
        // Facing away from the prop: nothing.
        let mut away = frame;
        away[0] = [1., 0., 0., 0.];
        away[2] = [0., 0., 1., 0.];
        assert!(native::query(&registry, &query(away)).unwrap().is_empty());
        // A tutorial DMO outside its lesson (disabled): nothing either.
        let mut off = registry;
        off.objects[0].disabled = true;
        assert!(native::query(&off, &query(frame)).unwrap().is_empty());
    }

    #[test]
    fn relocate_moves_the_drawn_pose_onto_the_retail_matrix() {
        let at = |yaw: f32, p: Vec3| {
            let r = Mat3::from_rotation_y(yaw);
            [r.x_axis, r.y_axis, r.z_axis].map(|a| a.extend(0.).to_array()).into_iter().chain([p.extend(1.).to_array()]).collect::<Vec<_>>().try_into().unwrap()
        };
        let drawn = Pose {
            rotation: Mat3::IDENTITY,
            centre: Vec3::new(11., 0.5, 0.),
        };
        let placed = relocate(drawn, at(0., Vec3::new(10., 0., 0.)), at(90f32.to_radians(), Vec3::new(0., 2., 5.)));
        // 1 m along the model's +X turns with it to -Z.
        assert!((placed.centre - Vec3::new(0., 2.5, 4.)).length() < 1e-4, "{placed:?}");
        assert!((placed.rotation * Vec3::X - Vec3::NEG_Z).length() < 1e-5);
    }

    /// One tutorial DMO (lesson "lesson", id 7) as a world's only query mesh, hidden and disabled as loaded.
    fn staged_world(half: Vec3, centre: Vec3) -> (BoardWorld, Props, Vec<native::Object>) {
        let mut prop = box_prop(half, centre);
        (prop.owner, prop.active, prop.id) = (Some("lesson".into()), false, 7);
        let tris = prop_triangles(&prop, prop.pose, material()).unwrap();
        let metadata = skate_core::physics::board_world::query_metadata::QueryMetadata {
            packed_surfaces: vec![METAL as u16; tris.len()],
            meshes: vec![QueryMesh {
                triangle_range: 0..tris.len(),
                local_to_world: RetailAffineTransform::IDENTITY,
                world_to_local: RetailAffineTransform::IDENTITY,
                local_bounds: Bounds::from_points(tris.iter().flat_map(|t| t.vertices)).unwrap(),
                matching_group: -1,
                rejection_flags: 0,
                geometry: GEOMETRY_BASE,
                pool: QueryPool::Ground,
            }],
            static_edges: vec![],
            island_flags: 0,
        };
        let mut world = BoardWorld::with_query_metadata(tris, metadata).unwrap();
        world.hide_mesh(0).unwrap();
        let mut object = grab_object(0, ASSEMBLY_BASE + 1, &prop);
        object.disabled = true;
        let mut props = Props::empty(material());
        props.by_instance = vec![Some(0)];
        props.props.push(prop);
        (world, props, vec![object])
    }

    fn solid(world: &BoardWorld, at: Vec3) -> bool {
        world.query_thin_line(vector(at + Vec3::Y * 10.), vector(at)).unwrap().is_some()
    }

    #[test]
    fn staged_props_appear_for_their_lesson_and_reset_to_their_pose() {
        let (half, centre) = (Vec3::new(1., 0.5, 0.5), Vec3::new(0., 0.5, 0.));
        let (mut world, mut props, mut objects) = staged_world(half, centre);
        assert!(!solid(&world, centre) && props.dmos().next().is_none());
        props.stage(&mut world, &mut objects, Some("other"));
        assert!(!solid(&world, centre) && objects[0].disabled && !props.delta(0).1);
        let revision = props.revision;
        props.stage(&mut world, &mut objects, Some("lesson"));
        assert!(solid(&world, centre) && !objects[0].disabled && props.delta(0).1 && props.revision > revision);
        assert_eq!(props.dmos().collect::<Vec<_>>(), vec![(7, centre)]);
        // Pushed 3 m off; event 34 for a volume round where it was authored brings it back.
        let moved = Pose {
            centre: centre + Vec3::X * 3.,
            ..props.props[0].pose
        };
        props.props[0].pose = moved;
        world.set_mesh_triangles(0, &prop_triangles(&props.props[0], moved, material()).unwrap()).unwrap();
        props.reset(&mut world, &mut objects, |p| p.x.abs() < 1.);
        assert_eq!(props.props[0].pose, props.props[0].authored);
        assert!(solid(&world, centre) && !solid(&world, moved.centre));
        assert_eq!(objects[0].frame, props.props[0].authored.frame());
        // The lesson over: gone again.
        props.stage(&mut world, &mut objects, None);
        assert!(!solid(&world, centre) && objects[0].disabled && props.dmos().next().is_none());
    }

    #[test]
    fn a_staged_prop_without_a_grab_object_never_becomes_solid() {
        // G8 I1: a rejected grab registry must not leave an ungrabbable wall in the lesson's way.
        let (mut world, mut props, _) = staged_world(Vec3::new(1., 0.5, 0.5), Vec3::new(0., 0.5, 0.));
        props.stage(&mut world, &mut [], Some("lesson"));
        assert!(!props.props[0].active && !solid(&world, Vec3::new(0., 0.5, 0.)) && props.dmos().next().is_none());
    }

    /// G8 data test: Meet Slappy's gap dumpster on the installed Skate 2 tutorial map.
    #[test]
    #[ignore = "requires SKATE_ASSET_ROOT (with private/skate2-tutorial/dmos.json) and SKATE_TUTORIAL_MAP (New_San_Vanelona.skate)"]
    fn tutorial_dumpster_spawns_at_the_retail_pose_and_leaves_its_volumes_when_moved() {
        let root = std::path::PathBuf::from(std::env::var_os("SKATE_ASSET_ROOT").unwrap());
        let map = SkateMap::load(Path::new(&std::env::var_os("SKATE_TUTORIAL_MAP").unwrap())).unwrap();
        let world = crate::skate_world::collision_world(&map, material()).unwrap();
        let (mut world, mut props, mut objects, bindings) = Props::load_staged(&root, &map, world, material()).unwrap();
        let lesson = "m_1a_01_goto_slappys";
        let i = props.props.iter().position(|p| p.id == 0xFDA7_2725_7834_E7A3).expect("DumpsterGrunge_DMO_1058");
        let (authored, half) = (props.props[i].authored, props.props[i].half);
        assert_eq!(props.props[i].owner.as_deref(), Some(lesson));
        assert!(!props.props[i].active && objects[i].disabled && !solid(&world, authored.centre));
        // The render side draws exactly the staged props, and only their triangles.
        let (package, instances) = staged_package(&root, &map.name).unwrap();
        let drawn: std::collections::HashSet<u32> = instances.iter().flatten().copied().collect();
        assert_eq!(drawn, props.props.iter().map(|p| p.instance).collect());
        let vertex = |v: u32| package.geometry.vertices[v as usize].material as usize;
        assert!(package.geometry.indices.chunks_exact(3).all(|t| instances[vertex(t[0]) - 1].is_some()));
        println!("staged={} drawn triangles={}", props.props.len(), package.geometry.indices.len() / 3);
        // Retail: origin (1697.162, 108.183, -539.224), rows [-0.5736 0 -0.8192] [0 1 0] [0.8192 0 -0.5736].
        let x = authored.rotation * Vec3::X;
        assert!((x - Vec3::new(-0.5736, 0., -0.8192)).length() < 1e-3, "{x}");
        let origin = Vec3::new(1697.162, 108.183, -539.224);
        let offset = authored.centre - origin;
        println!("centre={} half={half} offset={offset}", authored.centre);
        assert!(Vec2::new(offset.x, offset.z).length() < 0.5 && offset.y > 0. && offset.y < 2. * half.y + 0.05);
        // It stands on the NSV street (probed with its own mesh hidden).
        // Every upright staged prop (both dumpsters and the wedge) stands on the NSV ground (M5). A prop retail authors
        // tilted (Just get up there's WoodPanel_DMO_1002 leans, up axis y 0.77) has its centre nearer the ground under
        // it than half_y, by its pose.
        for p in props.props.iter().filter(|p| (p.authored.rotation * Vec3::Y).y > 0.99) {
            let rest = lift(&world, p.authored, p.half);
            println!("{:#x} owner={:?} lift={rest:?} half_y={}", p.id, p.owner, p.half.y);
            assert!(rest.is_some_and(|l| (l - p.half.y).abs() < STEP_UP), "{:#x}", p.id);
        }
        let tutorial: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("private/skate2-tutorial/tutorial.json")).unwrap()).unwrap();
        let volume = |name: &str| {
            let v = &tutorial["placement"]["volumes"][format!("m_1a_01_goto_slappys_{name}")];
            let corner = |k: &str| Vec3::from_slice(&v[k].as_array().unwrap().iter().map(|x| x.as_f64().unwrap() as f32).collect::<Vec<_>>());
            (corner("min"), corner("max"))
        };
        let inside = |p: Vec3, (min, max): (Vec3, Vec3)| p.cmpge(min).all() && p.cmple(max).all();
        let freedom = [volume("moveable_freedom_vol01"), volume("moveable_freedom_vol02")];
        // The lesson stages it: solid, grabbable, tested in its freedom volumes (IsDMOInVolume true).
        let _registry = super::super::offboard::grab_scene::Registry::new(&world, objects.clone(), bindings).unwrap();
        props.stage(&mut world, &mut objects, Some(lesson));
        assert!(props.props[i].active && !objects[i].disabled && solid(&world, authored.centre));
        let (_, at) = props.dmos().find(|d| d.0 == props.props[i].id).unwrap();
        assert!((at - origin).length() < 0.01, "testers see the retail origin (M1): {at}");
        assert!(freedom.iter().any(|&v| inside(at, v)), "retail pose is in a freedom volume");
        // Pushed out (as the mover would): IsDMOInVolume flips for both.
        let moved = Pose {
            centre: authored.centre - x * 2.,
            ..authored
        };
        props.props[i].pose = moved;
        world.set_mesh_triangles(props.props[i].mesh, &prop_triangles(&props.props[i], moved, material()).unwrap()).unwrap();
        let (_, centre) = props.dmos().find(|d| d.0 == props.props[i].id).unwrap();
        assert!(freedom.iter().all(|&v| !inside(centre, v)), "{centre}");
        // Event 34 (DMO_reset_vol02) puts it back.
        let reset = volume("DMO_reset_vol02");
        props.reset(&mut world, &mut objects, |p| inside(p, reset));
        assert_eq!(props.props[i].pose, authored);
    }

    /// User 10-28 (bug 3, "the ramp has a lip ... you hit the edge of it and stop"): the move-it-to-the-box wedge,
    /// probed straight down every centimetre up the middle of its ride, rises from the NSV street without a step. RED
    /// with the rigid body's 12 proxy boxes in its collision: the base plate ends 3.2 cm above the ride surface at
    /// template x -0.056, at a wheel's axle height (retail collides the skater with the ClusteredMesh only).
    #[test]
    #[ignore = "requires SKATE_ASSET_ROOT (with private/skate2-tutorial/dmos.json) and SKATE_TUTORIAL_MAP (New_San_Vanelona.skate)"]
    fn the_tutorial_wedge_rises_from_the_street_without_a_step() {
        let root = std::path::PathBuf::from(std::env::var_os("SKATE_ASSET_ROOT").unwrap());
        let map = SkateMap::load(Path::new(&std::env::var_os("SKATE_TUTORIAL_MAP").unwrap())).unwrap();
        let world = crate::skate_world::collision_world(&map, material()).unwrap();
        let (mut world, mut props, mut objects, _) = Props::load_staged(&root, &map, world, material()).unwrap();
        props.stage(&mut world, &mut objects, Some("m_1a_06_photo_puzzle"));
        let wedge = props.props.iter().find(|p| p.id == 0x98F0_7C06_3D11_C29D).expect("WedgeRamp_large_DMO_1000");
        assert!(wedge.active);
        let (axes, origin) = (wedge.pose.rotation, wedge.pose.apply(wedge.origin));
        // Template space (dmos.json matrix): the toe toward +x (its skirt reaches x 0.218), the flat top from x -4.65,
        // the ride 3 m wide over z -3..0.
        let surface = |x: f32| {
            let at = origin + axes * Vec3::new(x, 0., -1.5);
            let hit = world.query_thin_line(vector(at + Vec3::Y * 2.), vector(at - Vec3::Y)).unwrap().expect("the street");
            hit.geometry.position.y
        };
        let street = surface(0.5);
        let mut last = street;
        for i in 1..=520 {
            let x = 0.5 - i as f32 * 0.01;
            let y = surface(x);
            assert!((y - last).abs() < 0.006, "a {:.1} mm step at template x {x:.2}", (y - last) * 1000.);
            last = y;
        }
        assert!(last - street > 1.0, "the probes climbed onto the top ({street} -> {last})");
    }

    /// 10-29 bug 7: a front wheel rolling up the wedge's centre line meets only ground. The welded recompute pairs
    /// the crease at template x -0.28 (triangles 109, 110 and 116 share it) as a convex edge with cos -0.951, so the
    /// wheel gets a wall contact (normal ~+x) before it; retail's codes there are 0x5a (concave) / 0x6c (cos 0.9997).
    #[test]
    #[ignore = "requires SKATE_ASSET_ROOT (with private/skate2-tutorial/dmos.json) and SKATE_TUTORIAL_MAP (New_San_Vanelona.skate)"]
    fn a_wheel_rolling_up_the_tutorial_wedge_meets_no_wall() {
        use skate_core::physics::{
            board::BodyId, board_step::CollisionBody, board_world::BoardWorldVolume, collision::Sphere,
            world_contact::ContactPrimitive,
        };
        let root = std::path::PathBuf::from(std::env::var_os("SKATE_ASSET_ROOT").unwrap());
        let map = SkateMap::load(Path::new(&std::env::var_os("SKATE_TUTORIAL_MAP").unwrap())).unwrap();
        let world = crate::skate_world::collision_world(&map, material()).unwrap();
        let (mut world, mut props, mut objects, _) = Props::load_staged(&root, &map, world, material()).unwrap();
        props.stage(&mut world, &mut objects, Some("m_1a_06_photo_puzzle"));
        let wedge = props.props.iter().find(|p| p.id == 0x98F0_7C06_3D11_C29D).expect("WedgeRamp_large_DMO_1000");
        let (axes, origin) = (wedge.pose.rotation, wedge.pose.apply(wedge.origin));
        const WHEEL: f32 = 0.031; // RETAIL_WHEEL_RADIUS
        let up_ramp = axes * Vec3::NEG_X;
        for i in 0..=28 {
            let x = 0.30 - i as f32 * 0.02;
            let at = origin + axes * Vec3::new(x, 0., -1.5);
            let hit = world.query_thin_line(vector(at + Vec3::Y * 2.), vector(at - Vec3::Y)).unwrap().expect("the ride");
            let (p, n) = (hit.geometry.position, hit.geometry.normal);
            let n = Vec3::new(n.x, n.y, n.z) * if n.y < 0. { -1. } else { 1. };
            let centre = Vec3::new(p.x, p.y, p.z) + n * WHEEL;
            let velocity = (up_ramp - n * up_ramp.dot(n)).normalize() * 6.;
            let volume = BoardWorldVolume {
                body: CollisionBody::Board(BodyId::RightFrontWheel),
                primitive: ContactPrimitive::Sphere(Sphere { center: vector(centre), radius: WHEEL }),
                linear_velocity: vector(velocity),
                material: material(),
            };
            let (query, retention) = super::super::ground::query_settings();
            for k in world.query_primitives(&[volume], query, retention) {
                assert!(k.contact.normal.y > 0.8, "a wall at template x {x:.2}: normal {:?}", k.contact.normal);
            }
        }
    }

    /// Round2 bug 7: the held prop is retail's held DMO (its mover velocity and yaw rate, its characteristics row, in
    /// contact while on the ground); released, it is still listed (stopped: SFXCTL_DynamicObjectPhysics then ends the
    /// drag at speed 0.2 m/s or less) until the skater is past the game thread's 30 m (`sub_827A43C8`).
    #[test]
    fn the_held_prop_is_heard_as_the_held_dmo_until_out_of_range() {
        let mut props = Props::empty(material());
        let mut prop = box_prop(Vec3::splat(0.5), Vec3::new(0., 0.5, 0.));
        prop.row = characteristics("WedgeRamp_large_DMO_1000");
        props.props.push(prop);
        assert_eq!(props.audio_dmo([0.; 3]), None, "nothing held yet");
        let mut held = Held::bind(0, at(Vec3::new(0., 0.5, 0.)), Vec3::splat(0.5), 3, Vec3::new(0., 1., -1.), 0.);
        held.velocity = Vec3::new(2., 0., 0.);
        held.yaw_rate = 0.5;
        held.lift = Some(0.5);
        props.held = Some(held);
        props.heard = Some(0);
        let dmo = props.audio_dmo([0.; 3]).unwrap();
        assert_eq!((dmo.id, dmo.held, dmo.contact, dmo.upright, dmo.on_side), (1, true, true, true, false));
        assert_eq!((dmo.velocity, dmo.angular), ([2., 0., 0.], [0., 0.5, 0.]));
        assert_eq!(dmo.row, 0x733F_4D47_5186_E147);
        assert_eq!(dmo.position, [0., 0.5, 0.]);
        // Carried with no ground under it, it touches nothing.
        props.held.as_mut().unwrap().lift = None;
        assert!(!props.audio_dmo([0.; 3]).unwrap().contact);
        // Released: not held, still, in contact where it was put down, listed while near.
        props.held = None;
        let dmo = props.audio_dmo([10., 0., 0.]).unwrap();
        assert_eq!((dmo.held, dmo.contact, dmo.velocity, dmo.angular), (false, true, [0.; 3], [0.; 3]));
        assert_eq!(props.audio_dmo([40., 0., 0.]), None, "past 30 m");
    }

    /// INFERRED: a DMO's characteristics row is the one named as its template (the wedge's by round2-research2).
    #[test]
    fn a_dmo_is_its_templates_characteristics_row() {
        assert_eq!(characteristics("WedgeRamp_large_DMO_1000"), 0x733F_4D47_5186_E147);
        assert_eq!(characteristics("PicnicTable_Wood_1000"), 0x2E11_3B4A_02B4_568B);
        assert_eq!(characteristics("PicnicTable_Broken_DMO_1000"), 0xE3FE_AF50_E247_4164);
        assert_eq!(characteristics("OilDrum_DMO_1004"), 0x97AC_9E3F_D25A_CCEC);
        assert_eq!(characteristics("DumpsterGrunge_DMO_1058"), 0x0D93_10B8_1E3C_9E50);
        assert_eq!(characteristics("WoodPanel_DMO_1002"), 0x5644_875D_5170_323A);
    }

    /// Slappy's gate (round2 10-14): a DynamicHull row (owner "hull:state", staged from dmos.json `hulls`) is solid and
    /// drawn while its hull is in that state (its vault default until the career sets one), never grabbable, and
    /// absent outside a career (None: retail loads a hull only in career mode, load mask 4, 0x823a8be8 / 0x824abf58).
    /// A lesson's staging leaves it alone. A report row's retail surface (the gate's 929) is its tag.
    #[test]
    fn a_dynamic_hull_row_follows_its_hulls_state() {
        fn stage(
            world: &mut BoardWorld,
            props: &mut Props,
            objects: &mut [native::Object],
            hulls: Option<&std::collections::BTreeMap<String, String>>,
            lesson: Option<&str>,
        ) {
            props.set_hulls(hulls);
            props.stage(world, objects, lesson);
        }
        let (half, centre) = (Vec3::new(1., 0.5, 0.5), Vec3::new(0., 0.5, 0.));
        let (mut world, mut props, mut objects) = staged_world(half, centre);
        props.props[0].owner = Some("slappys:Locked".into());
        props.props[0].grab = false;
        props.hull_defaults.insert("slappys".into(), "Locked".into());
        let mut states = std::collections::BTreeMap::new();
        stage(&mut world, &mut props, &mut objects, None, None);
        assert!(!solid(&world, centre) && !props.delta(0).1, "no career: no hull");
        stage(&mut world, &mut props, &mut objects, Some(&states), None);
        assert!(solid(&world, centre) && props.delta(0).1 && objects[0].disabled, "new career: Locked, drawn, never grabbed");
        states.insert("slappys".to_owned(), "Unlock".to_owned());
        stage(&mut world, &mut props, &mut objects, Some(&states), None);
        assert!(!solid(&world, centre) && !props.delta(0).1, "Unlock opens it");
        states.insert("slappys".to_owned(), "Locked".to_owned());
        stage(&mut world, &mut props, &mut objects, Some(&states), Some("m_1a_07_follow_slappy"));
        assert!(solid(&world, centre) && props.props[0].active, "Locked again, whatever lesson runs");
        let gate: Instance = serde_json::from_value(serde_json::json!({
            "instance_id": "0006764C03E38706",
            "name": "SlappysGateDoor_1000",
            "matrix": [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [0., 0., 0., 1.]],
            "collision": moved_box(half, centre).iter().map(|t| t.map(|p| p.to_array())).collect::<Vec<_>>(),
            "grab": false,
            "surface": 929,
        }))
        .unwrap();
        assert_eq!(shape(&gate, &[]).unwrap().3, 929);
    }

    /// Slappy's gate on the installed tutorial map (old_tutorial_gate.py): Locked (a new career) it stands across the
    /// gateway, solid (retail surface 929) and drawn; Unlock opens it; outside a career it is never there.
    #[test]
    #[ignore = "requires SKATE_ASSET_ROOT (with private/skate2-tutorial/dmos.json) and SKATE_TUTORIAL_MAP (New_San_Vanelona.skate)"]
    fn slappys_gate_blocks_the_gateway_while_locked() {
        let root = std::path::PathBuf::from(std::env::var_os("SKATE_ASSET_ROOT").unwrap());
        let map = SkateMap::load(Path::new(&std::env::var_os("SKATE_TUTORIAL_MAP").unwrap())).unwrap();
        let world = crate::skate_world::collision_world(&map, material()).unwrap();
        let (mut world, mut props, mut objects, bindings) = Props::load_staged(&root, &map, world, material()).unwrap();
        let i = props.props.iter().position(|p| p.id == 0x0006_764C_03E3_8706).expect("SlappysGateDoor_1000");
        assert_eq!(props.props[i].owner.as_deref(), Some("slappys:Locked"));
        assert!(!props.props[i].active && objects[i].disabled, "loaded hidden, never grabbable");
        let _registry = super::super::offboard::grab_scene::Registry::new(&world, objects.clone(), bindings).unwrap();
        // Mid-panel height, square through its middle (retail ends (1595.215, -511.067), (1597.488, -508.36)), 0.5 m
        // each side: yard to street.
        let (a, b) = (Vec3::new(1595.215, 101.8, -511.067), Vec3::new(1597.488, 101.8, -508.36));
        let (mid, across) = ((a + b) / 2., (b - a).cross(Vec3::Y).normalize() * 0.5);
        let through = |world: &BoardWorld| world.query_thin_line(vector(mid - across), vector(mid + across)).unwrap();
        let mut states = std::collections::BTreeMap::new();
        props.set_hulls(None);
        props.stage(&mut world, &mut objects, None);
        assert!(through(&world).is_none(), "no career: the gateway is open (and nothing else stands in it)");
        props.set_hulls(Some(&states));
        props.stage(&mut world, &mut objects, None);
        assert_eq!(through(&world).expect("Locked: the gate blocks").tag, 929);
        assert!(objects[i].disabled && props.delta(props.props[i].instance).1, "drawn, never grabbable");
        states.insert("slappys".to_owned(), "Unlock".to_owned());
        props.set_hulls(Some(&states));
        props.stage(&mut world, &mut objects, Some("m_1a_07_follow_slappy"));
        assert!(through(&world).is_none() && !props.delta(props.props[i].instance).1, "Unlock opens it");
        // The render side draws it from the package, with its own material.
        let (_, instances) = staged_package(&root, &map.name).unwrap();
        assert!(instances.contains(&Some(props.props[i].instance)));
    }
}
