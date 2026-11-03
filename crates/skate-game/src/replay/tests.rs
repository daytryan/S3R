use super::*;

fn pose(x: f32) -> Snapshot {
    Snapshot {
        root: Transform::from_xyz(x, 0., 0.),
        bones: vec![Mat4::from_translation(Vec3::X * x)],
        camera: Transform::from_xyz(x, 2., 3.),
        fov: 1.0,
    }
}

fn newest_root_x(replay: &Replay) -> f32 {
    let ring = replay.frames();
    ring.get(ring.len() - 1).unwrap().actors[0].root.translation.x
}

#[test]
fn rolling_ring_holds_retail_5400_frames_with_variable_cadence() {
    let mut replay = Replay::default();
    for i in 0..6000 {
        replay.record(pose(i as f32), if i % 2 == 0 { 1. / 60. } else { 1. / 30. }, false);
    }
    assert_eq!(replay.frames().len(), record::RING_FRAMES);
    let (start, end) = replay.bounds();
    assert_eq!(newest_root_x(&replay), 5999.);
    replay.enter();
    replay.seek(-100.);
    assert_eq!(replay.cursor, start);
    replay.seek(1000.);
    assert_eq!(replay.cursor, end);
}

#[test]
fn scrubbing_interpolates_shared_pose_and_camera_without_recording() {
    let mut replay = Replay::default();
    replay.enter();
    assert!(!replay.active);
    replay.record(pose(0.), 0.1, false);
    replay.record(pose(1.), 0.1, false);
    replay.enter();
    replay.record(pose(99.), 0.1, false);
    assert_eq!(replay.frames().len(), 2);
    replay.seek(0.05);
    let (a, b, alpha) = replay.sample().unwrap();
    assert!((alpha - 0.5).abs() < 1e-6);
    assert!((blend(a.root, b.root, alpha).translation.x - 0.5).abs() < 1e-6);
    assert!((blend(a.camera, b.camera, alpha).translation.x - 0.5).abs() < 1e-6);
    // The editor's camera (hud.rs) stays put while the playhead moves; exit drops it.
    let free = Transform::from_xyz(9., 9., 9.);
    replay.free_camera = Some(free);
    replay.seek(0.1);
    assert_eq!(replay.free_camera.unwrap(), free);
    replay.exit();
    assert!(replay.free_camera.is_none());
    assert!(replay.sample().is_none());
    replay.record(pose(2.), 0.1, false);
    assert_eq!(newest_root_x(&replay), 2.);
}

#[test]
fn frame_steps_and_cuts_do_not_blend_through_teleports() {
    let mut replay = Replay::default();
    replay.record(pose(0.), 0.1, false);
    replay.record(pose(100.), 0.1, true);
    replay.record(pose(101.), 0.1, false);
    replay.enter();
    replay.seek(0.05);
    let (a, b, _) = replay.sample().unwrap();
    assert_eq!(a.root, b.root);
    assert_eq!(a.root.translation.x, 0.);
    replay.step(1);
    assert_eq!(replay.sample().unwrap().0.root.translation.x, 100.);
    replay.step(-1);
    assert_eq!(replay.cursor, 0.);
}

/// Review Focus 1: the Skate 2 photo challenge's highlight reel drives the replay through script / seek / now /
/// exit / scripted, by recording time, across a block boundary, with no Back gate in its way.
#[test]
fn scripted_replay_api_unchanged() {
    let mut replay = Replay::default();
    assert!(!replay.script(0.), "nothing recorded");
    assert!(!replay.active && !replay.scripted);
    // Fewer frames than the Back gate needs: a script still opens it.
    for i in 0..100 {
        replay.record(pose(i as f32), 1. / 60., false);
    }
    assert!(!replay.can_open_from_back());
    let now = replay.now();
    assert!((now - 99. / 60.).abs() < 1e-9);
    assert!(replay.script(now - 1. + 0.25 / 60.));
    assert!(replay.active && replay.scripted);
    let (a, _, _) = replay.sample().unwrap();
    assert!((a.root.translation.x - 39.).abs() < 1e-3, "{}", a.root.translation.x);
    replay.seek(30.25 / 60.);
    assert!((replay.sample().unwrap().0.root.translation.x - 30.).abs() < 1e-3);
    replay.seek(70.5 / 60.);
    let (a, b, alpha) = replay.sample().unwrap();
    assert_eq!((a.root.translation.x, b.root.translation.x), (70., 71.));
    assert!((alpha - 0.5).abs() < 1e-3);
    // Nothing records while scripted, and now() holds.
    replay.record(pose(500.), 1. / 60., false);
    assert_eq!(replay.now(), now);
    replay.exit();
    assert!(!replay.active && !replay.scripted && replay.sample().is_none());
    replay.record(pose(100.), 1. / 60., false);
    assert!(replay.now() > now);
}

/// Retail's Back gate (0x825EBC0C / 0x8250b8f8): 150 frames recorded, online too (free skate; hud.rs keeps online
/// matches out).
#[test]
fn back_entry_needs_150_frames() {
    let mut replay = Replay::default();
    for i in 0..MIN_BACK_FRAMES - 1 {
        replay.record(pose(i as f32), 1. / 60., false);
    }
    assert!(!replay.can_open_from_back());
    replay.record(pose(0.), 1. / 60., false);
    assert!(replay.can_open_from_back());
    replay.script(0.);
    assert!(!replay.can_open_from_back(), "never over a scripted replay");
}

#[test]
fn recording_pauses_while_editor_open() {
    let mut replay = Replay::default();
    for i in 0..10 {
        replay.record(pose(i as f32), 1. / 60., false);
    }
    replay.enter();
    for i in 0..10 {
        replay.record(pose(100. + i as f32), 1. / 60., false);
    }
    assert_eq!(replay.frames().len(), 10);
    replay.exit();
    replay.record(pose(7.), 1. / 60., false);
    assert_eq!(replay.frames().len(), 11);
}

#[test]
fn race_heat_resets_recording() {
    let mut replay = Replay::default();
    for i in 0..200 {
        replay.record(pose(i as f32), 1. / 60., false);
    }
    let before = replay.now();
    replay.reset_recording();
    assert!(replay.frames().is_empty());
    replay.enter();
    assert!(!replay.active, "an empty recording does not open");
    replay.record(pose(0.), 1. / 60., false);
    assert!(replay.now() > before, "the clock keeps counting for times a script already took");
    // Never under an open replay.
    replay.enter();
    replay.reset_recording();
    assert_eq!(replay.frames().len(), 1);
}

/// Review M6: the first frame after the replay closes is a cut (never blended across the gap), and a loaded save is
/// dropped when it closes instead of live play being appended to it.
#[test]
fn closing_marks_a_cut_and_drops_a_loaded_save() {
    let mut replay = Replay::default();
    for i in 0..10 {
        replay.record(pose(i as f32), 1. / 60., false);
    }
    replay.enter();
    replay.exit();
    replay.record(pose(50.), 1. / 60., false);
    replay.record(pose(51.), 1. / 60., false);
    let ring = replay.frames();
    let n = ring.len();
    assert!(!ring.get(n - 3).unwrap().cut);
    assert!(ring.get(n - 2).unwrap().cut, "the gap is a cut");
    assert!(!ring.get(n - 1).unwrap().cut, "once");
    let mut saved = Replay::default();
    for i in 0..20 {
        saved.record(pose(i as f32), 1. / 60., false);
    }
    replay.install(std::mem::take(&mut saved.ring));
    assert_eq!(replay.frames().len(), 20);
    replay.enter();
    replay.exit();
    assert!(replay.frames().is_empty(), "the loaded save goes with the editor");
    replay.record(pose(0.), 1. / 60., false);
    assert_eq!(replay.frames().len(), 1);
    // A live recording is kept across an open / close.
    replay.enter();
    replay.exit();
    assert_eq!(replay.frames().len(), 1);
}

/// Review I2: the export's audio records walk the clips at 60 Hz with each clip's speed, like live playback.
#[test]
fn export_audio_follows_the_timeline() {
    let mut replay = Replay::default();
    for i in 0..121 {
        replay.pending.audio = Some(Box::new(record::tests::audio(i)));
        replay.record(pose(i as f32), 1. / 60., false);
    }
    let mut editor = editor::Editor::new(false);
    editor.set_whole_clip_speed(50);
    let seconds = replay.position().1 as f32;
    let mut sent = None;
    let mut ticks = Vec::new();
    for frame in 0..8 {
        for record in export::audio_records(&replay, &editor, frame, 30, seconds, &mut sent) {
            let record = record.expect("a recorded audio record");
            assert_eq!(record.retail.time_scale, 0.5);
            assert!(!record.retail.paused);
            ticks.push(record.tick);
        }
    }
    // Half speed: 16 ticks walk about 8 recorded frames, in order, none skipped.
    assert_eq!(ticks.len(), 16);
    assert_eq!(ticks[0], 0);
    assert!(ticks.windows(2).all(|w| w[1] == w[0] || w[1] == w[0] + 1), "{ticks:?}");
    assert!((7..=8).contains(ticks.last().unwrap()), "{ticks:?}");
}

#[test]
fn remote_skaters_are_recorded() {
    use crate::multiplayer::appearance::RemoteCharacter;
    let mut app = App::new();
    app.init_resource::<Replay>()
        .add_message::<PlayerAudioObservation>()
        .add_systems(Update, gather);
    // perf 10-19: two outfit parts, each with its own copy of the hip and its joint, and a static mesh node.
    let joint = |world: &mut World| world.spawn(Transform::from_xyz(0., 1., 0.).with_rotation(Quat::from_rotation_x(0.4))).id();
    let (joint_a, joint_b) = (joint(app.world_mut()), joint(app.world_mut()));
    let hip_a = app.world_mut().spawn(Transform::from_xyz(0., 0.5, 0.)).add_child(joint_a).id();
    let hip_b = app.world_mut().spawn(Transform::from_xyz(0., 0.5, 0.)).add_child(joint_b).id();
    let mesh = app.world_mut().spawn(Transform::from_xyz(0., 0., 3.)).id();
    let joints = ReplayJoints::new([(hip_a, (0, None, false)), (joint_a, (1, Some(0), false)), (hip_b, (0, None, false)), (joint_b, (1, Some(0), false))]);
    let remote = app
        .world_mut()
        .spawn((RemoteCharacter(0x1_0000_0007), Transform::from_xyz(5., 0., 2.), joints))
        .add_children(&[hip_a, hip_b, mesh])
        .id();
    app.world_mut().write_message(record::tests::audio(3));
    app.update();
    app.world_mut().resource_mut::<Replay>().record(pose(0.), 1. / 60., false);
    let frame = app.world().resource::<Replay>().frames().get(0).unwrap();
    assert_eq!(frame.actors.len(), 2);
    let actor = &frame.actors[1];
    assert_eq!(actor.id, 7);
    assert_eq!(actor.root.translation, Vec3::new(5., 0., 2.));
    assert!(actor.bones.is_empty(), "no node-by-node copy");
    assert_eq!(actor.joints.len(), 2, "one per distinct joint, not per part copy");
    assert_eq!(actor.joints[1], Transform::from_xyz(0., 1., 0.).with_rotation(Quat::from_rotation_x(0.4)));
    assert_eq!(frame.audio.as_deref().map(|a| a.tick), Some(3));
    // The joints survive a block's encode and decode.
    let back = record::decode_block(&record::encode_block(std::slice::from_ref(&frame))).unwrap();
    let joint = back[0].actors[1].joints[1];
    assert!((joint.translation.y - 1.).abs() < 1e-3 && joint.rotation.angle_between(Quat::from_rotation_x(0.4)) < 1e-3);

    // Playback puts the recorded pose back on every part's copy; the static node is left alone.
    app.world_mut().entity_mut(remote).insert(Transform::from_xyz(50., 0., 0.));
    for e in [joint_a, joint_b] {
        app.world_mut().entity_mut(e).insert(Transform::from_xyz(0., 9., 0.));
    }
    app.world_mut().entity_mut(hip_b).insert(Transform::from_xyz(0., 7., 0.));
    app.add_systems(PostUpdate, play_back);
    app.world_mut().resource_mut::<Replay>().enter();
    app.update();
    assert_eq!(app.world().get::<Transform>(remote).unwrap().translation, Vec3::new(5., 0., 2.));
    for e in [joint_a, joint_b] {
        assert!((app.world().get::<Transform>(e).unwrap().translation.y - 1.).abs() < 1e-3);
        assert!(app.world().get::<Transform>(e).unwrap().rotation.angle_between(Quat::from_rotation_x(0.4)) < 1e-3);
    }
    assert!((app.world().get::<Transform>(hip_b).unwrap().translation.y - 0.5).abs() < 1e-3);
    assert_eq!(app.world().get::<Transform>(mesh).unwrap().translation.z, 3.);
}

#[test]
fn props_play_back_and_return_on_exit() {
    use crate::physics::props::PropPiece;
    let mut app = App::new();
    app.init_resource::<Replay>()
        .add_message::<PlayerAudioObservation>()
        .add_systems(Update, gather)
        .add_systems(PostUpdate, play_back);
    let piece = app.world_mut().spawn((PropPiece(4), Transform::from_xyz(1., 0., 0.), Visibility::Inherited)).id();
    app.update();
    app.world_mut().resource_mut::<Replay>().record(pose(0.), 1. / 60., false);
    app.world_mut().entity_mut(piece).insert(Transform::from_xyz(3., 0., 0.));
    app.world_mut().resource_mut::<Replay>().enter();
    app.update();
    assert_eq!(app.world().get::<Transform>(piece).unwrap().translation.x, 1.);
    app.world_mut().resource_mut::<Replay>().exit();
    app.update();
    assert_eq!(app.world().get::<Transform>(piece).unwrap().translation.x, 3.);
}

fn audio_app(frames: usize) -> App {
    let mut app = App::new();
    app.init_resource::<Replay>()
        .init_resource::<Time<Real>>()
        .add_message::<PlayerAudioObservation>()
        .add_systems(Update, feed_audio);
    let mut replay = app.world_mut().resource_mut::<Replay>();
    for i in 0..frames {
        replay.pending.audio = Some(Box::new(record::tests::audio(i)));
        replay.record(pose(i as f32), 1. / 60., false);
    }
    app
}
fn tick(app: &mut App) -> Vec<PlayerAudioObservation> {
    app.world_mut().resource_mut::<Messages<PlayerAudioObservation>>().clear();
    app.world_mut().resource_mut::<Time<Real>>().advance_by(std::time::Duration::from_secs_f64(1. / 60. + 1e-6));
    app.update();
    let messages = app.world().resource::<Messages<PlayerAudioObservation>>();
    messages.iter_current_update_messages().cloned().collect()
}

/// Lead ruling on Stage 0 §3: the recorded PhysOut-equivalent goes back into player_audio's input with the speed
/// as the time scale; paused is silent; a repeated frame (slow motion) drops its one-shots.
#[test]
fn playback_feeds_the_recorded_audio_record() {
    let mut app = audio_app(200);
    assert!(tick(&mut app).is_empty(), "nothing while live");
    app.world_mut().resource_mut::<Replay>().enter();
    app.world_mut().resource_mut::<Replay>().seek(89.25 / 60.);
    let paused = tick(&mut app);
    assert_eq!(paused.len(), 1);
    assert!(paused[0].retail.paused, "a stopped playhead is silent");
    assert!(paused[0].events.is_empty());
    {
        let mut replay = app.world_mut().resource_mut::<Replay>();
        replay.playing = true;
        replay.speed = 0.5;
        replay.seek(90.25 / 60.);
    }
    let landing = tick(&mut app);
    assert_eq!(landing[0].tick, 90);
    assert!(!landing[0].retail.paused);
    assert_eq!(landing[0].retail.time_scale, 0.5);
    assert!(landing[0].landed && !landing[0].events.is_empty(), "frame 90 lands");
    // Half speed: the same frame again carries no second landing.
    let again = tick(&mut app);
    assert_eq!(again[0].tick, 90);
    assert!(!again[0].landed && again[0].events.is_empty());
    // Reverse scrub: silent.
    app.world_mut().resource_mut::<Replay>().seek(60.25 / 60.);
    assert!(tick(&mut app)[0].retail.paused);
}

#[test]
fn scripted_replay_sends_no_audio() {
    let mut app = audio_app(100);
    assert!(app.world_mut().resource_mut::<Replay>().script(0.5));
    assert!(tick(&mut app).is_empty());
}

#[test]
fn replay_gates_all_fixed_gameplay_sets() {
    #[derive(Resource, Default)]
    struct Counts([usize; 3]);
    let mut app = App::new();
    app.init_resource::<Replay>()
        .init_resource::<Counts>()
        .configure_sets(
            FixedUpdate,
            (
                SimulationSet::Input,
                SimulationSet::Controls,
                SimulationSet::Physics,
            )
                .run_if(live),
        )
        .add_systems(
            FixedUpdate,
            (
                (|mut c: ResMut<Counts>| c.0[0] += 1).in_set(SimulationSet::Input),
                (|mut c: ResMut<Counts>| c.0[1] += 1).in_set(SimulationSet::Controls),
                (|mut c: ResMut<Counts>| c.0[2] += 1).in_set(SimulationSet::Physics),
            ),
        );
    app.world_mut().run_schedule(FixedUpdate);
    app.world_mut().resource_mut::<Replay>().active = true;
    for _ in 0..120 {
        app.world_mut().run_schedule(FixedUpdate);
    }
    assert_eq!(app.world().resource::<Counts>().0, [1; 3]);
    // Online (retail state 20) the simulation keeps running under the replay.
    app.world_mut().resource_mut::<Replay>().online = true;
    app.world_mut().run_schedule(FixedUpdate);
    assert_eq!(app.world().resource::<Counts>().0, [2; 3]);
    app.world_mut().resource_mut::<Replay>().exit();
    app.world_mut().run_schedule(FixedUpdate);
    assert_eq!(app.world().resource::<Counts>().0, [3; 3]);
}
