//! The port's camera as the engine's view (Task 16, spec 3.3). Not tested in play.
//!
//! Under the switch the view is Skate 2's own camera: the port's `CameraMan` frame and the projection it renders with
//! (`Frame::camera`), not the engine's camera graph. Skate 2's camera options (Amendment D) reach it through the
//! profile (Task 19), not here.

use skate2_physics_protocol::Camera;
use skate_core::camera::CameraFrame;
use skate_core::math::Basis3;

/// The engine's camera frame for the port's camera `c`, after `previous`:
/// - the basis: the columns right, up and forward (the engine's right, up and At);
/// - the position, with w 1;
/// - the field of view: `2 * atan(1 / y_scale)` in degrees, where `y_scale` is the projection's y scale. The port's
///   clip matrix is the projection times the view (`project_producer`, column-major), so its row 1 (elements 1, 5 and
///   9) is `y_scale` times the camera's up, and its length is `y_scale` for any turn of the camera; element 5 alone is
///   that only while the camera is level. A matrix with no y scale keeps `previous`'s field of view;
/// - the previous basis and position, the velocities, the shake, the opacity and the blur from `previous`. Not native
///   Skate 2 (user 10-20): the port sends none of these, and the engine's renderer reads only the basis, the position,
///   the field of view and the cut;
/// - no cut (`discontinuity`): the switch sets one for a placement (Task 19).
pub(crate) fn frame(c: &Camera, previous: &CameraFrame) -> CameraFrame {
    let clip = &c.clip_from_world;
    let y_scale = (clip[1] * clip[1] + clip[5] * clip[5] + clip[9] * clip[9]).sqrt();
    let field_of_view_degrees = if y_scale.is_finite() && y_scale > 0.0 {
        (2.0 * (1.0 / y_scale).atan()).to_degrees()
    } else {
        previous.field_of_view_degrees
    };
    CameraFrame {
        basis: Basis3 { columns: [c.right, c.up, c.forward] },
        position: [c.position[0], c.position[1], c.position[2], 1.0],
        previous_basis: previous.basis,
        previous_position: previous.position,
        linear_velocity: previous.linear_velocity,
        angular_velocity: previous.angular_velocity,
        shake_translation: previous.shake_translation,
        discontinuity: false,
        field_of_view_degrees,
        opacity: previous.opacity,
        blur: previous.blur,
    }
}

/// What `frame` follows when the engine's camera has no frame yet: `CameraFrame::new`'s values (it is private to
/// `skate-core`), except a 60-degree field of view (`new` has 0) in case the port's first matrix has no y scale.
pub(crate) fn neutral() -> CameraFrame {
    let basis = Basis3 { columns: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] };
    CameraFrame {
        basis,
        position: [0.0; 4],
        previous_basis: basis,
        previous_position: [0.0; 4],
        linear_velocity: [0.0; 4],
        angular_velocity: [0.0; 4],
        shake_translation: [0.0; 4],
        discontinuity: false,
        field_of_view_degrees: 60.0,
        opacity: 1.0,
        blur: 1.0,
    }
}

#[cfg(test)]
mod tests {
    use skate2_physics_protocol::Camera;
    use skate_core::math::Basis3;

    use super::*;

    /// A projection like the port's (`project_producer`, column-major) with `y_scale`, seen through a view whose row 1
    /// (the camera's up) is `up`: the clip matrix's row 1 is `y_scale * up`.
    fn clip(y_scale: f32, up: [f32; 3]) -> [f32; 16] {
        let mut clip = [0.0; 16];
        clip[0] = -1.3;
        clip[1] = y_scale * up[0];
        clip[5] = y_scale * up[1];
        clip[9] = y_scale * up[2];
        clip[10] = 1.0;
        clip[11] = 1.0;
        clip[14] = -0.1;
        clip
    }

    fn previous() -> CameraFrame {
        let basis = Basis3 { columns: [[0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]] };
        CameraFrame {
            basis,
            position: [7.0, 8.0, 9.0, 1.0],
            previous_basis: basis,
            previous_position: [6.0, 8.0, 9.0, 1.0],
            linear_velocity: [60.0, 0.0, 0.0, 0.0],
            angular_velocity: [0.0, 0.5, 0.0, 0.0],
            shake_translation: [0.01, 0.0, 0.0, 0.0],
            discontinuity: true,
            field_of_view_degrees: 45.0,
            opacity: 0.5,
            blur: 0.25,
        }
    }

    #[test]
    fn the_port_camera_drives_the_view() {
        let camera = Camera {
            position: [1.0, 2.0, 3.0],
            forward: [0.0, 0.0, 1.0],
            right: [1.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            clip_from_world: clip(1.732_050_8, [0.0, 1.0, 0.0]),
        };
        let before = previous();
        let view = frame(&camera, &before);
        assert_eq!(view.position, [1.0, 2.0, 3.0, 1.0]);
        assert!((view.field_of_view_degrees - 60.0).abs() < 1e-4, "{}", view.field_of_view_degrees);
        assert_eq!(view.basis.columns, [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], "right, up, forward");
        assert!(!view.discontinuity, "only the switch sets a cut");
        assert_eq!((view.previous_basis.columns, view.previous_position), (before.basis.columns, before.position));
        assert_eq!(view.linear_velocity, before.linear_velocity);
        assert_eq!(view.angular_velocity, before.angular_velocity);
        assert_eq!(view.shake_translation, before.shake_translation);
        assert_eq!((view.opacity, view.blur), (before.opacity, before.blur));

        // A camera pitched down 30 degrees keeps its 60-degree field of view: the view's turn is not the lens.
        let (s, c) = 30f32.to_radians().sin_cos();
        let pitched = Camera {
            up: [0.0, c, s],
            forward: [0.0, -s, c],
            clip_from_world: clip(1.732_050_8, [0.0, c, s]),
            ..camera
        };
        let view = frame(&pitched, &before);
        assert!((view.field_of_view_degrees - 60.0).abs() < 1e-4, "{}", view.field_of_view_degrees);
        assert_eq!(view.basis.columns, [[1.0, 0.0, 0.0], [0.0, c, s], [0.0, -s, c]]);
    }
}
