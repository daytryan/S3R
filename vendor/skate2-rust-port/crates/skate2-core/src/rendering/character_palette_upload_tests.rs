use super::*;

fn identity() -> AnimationMatrix {
    let mut matrix = [0.0; 16];
    for i in [0, 5, 10, 15] {
        matrix[i] = 1.0;
    }
    matrix
}
#[test]
fn upload_keeps_fused_four_term_graph_and_transposes_three_rows() {
    let mut world = identity();
    world[0] = -1.0;
    world[4] = 1.0 + f32::EPSILON;
    let mut matrix = identity();
    matrix[1] = 1.0 - f32::EPSILON;
    matrix[3] = 2.0;
    world[12] = 3.0;
    let rows = pack_rows(&[matrix], world);
    // W is consumed in the upload graph, unlike the affine bind composition.
    assert_eq!(
        rows[0][0].to_bits(),
        (-f32::EPSILON.powi(2)).mul_add(1.0, 6.0).to_bits()
    );
    world[12] = 0.0;
    let rows = pack_rows(&[matrix], world);
    assert_eq!(rows[0][0].to_bits(), (-f32::EPSILON.powi(2)).to_bits());
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[1][0].to_bits(), (1.0 - f32::EPSILON).to_bits());
    assert_eq!(rows[2][2].to_bits(), 1.0_f32.to_bits());
}

fn publication() -> CharacterPalette {
    CharacterPalette {
        identity: CharacterPaletteIdentity(Arc::new(())),
        scene: Arc::new(CharacterScene {
            customizations: None, schema_name: "test".into(),
            category: 0,
            components: vec![],
            textures: vec![],
        }),
        skinning: CharacterSkinningState {
            lod_level: 0,
            hierarchy_inverse_bind_pose: Arc::from([]),
            parts: vec![],
        },
        removed_component: None,
        parts: vec![PalettePart {
            matrices: Some(vec![identity(), identity()]),
            packed: None,
            rows: None,
            meshes: vec![0..3, 3..6],
        }],
        generation: 0,
        published: false,
        transforms_complete: true,
    }
}
#[test]
fn mesh_rows_share_completed_publication_and_cannot_repack_stale_matrices() {
    let mut palette = publication();
    assert!(matches!(
        palette.mesh_rows(0, 0),
        Err(CharacterPaletteError::NotPublished)
    ));
    palette.pack_for_upload(identity()).unwrap();
    let first = palette.mesh_rows(0, 0).unwrap();
    assert_eq!(first.generation, 1);
    assert_eq!(first.rows.len(), 3);
    assert_eq!(palette.mesh_rows(0, 1).unwrap().generation, 1);
    assert!(palette.pack_for_upload(identity()).is_err());
    assert!(matches!(
        palette.mesh_rows(0, 0),
        Err(CharacterPaletteError::NotPublished)
    ));
}
#[test]
fn later_missing_part_retains_earlier_rows_without_publishing_them() {
    let mut palette = publication();
    palette.parts.push(PalettePart {
        matrices: None,
        packed: None,
        rows: None,
        meshes: vec![],
    });
    assert!(matches!(
        palette.pack_for_upload(identity()),
        Err(CharacterPaletteError::MissingMatrices(1))
    ));
    assert!(palette.parts[0].rows.is_some());
    assert_eq!(palette.generation, 0);
    assert!(matches!(
        palette.mesh_rows(0, 0),
        Err(CharacterPaletteError::NotPublished)
    ));
}
#[test]
fn a_published_pose_repacks_under_another_local_to_world() {
    let mut palette = publication();
    assert!(matches!(
        palette.repack_for_upload(identity()),
        Err(CharacterPaletteError::NotPublished)
    ));
    palette.pack_for_upload(identity()).unwrap();
    let mut moved = identity();
    moved[12] = 5.0;
    palette.repack_for_upload(moved).unwrap();
    let rows = palette.mesh_rows(0, 0).unwrap();
    assert_eq!(rows.generation, 2);
    assert_eq!(rows.rows[0][3], 5.0);
}
