use skate_data::retail_collision::{triangle_count, visit_clusters, without_triggers};
use skate_data::skate_map::SkateMap;

const FIXTURE: &[u8] = include_bytes!("fixtures/retail-collision.rwcmset");

/// A mesh of one cluster (three vertices at the origin; group and surface ids one byte wide) with a triangle per
/// unit: its flags and the bytes after its vertex indices.
fn mesh(units: &[(u8, &[u8])]) -> Vec<u8> {
    let stream: Vec<u8> = units.iter().flat_map(|&(flags, rest)| [&[flags, 0, 1, 2][..], rest].concat()).collect();
    let mut cluster = vec![0; 64];
    cluster[0..2].copy_from_slice(&(units.len() as u16).to_be_bytes());
    cluster[2..4].copy_from_slice(&(stream.len() as u16).to_be_bytes());
    cluster[4..6].copy_from_slice(&3_u16.to_be_bytes()); // the units start at (3 + 1) * 16
    cluster[10] = 3;
    cluster.extend(stream);
    let size = cluster.len() as u16;
    cluster[8..10].copy_from_slice(&size.to_be_bytes());
    let mut mesh = vec![0; 96];
    mesh[40..44].copy_from_slice(&(units.len() as u32).to_be_bytes());
    mesh[52..56].copy_from_slice(&96_u32.to_be_bytes()); // the cluster table
    mesh[56..60].copy_from_slice(&1_f32.to_be_bytes());
    mesh[62] = 1;
    mesh[63] = 1;
    mesh[64..68].copy_from_slice(&1_u32.to_be_bytes());
    mesh.extend(100_u32.to_be_bytes());
    mesh.extend(cluster);
    let size = mesh.len() as u32;
    mesh[80..84].copy_from_slice(&size.to_be_bytes());
    mesh
}

fn archive(meshes: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut data = b"RWCMSET1".to_vec();
    data.extend((meshes.len() as u32).to_le_bytes());
    for (name, mesh) in meshes {
        data.extend((name.len() as u32).to_le_bytes());
        data.extend(name.as_bytes());
        data.extend((mesh.len() as u32).to_le_bytes());
        data.extend(mesh);
    }
    data
}

#[test]
fn drops_xbox_meshes_without_a_surfaced_triangle() {
    // User 10-27 (Bump Riser's giant invisible wall): retail's trigger volumes, kept by our Xbox (Skate 3) converter,
    // are the stream meshes with no surface id on any triangle. The PS3 converter (Skate 1 / 2) drops its own, so a
    // bare PS3 mesh is world.
    let bare = || mesh(&[(0x61, &[0x80; 4]), (0x01, &[])]); // its edge and group bytes would read as surface flags
    let data = archive(&[
        ("a.xsf#0", mesh(&[(0x01, &[]), (0x81, &[0])])), // one triangle with a surface (id 0) makes it world
        ("a.xsf#1", bare()),
        ("b.psf#0", bare()),
        ("a.xsf#2", mesh(&[(0xe1, &[0, 0, 0, 0, 3])])),
    ]);
    let (kept, dropped) = without_triggers(&data).unwrap().expect("a trigger to drop");
    let mut names = Vec::new();
    let triangles = visit_clusters(&kept, |name, _| {
        names.push(name.to_owned());
        Ok(())
    })
    .unwrap();
    assert_eq!((dropped, triangles), (1, 5));
    assert_eq!(names, ["a.xsf#0", "b.psf#0", "a.xsf#2"]);
    assert!(without_triggers(&kept).unwrap().is_none(), "nothing to drop: no copy");
    assert!(without_triggers(FIXTURE).unwrap().is_none());
    assert!(without_triggers(&archive(&[("a.xsf#0", bare())])).unwrap().is_none(), "never an archive of no mesh");
    for end in 0..data.len() {
        assert!(without_triggers(&data[..end]).is_err(), "prefix {end}");
    }
    assert!(without_triggers(&[data.as_slice(), &[0]].concat()).is_err(), "trailing bytes");
}

/// Each installed map drops exactly the meshes retail owns as trigger volumes (EB000A collision objects of a nonzero
/// kind in its Xbox Sim streams, counted per map 10-27); the Skate 1 / 2 maps (PS3 streams) none.
#[test]
#[ignore = "requires SKATE3_ASSET_ROOT (the installed maps beside it)"]
fn installed_maps_drop_exactly_retails_trigger_volumes() {
    const TRIGGERS: [(&str, usize); 7] = [
        ("Art_Gallery", 79),
        ("Zen_Plaza", 40),
        ("SkateSchool", 7),
        ("DownTown", 2),
        ("MaloofMoneyCup", 2),
        ("MegaPark", 1),
        ("Downtown_Night_Park", 1),
    ];
    let root = std::path::PathBuf::from(std::env::var_os("SKATE3_ASSET_ROOT").unwrap());
    let mut seen = Vec::new();
    for entry in std::fs::read_dir(root.parent().unwrap().join("maps")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "skate") {
            continue;
        }
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let map = SkateMap::load(&path).unwrap();
        let archive = map.extensions.iter().find(|e| e.tag == *b"RWCM").unwrap();
        let dropped = without_triggers(&archive.payload).unwrap().map_or(0, |(kept, n)| {
            visit_clusters(&kept, |_, _| Ok(())).unwrap();
            n
        });
        eprintln!("{name}: {dropped} trigger meshes");
        assert_eq!(dropped, TRIGGERS.iter().find(|t| t.0 == name).map_or(0, |t| t.1), "{name}");
        seen.push(name);
    }
    assert!(TRIGGERS.iter().all(|t| seen.iter().any(|s| s == t.0)), "{seen:?}");
}

#[test]
fn compressed_offsets_zero_extend_across_the_sign_boundary() {
    let mut bytes = FIXTURE.to_vec();
    // Locate the second fixture mesh, which uses 16-bit offsets.
    let mut at = 12;
    let mut mesh = 0;
    for _ in 0..2 {
        let name_len = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        at += 4 + name_len;
        let size = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        mesh = at + 4;
        at = mesh + size;
    }
    let cluster = mesh + 160;
    let base = [-40000_i32, -32768, i32::MAX - 2];
    for axis in 0..3 {
        bytes[cluster + 16 + axis * 4..cluster + 20 + axis * 4]
            .copy_from_slice(&base[axis].to_be_bytes());
    }
    for (i, raw) in [0x7fff_u16, 0x8000, 0xffff].into_iter().enumerate() {
        for axis in 0..3 {
            bytes[cluster + 28 + i * 6 + axis * 2..cluster + 30 + i * 6 + axis * 2]
                .copy_from_slice(&raw.to_be_bytes());
        }
    }
    visit_clusters(&bytes, |name, triangles| {
        if name == "compression-1" {
            assert_eq!(
                triangles[0].points,
                [
                    [-1808.25, -0.25, 536870912.],
                    [-1808., 0., 536870912.],
                    [6383.75, 8191.75, 536870912.],
                ]
            );
        }
        Ok(())
    })
    .unwrap();
}

#[test]
fn preserves_all_vertex_encodings_and_little_endian_unit_ids() {
    let mut clusters = 0;
    let total = visit_clusters(FIXTURE, |name, triangles| {
        assert_eq!(name, format!("compression-{clusters}"));
        assert_eq!(triangles.len(), 1);
        let t = triangles[0];
        let x = clusters as f32 * 10.;
        assert_eq!(t.points, [[x, 0., 0.], [x, 0., 1.], [x + 1., 0., 0.]]);
        assert_eq!(t.edges, Some([0x20, 0x42, 0x9a]));
        assert_eq!(t.surface, 0x4321);
        assert_eq!(t.group, 0x1234);
        assert!(t.one_sided);
        clusters += 1;
        Ok(())
    })
    .unwrap();
    assert_eq!(total, 3);
    assert_eq!(clusters, 3);
}

#[test]
fn rejects_every_truncated_prefix_and_trailing_bytes() {
    for end in 0..FIXTURE.len() {
        assert!(
            visit_clusters(&FIXTURE[..end], |_, _| Ok(())).is_err(),
            "prefix {end}"
        );
    }
    let mut bytes = FIXTURE.to_vec();
    bytes.push(0);
    assert!(visit_clusters(&bytes, |_, _| Ok(())).is_err());
}

#[test]
#[ignore = "requires SKATE_RWCM_TEST_PATH pointing to a private archive"]
fn private_archive_decodes_all_clusters() {
    let path = std::env::var("SKATE_RWCM_TEST_PATH").unwrap();
    let bytes = std::fs::read(path).unwrap();
    let count = visit_clusters(&bytes, |_, _| Ok(())).unwrap();
    assert!(count > 0);
    eprintln!("RWCM triangles decoded: {count}");
}

#[test]
fn triangle_count_reads_the_mesh_headers() {
    // A capacity hint for the collision build: the headers' total, None on a malformed archive (never a panic).
    assert_eq!(triangle_count(FIXTURE), Some(3));
    for end in 0..FIXTURE.len() {
        assert!(triangle_count(&FIXTURE[..end]).is_none_or(|n| n <= 3), "prefix {end}");
    }
}
