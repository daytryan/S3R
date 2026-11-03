use super::*;

#[test]
#[ignore = "requires the user's converted stock camera and graph assets"]
fn private_stock_camera_loads_every_graph_operation_and_shot_dependency() {
    let root = std::env::var_os("SKATE3_ASSET_ROOT").expect("set SKATE3_ASSET_ROOT");
    let root = std::path::Path::new(&root);
    let camera = CameraRuntime::load(root).expect("complete normal stock camera assets");
    assert!(camera.frame.is_none());
}

#[test]
#[ignore = "requires the user's converted stock camera and graph assets"]
fn minimized_window_keeps_the_last_aspect_ratio() {
    let root = std::env::var_os("SKATE3_ASSET_ROOT").expect("set SKATE3_ASSET_ROOT");
    let mut camera = CameraRuntime::load(std::path::Path::new(&root)).unwrap();
    camera.set_aspect_ratio(1280. / 720.);
    // A minimized window reports 0x0.
    camera.set_aspect_ratio(0. / 0.);
    camera.set_aspect_ratio(0.);
    assert_eq!(camera.manager.state.aspect_ratio, 1280. / 720.);
}

#[test]
#[ignore = "requires the user's converted stock camera and camera-styles assets"]
fn every_camera_style_loads_and_its_graph_only_chooses_defined_shots() {
    use skate_core::camera::ShotDatabase;
    let root = std::env::var_os("SKATE3_ASSET_ROOT").expect("set SKATE3_ASSET_ROOT");
    let root = std::path::Path::new(&root);
    for (style, name) in STYLES.iter().enumerate() {
        // load_style: load() would quietly fall back to the stock camera.
        let camera = CameraRuntime::load_style(root, style)
            .unwrap_or_else(|e| panic!("style {name:?}: {e}"));
        for shot in camera.graph.chosen_shots() {
            camera
                .shots
                .load(shot)
                .unwrap_or_else(|e| panic!("style {name:?}: {e}"));
        }
    }
}

#[test]
#[ignore = "requires the user's converted stock camera and camera-styles assets"]
fn switching_style_keeps_the_frame_offboard_input_steers_by() {
    let root = std::env::var_os("SKATE3_ASSET_ROOT").expect("set SKATE3_ASSET_ROOT");
    let root = std::path::Path::new(&root);
    let mut camera = CameraRuntime::load_style(root, 0).unwrap();
    let basis = skate_core::math::Basis3 {
        columns: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
    };
    let frame = CameraFrame {
        basis,
        position: [1.0, 2.0, 3.0, 1.0],
        previous_basis: basis,
        previous_position: [0.0; 4],
        linear_velocity: [0.0; 4],
        angular_velocity: [0.0; 4],
        shake_translation: [0.0; 4],
        discontinuity: false,
        field_of_view_degrees: 60.0,
        opacity: 1.0,
        blur: 1.0,
    };
    camera.frame = Some(frame);
    camera.switch_style(1, root);
    STYLE.store(0, Ordering::Relaxed);
    assert_eq!(camera.style, 1);
    assert_eq!(camera.frame.map(|f| f.position), Some(frame.position));
}
