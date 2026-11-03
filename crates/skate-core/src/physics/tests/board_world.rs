use super::*;
use crate::{
    math::Basis3,
    physics::{
        board::{BODY_COUNT, RETAIL_TRUCK_Y_POSITION, RETAIL_WHEEL_RADIUS},
        board_runtime::BoardMotion,
        board_step::BoardStepSettings,
        collision::TriangleFeature,
        drive_frames::{
            AuthoredTransformInputs, RetailAffineTransform, authored_body_transforms,
            default_truck_transforms,
        },
        drive_parameters::{RetailTruckDriveSettings, retail_truck_drive_dynamics},
        rigid_body::{
            RetailBodyMassProperties, RetailInertiaDynamics, RetailLocalMassFrame,
            RetailSimulationStep,
        },
    },
};

fn material(a: f32, b: f32, c: f32) -> RetailContactMaterial {
    RetailContactMaterial {
        static_friction: a,
        dynamic_friction: b,
        restitution: c,
    }
}
fn triangle() -> WorldTriangle {
    WorldTriangle::from_vertices(
        [
            Vector3::new(-10.0, 0.0, -10.0),
            Vector3::new(0.0, 0.0, 10.0),
            Vector3::new(10.0, 0.0, -10.0),
        ],
        material(0.8, 0.6, 0.2),
        42,
        TriangleFeature::ONE_SIDED | 0xe0,
        [1.0; 3],
        0.0,
    )
    .unwrap()
}
fn config() -> WheelWorldSettings {
    WheelWorldSettings {
        radius: RETAIL_WHEEL_RADIUS,
        material: material(0.2, 0.1, 0.8),
        query: WorldContactSettings {
            volume_padding: 0.0,
            maximum_separating_distance: 0.0,
            edge_cos_bend_normal_threshold: -1.0,
            convexity_epsilon: 0.0,
            is_object: false,
        },
        retention: ContactRetentionSettings {
            capacity: 100,
            duplicate_distance_squared: 0.000001,
            deferred_reduction: false,
        },
    }
}
fn board(gap: f32, gravity: Vector3) -> BoardRuntime {
    let mass = RetailBodyMassProperties {
        local_mass_frame: RetailLocalMassFrame::IDENTITY,
        dynamics: RetailInertiaDynamics {
            inverse_tensor: Vector3::new(3.0, 3.0, 3.0),
            inverse_mass: 0.5,
            spherical: 0.0,
            maximum_linear_velocity: 1000.0,
            maximum_angular_velocity: 1000.0,
            linear_drag: 0.0,
            angular_drag: 0.0,
        },
    };
    BoardRuntime::new(
        [mass; BODY_COUNT],
        authored_body_transforms(AuthoredTransformInputs::STOCK),
        RetailAffineTransform {
            translation: Vector3::new(
                0.0,
                RETAIL_WHEEL_RADIUS - RETAIL_TRUCK_Y_POSITION + gap,
                0.0,
            ),
            ..RetailAffineTransform::IDENTITY
        },
        RetailSimulationStep::fixed_60_hz(30, 0.001, gravity),
        BoardMotion::Active,
    )
}
fn close(a: f32, b: f32) {
    assert!((a - b).abs() < 2e-6, "{a} != {b}");
}

#[test]
fn finite_world_geometry_produces_four_ordered_wheel_contacts_with_combined_materials() {
    let mut world = BoardWorld::new(vec![triangle()]);
    let board = board(0.0, Vector3::ZERO);
    let contacts = world.query(&board, config());
    assert_eq!(contacts.len(), 4);
    for (index, collision) in contacts.iter().enumerate() {
        assert_eq!(collision.body_a, CollisionBody::Board(BodyId::ORDER[index]));
        assert_eq!(collision.body_b, CollisionBody::StaticWorld);
        close(collision.contact.normal.y, 1.0);
        close(collision.contact.position_on_a.y, 0.0);
        close(collision.contact.position_on_b.y, 0.0);
        assert_eq!(collision.contact.static_friction, 0.8);
        assert_eq!(collision.contact.dynamic_friction, 0.6);
        assert_eq!(collision.contact.restitution, 0.2);
        assert_eq!(collision.contact.tag, 42);
    }
}

#[test]
fn contact_query_uses_live_poses_and_rejects_air_remote_geometry_and_backfaces() {
    let mut world = BoardWorld::new(vec![triangle()]);
    let mut board = board(0.0, Vector3::ZERO);
    assert_eq!(world.query(&board, config()).len(), 4);
    for body in board.bodies_mut() {
        body.rates.position.y += 1.0;
    }
    assert!(world.query(&board, config()).is_empty());
    for body in board.bodies_mut() {
        body.rates.position.y -= 1.0;
        body.rates.position.x += 30.0;
    }
    assert!(world.query(&board, config()).is_empty());
    for body in board.bodies_mut() {
        body.rates.position.x -= 30.0;
        body.rates.position.y -= RETAIL_WHEEL_RADIUS * 1.5;
    }
    assert!(world.query(&board, config()).is_empty());
}

#[test]
fn approaching_velocity_padding_and_world_fatness_control_real_contact_acceptance() {
    let mut world = BoardWorld::new(vec![triangle()]);
    let mut board = board(0.01, Vector3::ZERO);
    let mut settings = config();
    settings.query.maximum_separating_distance = 0.05;
    assert!(world.query(&board, settings).is_empty());
    for body in board.bodies_mut() {
        body.rates.linear_velocity.y = -1.0;
    }
    assert_eq!(world.query(&board, settings).len(), 4);
    settings.query.maximum_separating_distance = 0.004;
    assert!(world.query(&board, settings).is_empty());
    for body in board.bodies_mut() {
        body.rates.linear_velocity.y = 1.0;
    }
    settings.query.volume_padding = 0.02;
    assert_eq!(world.query(&board, settings).len(), 4);
    settings.query.volume_padding = 0.0;
    let mut thick = triangle();
    thick.fatness = 0.02;
    let mut thick_world = BoardWorld::new(vec![thick]);
    let contacts = thick_world.query(&board, settings);
    assert_eq!(contacts.len(), 4);
    close(contacts[0].contact.position_on_b.y, 0.02);
}

#[test]
fn duplicate_triangles_and_explicit_contact_capacity_do_not_multiply_solver_rows() {
    let board = board(0.0, Vector3::ZERO);
    let mut world = BoardWorld::new(vec![triangle(), triangle()]);
    assert_eq!(world.query(&board, config()).len(), 4);
    assert_eq!(world.dropped_contacts(), 4);
    let mut settings = config();
    settings.retention.capacity = 3;
    assert_eq!(world.query(&board, settings).len(), 3);
    assert!(world.dropped_contacts() > 0);
    settings.retention.capacity = 100;
    settings.retention.deferred_reduction = true;
    let mut repeated = BoardWorld::new(vec![triangle(); 8]);
    assert_eq!(repeated.query(&board, settings).len(), 4);
}

#[test]
fn world_contacts_feed_supporting_reactions_into_the_persistent_board_solver() {
    let gravity = Vector3::new(0.0, -9.8, 0.0);
    let mut supported = board(0.0, gravity);
    let mut falling = board(0.0, gravity);
    let mut world = BoardWorld::new(vec![triangle()]);
    let collisions = world.query(&supported, config());
    assert_eq!(collisions.len(), 4);
    let settings = BoardStepSettings {
        simulation: RetailSimulationStep::fixed_60_hz(30, 0.001, gravity),
        iterations: 25,
        base_truck_transforms: default_truck_transforms(),
        truck_dynamics: retail_truck_drive_dynamics(RetailTruckDriveSettings::STOCK),
        force_point_y_offset: 0.0,
    };
    supported.advance(collisions, [0.0; 2], settings);
    falling.advance(&[], [0.0; 2], settings);
    let momentum = |board: &BoardRuntime| {
        board
            .bodies()
            .iter()
            .map(|body| body.rates.linear_velocity.y / body.inertia.inverse_mass)
            .sum::<f32>()
    };
    assert!(momentum(&supported) > momentum(&falling) + 0.01);
    assert_eq!(
        supported.hook().body.rates.world_inverse_inertia,
        Basis3 {
            columns: [[0.0; 3]; 3]
        }
    );
}

#[test]
fn host_geometry_rejects_degenerate_triangles() {
    assert!(
        WorldTriangle::from_vertices(
            [Vector3::ZERO; 3],
            material(0.0, 0.0, 0.0),
            0,
            0,
            [0.0; 3],
            0.0
        )
        .is_none()
    );
}

#[test]
fn water_is_culled_only_for_the_surface_tables_groups() {
    let volume = BoardWorldVolume {
        body: CollisionBody::Attached(0),
        primitive: ContactPrimitive::Sphere(Sphere {
            center: Vector3::new(0.0, RETAIL_WHEEL_RADIUS, 0.0),
            radius: RETAIL_WHEEL_RADIUS,
        }),
        linear_velocity: Vector3::new(0.0, -1.0, 0.0),
        material: material(0.0, 0.0, 0.0),
    };
    let water = WorldTriangle { tag: 0x637, ..triangle() };
    let ground = WorldTriangle { tag: 0x80, ..triangle() };
    let config = config();
    for (triangle, group, contacts) in [
        (water, 7, 0),
        (water, 16, 0),
        (water, 4, 1),
        (water, 6, 1),
        (ground, 7, 1),
    ] {
        let mut world = BoardWorld::new(vec![triangle]);
        let found = world.query_primitives_culled(&[volume], config.query, config.retention, |_, surface| {
            surface_culled(group, surface)
        });
        assert_eq!(found.len(), contacts, "group {group} tag {:#x}", triangle.tag);
    }
}

#[test]
fn attached_volume_contact_reaches_its_actual_solver_body() {
    use crate::physics::board_step::AttachedStep;
    let mut board = board(0.0, Vector3::ZERO);
    let mut attached = board.bodies()[0];
    attached.rates.position = Vector3::new(0.0, RETAIL_WHEEL_RADIUS, 0.0);
    attached.rates.linear_velocity.y = -1.0;
    let before = attached.rates.linear_velocity.y;
    let volume = BoardWorldVolume {
        body: CollisionBody::Attached(0),
        primitive: ContactPrimitive::Sphere(Sphere {
            center: attached.rates.position,
            radius: RETAIL_WHEEL_RADIUS,
        }),
        linear_velocity: attached.rates.linear_velocity,
        material: material(0.0, 0.0, 0.0),
    };
    let mut world = BoardWorld::new(vec![triangle(), triangle()]);
    let config = config();
    let contacts = world.query_primitives(&[volume], config.query, config.retention);
    assert_eq!(
        contacts.len(),
        1,
        "retention must preserve attached body identity"
    );
    assert_eq!(contacts[0].body_a, CollisionBody::Attached(0));
    let settings = BoardStepSettings {
        simulation: RetailSimulationStep::fixed_60_hz(30, 0.001, Vector3::ZERO),
        iterations: 25,
        base_truck_transforms: default_truck_transforms(),
        truck_dynamics: retail_truck_drive_dynamics(RetailTruckDriveSettings::STOCK),
        force_point_y_offset: 0.0,
    };
    board.advance_attached(
        contacts,
        [0.0; 2],
        settings,
        AttachedStep {
            bodies: vec![&mut attached],
            contacts: &mut [],
            joints: &mut [],
            drives: &mut [],
        },
    );
    assert!(attached.rates.linear_velocity.y > before + 0.5);
    assert!(
        board.contact_reports().is_empty(),
        "skater contacts are not board observations"
    );
}

/// #16 (fast into a ramp at an odd angle loses speed): the stock board at 8 m/s up a quarter pipe (radius 3 m, 6-degree
/// facets, edge cosines from the neighbouring faces as an archive's would give) at 0, 20 and 45 degrees of yaw, with
/// and without truck capsules. Prints, per facet seam the front truck crosses, the speed, the energy lost beyond the
/// climb since the last seam and the truck contacts in between. Materials and the truck capsule (8.5 mm by 64 mm,
/// collision-report.md) are test values, not the vault's.
#[test]
#[ignore = "diagnostic for #16: prints, asserts nothing (--ignored --nocapture)"]
fn ramp_seam_speed_loss_by_yaw() {
    const R: f32 = 3.0;
    const STEP_DEGREES: f32 = 6.0;
    const FACETS: i32 = 12;
    const W: f32 = 15.0;
    const SPEED: f32 = 8.0;
    const G: f32 = 9.8;
    let sub = |a: Vector3, b: Vector3| Vector3::new(a.x - b.x, a.y - b.y, a.z - b.z);
    let unit_normal = |f: &[Vector3; 3]| {
        let (u, v) = (sub(f[1], f[0]), sub(f[2], f[0]));
        let n = Vector3::new(u.y * v.z - u.z * v.y, u.z * v.x - u.x * v.z, u.x * v.y - u.y * v.x);
        let l = (n.x * n.x + n.y * n.y + n.z * n.z).sqrt();
        Vector3::new(n.x / l, n.y / l, n.z / l)
    };
    let same = |a: Vector3, b: Vector3| a.x == b.x && a.y == b.y && a.z == b.z;
    let rows: Vec<(f32, f32)> = [(-30.0, 0.0), (0.0, 0.0)]
        .into_iter()
        .chain((1..=FACETS).map(|k| {
            let a = (k as f32 * STEP_DEGREES).to_radians();
            (R * a.sin(), R * (1.0 - a.cos()))
        }))
        .collect();
    let at = |x: f32, (z, y): (f32, f32)| Vector3::new(x, y, z);
    let mut faces: Vec<[Vector3; 3]> = Vec::new();
    for w in rows.windows(2) {
        let (a, b, c, d) = (at(-W, w[0]), at(W, w[0]), at(W, w[1]), at(-W, w[1]));
        faces.push([a, c, b]);
        faces.push([a, d, c]);
    }
    // Edge i runs from vertex i to vertex i + 1 (fixup pairs edge_cosines[i] with edges[2 - i]); no neighbour: -1.
    let triangles: Vec<WorldTriangle> = (faces.iter().enumerate())
        .map(|(t, f)| {
            let n = unit_normal(f);
            let cosines = core::array::from_fn(|i| {
                let (a, b) = (f[i], f[(i + 1) % 3]);
                (faces.iter().enumerate())
                    .find(|(o, g)| *o != t && g.iter().any(|&v| same(v, a)) && g.iter().any(|&v| same(v, b)))
                    .map_or(-1.0, |(_, g)| {
                        let m = unit_normal(g);
                        n.x * m.x + n.y * m.y + n.z * m.z
                    })
            });
            let flags = 1 | TriangleFeature::ONE_SIDED | TriangleFeature::USE_EDGE_COSINES;
            WorldTriangle::from_vertices(*f, material(0.8, 0.6, 0.0), 0x80, flags, cosines, 0.0).unwrap()
        })
        .collect();
    let query = WorldContactSettings {
        volume_padding: 0.05,
        maximum_separating_distance: 0.5,
        edge_cos_bend_normal_threshold: 0.999,
        convexity_epsilon: 0.01,
        is_object: false,
    };
    let retention = ContactRetentionSettings { capacity: u32::MAX, duplicate_distance_squared: -1.0, deferred_reduction: true };
    let gravity = Vector3::new(0.0, -G, 0.0);
    let settings = BoardStepSettings {
        simulation: RetailSimulationStep::fixed_60_hz(30, 0.001, gravity),
        iterations: 25,
        base_truck_transforms: default_truck_transforms(),
        truck_dynamics: retail_truck_drive_dynamics(RetailTruckDriveSettings::STOCK),
        force_point_y_offset: 0.0,
    };
    let volumes = |board: &BoardRuntime, trucks: bool| -> Vec<BoardWorldVolume> {
        let poses = board.part_transforms();
        (BodyId::ORDER[..if trucks { 6 } else { 4 }].iter())
            .map(|&id| {
                let pose = poses[id.index()];
                let primitive = if id.index() < 4 {
                    ContactPrimitive::Sphere(Sphere { center: pose.translation, radius: RETAIL_WHEEL_RADIUS })
                } else {
                    let z = pose.basis.columns[2];
                    ContactPrimitive::Capsule {
                        center: pose.translation,
                        axis: Vector3::new(z[0], z[1], z[2]),
                        half_length: 0.064,
                        radius: 0.0085,
                    }
                };
                BoardWorldVolume {
                    body: CollisionBody::Board(id),
                    primitive,
                    linear_velocity: board.bodies()[id.index()].rates.linear_velocity,
                    material: material(0.8, 0.6, 0.0),
                }
            })
            .collect()
    };
    let seam_of = |p: Vector3| if p.z <= 0.0 { -1 } else { (p.z.atan2(R - p.y).to_degrees() / STEP_DEGREES) as i32 };
    let energy = |board: &BoardRuntime| {
        let (mut e, mut m) = (0.0, 0.0);
        for b in board.bodies() {
            let (v, mass) = (b.rates.linear_velocity, 1.0 / b.inertia.inverse_mass);
            e += (0.5 * (v.x * v.x + v.y * v.y + v.z * v.z) + G * b.rates.position.y) * mass;
            m += mass;
        }
        e / m
    };
    let speed = |board: &BoardRuntime| {
        let v = board.bodies()[BodyId::Deck.index()].rates.linear_velocity;
        (v.x * v.x + v.y * v.y + v.z * v.z).sqrt()
    };
    for trucks in [false, true] {
        for yaw in [0.0_f32, 20.0, 45.0] {
            let (s, c) = yaw.to_radians().sin_cos();
            let mut board = BoardRuntime::new(
                crate::physics::mass::default_skateboard_mass_properties(),
                authored_body_transforms(AuthoredTransformInputs::STOCK),
                RetailAffineTransform {
                    basis: Basis3 { columns: [[c, 0.0, -s], [0.0, 1.0, 0.0], [s, 0.0, c]] },
                    translation: Vector3::new(-4.0 * s / c, RETAIL_WHEEL_RADIUS - RETAIL_TRUCK_Y_POSITION, -4.0),
                },
                RetailSimulationStep::fixed_60_hz(30, 0.001, gravity),
                BoardMotion::Active,
            );
            for (i, body) in board.bodies_mut().iter_mut().enumerate() {
                body.rates.linear_velocity = Vector3::new(SPEED * s, 0.0, SPEED * c);
                if i < 4 {
                    // Rolling, so the wheels don't skid at the start: (up x v) / r.
                    body.rates.angular_velocity =
                        Vector3::new(SPEED * c / RETAIL_WHEEL_RADIUS, 0.0, -SPEED * s / RETAIL_WHEEL_RADIUS);
                }
            }
            let mut world = BoardWorld::new(triangles.clone());
            let (start_speed, start_energy) = (speed(&board), energy(&board));
            let mut seam = seam_of(board.bodies()[BodyId::FrontTruck.index()].rates.position);
            let (mut last_energy, mut truck_hits) = (start_energy, 0);
            println!("yaw {yaw} trucks {trucks}: in {start_speed:.3} m/s");
            for tick in 0..300 {
                let collisions = world.query_primitives(&volumes(&board, trucks), query, retention);
                truck_hits += (collisions.iter())
                    .filter(|k| matches!(k.body_a, CollisionBody::Board(BodyId::FrontTruck | BodyId::BackTruck)))
                    .count();
                board.advance(collisions, [0.0; 2], settings);
                let now = seam_of(board.bodies()[BodyId::FrontTruck.index()].rates.position);
                if now != seam {
                    let e = energy(&board);
                    let height = board.bodies()[BodyId::Deck.index()].rates.position.y;
                    println!(
                        "  tick {tick:3} seam {seam:>2}->{now:>2} speed {:.3} height {height:.3} lost {:.4} J/kg truck contacts {truck_hits}",
                        speed(&board),
                        last_energy - e
                    );
                    (seam, last_energy, truck_hits) = (now, e, 0);
                }
                let v = board.bodies()[BodyId::Deck.index()].rates.linear_velocity;
                if now >= FACETS || (v.x * s + v.z * c <= 0.0 && v.y <= 0.0) {
                    break;
                }
            }
            println!("  out {:.3} m/s, lost beyond the climb {:.4} J/kg", speed(&board), start_energy - energy(&board));
        }
    }
}
