use super::*;

/// A key's outs without its FE sounds (the sounds have their own tests).
fn k(e: &mut Editor, pad: Pad) -> Vec<Out> {
    e.key(pad).into_iter().filter(|o| !matches!(o, Out::Sound(_))).collect()
}
/// A key's FE sounds.
fn heard(e: &mut Editor, pad: Pad) -> Vec<u64> {
    e.key(pad).into_iter().filter_map(|o| if let Out::Sound(id) = o { Some(id) } else { None }).collect()
}

#[test]
fn scrub_full_trigger_reaches_retail_clamp() {
    let mut e = Editor::new(false);
    for _ in 0..40 {
        e.scrub(1.0, 0.0);
    }
    assert!((e.scrub_velocity - 0.005).abs() < 1e-6);
}

#[test]
fn scrub_release_settles_to_zero() {
    let mut e = Editor::new(false);
    e.scrub(1.0, 0.0);
    for _ in 0..60 {
        e.scrub(0.0, 0.0);
    }
    assert_eq!(e.scrub_velocity, 0.0);
}

/// Stage 0 §1: a full-trigger sweep of the whole timeline takes 200 updates (3.33 s at 60 Hz) once eased in.
#[test]
fn full_sweep_takes_about_200_updates() {
    let mut e = Editor::new(false);
    e.playhead = 0.;
    let mut updates = 0;
    while e.playhead < 1. {
        e.scrub(1.0, 0.0);
        updates += 1;
    }
    assert!((200..215).contains(&updates), "{updates}");
}

#[test]
fn five_seconds_at_half_speed_encode_as_ten() {
    let mut e = Editor::new(false);
    e.set_whole_clip_speed(50);
    assert_eq!(e.encoded_seconds(5.0), 10);
}

#[test]
fn encoded_length_follows_marks_and_clip_speeds() {
    let mut e = Editor::new(false);
    e.state = State::MainEdit;
    e.playhead = 0.5;
    e.drop_marker().unwrap();
    // The second clip (0.5..1) at 50 %: 10 s of 20 s plays as 20 s, the first half as 10 s.
    e.playhead = 0.75;
    e.state = State::Effects;
    e.channel = Channel::Speed;
    e.pending = [50, 0, 0, 0];
    k(&mut e, Pad::A);
    assert_eq!(e.speed_at(0.25), 100);
    assert_eq!(e.speed_at(0.6), 50);
    assert_eq!(e.encoded_seconds(20.), 30);
    e.in_mark = 0.25;
    assert_eq!(e.encoded_seconds(20.), 25);
}

#[test]
fn speed_steps_and_caps() {
    let mut e = Editor::new(false);
    e.adjust(Channel::Speed, true);
    assert_eq!(e.speed_at(0.5), 100);
    for _ in 0..20 {
        e.adjust(Channel::Speed, false);
    }
    assert_eq!(e.speed_at(0.5), 10);
}

#[test]
fn shake_wraps() {
    let mut e = Editor::new(false);
    let mut seen = Vec::new();
    for _ in 0..5 {
        e.adjust(Channel::Shake, true);
        seen.push(e.int_at(Channel::Shake, 0.5));
    }
    assert_eq!(seen, [1, 2, 3, 0, 1]);
}

#[test]
fn dlc_channel_ranges() {
    let mut e = Editor::new(false);
    for (channel, up, expect) in [
        (Channel::Brightness, true, 100),
        (Channel::Brightness, false, -100),
        (Channel::Contrast, true, 30),
        (Channel::Contrast, false, 10),
        (Channel::Saturation, true, 100),
        (Channel::Saturation, false, 0),
    ] {
        for _ in 0..30 {
            e.adjust(channel, up);
        }
        assert_eq!(e.int_at(channel, 0.5), expect, "{channel:?} {up}");
    }
    // Defaults (Stage 0 §6): B 0, C 10, S 0; slider B/20+5, (C-10)/2, S/10.
    let e = Editor::new(false);
    assert_eq!(
        [Channel::Brightness, Channel::Contrast, Channel::Saturation].map(|c| e.int_at(c, 0.5)),
        [0, 10, 0]
    );
    let mut e = Editor::new(false);
    e.state = State::Effects;
    e.channel = Channel::Brightness;
    e.pending = [-100, 0, 0, 0];
    assert_eq!(e.slider_value(), 0);
    e.channel = Channel::Contrast;
    e.pending = [30, 0, 0, 0];
    assert_eq!(e.slider_value(), 10);
    // Tint: 0..250 on the 25 grid, per sub-channel.
    e.channel = Channel::Tint;
    e.pending = [0; 4];
    e.tint_channel = 2;
    for _ in 0..12 {
        e.adjust(Channel::Tint, true);
    }
    assert_eq!(e.pending, [0, 0, 250, 0]);
    e.adjust(Channel::Tint, false);
    assert_eq!(e.pending[2], 225);
}

/// Not retail, by request (user 10-27: "there should be no limit for keyframes"): retail's 0x82613500 stopped at 100
/// clip markers or 100 keys on a channel (ID_REPLAY_EDITOR_NO_MORE_MARKERS); ours has no limit.
#[test]
fn marker_proximity_and_no_limit() {
    let mut e = Editor::new(false);
    e.playhead = 0.5;
    e.drop_marker().unwrap();
    // 0x826132a8: on a clip marker or outside the margins it is the error sound, no popup.
    e.playhead = 0.505;
    assert_eq!(e.drop_marker(), Err(None));
    e.playhead = 0.005;
    assert_eq!(e.drop_marker(), Err(None), "inside the timeline's margin");
    let mut e = Editor::new(false);
    for i in 0..101 {
        e.playhead = 0.01 + i as f32 * 0.0095;
        e.drop_marker().unwrap();
    }
    assert_eq!(e.clip_markers.len(), 101, "the 101st clip marker too");
    let mut e = Editor::new(false);
    e.channel = Channel::Speed;
    for i in 0..150 {
        e.playhead = 0.01 + i as f32 * 0.006;
        e.pending = [50, 0, 0, 0];
        e.create_key();
    }
    assert_eq!(e.keys[Channel::Speed.index()].iter().filter(|k| k.soft).count(), 150, "150 keys on one channel");
}

/// 1f: Instant Replay opens at 0.98 (0x82610734 seeks there), inside the edit window, so the stick orbit and the
/// timeline's keys work at once; at 1.0 they did nothing until a scrub back.
#[test]
fn the_editor_opens_inside_the_edit_window() {
    let e = Editor::new(false);
    assert_eq!(e.playhead, 0.98);
    assert!(e.inside());
}

#[test]
fn empty_recording_opens_without_nan() {
    let mut e = Editor::new(false);
    assert_eq!(e.encoded_seconds(0.0), 0);
    assert!(e.playhead.is_finite());
    e.playing = true;
    assert!(!e.play(1. / 60., 0.));
    assert!(e.playhead.is_finite());
}

#[test]
fn online_editor_is_state_20_only() {
    let mut e = Editor::new(true);
    assert!(matches!(e.state, State::OnlineInstant));
    assert_eq!(e.movie_state(), 20);
    // No timeline edit, channel change or help: B exits, X opens the menu with Save Replay and Take Photo only.
    for pad in [Pad::A, Pad::Y, Pad::Start, Pad::Down, Pad::Left] {
        k(&mut e, pad);
        assert_eq!(e.state, State::OnlineInstant, "{pad:?}");
    }
    assert_eq!(k(&mut e, Pad::X), vec![Out::Menu]);
    assert_eq!(e.menu_rows(10.), vec![MenuItem::SaveReplay, MenuItem::TakePhoto]);
    assert_eq!(e.pick(Some(MenuItem::TakePhoto)), vec![Out::ClosePopup, Out::Popup(PopupKind::SavePhoto)]);
    assert_eq!(e.answer(1), vec![Out::ClosePopup, Out::Photo]);
    assert_eq!(e.state, State::OnlineInstant);
    assert_eq!(k(&mut e, Pad::B), vec![Out::Popup(PopupKind::Quit)]);
    assert_eq!(e.answer(1), vec![Out::ClosePopup, Out::Exit]);
}

// ---------------------------------------------------------------- RE3 state transitions (research Q1 / Q3)

#[test]
fn scrub_mode_keys() {
    let mut e = Editor::new(false);
    assert_eq!(e.movie_state(), 0);
    // RB: kStartPlaybackReplay / kStopPlaybackReplay.
    e.playhead = e.out_mark;
    assert_eq!(k(&mut e, Pad::Rb), vec![Out::State(18)]);
    assert!(e.playing && e.playhead == 0., "play from the out mark starts at the in mark");
    assert_eq!(k(&mut e, Pad::Rb), vec![Out::State(19)]);
    // D-pad left / right: start / end.
    e.playhead = 0.4;
    k(&mut e, Pad::Right);
    assert_eq!(e.playhead, 1.);
    k(&mut e, Pad::Left);
    assert_eq!(e.playhead, 0.);
    // L3 hides and shows (kHideEditor / kShowEditor).
    assert_eq!(k(&mut e, Pad::L3), vec![Out::State(1003)]);
    assert_eq!(k(&mut e, Pad::L3), vec![Out::State(1002)]);
    // Start: help, B back out of it.
    assert_eq!(k(&mut e, Pad::Start), vec![Out::Help(true)]);
    assert_eq!(e.state, State::Help);
    assert_eq!(e.movie_state(), 0);
    assert_eq!(k(&mut e, Pad::B), vec![Out::Help(false)]);
    assert_eq!(e.state, State::Scrub);
    // Y does nothing in scrub (0x82612c98: only RB / X / B / L3 before the editor is ready).
    assert!(k(&mut e, Pad::Y).is_empty());
    // A: edit timeline (state 21, mode 6, state 9).
    assert_eq!(k(&mut e, Pad::A), vec![Out::State(21), Out::State(9)]);
    assert_eq!(e.state, State::MainEdit);
}

#[test]
fn main_edit_keys() {
    let mut e = Editor::new(false);
    k(&mut e, Pad::A);
    // Y: change channel, kUpdateMode, wrapping after Tint.
    for i in 1..=7 {
        assert_eq!(k(&mut e, Pad::Y), vec![Out::State(23)]);
        assert_eq!(e.channel, Channel::ALL[i % 7]);
    }
    // D-pad down drops a marker (kOverClipMarker), up over it asks to delete it.
    e.playhead = 0.3;
    assert_eq!(k(&mut e, Pad::Down), vec![Out::State(1006)]);
    assert_eq!(e.clip_markers, vec![0.3]);
    assert!(k(&mut e, Pad::Down).is_empty(), "on a marker: the error sound only");
    assert_eq!(e.state, State::MainEdit);
    // D-pad left / right: previous / next marker.
    e.playhead = 0.6;
    assert_eq!(k(&mut e, Pad::Left), vec![Out::State(999)]);
    assert_eq!(e.playhead, 0.3);
    k(&mut e, Pad::Left);
    assert_eq!(e.playhead, 0.);
    k(&mut e, Pad::Right);
    assert_eq!(e.playhead, 0.3);
    assert_eq!(k(&mut e, Pad::Up), vec![Out::Popup(PopupKind::DeleteClip)]);
    e.answer(1);
    assert!(e.clip_markers.is_empty());
    // X: the REPLAY MENU; B: quit.
    assert_eq!(k(&mut e, Pad::X), vec![Out::Menu]);
    assert_eq!(e.pick(None), vec![Out::ClosePopup]);
    assert_eq!(e.state, State::MainEdit);
    assert_eq!(k(&mut e, Pad::B), vec![Out::Popup(PopupKind::Quit)]);
    e.answer(0);
    assert_eq!(e.state, State::MainEdit);
    // A: edit clip, the channel's panel (kHideToggle 22, then 4).
    e.playhead = 0.5;
    assert_eq!(k(&mut e, Pad::A), vec![Out::State(22), Out::State(4)]);
    assert_eq!(e.state, State::Effects);
}

#[test]
fn camera_panel_selects_edits_and_commits() {
    let mut e = Editor::new(false);
    k(&mut e, Pad::A);
    e.playhead = 0.5;
    k(&mut e, Pad::A);
    assert_eq!(e.camera_index, CameraKind::Game.index(), "a clip starts on the Game camera");
    assert_eq!(k(&mut e, Pad::Up), vec![Out::State(5)]);
    assert_eq!(k(&mut e, Pad::Up), vec![Out::State(5)]);
    assert_eq!(CameraKind::LIST[e.camera_index], CameraKind::Follow);
    assert_eq!(k(&mut e, Pad::A), vec![Out::State(1)]);
    assert_eq!(e.state, State::Camera(CameraKind::Follow));
    e.shot.lens = 50.;
    assert_eq!(k(&mut e, Pad::A), vec![Out::State(21), Out::State(9)]);
    assert_eq!(e.state, State::MainEdit);
    assert_eq!(e.camera_at(0.2).kind, CameraKind::Follow);
    assert_eq!(e.camera_at(0.2).lens, 50.);
    // Retail's camera edit (0x826127b8 modes 7..10) has no B: only Start and A; A commits.
    k(&mut e, Pad::A);
    k(&mut e, Pad::Down);
    k(&mut e, Pad::A);
    assert_eq!(e.state, State::Camera(CameraKind::Tripod));
    assert!(k(&mut e, Pad::B).is_empty());
    assert_eq!(e.state, State::Camera(CameraKind::Tripod));
    k(&mut e, Pad::A);
    assert_eq!(e.camera_at(0.2).kind, CameraKind::Tripod);
}

#[test]
fn advanced_cameras_key_inside_a_clip() {
    let mut e = Editor::new(false);
    k(&mut e, Pad::A);
    for t in [0.2, 0.6] {
        e.playhead = t;
        k(&mut e, Pad::A);
        e.camera_index = CameraKind::AdvFree.index();
        assert_eq!(k(&mut e, Pad::A), vec![Out::State(14)]);
        e.shot.eye = Vec3::splat(t);
        k(&mut e, Pad::A);
    }
    let keys = e.camera_keys(0.5);
    assert_eq!(keys.iter().map(|k| k.0).collect::<Vec<_>>(), [0.2, 0.6]);
    assert_eq!(keys[1].1.eye, Vec3::splat(0.6));
    assert!(keys.iter().all(|k| k.3 == Ease::Smooth), "retail's camera spline");
    // A over a camera key (0x82613050) locks until A's release (+157, code 46), which opens that key's camera.
    e.playhead = 0.6;
    assert!(k(&mut e, Pad::A).is_empty());
    assert!(e.a_lock && e.state == State::MainEdit);
    assert!(k(&mut e, Pad::Y).is_empty(), "only the release passes the lock");
    assert_eq!(k(&mut e, Pad::ARelease), vec![Out::State(22), Out::State(14)]);
    assert_eq!((e.state.clone(), e.shot.eye), (State::Camera(CameraKind::AdvFree), Vec3::splat(0.6)));
    k(&mut e, Pad::A);
    // Another camera on that clip asks first: it drops the keys.
    e.playhead = 0.4;
    k(&mut e, Pad::A);
    e.camera_index = CameraKind::Tripod.index();
    assert_eq!(k(&mut e, Pad::A), vec![Out::Popup(PopupKind::ReplaceCamera)]);
    assert_eq!(e.answer(1), vec![Out::ClosePopup, Out::State(2)]);
    k(&mut e, Pad::A);
    assert!(e.camera_keys(0.5).is_empty());
}

#[test]
fn effects_prompts_follow_soft_markers() {
    let mut e = Editor::new(false);
    k(&mut e, Pad::A);
    e.channel = Channel::Brightness;
    e.playhead = 0.5;
    k(&mut e, Pad::A);
    assert_eq!(e.pending[0], 0);
    assert_eq!(k(&mut e, Pad::Right), vec![Out::State(6)]);
    // No key markers: X creates one and goes back to Edit Timeline (state 21, mode 6, state 9).
    assert_eq!(k(&mut e, Pad::X), vec![Out::State(21), Out::State(9)]);
    assert_eq!(e.state, State::MainEdit);
    assert!(e.any_soft_markers_in_clip());
    assert_eq!(e.int_at(Channel::Brightness, 0.6), 20);
    assert_eq!(e.int_at(Channel::Brightness, 0.4), 0);
    // Keys exist, not over one: A creates; over one: A applies to it (each back to Edit Timeline).
    e.playhead = 0.8;
    assert_eq!(k(&mut e, Pad::A), vec![Out::State(22), Out::State(4)]);
    e.pending = [-40, 0, 0, 0];
    assert_eq!(k(&mut e, Pad::A), vec![Out::State(21), Out::State(9)]);
    assert_eq!(e.int_at(Channel::Brightness, 0.9), -40);
    k(&mut e, Pad::A);
    assert_eq!(e.state, State::Effects, "A over the key opens it");
    e.pending = [60, 0, 0, 0];
    k(&mut e, Pad::A);
    assert_eq!(e.keys[Channel::Brightness.index()].iter().filter(|k| k.soft).count(), 2);
    assert_eq!(e.int_at(Channel::Brightness, 0.9), 60);
    // Shake never creates keys.
    e.channel = Channel::Shake;
    e.playhead = 0.3;
    k(&mut e, Pad::A);
    assert!(k(&mut e, Pad::X).is_empty());
    // B back to the timeline.
    assert_eq!(k(&mut e, Pad::B), vec![Out::State(21), Out::State(9)]);
}

#[test]
fn tint_rows_and_values() {
    let mut e = Editor::new(false);
    k(&mut e, Pad::A);
    e.channel = Channel::Tint;
    e.playhead = 0.5;
    k(&mut e, Pad::A);
    assert_eq!(k(&mut e, Pad::Down), vec![Out::State(24)]);
    assert_eq!(e.tint_channel, 1);
    k(&mut e, Pad::Right);
    k(&mut e, Pad::Right);
    k(&mut e, Pad::A);
    assert_eq!(e.tint_at(0.1), [0, 50, 0, 0]);
}

#[test]
fn removing_a_clip_marker_drops_both_clips_keys() {
    let mut e = Editor::new(false);
    e.state = State::MainEdit;
    e.playhead = 0.5;
    e.drop_marker().unwrap();
    e.channel = Channel::Saturation;
    for (t, v) in [(0.3, 20), (0.7, 40)] {
        e.state = State::Effects;
        e.playhead = t;
        e.pending = [v, 0, 0, 0];
        k(&mut e, Pad::X);
    }
    assert_eq!(e.int_at(Channel::Saturation, 0.8), 40);
    e.playhead = 0.5;
    e.remove_marker().unwrap();
    assert!(e.keys[Channel::Saturation.index()].iter().all(|k| !k.soft));
    assert_eq!(e.int_at(Channel::Saturation, 0.8), 0);
}

#[test]
fn resets() {
    let mut e = Editor::new(false);
    e.playhead = 0.5;
    e.drop_marker().unwrap();
    assert_eq!(e.clip_markers, vec![0.5]);
    e.adjust(Channel::Speed, false);
    e.reset_timeline(Some(Channel::Speed));
    assert_eq!(e.speed_at(0.5), 100);
    assert_eq!(e.clip_markers, vec![0.5]);
    e.reset_timeline(None);
    assert!(e.clip_markers.is_empty());
}

#[test]
fn menu_routes_to_its_popups() {
    let mut e = Editor::new(false);
    for (item, out) in [
        (MenuItem::SaveReplay, Out::Popup(PopupKind::SaveReplay)),
        (MenuItem::Upload, Out::Popup(PopupKind::Upload)),
        (MenuItem::TakePhoto, Out::Popup(PopupKind::SavePhoto)),
        (MenuItem::ResetTimeline, Out::Popup(PopupKind::ResetTimeline)),
        (MenuItem::ResetChannel, Out::Popup(PopupKind::ResetChannel)),
        (MenuItem::ReplaySettings, Out::Settings),
    ] {
        k(&mut e, Pad::X);
        assert_eq!(e.pick(Some(item)), vec![Out::ClosePopup, out.clone()], "{item:?}");
        match out {
            Out::Settings => {
                e.setting(Some(0));
                assert!(e.look.green_screen);
                e.setting(Some(2));
                assert!(e.look.hide_skaters);
                e.setting(Some(0));
                e.setting(Some(2));
                assert_eq!(e.look, Look::default());
                assert_eq!(e.setting(None), vec![Out::ClosePopup]);
            }
            _ => drop(e.answer(0)),
        }
        assert_eq!(e.state, State::Scrub);
    }
    k(&mut e, Pad::X);
    assert_eq!(e.pick(Some(MenuItem::SaveReplay)), vec![Out::ClosePopup, Out::Popup(PopupKind::SaveReplay)]);
    assert_eq!(e.answer(1), vec![Out::ClosePopup, Out::SaveReplay]);
}

#[test]
fn playback_runs_at_the_speed_channel_and_stops_at_the_out_mark() {
    let mut e = Editor::new(false);
    e.set_whole_clip_speed(50);
    e.out_mark = 0.5;
    k(&mut e, Pad::Rb);
    assert_eq!(e.playhead, 0.);
    // 10 s recording, half speed: 1 s of real time moves 0.05.
    assert!(e.play(1., 10.));
    assert!((e.playhead - 0.05).abs() < 1e-6);
    while e.play(1., 10.) {}
    assert_eq!(e.playhead, 0.5);
    assert!(!e.playing);
}

/// Audit 3b (user 10-27: retail's): the marks stop a tenth of the recording apart (0x82509DA0 / 0x82509EB0).
#[test]
fn marks_move_with_lb_and_stay_a_tenth_apart() {
    let mut e = Editor::new(false);
    e.state = State::MainEdit;
    for _ in 0..400 {
        e.move_mark(false, 1.);
    }
    assert!((e.in_mark - (1. - MIN_CLIP)).abs() < 1e-5, "{}", e.in_mark);
    for _ in 0..400 {
        e.move_mark(true, -1.);
    }
    assert!((e.out_mark - (e.in_mark + MIN_CLIP)).abs() < 1e-5, "{} {}", e.in_mark, e.out_mark);
}

/// Review I8: 0x82614f38's rows, in retail's order; Upload is hidden (not refused) over 60 encoded seconds.
#[test]
fn menu_rows_follow_retail() {
    let e = Editor::new(false);
    assert_eq!(
        e.menu_rows(60.),
        vec![
            MenuItem::Upload,
            MenuItem::SaveReplay,
            MenuItem::TakePhoto,
            MenuItem::ReplaySettings,
            MenuItem::ResetChannel,
            MenuItem::ResetTimeline
        ]
    );
    assert_eq!(e.menu_rows(61.)[0], MenuItem::SaveReplay, "61 s: no Upload row");
    assert_eq!(SETTINGS.len(), 3);
}

/// Retail with the HUD hidden (0x82612c28 / 0x82613988): playback (mode 1) ignores X; Edit Timeline's X and Instant
/// Replay's Back stop (19), show the HUD (1002), then open the menu.
#[test]
fn x_with_the_hud_hidden_shows_it_and_opens_the_menu() {
    let mut e = Editor::new(false);
    k(&mut e, Pad::Rb);
    k(&mut e, Pad::L3);
    assert!(e.hidden && e.playing);
    assert!(k(&mut e, Pad::X).is_empty(), "mode 1 ignores X");
    assert_eq!(k(&mut e, Pad::Rb), vec![Out::State(19)]);
    assert!(k(&mut e, Pad::X).is_empty(), "Instant Replay's hidden X does nothing");
    assert_eq!(k(&mut e, Pad::Back), vec![Out::State(19), Out::State(1002), Out::Menu]);
    assert!(!e.hidden && !e.playing);
    assert_eq!(e.state, State::Menu);
    let mut e = Editor::new(false);
    k(&mut e, Pad::A);
    k(&mut e, Pad::L3);
    assert_eq!(k(&mut e, Pad::X), vec![Out::State(19), Out::State(1002), Out::Menu]);
    assert_eq!(e.state, State::Menu);
}

/// Review M2: a mark eases in and settles after the stick is released, as the scrub does.
#[test]
fn marks_ease_and_settle() {
    let mut e = Editor::new(false);
    e.state = State::MainEdit;
    e.in_mark = 0.5;
    e.move_mark(false, 1.);
    let first = e.in_mark - 0.5;
    assert!(first > 0. && first < SCRUB_GAIN, "eases in: {first}");
    for _ in 0..10 {
        e.move_mark(false, 1.);
    }
    let held = e.in_mark;
    for _ in 0..60 {
        e.move_mark(false, 0.);
    }
    assert!(e.in_mark > held, "coasts while settling");
    let settled = e.in_mark;
    e.move_mark(false, 0.);
    assert_eq!(e.in_mark, settled, "settled");
}

/// Review I4 (a): a hostile timeline is clamped into range or refused, never left to panic later.
#[test]
fn sanitise_clamps_and_refuses() {
    let mut e = Editor::new(false);
    (e.in_mark, e.out_mark) = (0.8, 0.2);
    assert!(super::sanitise(&mut e).is_err(), "in after out");
    // A save from before the 0.1 gap (3b) widens to it.
    (e.in_mark, e.out_mark) = (0.995, 0.996);
    super::sanitise(&mut e).unwrap();
    assert!((e.in_mark - 0.9).abs() < 1e-6 && e.out_mark == 1., "{} {}", e.in_mark, e.out_mark);
    let mut e = Editor::new(false);
    e.keys[Channel::Speed.index()].push(Key { t: 0., soft: false, cut: false, value: Value::Int(0), ease: Ease::Hold });
    e.keys[Channel::Tint.index()].push(Key { t: 0., soft: false, cut: false, value: Value::Tint([13, -7, 999, 30]), ease: Ease::Hold });
    let mut shot = Shot::new(CameraKind::Free);
    shot.lens = 500.;
    shot.roll = 540.;
    e.keys[0].push(Key { t: 0., soft: false, cut: false, value: Value::Camera(shot), ease: Ease::Smooth });
    super::sanitise(&mut e).unwrap();
    assert_eq!(e.speed_at(0.5), 10);
    assert_eq!(e.tint_at(0.5), [25, 0, 250, 25]);
    assert_eq!(e.camera_at(0.5).lens, LENS_MAX);
    assert_eq!(e.camera_at(0.5).roll, -180., "the roll wraps into -180..180");
    e.state = State::MainEdit;
    e.move_mark(true, -1.);
    e.move_mark(false, 1.);
    shot.eye = Vec3::NAN;
    e.keys[0][0].value = Value::Camera(shot);
    assert!(super::sanitise(&mut e).is_err());
}

// ---------------------------------------------------------------- item 16: retail keys, sounds, the extensions

/// TU3 RE passes 2 / 3 (replay-progress.md): the FE sound each key plays (the exe plays them, the movie none).
#[test]
fn keys_play_retail_sounds() {
    use sound::*;
    let mut e = Editor::new(false);
    assert_eq!(heard(&mut e, Pad::Start), [HELP]);
    k(&mut e, Pad::B);
    assert_eq!(heard(&mut e, Pad::A), [KEY], "Instant Replay's A");
    assert_eq!(heard(&mut e, Pad::Y), [CHANNEL]);
    e.channel = Channel::Camera;
    e.playhead = 0.3;
    assert_eq!(heard(&mut e, Pad::Up), [ERROR], "nothing to remove");
    assert_eq!(heard(&mut e, Pad::Down), [ADD]);
    assert_eq!(heard(&mut e, Pad::Down), [ERROR], "on the marker");
    assert_eq!(heard(&mut e, Pad::Right), [NEXT]);
    assert_eq!(heard(&mut e, Pad::Left), [PREVIOUS]);
    assert_eq!(heard(&mut e, Pad::Up), [REMOVE]);
    e.answer(0);
    assert_eq!(heard(&mut e, Pad::X), [MENU]);
    e.pick(None);
    assert_eq!(heard(&mut e, Pad::B), [QUIT]);
    e.answer(0);
    assert_eq!(heard(&mut e, Pad::Lb), [PREVIOUS]);
    k(&mut e, Pad::LbRelease);
    // Effects: the list moves, B goes back, a value sounds only when it changes, A applies.
    e.playhead = 0.5;
    assert_eq!(heard(&mut e, Pad::A), [EDIT]);
    assert_eq!(heard(&mut e, Pad::Down), [DOWN]);
    assert_eq!(heard(&mut e, Pad::Up), [UP]);
    assert_eq!(heard(&mut e, Pad::B), [BACK]);
    e.channel = Channel::Speed;
    k(&mut e, Pad::A);
    assert!(heard(&mut e, Pad::Right).is_empty(), "already 100 %");
    assert_eq!(heard(&mut e, Pad::Left), [DEC]);
    assert_eq!(heard(&mut e, Pad::A), [SELECT]);
    // Modes 5 / 6 toggle the HUD silently; mode 1 (playback) with its sound.
    assert!(heard(&mut e, Pad::L3).is_empty());
    k(&mut e, Pad::L3);
    k(&mut e, Pad::Rb);
    assert_eq!(heard(&mut e, Pad::L3), [HUD]);
}

/// Mode 1 (playback): B, RB, a trigger and L3 only; the stop helper 0x82615648 sends [21, 19] from Edit Timeline and
/// [19] from Instant Replay (also when playback runs out: Editor::stop_states).
#[test]
fn playback_keys_follow_mode_1() {
    let mut e = Editor::new(false);
    k(&mut e, Pad::A);
    assert_eq!(k(&mut e, Pad::Rb), [Out::State(18)]);
    for pad in [Pad::A, Pad::X, Pad::Y, Pad::Start, Pad::Up, Pad::Down, Pad::Left, Pad::Lb, Pad::RsUp, Pad::Back] {
        assert!(k(&mut e, pad).is_empty(), "{pad:?}");
        assert!(e.playing);
    }
    assert_eq!(k(&mut e, Pad::Trigger), [Out::State(21), Out::State(19)]);
    assert_eq!(e.stop_states(), [21, 19]);
    k(&mut e, Pad::Rb);
    assert_eq!(k(&mut e, Pad::B), [Out::State(21), Out::State(19), Out::State(1002), Out::Popup(PopupKind::Quit)]);
    assert!(!e.playing);
    let mut e = Editor::new(false);
    k(&mut e, Pad::Rb);
    assert_eq!(k(&mut e, Pad::Trigger), [Out::State(19)]);
    assert_eq!(e.stop_states(), [19]);
}

/// LB in Edit Timeline enters mode 2 (state 1005): the sticks move the marks, nothing else passes until LB's release
/// (code 53: states 21, 1004).
#[test]
fn lb_holds_the_sticks_on_the_marks_until_released() {
    let mut e = Editor::new(false);
    k(&mut e, Pad::A);
    assert_eq!(k(&mut e, Pad::Lb), [Out::State(1005)]);
    assert!(e.moving_marks);
    for pad in [Pad::A, Pad::B, Pad::X, Pad::Rb, Pad::Down, Pad::L3, Pad::Start] {
        assert!(k(&mut e, pad).is_empty(), "{pad:?}");
    }
    assert_eq!(k(&mut e, Pad::LbRelease), [Out::State(21), Out::State(1004)]);
    assert!(!e.moving_marks);
}

const PADS: [Pad; 21] = [
    Pad::Start,
    Pad::Back,
    Pad::A,
    Pad::B,
    Pad::X,
    Pad::Y,
    Pad::Rb,
    Pad::Trigger,
    Pad::R3,
    Pad::Lb,
    Pad::L3,
    Pad::Up,
    Pad::Down,
    Pad::Left,
    Pad::Right,
    Pad::ARelease,
    Pad::LbRelease,
    Pad::RsLeft,
    Pad::RsRight,
    Pad::RsUp,
    Pad::RsDown,
];
/// The FE key code a pad arrives as (Apt input index - 124; the D-pad's release codes on Edit Timeline).
fn code(pad: Pad, release_dpad: bool) -> u32 {
    let dpad = if release_dpad { 32 } else { 16 };
    match pad {
        Pad::Start => 0,
        Pad::Back => 1,
        Pad::A => 2,
        Pad::B => 3,
        Pad::Y => 4,
        Pad::X => 5,
        Pad::Rb => 6,
        Pad::Trigger => 7,
        Pad::R3 => 8,
        Pad::Lb => 9,
        Pad::L3 => 11,
        Pad::Up => dpad,
        Pad::Down => dpad + 1,
        Pad::Left => dpad + 2,
        Pad::Right => dpad + 3,
        Pad::RsLeft => 20,
        Pad::RsRight => 21,
        Pad::RsUp => 22,
        Pad::RsDown => 23,
        Pad::ARelease => 46,
        Pad::LbRelease => 53,
    }
}

/// Item 16: the action-map walk. Per screen, the codes retail's handler reads (TU3: Edit Timeline 0x82612c28 0, 2..6,
/// 9, 11, 32..35, 46; Effects 0x82612328 0, 2, 3, 5, 16..19; the camera edits' dispatcher 0x826127b8 0 and 2, their
/// updates only the sticks and the held D-pad). Every extension binding is outside that set, is wired, and every
/// other code retail ignores still does nothing.
#[test]
fn extension_bindings_are_free_in_retail() {
    let mut main = Editor::new(false);
    k(&mut main, Pad::A);
    main.channel = Channel::Saturation;
    main.playhead = 0.4;
    k(&mut main, Pad::A);
    main.pending = [50, 0, 0, 0];
    k(&mut main, Pad::X);
    main.playhead = 0.6;
    let mut effects = main.clone();
    k(&mut effects, Pad::A);
    assert_eq!(effects.state, State::Effects);
    let mut screens: Vec<(String, Editor, Vec<u32>, Vec<Pad>, bool)> = vec![
        ("Edit Timeline".into(), main, vec![0, 2, 3, 4, 5, 6, 9, 11, 32, 33, 34, 35, 46], vec![Pad::RsLeft, Pad::RsRight, Pad::RsUp, Pad::RsDown], true),
        ("Effects".into(), effects, vec![0, 2, 3, 5, 16, 17, 18, 19], vec![Pad::Y], false),
    ];
    for kind in CameraKind::LIST {
        let mut e = Editor::new(false);
        e.state = State::Camera(kind);
        e.shot = Shot::new(kind);
        // LB / RB are read held by hud.rs (the roll), not as presses.
        let mut extensions = vec![Pad::R3, Pad::X, Pad::Back, Pad::Lb, Pad::Rb];
        if kind.advanced() {
            extensions.push(Pad::Y);
        }
        screens.push((format!("{kind:?} camera edit"), e, vec![0, 2], extensions, false));
    }
    for (screen, editor, retail, extensions, release_dpad) in screens {
        for pad in PADS {
            let c = code(pad, release_dpad);
            let outs = editor.clone().key(pad);
            if extensions.contains(&pad) {
                assert!(!retail.contains(&c), "{screen}: {pad:?} (code {c}) is retail's");
                assert!(outs.is_empty() == matches!(pad, Pad::Lb | Pad::Rb), "{screen}: {pad:?} is wired: {outs:?}");
            } else if !retail.contains(&c) {
                assert!(outs.is_empty(), "{screen}: {pad:?} (code {c}) is unhandled in retail: {outs:?}");
            }
        }
    }
}

/// Item 16: a key's ease shapes its value toward the clip's next key; Hold (every retail key) steps; a clip marker
/// cuts the ease; the value holds before the first key and after the last.
#[test]
fn eases_shape_the_channel_between_keys() {
    let quarter = catmull(0., 0., 100., 100., 0.25);
    for (ease, at_015, at_02) in [(Ease::Hold, 0., 0.), (Ease::Linear, 25., 50.), (Ease::Smooth, quarter, 50.)] {
        let mut e = Editor::new(false);
        e.clip_markers = vec![0.5];
        let key = |t: f32, v: i32, ease: Ease| Key { t, soft: t != 0. && t != 0.5, cut: false, value: Value::Int(v), ease };
        e.keys[Channel::Brightness.index()] =
            vec![key(0., 0, Ease::Hold), key(0.1, 0, ease), key(0.3, 100, ease), key(0.5, -100, ease)];
        let at = |t: f32| e.value_at(Channel::Brightness, t);
        assert!((at(0.15) - at_015).abs() < 1e-3, "{ease:?} 0.15: {}", at(0.15));
        assert!((at(0.2) - at_02).abs() < 1e-3, "{ease:?} 0.2: {}", at(0.2));
        assert_eq!((at(0.), at(0.05), at(0.3), at(0.45), at(0.49)), (0., 0., 100., 100., 100.), "{ease:?}: holds, cut at the marker");
        assert_eq!((at(0.5), at(1.)), (-100., -100.), "{ease:?}: the next clip, the end");
        assert_eq!(e.int_at(Channel::Brightness, 0.2), 0, "the panel still reads the key");
    }
    assert!((quarter - 25.).abs() > 1., "Smooth is not Linear");
}

/// Item 16: Effects' Y (ignored by 0x82612328) cycles the key's ease, shown in the panel header; the new key keeps
/// it. Shake has no ease.
#[test]
fn effects_y_cycles_the_key_ease() {
    let mut e = Editor::new(false);
    k(&mut e, Pad::A);
    e.channel = Channel::Speed;
    e.playhead = 0.3;
    k(&mut e, Pad::A);
    assert_eq!(e.pending_ease, Ease::Hold);
    assert_eq!(e.mode_text(), "ID_REPLAYEDITOR_CHANNEL_SPEED", "retail's header untouched");
    assert_eq!(e.key(Pad::Y), [Out::Sound(sound::INC), Out::Header]);
    assert_eq!(e.pending_ease, Ease::Linear);
    assert_eq!(e.mode_text(), "#ID_REPLAYEDITOR_CHANNEL_SPEED (Linear)");
    e.pending = [50, 0, 0, 0];
    k(&mut e, Pad::X);
    let key = e.keys[Channel::Speed.index()].iter().find(|k| k.soft).unwrap();
    assert_eq!((key.t, key.ease), (0.3, Ease::Linear));
    e.channel = Channel::Shake;
    e.playhead = 0.6;
    k(&mut e, Pad::A);
    assert_eq!(e.state, State::Effects);
    assert!(e.key(Pad::Y).is_empty());
}

/// Item 16 key tools on the right stick: previous / next key of the channel, copy, paste (onto a key: its value;
/// elsewhere: a new key; another channel: the error). Camera edits: X copies the shot, Back pastes it onto the same
/// camera only, R3 resets the roll, Y cycles an Adv key's ease.
#[test]
fn key_tools_step_copy_and_paste() {
    use sound::*;
    let mut e = Editor::new(false);
    k(&mut e, Pad::A);
    e.channel = Channel::Saturation;
    for (t, v) in [(0.2, 30), (0.6, 70)] {
        e.playhead = t;
        k(&mut e, Pad::A);
        e.pending = [v, 0, 0, 0];
        k(&mut e, Pad::X);
    }
    e.playhead = 0.4;
    assert_eq!(heard(&mut e, Pad::RsRight), [NEXT]);
    assert_eq!(e.playhead, 0.6);
    assert_eq!(heard(&mut e, Pad::RsRight), [ERROR]);
    assert_eq!(e.playhead, 0.6);
    assert_eq!(heard(&mut e, Pad::RsLeft), [PREVIOUS]);
    assert_eq!(e.playhead, 0.2);
    assert_eq!(heard(&mut e, Pad::RsUp), [SELECT]);
    e.playhead = 0.4;
    assert_eq!(heard(&mut e, Pad::RsDown), [CREATE]);
    assert_eq!(e.keys[Channel::Saturation.index()].len(), 3);
    assert_eq!(e.int_at(Channel::Saturation, 0.45), 30);
    e.playhead = 0.6;
    assert_eq!(heard(&mut e, Pad::RsDown), [CREATE]);
    assert_eq!(e.keys[Channel::Saturation.index()].len(), 3, "onto the key under the playhead");
    assert_eq!(e.int_at(Channel::Saturation, 0.65), 30);
    e.channel = Channel::Brightness;
    assert_eq!(heard(&mut e, Pad::RsDown), [ERROR], "another channel");

    let mut e = Editor::new(false);
    e.state = State::Camera(CameraKind::Tripod);
    e.shot = Shot::new(CameraKind::Tripod);
    e.shot.roll = 20.;
    assert_eq!(heard(&mut e, Pad::X), [SELECT]);
    e.shot.roll = 0.;
    e.shot.lens = 40.;
    assert_eq!(heard(&mut e, Pad::Back), [CREATE]);
    assert_eq!((e.shot.roll, e.shot.lens), (20., LENS_MIN));
    assert_eq!(heard(&mut e, Pad::R3), [DEC]);
    assert_eq!(e.shot.roll, 0.);
    assert!(e.key(Pad::Y).is_empty(), "a basic camera has no keys to ease");
    e.state = State::Camera(CameraKind::Free);
    assert_eq!(heard(&mut e, Pad::Back), [ERROR], "another camera");
    e.state = State::Camera(CameraKind::AdvTripod);
    e.pending_ease = Ease::Smooth;
    assert_eq!(heard(&mut e, Pad::Y), [INC]);
    assert_eq!(e.pending_ease, Ease::Hold);
}

/// Item 16: Replay Settings keeps retail's three rows first; the letterbox cycles Off, 2.39, 1.85, 4:3 and the
/// camera path toggles, both off by default.
#[test]
fn replay_settings_extension_rows() {
    let mut look = Look::default();
    assert_eq!(&look.rows()[..3], &SETTINGS.map(String::from)[..]);
    assert_eq!(look.rows().len(), 5);
    assert_eq!((look.letterbox, look.camera_path, look.aspect()), (0, false, 0.));
    for aspect in [2.39, 1.85, 4. / 3., 0.] {
        look.toggle(3);
        assert_eq!(look.aspect(), aspect);
    }
    look.toggle(4);
    assert!(look.camera_path);
    assert!(look.rows()[3].ends_with("Letterbox: Off") && look.rows()[4].ends_with("Camera Path: On"));
}

/// Bug 10-17 #28: the HUD's loop (whole 60 Hz updates of real time) then the shown playhead. At 144 Hz the shown
/// replay moves on every drawn frame, by that frame's time at the replay's speed; at 60 Hz it is the playhead, as
/// before; it never passes the out mark.
#[test]
fn playback_moves_every_drawn_frame() {
    let length = 30.;
    let run = |dt: f32, frames: usize, out: f32| {
        let mut e = Editor::new(false);
        (e.in_mark, e.out_mark, e.playhead, e.playing) = (0., out, 0., true);
        let mut accumulated = 0.;
        let mut shown = Vec::new();
        for _ in 0..frames {
            accumulated = (accumulated + dt).min(4. * UPDATE);
            while accumulated >= UPDATE {
                accumulated -= UPDATE;
                e.play(UPDATE, length);
            }
            shown.push((e.shown(accumulated, length, false), e.playhead));
        }
        shown
    };
    let speed = Editor::new(false).value_at(Channel::Speed, 0.) * 0.01;
    let fast = run(1. / 144., 144, 1.);
    let step = 1. / 144. * speed / length;
    for w in fast.windows(2) {
        let moved = w[1].0 - w[0].0;
        assert!((moved - step).abs() <= step * 0.01, "moved {moved}, frame {step}");
    }
    for (shown, playhead) in run(UPDATE, 60, 1.) {
        assert_eq!(shown, playhead, "60 Hz: the playhead, as before");
    }
    let out = 0.01;
    assert!(run(1. / 144., 144, out).iter().all(|&(shown, _)| shown <= out), "never past the out mark");
}

/// Bug 10-17 #28: a trigger scrub is shown ahead by the leftover time, at its eased velocity; at rest it is the playhead.
#[test]
fn a_scrub_is_shown_between_updates() {
    let mut e = Editor::new(false);
    e.playhead = 0.5;
    assert_eq!(e.shown(UPDATE / 2., 30., true), 0.5);
    e.scrub(1., 0.);
    let v = e.scrub_velocity;
    assert!(v > 0. && (e.shown(UPDATE / 2., 30., true) - (e.playhead + v / 2.)).abs() < 1e-7);
    assert_eq!(e.shown(UPDATE / 2., 30., false), e.playhead, "a state without the scrub");
}

/// Bug 10-17 #31 audit: D-pad Up over a key asks to delete it; No keeps it, Yes removes that key alone.
#[test]
fn deleting_a_key_asks_then_removes_only_that_key() {
    let mut e = Editor::new(false);
    k(&mut e, Pad::A);
    let c = e.channel.index();
    e.keys[c].push(Key { t: 0.4, soft: true, cut: false, value: Value::Int(50), ease: Ease::Hold });
    e.keys[c].push(Key { t: 0.7, soft: true, cut: false, value: Value::Int(80), ease: Ease::Hold });
    e.playhead = 0.4;
    assert!(k(&mut e, Pad::Up).contains(&Out::Popup(PopupKind::DeleteKey)));
    e.answer(0);
    assert_eq!(e.keys[c].len(), 2, "No keeps it");
    assert!(k(&mut e, Pad::Up).contains(&Out::Popup(PopupKind::DeleteKey)));
    e.answer(1);
    assert_eq!(e.keys[c].iter().map(|k| k.t).collect::<Vec<_>>(), [0.7], "Yes removes that key alone");
    assert_eq!(e.state, State::MainEdit);
}

/// 1a: entering a camera the clip doesn't use marks the new shot unplaced, so hud.rs starts it at the camera on screen
/// (cameras::start_edit) before it draws; the clip's own camera keeps its shot; the Game camera needs no start.
#[test]
fn a_new_camera_edit_waits_for_its_start() {
    let mut e = Editor::new(false);
    e.state = State::MainEdit;
    e.playhead = 0.5;
    e.enter_camera(CameraKind::Tripod);
    assert!(e.unplaced && e.shot.kind == CameraKind::Tripod);
    e.shot.eye = Vec3::new(1., 2., 3.);
    e.unplaced = false;
    e.key(Pad::A);
    assert_eq!(e.camera_at(0.5).kind, CameraKind::Tripod, "A keeps it as the clip's camera");
    e.enter_camera(CameraKind::Tripod);
    assert!(!e.unplaced && e.shot.eye == Vec3::new(1., 2., 3.), "the clip's own shot");
    e.enter_camera(CameraKind::Game);
    assert!(!e.unplaced, "the Game camera is the recorded one");
}

/// Report Q2 (0x82c80044 / 0x82c80324 / 0x82c805dc / 0x82c80870): every camera entry, the same camera again included,
/// zeroes the D-pad's total (cam+0x38) and the Free rise; the clip's own aim offset stays.
#[test]
fn entering_a_camera_starts_its_dpad_total_again() {
    let mut e = Editor::new(false);
    e.state = State::MainEdit;
    e.enter_camera(CameraKind::Tripod);
    (e.unplaced, e.shot.height, e.aim_total, e.rise) = (false, 0.7, 0.7, 0.1);
    e.key(Pad::A);
    e.enter_camera(CameraKind::Tripod);
    assert_eq!((e.aim_total, e.rise, e.shot.height), (0., 0., 0.7));
}

/// Review I1: a scrub commits Instant Replay's orbit (retail's, over a Game, Follow or single-shot clip), but never
/// over a clip with Adv keys: what retail does to the keys between is unverified, and the commit would drop them all.
#[test]
fn an_orbit_never_commits_over_a_keyed_clip() {
    let mut e = Editor::new(false);
    let mut free = Shot::new(CameraKind::AdvFree);
    free.eye = Vec3::X;
    let key = |t: f32, soft: bool, shot: Shot| Key { t, soft, cut: false, value: Value::Camera(shot), ease: Ease::Smooth };
    e.keys[0] = vec![key(0., false, free), key(0.5, true, free)];
    let keyed = e.keys[0].clone();
    e.orbit = Some(Shot::new(CameraKind::Follow));
    e.commit_orbit();
    assert_eq!((e.orbit, &e.keys[0]), (None, &keyed), "the orbit goes, the keys stay");
    let mut tripod = Shot::new(CameraKind::Tripod);
    tripod.eye = Vec3::Y;
    e.keys[0] = vec![key(0., false, tripod)];
    e.orbit = Some(Shot::new(CameraKind::Follow));
    e.commit_orbit();
    assert_eq!((e.orbit, e.camera_at(0.5).kind), (None, CameraKind::Follow), "a single-shot clip takes it");
}
