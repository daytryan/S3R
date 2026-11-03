//! The port's pose on the engine's skater (Task 16). Not tested in play.
//!
//! The port's matrices (`Frame::bones`) are world matrices in row-vector order (host/src/frame.rs), the layout of the
//! engine's `NativeMatrix`: rows x, y and z, then the position, so "`a` then `b`" is the row-major product `a * b`.
//! The engine draws its skater from `render_pose`, in animation space, placed by `animation_to_world`
//! (presentation.rs), so each bone goes into animation space first (the Task 5 ruling):
//! `render_pose[i] = bones[i] * inverse(animation_to_world)`, and `render_pose[i] * animation_to_world` is where the
//! port draws it.
//!
//! `animation_to_world` follows the engine's convention: a rigid frame at the skater's root (`SkeletonRootFrames`: its
//! rows the turned axes, made orthonormal z first, and the position, every w lane 0). The skater root is the port's own,
//! `bones[0]` (the trajectory, `GetSkaterRoot`). `Frame::root`, the presentation's `local_to_world` at the hips, is not
//! used.

use skate2_physics_protocol::{Frame, PORT_BONES};
use skate_core::animation::output::NativeMatrix;

use crate::physics::{REPARENTED_HELPERS, reparented_helpers};

/// The port's matrix `m` as a `NativeMatrix` (the same layout, by rows).
fn native(m: &[f32; 16]) -> NativeMatrix {
    std::array::from_fn(|row| std::array::from_fn(|column| m[row * 4 + column]))
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// "`a` then `b`" for affine matrices: the rows of the row-major product `a * b`, with `b`'s position added to the
/// position row whatever `a`'s w lanes hold (the engine stores 0 there, the port 1). Column 3 stays `a`'s.
pub(crate) fn then(a: &NativeMatrix, b: &NativeMatrix) -> NativeMatrix {
    let mut out = *a;
    for (row, line) in out.iter_mut().enumerate() {
        for column in 0..3 {
            let turned = a[row][0] * b[0][column] + a[row][1] * b[1][column] + a[row][2] * b[2][column];
            line[column] = if row == 3 { turned + b[3][column] } else { turned };
        }
    }
    out
}

/// The inverse of a rigid `m` (orthonormal rows): the turn transposed, and the position taken back through it. Every
/// w lane is 0, as the engine's `inverse_rigid` leaves them.
pub(crate) fn inverse_rigid(m: &NativeMatrix) -> NativeMatrix {
    let mut out = [[0.0; 4]; 4];
    for row in 0..3 {
        for column in 0..3 {
            out[row][column] = m[column][row];
        }
    }
    for column in 0..3 {
        out[3][column] = -(m[3][0] * out[0][column] + m[3][1] * out[1][column] + m[3][2] * out[2][column]);
    }
    out
}

/// The engine's `animation_to_world` for `frame`: the port's skater root (`bones[0]`), its axes made orthonormal in the
/// order the engine's `orthonormalize` (825C5710) uses, z, then y, then x, each without its part along the ones before.
/// An axis that collapses keeps the identity's.
pub(crate) fn animation_to_world(frame: &Frame) -> NativeMatrix {
    let root = native(&frame.bones[0]);
    let mut axes = [[0.0f32; 3]; 3];
    for axis in (0..3).rev() {
        let mut v = [root[axis][0], root[axis][1], root[axis][2]];
        for prior in axis + 1..3 {
            let along = dot(axes[prior], v);
            for (lane, value) in v.iter_mut().enumerate() {
                *value -= along * axes[prior][lane];
            }
        }
        let length = dot(v, v).sqrt();
        axes[axis] = if length.is_finite() && length > 1e-6 {
            v.map(|value| value / length)
        } else {
            std::array::from_fn(|lane| if lane == axis { 1.0 } else { 0.0 })
        };
    }
    let [x, y, z] = axes;
    [
        [x[0], x[1], x[2], 0.0],
        [y[0], y[1], y[2], 0.0],
        [z[0], z[1], z[2], 0.0],
        [root[3][0], root[3][1], root[3][2], 0.0],
    ]
}

/// The port's pose in animation space, by `ours`' names (the engine skater's bones, in its order): each of the 32 port
/// bones at the bone of the same name (case aside), and the four `*_REPARENTED` helpers by the engine's helper rule
/// (`physics::reparented_helpers`). A name of neither kind gets the root's matrix; the stock skater has none.
pub(crate) fn retarget(frame: &Frame, ours: &[&str]) -> Vec<NativeMatrix> {
    let to_animation = inverse_rigid(&animation_to_world(frame));
    let port: [NativeMatrix; 32] = std::array::from_fn(|i| then(&native(&frame.bones[i]), &to_animation));
    let helpers = reparented_helpers(&port);
    let find = |names: &[&str], name: &str| names.iter().position(|n| n.eq_ignore_ascii_case(name));
    ours.iter()
        .map(|name| {
            find(&PORT_BONES, name)
                .map(|i| port[i])
                .or_else(|| find(&REPARENTED_HELPERS, name).map(|i| helpers[i]))
                .unwrap_or(port[0])
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use skate2_physics_protocol::{Frame, PORT_BONES};

    use super::*;

    /// `tools/author_bike_poses.py` `BONES`: the engine skater's bones, in its order.
    const OURS: [&str; 36] = [
        "TRAJECTORY", "HIPS", "SPINE", "SPINE1", "SPINE2", "SPINE3", "NECK", "NECK1",
        "HEAD", "RIGHTSHOULDER", "RIGHTARM", "RIGHTFOREARM", "RIGHTHAND", "LEFTSHOULDER",
        "LEFTARM", "LEFTFOREARM", "LEFTHAND", "RIGHTUPLEG", "RIGHTLEG", "RIGHTFOOT",
        "RIGHTTOEBASE", "LEFTUPLEG", "LEFTLEG", "LEFTFOOT", "LEFTTOEBASE",
        "SKATEBOARD_ROOT", "TRUCK_FRONT", "RIGHT_WHEELFRONT", "LEFT_WHEELFRONT",
        "TRUCK_BACK", "LEFT_WHEELBACK", "RIGHT_WHEELBACK", "RIGHTTOEBASE_REPARENTED",
        "LEFTTOEBASE_REPARENTED", "RIGHTHAND_REPARENTED", "LEFTHAND_REPARENTED",
    ];

    /// A rigid world matrix in the port's layout (row-vector, rows x, y, z, position): a turn of `yaw` about y, then
    /// a tilt of `roll` about the turned z, at `at`.
    fn rigid(yaw: f32, roll: f32, at: [f32; 3]) -> [f32; 16] {
        let (sy, cy) = yaw.sin_cos();
        let (sr, cr) = roll.sin_cos();
        let x = [cy * cr, sr, -sy * cr];
        let y = [-cy * sr, cr, sy * sr];
        let z = [sy, 0.0, cy];
        [
            x[0], x[1], x[2], 0.0, y[0], y[1], y[2], 0.0, z[0], z[1], z[2], 0.0, at[0], at[1], at[2], 1.0,
        ]
    }

    /// A frame whose 32 bones are distinct rigid matrices far from the origin, as a skater out in the city.
    pub(crate) fn posed() -> Frame {
        let mut frame = Frame::default();
        for (i, bone) in frame.bones.iter_mut().enumerate() {
            let i = i as f32;
            *bone = rigid(0.7 + 0.1 * i, 0.03 * i, [-183.4 + 0.02 * i, 12.3 + 0.05 * i, 433.2 - 0.01 * i]);
        }
        frame
    }

    fn native(m: &[f32; 16]) -> NativeMatrix {
        std::array::from_fn(|row| std::array::from_fn(|column| m[row * 4 + column]))
    }

    fn assert_close(got: &NativeMatrix, want: &NativeMatrix, what: &str) {
        for row in 0..4 {
            for column in 0..3 {
                let (g, w) = (got[row][column], want[row][column]);
                assert!((g - w).abs() < 1e-4, "{what}: [{row}][{column}] is {g}, not {w}\n{got:?}\n{want:?}");
            }
        }
    }

    #[test]
    fn the_bones_map_by_name() {
        let frame = posed();
        let render = retarget(&frame, &OURS);
        assert_eq!(render.len(), OURS.len());
        let to_world = animation_to_world(&frame);
        // The engine's convention: the skater root (the port's trajectory, `bones[0]`) as a rigid frame.
        assert_eq!(to_world[3][..3], frame.bones[0][12..15], "the root stands where the port's skater root is");
        assert_close(&render[0], &[[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0; 4]], "trajectory");
        // Render space, then animation_to_world, is where the port draws each bone (the Task 5 ruling).
        for (i, name) in PORT_BONES.iter().enumerate() {
            let ours = OURS.iter().position(|n| n.eq_ignore_ascii_case(name)).expect("our skeleton has every port bone");
            assert_close(&then(&render[ours], &to_world), &native(&frame.bones[i]), name);
        }
        // The four helpers follow the engine's helper rule, from the same 32.
        let first: [NativeMatrix; 32] = std::array::from_fn(|i| render[i]);
        assert_eq!(render[32..], reparented_helpers(&first), "the helpers bit for bit");
        // By name, not by position: the same skeleton in another order gets the same matrices.
        let mut reversed = OURS;
        reversed.reverse();
        let shuffled = retarget(&frame, &reversed);
        for (i, matrix) in shuffled.iter().enumerate() {
            assert_eq!(*matrix, render[OURS.len() - 1 - i], "{}", reversed[i]);
        }
    }
}
