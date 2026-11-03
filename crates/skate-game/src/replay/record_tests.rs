use super::*;
use crate::skate_audio::RetailAudioInputs;
use skate_core::player::state::PhysicalStateId;

const BONES: usize = 70;

/// The local skater's audio record mid-roll at frame `i` (headless.rs's rolling observation, moving).
pub(crate) fn audio(i: usize) -> PlayerAudioObservation {
    let t = i as f32 / 60.;
    let mut retail = RetailAudioInputs::default();
    retail.dt = 1. / 60.;
    retail.ground_speed = 4. + t.sin();
    retail.com_velocity = [4. + t.sin(), 0.1 * t.cos(), 0.];
    retail.position = [t * 4., 0.2, 1.];
    retail.wheel_count = 4;
    retail.wheel_contacts = [true; 4];
    retail.state = 100;
    retail.camera = Some(([1., 0., 0.], [t * 4. - 3., 1.5, 0.]));
    retail.time_scale = 1.;
    PlayerAudioObservation {
        tick: i as u64,
        state: 100,
        board_speed: 4. + t.sin(),
        rider_speed: 4.,
        grounded: true,
        grinding: false,
        grind_family: 0,
        grind_substate: 0,
        grind_audio_surface: 0,
        grind_impact_speed: 0.,
        wiping_out: false,
        landed: i % 90 == 0,
        landing_impact_speed: 0.,
        landing_clean: false,
        landing_sketchy: false,
        landing_type: 0,
        landing_spin: 0.,
        landing_sideways_speed: 0.,
        footstep_strength: 0.,
        footstep_bone: 0,
        foot_push_speed: 0.,
        feet_supported: [false; 2],
        foot_surface: 0,
        contact_count: 4,
        powersliding: false,
        trick_id: (i % 90 == 0).then_some(7),
        trick_identifier: Some("Kickflip".into()),
        animation_name: Some("ollie_land".into()),
        riding_switch: false,
        riding_fakie: false,
        nollie: false,
        wheel_surface: 3,
        events: if i % 90 == 0 {
            vec![
                PhysicsEvent::StateChanged { from: PhysicalStateId::KnownAir, to: PhysicalStateId::PhysicsGround },
                PhysicsEvent::Landing,
                PhysicsEvent::Contact { body: PhysicsBody::Board },
            ]
        } else {
            Vec::new()
        },
        frontend_sounds: Vec::new(),
        volumes: [1.; 3],
        map: "New_San_Vanelona".into(),
        skate2: false,
        district: None,
        retail,
        remote_records: Vec::new(),
        watched: None,
        riders: Vec::new(),
        traffic: None,
        dmos: Vec::new(),
    }
}

fn actor(id: u32, t: f32, bones: usize) -> Actor {
    let root = Transform::from_xyz(t * 4. + id as f32, 0.1 * t.sin(), id as f32)
        .with_rotation(Quat::from_rotation_y(t * 0.5 + id as f32));
    Actor {
        id,
        root,
        bones: (0..bones)
            .map(|b| {
                let bend = Quat::from_rotation_x(0.3 * (t * 3. + b as f32).sin());
                Mat4::from_rotation_translation(root.rotation * bend, root.translation + Vec3::Y * (b as f32 * 0.02))
            })
            .collect(),
        joints: Vec::new(),
        visible: true,
    }
}

/// `skaters` actors × BONES bones, a moving camera, two props and the local audio record.
pub(crate) fn frame(i: usize, skaters: u32) -> RecFrame {
    let t = i as f32 / 60.;
    RecFrame {
        time: i as f64 / 60.,
        actors: (0..skaters).map(|id| actor(id, t, BONES)).collect(),
        camera: Transform::from_xyz(t * 4. - 3., 1.5, 0.).looking_at(Vec3::new(t * 4., 1., 0.), Vec3::Y),
        fov: 1.1,
        props: vec![
            Prop { id: 3, transform: Transform::from_xyz(5., 0., 5.), visible: true },
            Prop { id: 9, transform: Transform::from_xyz(5. + t, 0., 6.), visible: i % 2 == 0 },
        ],
        audio: Some(Box::new(audio(i))),
        cut: i == 30,
        ..RecFrame::default()
    }
}

fn sample_frames(n: usize) -> Vec<RecFrame> {
    (0..n).map(|i| frame(i, 2)).collect()
}

fn numbered_frame(i: usize) -> RecFrame {
    let skater = Actor { id: 0, root: Transform::IDENTITY, bones: Vec::new(), joints: Vec::new(), visible: true };
    RecFrame { time: i as f64 / 60., actors: vec![skater], ..RecFrame::default() }
}

fn frame_number(frame: &RecFrame) -> usize {
    (frame.time * 60.).round() as usize
}

/// Bones whose every component is noise: nothing for zstd to find.
fn noisy_frame(i: usize) -> RecFrame {
    let mut seed = (i as u64 + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 40) as f32 / (1u64 << 24) as f32
    };
    let mut actor = |id| Actor {
        id,
        root: Transform::IDENTITY,
        bones: (0..BONES)
            .map(|_| {
                let rotation = Quat::from_xyzw(next() - 0.5, next() - 0.5, next() - 0.5, next() - 0.5).normalize();
                Mat4::from_rotation_translation(rotation, Vec3::new(next(), next(), next()) * 1000.)
            })
            .collect(),
        joints: Vec::new(),
        visible: true,
    };
    RecFrame { time: i as f64 / 60., actors: vec![actor(0), actor(1)], ..RecFrame::default() }
}

/// Within the i16 smallest-three step (f32 dot products cannot resolve angles this small).
fn same_rotation(a: Quat, b: Quat) -> bool {
    (a - b).length().min((a + b).length()) < 1e-4
}

#[test]
fn block_round_trips_within_quantisation() {
    let frames = sample_frames(BLOCK_FRAMES); // 2 actors x 70 bones, moving camera, props, audio
    let back = decode_block(&encode_block(&frames)).unwrap();
    assert_eq!(back.len(), frames.len());
    for (a, b) in frames.iter().zip(&back) {
        assert!(a.camera.translation.distance(b.camera.translation) < 0.001);
        assert!(same_rotation(a.camera.rotation, b.camera.rotation));
        assert_eq!(a.audio, b.audio);
        assert_eq!(a.cut, b.cut);
        assert_eq!(a.props.iter().map(|p| (p.id, p.visible)).collect::<Vec<_>>(),
                   b.props.iter().map(|p| (p.id, p.visible)).collect::<Vec<_>>());
        assert!((a.time - b.time).abs() < 1e-5);
        for (x, y) in a.actors.iter().zip(&b.actors) {
            assert_eq!((x.id, x.bones.len()), (y.id, y.bones.len()));
            for (p, q) in x.bones.iter().zip(&y.bones) {
                let (_, pr, pt) = p.to_scale_rotation_translation();
                let (_, qr, qt) = q.to_scale_rotation_translation();
                assert!(pt.distance(qt) < 0.001 && same_rotation(pr, qr));
            }
        }
    }
}

#[test]
fn ring_drops_oldest_at_the_frame_limit() {
    let mut ring = Ring::default();
    for i in 0..RING_FRAMES + 10 {
        ring.push(numbered_frame(i));
    }
    assert_eq!(ring.len(), RING_FRAMES);
    assert_eq!(frame_number(&ring.get(0).unwrap()), 10);
    assert_eq!(frame_number(&ring.get(RING_FRAMES - 1).unwrap()), RING_FRAMES + 9);
    assert!((ring.time(0).unwrap() - 10. / 60.).abs() < 1e-5);
}

#[test]
fn ring_drops_oldest_at_the_byte_budget() {
    let mut ring = Ring::default();
    for i in 0..RING_FRAMES {
        ring.push(noisy_frame(i)); // incompressible bones
    }
    ring.flush();
    assert!(ring.compressed_bytes() <= RING_BYTES);
    assert!(ring.len() < RING_FRAMES);
    assert!((ring.get(ring.len() - 1).unwrap().time - (RING_FRAMES - 1) as f64 / 60.).abs() < 1e-3);
}

#[test]
fn corrupt_block_is_an_error_not_a_panic() {
    let mut bytes = encode_block(&sample_frames(10));
    bytes.truncate(bytes.len() / 2);
    assert!(decode_block(&bytes).is_err());
    // Valid zstd around a cut-short payload is an error too.
    let raw = zstd::stream::decode_all(&encode_block(&sample_frames(10))[..]).unwrap();
    assert!(decode_block(&zstd::bulk::compress(&raw[..raw.len() / 3], 3).unwrap()).is_err());
    assert!(decode_block(b"not a block").is_err());
}

#[test]
fn exported_blocks_rebuild_the_same_ring() {
    let mut ring = Ring::default();
    for i in 0..RING_FRAMES + 75 {
        ring.push(numbered_frame(i));
    }
    let back = Ring::from_blocks(ring.export_blocks()).unwrap();
    assert_eq!(back.len(), ring.len());
    for i in [0, 1, 59, 60, ring.len() - 1] {
        assert_eq!(back.get(i), ring.get(i));
    }
}

/// The lead's size check: bytes per frame and a full 90 s ring with 4 skaters (70 bones each), a moving camera,
/// props and the local audio record. `-- --nocapture` prints the numbers.
#[test]
fn full_ring_with_four_skaters_stays_small() {
    let mut ring = Ring::default();
    for i in 0..RING_FRAMES {
        ring.push(frame(i, 4));
    }
    ring.flush();
    let per_frame = ring.compressed_bytes() as f64 / (ring.len() - ring.len() % BLOCK_FRAMES) as f64;
    println!(
        "replay ring (synthetic sinusoid motion, review M13): 4 skaters x {BONES} bones + camera + 2 props + audio record: {per_frame:.0} B/frame, {} frames = {:.2} MiB for 90 s",
        ring.len(),
        ring.compressed_bytes() as f64 / 1048576.
    );
    assert_eq!(ring.len(), RING_FRAMES);
    assert!(ring.compressed_bytes() < RING_BYTES / 2);
}

/// Review I4: blocks that decode but break the ring's rules are errors, never panics.
#[test]
fn bad_blocks_are_refused() {
    let mut late = sample_frames(3);
    late[2].time = late[0].time;
    assert!(decode_block(&encode_block(&late)).is_err(), "times must rise");
    let mut nan = sample_frames(3);
    nan[1].time = f64::NAN;
    assert!(decode_block(&encode_block(&nan)).is_err());
    let mut fov = sample_frames(3);
    fov[1].fov = f32::INFINITY;
    assert!(decode_block(&encode_block(&fov)).is_err());
    let mut alone = sample_frames(3);
    alone[1].actors.clear();
    assert!(Ring::from_blocks(vec![encode_block(&alone)]).is_err(), "every frame needs the skater");
    assert!(decode_block(&encode_block(&sample_frames(BLOCK_FRAMES + 1))).is_err(), "a block is at most 60 frames");
    // A zstd bomb: 64 MiB of zeros compresses to almost nothing and must stop at the cap.
    let bomb = zstd::bulk::compress(&vec![0u8; 64 << 20], 3).unwrap();
    assert!(decode_block(&bomb).is_err());
    let many: Vec<Vec<u8>> = (0..RING_FRAMES / BLOCK_FRAMES + 1)
        .map(|b| encode_block(&(b * BLOCK_FRAMES..(b + 1) * BLOCK_FRAMES).map(numbered_frame).collect::<Vec<_>>()))
        .collect();
    assert!(Ring::from_blocks(many).is_err(), "more than RING_FRAMES");
    let mut bones = sample_frames(2);
    bones[1].actors[0].bones.pop();
    assert_eq!(Ring::from_blocks(vec![encode_block(&bones)]).unwrap().rigs(), &[BONES, BONES - 1]);
}

/// Review I9, measured: the per-frame recorder cost with 4 skaters (the SKATE_PERF_REPORT "replay_record" scope
/// times the same call in game). `-- --nocapture` prints p50/p99/max for the incremental recorder and for the old
/// whole-block encode it replaced. Debug builds are several times slower than release; compare the two lines.
#[test]
fn per_frame_record_cost() {
    fn stats(mut ms: Vec<f64>) -> (f64, f64, f64) {
        ms.sort_by(f64::total_cmp);
        (ms[ms.len() / 2], ms[ms.len() * 99 / 100], ms[ms.len() - 1])
    }
    let frames: Vec<RecFrame> = (0..1200).map(|i| frame(i, 4)).collect();
    let mut ring = Ring::default();
    let mut new = Vec::new();
    for f in frames.iter().cloned() {
        let t = std::time::Instant::now();
        ring.push(f);
        new.push(t.elapsed().as_secs_f64() * 1000.);
    }
    ring.flush();
    assert_eq!(ring.len(), 1200);
    // The old path: frames buffered, then the whole block quantised and zstd'd on the 60th push.
    let mut old = Vec::new();
    let mut open = Vec::new();
    for f in frames.iter().cloned() {
        let t = std::time::Instant::now();
        open.push(f);
        if open.len() == BLOCK_FRAMES {
            std::hint::black_box(encode_block(&open));
            open.clear();
        }
        old.push(t.elapsed().as_secs_f64() * 1000.);
    }
    let (n, o) = (stats(new), stats(old));
    println!("replay_record per frame, 4 skaters x {BONES} bones (ms): incremental p50 {:.3} p99 {:.3} max {:.3}; \
              old whole-block p50 {:.3} p99 {:.3} max {:.3}", n.0, n.1, n.2, o.0, o.1, o.2);
    assert!(n.1 < o.2, "the incremental recorder's p99 must sit under the old 60th-frame spike");
}
