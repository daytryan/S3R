//! The push, from intent to the deck's acceleration accumulators.

use super::float::{V4, fadds, fdivs, fmuls, fsel, fsubs, permwi, splat, vaddfp, vmaddfp, vmulfp, vnmsubfp};
use super::integrate::RigidBody;

/// `ProcessedPhysIn +2244` bit: the skater is pushing this tick.
pub const PUSH_REQUESTED: u32 = 0x0200_0000;
/// `ProcessedPhysIn +2248` bit: a push is requested but not allowed.
pub const PUSH_BLOCKED: u32 = 0x0000_0800;
/// Skateboard `+1328` bit the push sets when it is blocked.
pub const PUSH_BLOCKED_FLAG: u32 = 0x4000_0000;

/// The `ProcessedPhysIn` fields `Toolkit_CalcPushAcceleration` reads (native byte offsets).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PushInput {
    /// `+2244`.
    pub flags: u32,
    /// `+2248`.
    pub flags2: u32,
    /// `+2364`.
    pub delta_seconds: f32,
    /// `+2368`: the speed the push animation wants.
    pub target_speed: f32,
    /// `+2372`: the speed it is measured against.
    pub current_speed: f32,
    /// `+2376`: the board's speed for the pushable-speed cap.
    pub body_speed: f32,
    /// `+2416`: the summed body mass (the native field the TU3 port calls `scale`).
    pub mass: f32,
    /// `+352`: push direction, all four lanes used.
    pub direction: V4,
}

/// `physics_push` attributes; a missing collection or field reads `0.0` (the image's zero block
/// at `0x82E57998`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PushTweaks {
    /// `0x76FEEE54A3AD2AF5` (name not recovered): the largest speed change one push tick may ask for.
    pub speed_change_limit: f32,
    /// `MaxPushableSpeed`.
    pub max_pushable_speed: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PushOutput {
    /// World force (all four lanes).
    pub force: V4,
    /// Application point in the body frame: always zero.
    pub point: V4,
    /// The blocked bit was raised on the skateboard (`+1328 |= 0x40000000`).
    pub blocked: bool,
}

/// `Skateboard::Toolkit_CalcPushAcceleration`, `0x8297B7A8`.
///
/// Asks for the gap between target and current speed, never negative, clamped to the per-tick
/// limit both ways, then capped so the board does not pass `MaxPushableSpeed`. The force is
/// `mass * change / dt` along the push direction.
pub fn calc_push_acceleration(input: &PushInput, tweaks: &PushTweaks) -> PushOutput {
    let mut output = PushOutput::default();
    if input.flags & PUSH_REQUESTED == 0 {
        return output;
    }
    if input.flags2 & PUSH_BLOCKED != 0 {
        output.blocked = true;
        return output;
    }
    // `bgt`: a NaN gap takes the zero branch.
    let mut change = fsubs(input.target_speed, input.current_speed);
    if !(change > 0.0) {
        change = 0.0;
    }
    let limit = tweaks.speed_change_limit;
    let (above, below) = (change > limit, change < -limit);
    if above {
        change = limit;
    }
    if below {
        change = -limit;
    }
    let max = tweaks.max_pushable_speed;
    if fadds(input.body_speed, change) > max {
        let room = fsubs(max, input.body_speed);
        change = fsel(room, room, 0.0);
    }
    let magnitude = fdivs(fmuls(input.mass, change), input.delta_seconds);
    output.force = vmulfp(input.direction, [magnitude; 4]);
    output
}

/// `vpermwi128 …,99`: `[y, z, x, w]`.
fn yzx(v: V4) -> V4 {
    permwi(v, 0b01_10_00_11)
}

/// Records one tick may hold; `0x8297EB80` drops anything past this.
pub const FORCE_RECORD_CAPACITY: usize = 21;

/// One queued point force (native: 48 bytes, tag at `+0`, force `+16`, point `+32`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ForceRecord {
    /// Who queued it (push = 3). Not read by the accumulator.
    pub tag: u32,
    pub force: V4,
    pub point: V4,
}

/// `0x8297EB80(skateboard, tag, *{force, point})`: append unless 21 are already queued.
pub fn queue_force(records: &mut Vec<ForceRecord>, tag: u32, force: V4, point: V4) -> bool {
    if records.len() >= FORCE_RECORD_CAPACITY {
        return false;
    }
    records.push(ForceRecord { tag, force, point });
    true
}

/// `0x8297A8B8(skateboard)`: every queued record, whatever its tag, adds `force / mass` to the deck
/// body's linear accumulator and `I⁻¹ (R p × f)` to the angular one, `p` being the record's point
/// lifted by the deck's `DeckForceYOffset` (physicsdeck; `0.0` when absent), and wakes the body
/// (sleep counter `0`). The energy lane and everything else is left alone; records are not cleared.
pub fn accumulate_forces(records: &[ForceRecord], deck_force_y_offset: f32, body: &mut RigidBody) {
    let offset = [0.0, deck_force_y_offset, 0.0, 0.0];
    let [row0, row1, row2] = body.rotation;
    let [packed_a, packed_b] = body.inverse_inertia;
    let inverse_mass = packed_a[3];
    // Columns of the symmetric inverse inertia, as `vperm` assembles them from the two packed
    // vectors (`0x82108CC0` and `0x82108CD0`).
    let column_y = [packed_a[1], packed_b[1], packed_b[2], packed_a[3]];
    let column_z = [packed_a[2], packed_b[2], packed_b[0], packed_a[3]];
    for record in records {
        let point = vaddfp(record.point, offset);
        let lever = vmaddfp(row2, splat(point, 2), vmaddfp(row1, splat(point, 1), vmulfp(row0, splat(point, 0))));
        let linear = vmulfp(record.force, [inverse_mass; 4]);
        // lever × force, built as lever * force.yzx - lever.yzx * force, then rotated back.
        let cross = vnmsubfp(yzx(lever), record.force, vmulfp(lever, yzx(record.force)));
        let torque = yzx(cross);
        let angular = vmaddfp(
            column_z,
            splat(torque, 2),
            vmaddfp(column_y, splat(torque, 1), vmulfp(packed_a, splat(torque, 0))),
        );
        let a = &mut body.acceleration;
        *a = [fadds(linear[0], a[0]), fadds(a[1], linear[1]), fadds(a[2], linear[2]), a[3]];
        let w = &mut body.angular_acceleration;
        *w = [fadds(w[0], angular[0]), fadds(angular[1], w[1]), fadds(angular[2], w[2]), 0.0];
    }
}
