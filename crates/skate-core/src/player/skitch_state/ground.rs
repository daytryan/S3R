//! Ground's skitch reach, TU3 82D39BB8 and its record loop 82D39D98. Ground's
//! update tail runs this when ProcessedPhysIn+2476 bit22 is set (else it
//! invalidates the grab owner). The caller submits `query` and the last
//! `request` to the PlayerGrabSpline owner and, on `attached`, disables
//! handplant contacts for 2 frames (82D91298). Not play-tested.
use super::math::{Frame, Vector, dot3, fsel, madd, recip, sub, transform};
use crate::player::offboard::grab_scene::{Descriptor, Record, qualify};
use crate::player::offboard::ground_sync::{BoardLimits, Bounds, skitch_bounds};
use crate::riding::grounded::state::PhysicsGroundState;

/// 0x8206D110, degrees to radians.
const DEGREES: f32 = f32::from_bits(0x3c8e_fa35);

/// physics_state_skitching/default, read live by retail through 0x830CFDA4+0x188.
#[derive(Clone, Copy, Debug)]
pub struct GroundReachSettings {
    /// Hash_8E3C1298E8072745, stock (0, 0.74, 0).
    pub offset: Vector,
    /// Hash_978B2A7533B6675D, stock (0.5, 0.7, 2.0).
    pub extents: Vector,
    /// Hash_BF433C58D0F12390, stock 1.0: distance subtracted before time-to-skitch.
    pub distance_bias: f32,
    /// Hash_424A1C69E0076DBB, stock 0.1: attach when time-to-skitch is below it.
    pub attach_time: f32,
}

pub struct GroundReachInput<'a> {
    /// ProcessedPhysIn+0xC0.
    pub frame_192: Frame,
    /// ProcessedPhysIn+0x250, the query and qualify position.
    pub position_592: Vector,
    /// ProcessedPhysIn+0x190.
    pub velocity_400: Vector,
    /// ProcessedPhysIn+0xB0.
    pub effective_position_176: Vector,
    /// ProcessedPhysIn+0xB20.
    pub ground_timer_2848: f32,
    /// ProcessedPhysIn+0xA20.
    pub skitch_value_2592: u32,
    /// Owner+0x2C30 validated records; Some only when owner flags_12836 & 0x40.
    pub validated: Option<&'a [Record]>,
}

#[derive(Clone, Copy, Debug)]
pub struct GroundReach {
    /// 82D74200 mode 0xFF, capacity 5, position = sort position = P+0x250.
    pub bounds: Bounds,
    pub limits: BoardLimits,
    /// Each 82585DD8 + vcall 0x2C overwrites the same primary request slot.
    pub request: Option<Descriptor>,
    pub attached: bool,
}

pub fn ground_reach(
    state: &mut PhysicsGroundState,
    settings: &GroundReachSettings,
    p: &GroundReachInput<'_>,
) -> GroundReach {
    let bounds = skitch_bounds(p.frame_192, settings.offset, settings.extents);
    let mut out = GroundReach {
        bounds,
        limits: BoardLimits {
            margin: f32::from_bits(0x3e80_0000),
            angle_a: 60.0 * DEGREES,
            angle_b: 30.0 * DEGREES,
        },
        request: None,
        attached: false,
    };
    if let Some(records) = p.validated {
        reach_records(state, settings, p, bounds, records, &mut out);
    }
    out
}

/// 82D39D98.
fn reach_records(
    state: &mut PhysicsGroundState,
    settings: &GroundReachSettings,
    p: &GroundReachInput<'_>,
    bounds: Bounds,
    records: &[Record],
    out: &mut GroundReach,
) {
    state.flag_2729 = false;
    state.word_2560 = 0;
    state.word_2564 = 0;
    state.scalar_2668 = 0.0;
    let ext = bounds.extents;
    let inner_extents = [ext[0] * 0.75, ext[1], ext[2] * 0.75, ext[3]];
    let mut inner = bounds;
    inner.extents = inner_extents;
    inner.frame[3] = madd(bounds.frame[2], inner_extents[2] - ext[2], bounds.frame[3]);
    let limits = BoardLimits {
        margin: 0.0,
        angle_a: 60.0 * DEGREES,
        angle_b: 40.0 * DEGREES,
    };
    for record in records {
        let [mut a, mut b] = record.endpoints();
        if 0.0 > dot3(sub(b, a), bounds.frame[0]) {
            std::mem::swap(&mut a, &mut b);
        }
        let key = record.descriptor();
        let chosen = if key.kind == 2 { inner } else { bounds };
        if !qualify(record, p.position_592, chosen, limits) {
            continue;
        }
        // fcmpu/ble: a NaN timer takes the check too.
        if !(p.ground_timer_2848 <= 0.0) && (key.kind != 1 || p.skitch_value_2592 == key.id) {
            continue;
        }
        let la = local(&chosen.frame, a);
        let lb = local(&chosen.frame, b);
        let t = recip(la[0] - lb[0]) * la[0];
        let on_line = madd(sub(lb, la), t, la);
        let world = transform(&chosen.frame, on_line);
        state.scalar_2668 = world[1] - p.effective_position_176[1];
        let closing = dot3(sub(p.velocity_400, record.vector(128)), p.frame_192[2]);
        // vmaxfp keeps a NaN; f32::max would not.
        let closing = if closing < 0.5 { 0.5 } else { closing };
        let distance = dot3(sub(world, p.frame_192[3]), p.frame_192[2]);
        let time = recip(closing) * (distance - settings.distance_bias);
        let time = fsel(time, time, 0.0);
        state.scalar_2664 = time;
        out.request = Some(key);
        if time < settings.attach_time {
            // this+0xAB0 = id too; no reader found, so it has no field here.
            state.flag_2729 = true;
            state.word_2560 = key.kind;
            state.word_2564 = key.id;
            out.attached = true;
            return;
        }
    }
}

/// Rigid inverse point transform in the 82D3A074 order.
fn local(f: &Frame, v: Vector) -> Vector {
    let col = |c: usize| [f[0][c], f[1][c], f[2][c], 0.0];
    let (c0, c1, c2) = (col(0), col(1), col(2));
    let p = f[3].map(|x| 0.0 - x);
    let t = madd(c0, p[0], madd(c1, p[1], c2.map(|x| x * p[2])));
    madd(c2, v[2], madd(c1, v[1], madd(c0, v[0], t)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::offboard::grab_scene::{Geometry, RecordInput};
    use std::sync::Arc;

    fn prop_line() -> Record {
        Record::from_geometry(RecordInput {
            descriptor: Descriptor { kind: 2, id: 7 },
            geometry: Arc::new(Geometry {
                id: 9,
                points: vec![[-2.0, 1.0, 2.0, 1.0], [2.0, 1.0, 2.0, 1.0]],
                approach_vectors: vec![[0.0, 0.0, -1.0, 0.0]],
                word_60: 0,
            }),
            frame: super::super::math::IDENTITY,
            object_vector_128: [0.0; 4],
            assembly: None,
            word_272: 0,
        })
        .unwrap()
    }

    fn reach(speed: f32, timer: f32, records: &[Record]) -> (PhysicsGroundState, GroundReach) {
        let settings = GroundReachSettings {
            offset: [0.0, 0.74, 0.0, 0.0],
            extents: [0.5, 0.7, 2.0, 0.0],
            distance_bias: 1.0,
            attach_time: 0.1,
        };
        let mut state = PhysicsGroundState::before_first_enter(true);
        state.word_2560 = 5;
        let input = GroundReachInput {
            frame_192: super::super::math::IDENTITY,
            position_592: [0.0; 4],
            velocity_400: [0.0, 0.0, speed, 0.0],
            effective_position_176: [0.0; 4],
            ground_timer_2848: timer,
            skitch_value_2592: 0,
            validated: Some(records),
        };
        let out = ground_reach(&mut state, &settings, &input);
        (state, out)
    }

    #[test]
    fn a_prop_line_ahead_is_timed_then_attached_when_close_enough() {
        let line = [prop_line()];
        let (state, out) = reach(5.0, 0.0, &line);
        assert_eq!(out.request, Some(Descriptor { kind: 2, id: 7 }));
        assert!(!out.attached && !state.flag_2729 && state.word_2560 == 0);
        assert!((state.scalar_2664 - 0.2).abs() < 1e-4, "{}", state.scalar_2664);
        assert!((state.scalar_2668 - 1.0).abs() < 1e-4);
        assert!((out.bounds.frame[3][1] - 0.74).abs() < 1e-6 && out.bounds.frame[3][2] == 2.0);

        let (state, out) = reach(20.0, 0.0, &line);
        assert!(out.attached && state.flag_2729);
        assert_eq!((state.word_2560, state.word_2564), (2, 7));
    }

    #[test]
    fn a_running_ground_timer_only_reaches_vehicles() {
        let (state, out) = reach(20.0, 0.5, &[prop_line()]);
        assert_eq!(out.request, None);
        assert!(!out.attached && state.word_2560 == 0 && state.scalar_2668 == 0.0);
    }
}
