//! The HUD against the retail movies (SKATE_REPLAY_UI_ROOT = a prepared private/replay-ui; tools/prepare_replay_ui.py)
//! and its Bevy-free rules.
use super::*;
use crate::{apt_vm::Value, presentation::Snapshot, replay::editor::{PopupKind, Shot}};
use bevy::ecs::system::RunSystemOnce;
use std::collections::BTreeSet;

fn ui_root() -> PathBuf {
    PathBuf::from(std::env::var("SKATE_REPLAY_UI_ROOT").expect("SKATE_REPLAY_UI_ROOT"))
}

fn editor_screen(editor: &Editor) -> Screen {
    let answers = answers(editor, 30., &|id| id.to_owned());
    let mut screen = Screen::load(&ui_root().join("runtime/replay_editor3.json"), &answers).unwrap();
    answer_all(&mut screen, editor, 30.);
    screen
}

/// Bug 10-16 #29: mReplay.mModeText (top left) is set only by the constructor's GetModeText, so a load without
/// answers (as `open` did) left it "undefined" through Instant Replay. Loaded as `open` loads it, it's the channel.
#[test]
#[ignore = "needs SKATE_REPLAY_UI_ROOT (the prepared replay-ui)"]
fn instant_replay_mode_text_is_the_channel() {
    let path = ui_root().join("runtime/replay_editor3.json");
    let editor = Editor::new(false);
    let mode = |screen: &Screen| screen.vm.get(screen.clip("mReplay.mModeText").expect("mModeText"), "text");
    let mut bare = Screen::load(&path, &[]).unwrap();
    answer_all(&mut bare, &editor, 30.);
    set_state(&mut bare, 0).unwrap();
    assert_eq!(mode(&bare), Value::Undefined, "the bug: booted unanswered");
    let mut screen = editor_screen(&editor);
    set_state(&mut screen, 0).unwrap();
    assert_eq!(mode(&screen), Value::Text(editor.mode_text()));
    let mut online = editor_screen(&Editor::new(true));
    set_state(&mut online, 20).unwrap();
    assert!(texts(&online).contains("ID_REPLAY_EDITOR_OPTIONS"), "mSignedIn = CanSaveMedia() at load");
}

/// Every text a text field holds (a fresh screen per state, so nothing stale is counted).
fn texts(screen: &Screen) -> BTreeSet<String> {
    (screen.vm.objects.iter())
        .filter_map(|o| match o.fields.get("text") {
            Some(Value::Text(t)) => Some(t.clone()),
            _ => None,
        })
        .collect()
}

fn number(screen: &Screen, path: &str, field: &str) -> f64 {
    match screen.clip(path).map(|id| screen.vm.get(id, field)) {
        Some(Value::Number(n)) => n,
        other => panic!("{path}.{field}: {other:?}"),
    }
}

/// research Q1 (replay_editor3.as SetState): each state's title and button prompts.
#[test]
#[ignore = "needs SKATE_REPLAY_UI_ROOT (the prepared replay-ui)"]
fn every_editor_state_presents_retail_prompts() {
    let commit = ["ID_REPLAYEDITOR_HELP", "ID_REPLAYEDITOR_COMMIT"];
    let cases: [(u32, &str, &[&str]); 9] = [
        (0, "ID_REPLAYEDITOR_INSTANT_REPLAY", &["ID_REPLAYEDITOR_HELP", "ID_REPLAYEDITOR_EXIT", "ID_REPLAY_EDITOR_OPTIONS", "ID_REPLAYEDITOR_EDIT_TIMELINE"]),
        (9, "ID_REPLAYEDITOR_EDIT_TIMELINE", &["ID_REPLAYEDITOR_HELP", "ID_REPLAYEDITOR_EXIT", "ID_REPLAY_EDITOR_OPTIONS", "ID_REPLAYEDITOR_EDIT_CLIP"]),
        (1, "ID_REPLAYEDITOR_FOLLOW_CAMERA", &commit),
        (2, "ID_REPLAYEDITOR_TRIPOD_CAMERA", &commit),
        (7, "ID_REPLAYEDITOR_FREE_CAMERA", &commit),
        (10, "ID_REPLAYEDITOR_DEFAULT_CAMERA", &commit),
        (12, "ID_REPLAYEDITOR_ADVANCED_FOLLOW_CAMERA", &commit),
        (13, "ID_REPLAYEDITOR_ADVANCED_TRIPOD_CAMERA", &commit),
        (14, "ID_REPLAYEDITOR_ADVANCED_FREE_CAMERA", &commit),
    ];
    for (state, title, prompts) in cases {
        let mut screen = editor_screen(&Editor::new(false));
        set_state(&mut screen, state).unwrap();
        for _ in 0..30 {
            screen.advance().unwrap();
        }
        screen.draws().unwrap();
        let shown = texts(&screen);
        assert!(shown.contains(title), "state {state}: no title {title}");
        for prompt in prompts {
            assert!(shown.contains(*prompt), "state {state}: no prompt {prompt}");
        }
    }
    // Effects (4): the panel's header is the channel; the timeline box goes transparent.
    let mut editor = Editor::new(false);
    editor.state = State::Effects;
    let mut screen = editor_screen(&editor);
    set_state(&mut screen, 9).unwrap();
    set_state(&mut screen, 4).unwrap();
    assert!(texts(&screen).contains("ID_REPLAYEDITOR_CHANNEL_CAMERA"));
    assert!(!visible(&screen, "mCustomRender"), "EnableTimeline(false)");
    // Online (20): no timeline.
    let mut screen = editor_screen(&Editor::new(true));
    set_state(&mut screen, 0).unwrap();
    set_state(&mut screen, 20).unwrap();
    assert!(!visible(&screen, "mCustomRender"));
}

/// Opens the editor movie as `open` does (intro, the editor's state, 20 online), then runs 30 frames: the intro's
/// frame action (37696) calls InstantDone on the way. The native calls the movie made.
fn opened_for_30_frames(editor: &Editor, instant_answer: Option<bool>) -> (Screen, Vec<String>) {
    let mut screen = editor_screen(editor);
    if let Some(answer) = instant_answer {
        screen.answer("FEReplayMenu.IsInstantReplayMode", json!(answer));
    }
    screen.play("intro").unwrap();
    set_state(&mut screen, editor.movie_state()).unwrap();
    if editor.online {
        set_state(&mut screen, 20).unwrap();
    }
    screen.take_calls();
    for _ in 0..30 {
        screen.advance().unwrap();
    }
    let calls = screen.take_calls().into_iter().map(|(call, _)| call).collect();
    (screen, calls)
}

/// Review I5: online (state 20) IsInstantReplayMode is true, so the intro's InstantDone leaves the movie in state 20:
/// no UpdateReplayEditorReady / SetState(kMainEditMode), no timeline. The old answer (false) did switch it.
#[test]
#[ignore = "needs SKATE_REPLAY_UI_ROOT (the prepared replay-ui)"]
fn online_instant_replay_stays_in_state_20_after_the_intro() {
    let editor = Editor::new(true);
    let (screen, calls) = opened_for_30_frames(&editor, None);
    assert!(calls.iter().any(|c| c == "FEReplayMenu.IsInstantReplayMode"), "the intro reached InstantDone: {calls:?}");
    assert!(!calls.iter().any(|c| c == "FEReplayMenu.UpdateReplayEditorReady"), "InstantDone left state 20");
    assert!(!visible(&screen, "mCustomRender"), "no timeline online");
    // Edit Timeline's own prompt (A Edit Clip, state 9) is not up.
    assert!(!texts(&screen).contains("ID_REPLAYEDITOR_EDIT_CLIP"));
    // The inverted answer the review found: the movie switches itself to Edit Timeline.
    let (screen, calls) = opened_for_30_frames(&editor, Some(false));
    assert!(calls.iter().any(|c| c == "FEReplayMenu.UpdateReplayEditorReady"));
    assert!(texts(&screen).contains("ID_REPLAYEDITOR_EDIT_CLIP"));
    // Offline Instant Replay (0) stays too; Edit Timeline is the player's A.
    let (_, calls) = opened_for_30_frames(&Editor::new(false), None);
    assert!(!calls.iter().any(|c| c == "FEReplayMenu.UpdateReplayEditorReady"));
}

/// A walk through every state and channel leaves no FEReplayMenu native unanswered (build-progress §9).
#[test]
#[ignore = "needs SKATE_REPLAY_UI_ROOT (the prepared replay-ui)"]
fn all_30_natives_answered() {
    let mut editor = Editor::new(false);
    let mut screen = editor_screen(&editor);
    let mut called = BTreeSet::new();
    let mut walk = |screen: &mut Screen, editor: &Editor, states: &[u32]| {
        answer_all(screen, editor, 30.);
        for &state in states {
            set_state(screen, state).unwrap();
        }
        for _ in 0..20 {
            screen.advance().unwrap();
        }
        called.extend(screen.take_calls().into_iter().map(|(call, _)| call).filter(|c| c.starts_with("FEReplayMenu.")));
    };
    walk(&mut screen, &editor, &[0, 18, 19, 999, 1002, 1003, 1002]);
    editor.state = State::MainEdit;
    walk(&mut screen, &editor, &[9, 1000, 1001, 1006, 1007, 1008, 1009, 23]);
    for channel in Channel::ALL {
        editor.state = State::MainEdit;
        editor.channel = channel;
        editor.state = State::Effects;
        walk(&mut screen, &editor, &[4, 5, 6, 24]);
    }
    for kind in CameraKind::LIST {
        editor.state = State::Camera(kind);
        walk(&mut screen, &editor, &[kind.edit_state(), 11]);
    }
    walk(&mut screen, &editor, &[20, 21, 22, 1010, 1011]);
    let unanswered: Vec<&String> = screen.bindings.unanswered.iter().filter(|k| k.starts_with("FEReplayMenu.")).collect();
    assert!(unanswered.is_empty(), "unanswered: {unanswered:?}");
    let answered: BTreeSet<&str> = answers(&editor, 30., &|id| id.to_owned()).into_iter().map(|(k, _)| k).collect();
    for call in &called {
        assert!(answered.contains(call.as_str()), "{call} has no answer");
    }
    assert!(called.len() >= 20, "the walk reached {} natives: {called:?}", called.len());
    // replay_help2 reads GetHelpMode.
    let help = Screen::load(&ui_root().join("runtime/replay_help2.json"), &answers(&editor, 30., &|id| id.to_owned())).unwrap();
    assert!(!help.bindings.unanswered.iter().any(|k| k.starts_with("FEReplayMenu.")));
}

/// L3 (0x82612c60): kHideEditor moves mReplay off screen (_y 1100), kShowEditor brings it back.
#[test]
#[ignore = "needs SKATE_REPLAY_UI_ROOT (the prepared replay-ui)"]
fn l3_hides_editor() {
    let mut editor = Editor::new(false);
    let mut screen = editor_screen(&editor);
    set_state(&mut screen, 0).unwrap();
    let shown_y = number(&screen, "mReplay", "_y");
    for (expected, y) in [(1003, 1100.), (1002, shown_y)] {
        let outs = editor.key(Pad::L3);
        assert_eq!(outs, [Out::State(expected)]);
        set_state(&mut screen, expected).unwrap();
        assert_eq!(number(&screen, "mReplay", "_y"), y);
    }
    assert!(!editor.hidden);
    // Off the camera edits (CanDisplayShowHide false) L3 does nothing.
    editor.state = State::Camera(CameraKind::Follow);
    assert!(editor.key(Pad::L3).is_empty());
}

/// Start opens replay_help2 on the editor state's help mode; A pages, B closes.
#[test]
#[ignore = "needs SKATE_REPLAY_UI_ROOT (the prepared replay-ui)"]
fn start_opens_help2() {
    let mut editor = Editor::new(false);
    assert_eq!(editor.key(Pad::Start), [Out::Sound(crate::replay::editor::sound::HELP), Out::Help(true)]);
    assert_eq!(help_mode(editor.movie_state()), 0, "over Instant Replay");
    let answers = answers(&editor, 30., &|id| id.to_owned());
    let mut help = Screen::load(&ui_root().join("runtime/replay_help2.json"), &answers).unwrap();
    for _ in 0..10 {
        help.advance().unwrap();
    }
    assert!(texts(&help).contains("ID_REPLAYEDITOR_HELP_TEXT_INSTANTREPLAY1"));
    assert!(texts(&help).contains("ID_COMMON_NEXT"), "two pages: retail's A Next");
    assert_eq!(next_help_page(&mut help, 0, 0).unwrap(), 1);
    assert!(texts(&help).contains("ID_REPLAYEDITOR_HELP_TEXT_INSTANTREPLAY2"), "A: the next page");
    // Item 16: A past retail's pages shows the Video Tools page, then wraps to the first.
    assert_eq!(next_help_page(&mut help, 0, 1).unwrap(), 2);
    // Bug 10-17 #31: Instant Replay's page has no key tools (its right stick orbits); Main Edit's has them.
    assert!(texts(&help).iter().any(|t| t.contains("Camera Path")));
    assert!(!texts(&help).iter().any(|t| t.contains("Paste Key")));
    assert_eq!(next_help_page(&mut help, 0, 2).unwrap(), 0);
    assert!(texts(&help).contains("ID_REPLAYEDITOR_HELP_TEXT_INSTANTREPLAY1"));
    assert!(!texts(&help).iter().any(|t| t.contains("Camera Path")));
    assert_eq!(editor.key(Pad::B), [Out::Help(false)]);
    assert_eq!(editor.state, State::Scrub);
    // Over the camera edits the help follows the camera.
    editor.state = State::Camera(CameraKind::AdvTripod);
    editor.key(Pad::Start);
    assert_eq!(help_mode(editor.movie_state()), 8);
    // A one-page help (the Follow camera edit, mode 3) gets an A Next item for the extra page and comes back to
    // retail's page with the item kept.
    let mut editor = Editor::new(false);
    editor.state = State::Camera(CameraKind::Follow);
    editor.key(Pad::Start);
    let mode = help_mode(editor.movie_state());
    assert_eq!((mode, retail_help_pages(mode)), (3, 1));
    let answers = super::answers(&editor, 30., &|id| id.to_owned());
    let mut help = Screen::load(&ui_root().join("runtime/replay_help2.json"), &answers).unwrap();
    for _ in 0..10 {
        help.advance().unwrap();
    }
    // button_menu's item count (ClearItems keeps the clips, so texts alone can't tell).
    let items = |help: &Screen| number(help, "mHelp.panelHelp.buttonHelp", "mItemCount");
    assert_eq!(items(&help), 1., "retail: one page, B only");
    let first = texts(&help);
    add_next(&mut help).unwrap();
    for _ in 0..30 {
        help.advance().unwrap();
    }
    assert_eq!(items(&help), 2., "the A Next item, not rebuilt away by the movie");
    assert_eq!(next_help_page(&mut help, mode, 0).unwrap(), 1);
    assert!(texts(&help).iter().any(|t| t.contains("Reset Roll")));
    assert!(!texts(&help).iter().any(|t| t.contains("Key Ease")), "a basic camera has no keys");
    assert_eq!(next_help_page(&mut help, mode, 1).unwrap(), 0);
    assert_eq!(items(&help), 2., "the A Next item again");
    assert!(first.is_subset(&texts(&help)), "retail's page back");
}

/// Item 16: the camera edit's zoom readout carries the roll and an Adv key's ease (only when they leave retail's
/// defaults); the Effects header the key ease.
#[test]
fn zoom_readout_carries_roll_and_ease() {
    let localize = |id: &str| if id == "ID_REPLAYEDITOR_ZOOM" { "Zoom {0:d} mm".to_owned() } else { id.to_owned() };
    let get = |e: &Editor, key: &str| answers(e, 30., &localize).into_iter().find(|(k, _)| *k == key).unwrap().1;
    let mut editor = Editor::new(false);
    editor.state = State::Camera(CameraKind::AdvFree);
    editor.shot = Shot::new(CameraKind::AdvFree);
    editor.pending_ease = Ease::Smooth;
    assert_eq!(get(&editor, "FEReplayMenu.GetCameraZoomString"), json!("Zoom 24 mm"));
    editor.shot.roll = -12.4;
    editor.pending_ease = Ease::Linear;
    assert_eq!(get(&editor, "FEReplayMenu.GetCameraZoomString"), json!("Zoom 24 mm  Roll -12\u{b0}  Linear"));
    editor.state = State::Effects;
    editor.channel = Channel::Speed;
    assert_eq!(get(&editor, "FEReplayMenu.GetModeText"), json!("#ID_REPLAYEDITOR_CHANNEL_SPEED (Linear)"));
    editor.pending_ease = Ease::Hold;
    assert_eq!(get(&editor, "FEReplayMenu.GetModeText"), json!("ID_REPLAYEDITOR_CHANNEL_SPEED"));
}

fn recorded(frames: usize) -> Replay {
    let mut replay = Replay::default();
    for i in 0..frames {
        let pose = Snapshot { root: Transform::from_xyz(i as f32, 0., 0.), bones: Vec::new(), camera: Transform::IDENTITY, fov: 1. };
        replay.record(pose, 1. / 60., false);
    }
    replay
}

/// Not retail, by request (user 10-27: "i want to be able to use the replay editor in freeskate"): retail's online
/// editor is state 20, view and photo only (0x82613988 / 0x82614f38); ours opens the whole editor in online free
/// skate, Back included (retail's own type-22 Back gate), and the session's simulation keeps running under it
/// (review Focus 5).
#[test]
fn online_free_skate_opens_the_whole_editor() {
    let mut replay = recorded(200);
    assert!(replay.can_open_from_back(), "Back opens it online too");
    let mut hud = ReplayHud::default();
    assert!(open(&mut hud, &mut replay, None, Path::new("."), true, None));
    let editor = hud.editor.as_mut().unwrap();
    assert_eq!((editor.state.clone(), editor.movie_state()), (State::Scrub, 0));
    assert_eq!(editor.key(Pad::A).last(), Some(&Out::State(9)), "A edits the timeline");
    assert!(editor.menu_rows(30.).contains(&MenuItem::Upload), "Render Video online too");
    assert!(replay.active && replay.online);
    let mut world = World::new();
    world.insert_resource(replay);
    use bevy::ecs::system::RunSystemOnce;
    assert!(world.run_system_once(super::super::live).unwrap(), "the session's simulation is not gated");
}

/// Review Focus 4: an almost empty recording opens without a zero-length division.
#[test]
fn opening_with_a_few_frames_stays_finite() {
    let mut replay = recorded(2);
    let mut hud = ReplayHud::default();
    assert!(open(&mut hud, &mut replay, None, Path::new("."), false, None));
    let editor = hud.editor.as_ref().unwrap();
    for (key, value) in answers(editor, replay.position().1 as f32, &|id| id.to_owned()) {
        if let Some(n) = value.as_f64() {
            assert!(n.is_finite(), "{key} = {n}");
        }
    }
    replay.seek_fraction(0.5);
    assert!(replay.sample().is_some());
    let mut empty = Replay::default();
    assert!(!open(&mut ReplayHud::default(), &mut empty, None, Path::new("."), false, None), "nothing recorded");
}

/// Review Focus 1: the Skate 2 highlight reel's scripted replay never opens the editor or its HUD.
#[test]
fn scripted_replay_shows_no_editor() {
    let mut replay = recorded(100);
    assert!(replay.script(replay.now()));
    assert!(!replay.can_open_from_back());
    let hud = ReplayHud::default();
    assert!(!hud.open());
}

#[test]
fn encoded_length_and_zoom_read_like_retail() {
    let editor = Editor::new(false);
    let localize = |id: &str| match id {
        "ID_REPLAYEDITOR_ZOOM" => "Zoom {0:d} mm".to_owned(),
        "ID_REPLAY_EDITOR_LENGTH" => "Estimated Video Length: {0:time}".to_owned(),
        other => other.to_owned(),
    };
    let answers = answers(&editor, 75., &localize);
    let get = |k: &str| answers.iter().find(|(key, _)| *key == k).map(|(_, v)| v.clone()).unwrap();
    assert_eq!(get("FEReplayMenu.GetEncodedLength"), json!("Estimated Video Length: 1:15"));
    assert_eq!(get("FEReplayMenu.IsEncodedLengthValid"), json!(false), "over 60 s is red");
    assert_eq!(get("FEReplayMenu.GetCameraZoomString"), json!("Zoom 24 mm"));
    assert_eq!(get("FEReplayMenu.GetFrameCount"), json!(2250));
}

#[test]
fn photo_path_in_pictures_subfolder() {
    let pictures = file::folder(Folder::Pictures).expect("a Pictures folder");
    assert!(pictures.ends_with("Skate 3 Remaster"));
    let path = photo_path(&pictures, "San Vanelona", 0);
    // Local time (review M11): the stamp file.rs's stamps_are_civil checks.
    assert_eq!(path, pictures.join(format!("San_Vanelona-{}.png", file::stamp(0))));
}

/// Take Photo puts the screens aside for the capture and back after (retail hides the HUD, 0x82615560(1)).
#[test]
fn photo_hides_hud() {
    let mut editor = Editor::new(false);
    editor.key(Pad::X);
    assert_eq!(editor.state, State::Menu);
    let outs = editor.pick(Some(MenuItem::TakePhoto));
    assert_eq!(outs, [Out::ClosePopup, Out::Popup(PopupKind::SavePhoto)]);
    assert_eq!(editor.answer(1), [Out::ClosePopup, Out::Photo]);
    let mut hud = ReplayHud::default();
    let mut replay = recorded(10);
    hud.editor = Some(editor);
    apply(&mut hud, &mut replay, None, Path::new("."), vec![Out::Photo], ("m", None));
    assert_eq!(hud.photo, Some(2), "two frames: hide, then capture");
}

/// CanSaveMedia (0x825ae500) gates Take Photo on a signed-in profile; PC has none (ruling): always offered.
#[test]
fn photo_hidden_when_retail_hides_it() {
    let editor = Editor::new(false);
    let answers = answers(&editor, 10., &|id| id.to_owned());
    assert!(answers.contains(&("FEReplayMenu.CanSaveMedia", json!(true))));
    // Online it is on state 20's X menu.
    let mut online = Editor::new(true);
    online.key(Pad::X);
    assert_eq!(online.pick(Some(MenuItem::TakePhoto)), [Out::ClosePopup, Out::Popup(PopupKind::SavePhoto)]);
}

/// A temp asset root holding the prepared popup movie, for systems that open sk8popup from `Config::asset_root`.
fn asset_root_with_popup(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("s3r-replay-hud-{name}-{}", std::process::id()));
    let runtime = root.join(FOLDER).join("runtime");
    std::fs::create_dir_all(&runtime).unwrap();
    std::fs::copy(ui_root().join("runtime/sk8popup.json"), runtime.join("sk8popup.json")).unwrap();
    root
}

fn hud_world(asset_root: PathBuf) -> World {
    let mut world = World::new();
    world.insert_resource(Config {
        asset_root,
        verification_capture: None,
        hero_clip: None,
        skate2_physics_dir: None,
        physics_override: None,
        map: None,
        map_path: None,
        difficulty: Default::default(),
        check_assets: false,
        start_paused: false,
        multiplayer: Default::default(),
        map_fingerprint: 0,
        teleport: None,
    });
    world.insert_resource(recorded(200));
    world.init_resource::<ReplayHud>();
    world.insert_resource(crate::input::ControllerInput::default());
    world.init_resource::<ButtonInput<KeyCode>>();
    world.init_resource::<crate::customiser::Navigation>();
    world.init_resource::<bevy::ecs::message::Messages<FrontEndSoundRequest>>();
    world
}

/// One run of `interact` with `keys` just pressed.
fn press(world: &mut World, keys: &[KeyCode]) {
    let mut input = world.resource_mut::<ButtonInput<KeyCode>>();
    input.reset_all();
    for key in keys {
        input.press(*key);
    }
    world.run_system_once(interact).unwrap();
}

/// Review I6: with a lesson or challenge running, the pause menu's Replay asks ID_REPLAY_QUIT_CONF and Back does
/// nothing (Stage 0 §5's free-skate gate); Yes abandons the lesson and opens the editor. Runs the real `interact`.
#[test]
#[ignore = "needs SKATE_REPLAY_UI_ROOT (the prepared replay-ui)"]
fn a_running_challenge_asks_before_the_editor() {
    let root = asset_root_with_popup("quit-conf");
    let mut world = hud_world(root.clone());
    world.insert_resource(crate::challenge::LessonMenu { active: true, request: None });
    // Back: blocked.
    press(&mut world, &[KeyCode::F6]);
    assert!(!world.resource::<ReplayHud>().open() && world.resource::<ReplayHud>().popup.is_none());
    // The pause menu's Replay row: the popup, not the editor.
    world.resource_mut::<ReplayHud>().request = Some(false);
    press(&mut world, &[]);
    {
        let hud = world.resource::<ReplayHud>();
        assert!(!hud.open(), "the editor waits for the answer");
        assert!(matches!(hud.popup, Some((Ask::QuitMode(false), _))), "ID_REPLAY_QUIT_CONF");
        assert!(!world.resource::<Replay>().active);
    }
    // No / Yes: the left stick down to Yes (customiser::Navigation's stick bit), then A.
    world.resource_mut::<crate::customiser::Navigation>().pressed = DOWN;
    press(&mut world, &[]);
    press(&mut world, &[KeyCode::Enter]);
    assert!(world.resource::<ReplayHud>().open(), "Yes opens the editor");
    assert_eq!(world.resource::<crate::challenge::LessonMenu>().request, Some(crate::challenge::LessonRequest::Abandon));
    // A match and Party Play end through their own paths.
    let mut m = crate::match_core::Match { mode: Some(crate::match_core::Mode::DeathRace), ..Default::default() };
    let mut party = crate::party_play::PartyPlay::default();
    party.active = true;
    assert!(blocked(None, Some(&m), Some(&party)));
    end_modes(None, Some(&mut m), Some(&mut party));
    assert!(m.mode.is_none());
    assert_eq!(party.confirmed, Some(crate::party_play::Ask::Exit));
    // Free skate: Back opens it at once.
    let mut world = hud_world(root.clone());
    world.insert_resource(crate::challenge::LessonMenu::default());
    press(&mut world, &[KeyCode::F6]);
    assert!(world.resource::<ReplayHud>().open());
    let _ = std::fs::remove_dir_all(&root);
}

/// Review I3: skate.Reel > My Replays lists every save (61 here, one past the save limit), reached with the
/// movie's own paging; A on the 61st picks it, Y offers its delete. The listing runs off the main thread.
#[test]
#[ignore = "needs SKATE_REPLAY_UI_ROOT (the prepared replay-ui)"]
fn reel_lists_every_save_and_reaches_the_61st() {
    let dir = std::env::temp_dir().join(format!("s3r-replay-reel-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let replay = recorded(200);
    let first = file::save(&dir, &file::Header::new("San Vanelona", None, replay.frames()), replay.frames(), &Editor::new(false)).unwrap();
    let extension = first.extension().unwrap().to_string_lossy().into_owned();
    for i in 0..60 {
        std::fs::copy(&first, dir.join(format!("copy-{i:02}.{extension}"))).unwrap();
    }
    std::fs::write(dir.join(format!("broken.{extension}")), b"not a replay").unwrap();
    let mut reel = Reel::start(Some(dir.clone()));
    let mut screen = Screen::load(&ui_root().join("runtime/browser.json"), &[]).unwrap();
    reel.boot(&mut screen).unwrap();
    let started = Instant::now();
    while !reel.poll() {
        assert!(started.elapsed() < Duration::from_secs(10), "the listing never landed");
        std::thread::sleep(Duration::from_millis(5));
    }
    reel.refresh(&mut screen).unwrap();
    for _ in 0..10 {
        screen.advance().unwrap();
    }
    assert_eq!(reel.saves.as_ref().unwrap().len(), 62, "61 saves and the damaged file");
    let shown = texts(&screen);
    assert!(shown.iter().any(|t| t.contains("San Vanelona")), "the selected save's map: {shown:?}");
    // The 61st save (index 60), past the first window of 8.
    for _ in 0..60 {
        reel.step(&mut screen, true).unwrap();
    }
    assert_eq!(reel.index, 60);
    // The damaged file is listed (by its file time) with no header: A on it is the damaged-save popup.
    assert!(reel.saves.as_ref().unwrap().iter().any(|(_, h)| h.is_none()));
    // The movie hands back the absolute index (more than 8 items): the 61st entry, header and all.
    let sixty_first = reel.saves.as_ref().unwrap()[60].clone();
    assert_eq!(reel.press(&mut screen, true).unwrap(), Some(Pick::Select(sixty_first.0, sixty_first.1)));
    match reel.press(&mut screen, false).unwrap() {
        Some(Pick::Delete(path)) => {
            reel.remove(&path);
            assert_eq!(reel.saves.as_ref().unwrap().len(), 61);
        }
        other => panic!("Y on the 61st: {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Bug #33: skate.Reel's footage screen fills its tab strips as retail's video browser does (My Footage / Community
/// over Saved Replays / Shared Videos, plain text tabs), so no bare tab stubs overlap the panel.
#[test]
#[ignore = "needs SKATE_REPLAY_UI_ROOT (the prepared replay-ui)"]
fn reel_fills_retails_tab_strips() {
    let reel = Reel::start(None);
    let mut screen = Screen::load(&ui_root().join("runtime/browser.json"), &[]).unwrap();
    reel.boot(&mut screen).unwrap();
    for _ in 0..30 {
        screen.advance().unwrap();
    }
    for strip in ["topTabs", "subTabs"] {
        let path = format!("mCommonBrowser.panelTabs.{strip}");
        assert_eq!(number(&screen, &path, "numTabs"), 2., "{strip}: SetupFilterTabs never ran");
        assert_eq!(screen.clip(&path).map(|id| screen.vm.get(id, "bIconTabs")), Some(Value::Bool(false)), "{strip}");
        for tab in ["tab0", "tab1"] {
            assert!(screen.clip(&format!("{path}.mTabs.{tab}")).is_some(), "{strip}.{tab} attached");
        }
    }
    let shown = texts(&screen);
    for id in [
        "ID_COMMON_BROWSER_SKATE_REEL_VIDEO",
        "ID_COMMON_BROWSER_MY_FOOTAGE",
        "ID_COMMON_BROWSER_COMMUNITY",
        "ID_COMMON_BROWSER_SAVED_VIDEOS",
        "ID_COMMON_BROWSER_SHARED_VIDEOS",
    ] {
        assert!(shown.contains(id), "{id} not on screen: {shown:?}");
    }
}

#[test]
fn export_settings_come_from_the_tags_rows() {
    let answers = tags_answers();
    let get = |k: &str| answers.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone());
    assert_eq!(get("Tags.GetCategoryHALName(0)"), Some(json!("Resolution")));
    assert_eq!(get("Tags.GetTagHALName(0,3)"), Some(json!("2160p")));
    assert_eq!(get("Tags.GetNumTagsInCategory(1)"), Some(json!(2)));
    let settings = export::ExportSettings::from_tags(1, 1, 0, 2);
    assert_eq!((settings.width, settings.height, settings.fps, settings.watermark), (1920, 1080, 60, export::Watermark::Pro));
}

/// Bug 10-17 #31 audit: the VIDEO TOOLS page lists only what the mode handles (no right-stick key tools in Instant
/// Replay: the right stick orbits there), in the active pad's button names.
#[test]
fn video_tools_page_matches_the_mode_and_the_pad() {
    let text = |mode, ps| extra_help(mode, ps);
    assert!(!text(0, false).contains("Right Stick") && text(0, false).contains("Letterbox"));
    assert!(text(1, false).contains("Right Stick Up: Copy Key"));
    let ps = text(7, true);
    assert!(ps.contains("L1 / R1 (hold): Roll") && ps.contains("Triangle: Key Ease") && ps.contains("Square: Copy Camera") && ps.contains("Create: Paste Camera"), "{ps}");
    assert!(!ps.contains("LB") && !ps.contains("Back:"), "{ps}");
    assert!(text(7, false).contains("LB / RB (hold): Roll") && text(7, false).contains("Back: Paste Camera"));
}

/// Bug 10-27 (a player's report; every frame of an export on this PC was black too, while Media Foundation wrote a
/// test pattern back intact): the export camera came out of the cloner without Camera3d. Bevy's opt-out cloner also
/// denies whatever requires a denied component; Camera requires RenderTarget and Camera3d requires Camera, so denying
/// RenderTarget dropped both, and no 3D pass drew into the export's image. The copy must be a whole 3D camera.
#[test]
fn the_export_camera_is_a_whole_copy_of_the_gameplay_camera() {
    use bevy::{camera::RenderTarget, core_pipeline::tonemapping::Tonemapping, render::view::Hdr};
    let mut world = World::new();
    let scene: Handle<Image> = bevy::asset::uuid_handle!("8c1d4a52-6b3e-4f70-9d21-3e5f7a9b0c11");
    let image: Handle<Image> = bevy::asset::uuid_handle!("8c1d4a52-6b3e-4f70-9d21-3e5f7a9b0c12");
    let gameplay = world
        .spawn((
            crate::camera::GameplayCamera,
            Camera3d::default(),
            Camera { order: -1, ..default() },
            RenderTarget::Image(scene.into()),
            Hdr,
            Tonemapping::None,
            crate::retail_render::RetailTone::default(),
            Msaa::Sample4,
        ))
        .id();
    // Bevy's rule that broke it: a denied RenderTarget takes Camera and Camera3d with it.
    let cut = world
        .commands()
        .entity(gameplay)
        .clone_and_spawn_with_opt_out(|b| {
            b.deny::<RenderTarget>();
        })
        .id();
    world.flush();
    assert!(!world.entity(cut).contains::<Camera3d>() && !world.entity(cut).contains::<Camera>());
    world.despawn(cut);
    spawn_export_camera(&mut world.commands(), gameplay, image.clone());
    world.flush();
    let copy = world.query_filtered::<Entity, With<ExportCamera>>().single(&world).unwrap();
    let copy = world.entity(copy);
    assert!(copy.contains::<Camera3d>(), "a 3D camera: the 3D passes draw into it");
    assert!(copy.contains::<Hdr>() && copy.contains::<crate::retail_render::RetailTone>(), "the gameplay camera's tone");
    assert_eq!((copy.get::<Tonemapping>(), copy.get::<Msaa>()), (Some(&Tonemapping::None), Some(&Msaa::Sample4)));
    assert!(!copy.contains::<crate::camera::GameplayCamera>(), "gameplay systems leave it alone");
    assert!(matches!(copy.get::<RenderTarget>(), Some(RenderTarget::Image(target)) if target.handle == image));
    assert_eq!(copy.get::<Camera>().map(|c| (c.order, c.is_active)), Some((-2, true)));
}

/// Bug 10-27 end to end: the export's copy of a gameplay camera (set up as camera.rs and graphics_menu.rs set it up
/// on a retail map: HDR, no Bevy tonemapping, the retail tone marker, 4x MSAA, the depth prepass) draws the red cube
/// the gameplay camera draws, read back the way the export reads its frames. Headless; needs a GPU adapter.
#[test]
fn the_export_camera_draws_what_the_gameplay_camera_sees() {
    use bevy::{
        app::AppExit,
        camera::RenderTarget,
        core_pipeline::{prepass::DepthPrepass, tonemapping::Tonemapping},
        render::{
            RenderApp, RenderPlugin,
            pipelined_rendering::PipelinedRenderingPlugin,
            render_resource::{PipelineCache, TextureFormat, TextureUsages},
            settings::{Backends, RenderCreation, WgpuSettings},
            view::Hdr,
        },
        window::{ExitCondition, WindowPlugin},
        winit::WinitPlugin,
    };
    let mut app = App::new();
    app.add_plugins(crate::app::without_dlss(
        DefaultPlugins
            .set(WindowPlugin { primary_window: None, exit_condition: ExitCondition::DontExit, close_when_requested: false, ..default() })
            .set(RenderPlugin {
                render_creation: RenderCreation::Automatic(WgpuSettings { backends: Some(Backends::VULKAN | Backends::DX12), ..default() }),
                ..default()
            })
            .disable::<WinitPlugin>()
            .disable::<bevy::log::LogPlugin>()
            .disable::<PipelinedRenderingPlugin>()
            .disable::<bevy::gilrs::GilrsPlugin>(),
    ));
    app.finish();
    app.cleanup();
    // Frames until every render pipeline the cameras asked for has compiled.
    let settle = |app: &mut App| {
        for frame in 0.. {
            app.update();
            let waiting = app.sub_app(RenderApp).world().resource::<PipelineCache>().waiting_pipelines().next().is_some();
            if frame >= 4 && !waiting {
                break;
            }
            assert!(frame < 2000, "pipelines still compiling");
        }
    };
    let (w, h) = (160, 90);
    let (scene, export) = {
        let mut images = app.world_mut().resource_mut::<Assets<Image>>();
        let mut target = || images.add(Image::new_target_texture(w, h, TextureFormat::Rgba8UnormSrgb, None));
        (target(), target())
    };
    let cube = app.world_mut().resource_mut::<Assets<Mesh>>().add(Cuboid::new(2., 2., 2.));
    let red = app.world_mut().resource_mut::<Assets<StandardMaterial>>().add(StandardMaterial {
        base_color: Color::srgb(1., 0., 0.),
        unlit: true,
        ..default()
    });
    app.world_mut().spawn((Mesh3d(cube), MeshMaterial3d(red)));
    let gameplay = app
        .world_mut()
        .spawn((
            crate::camera::GameplayCamera,
            Camera3d { depth_texture_usages: (TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING).into(), ..default() },
            Camera { order: -1, ..default() },
            RenderTarget::Image(scene.clone().into()),
            Hdr,
            Tonemapping::None,
            crate::retail_render::RetailTone::default(),
            Msaa::Sample4,
            DepthPrepass,
            Transform::from_xyz(0., 0., 6.).looking_at(Vec3::ZERO, Vec3::Y),
        ))
        .id();
    settle(&mut app);
    // The export starts.
    spawn_export_camera(&mut app.world_mut().commands(), gameplay, export.clone());
    app.world_mut().flush();
    settle(&mut app);
    let seen: Arc<Mutex<Vec<(usize, Image)>>> = Arc::default();
    for (n, image) in [&scene, &export].into_iter().enumerate() {
        let seen = seen.clone();
        app.world_mut().spawn(Screenshot::image(image.clone())).observe(move |captured: On<ScreenshotCaptured>| {
            seen.lock().unwrap().push((n, captured.image.clone()));
        });
    }
    for _ in 0..600 {
        app.update();
        if seen.lock().unwrap().len() == 2 {
            break;
        }
    }
    app.world_mut().write_message(AppExit::Success);
    let seen = seen.lock().unwrap();
    let middle = |n: usize| {
        let (_, image) = seen.iter().find(|(i, _)| *i == n).expect("both read back");
        let at = ((h / 2 * w + w / 2) * 4) as usize;
        image.data.as_ref().expect("pixels")[at..at + 3].to_vec()
    };
    let (drawn, exported) = (middle(0), middle(1));
    assert!(drawn[0] > 200 && drawn[1] < 40 && drawn[2] < 40, "the gameplay camera draws the cube: {drawn:?}");
    assert!(drawn.iter().zip(&exported).all(|(a, b)| a.abs_diff(*b) <= 2), "the export camera draws it too: {exported:?}, gameplay {drawn:?}");
}

/// Render speed (10-27): an export asks its camera for up to AHEAD frames before the encoder has written any, one per
/// ask, and they go on in frame order whatever order their captures land in; a capture nobody asked for is dropped.
#[test]
fn export_frames_run_ahead_and_go_on_in_order() {
    let now = Instant::now();
    let mut frames = Frames::default();
    let asked: Vec<u32> = std::iter::from_fn(|| frames.ask(100, 0, now)).collect();
    assert_eq!(asked, (0..AHEAD).collect::<Vec<_>>(), "AHEAD out before the encoder writes one");
    assert_eq!(frames.ask(100, 1, now), Some(AHEAD), "each frame written frees one");
    frames.land(1, 'b');
    frames.land(0, 'a');
    frames.land(50, 'x');
    assert!(frames.has(0) && frames.has(1) && !frames.has(2));
    assert_eq!((frames.take(0), frames.take(1), frames.take(50)), (Some('a'), Some('b'), None), "never asked for");
    let mut short = Frames::<char>::default();
    assert_eq!(std::iter::from_fn(|| short.ask(3, 0, now)).count(), 3, "no frame past the last");
}

/// Review I1 kept: a frame whose capture never lands fails the export after CAPTURE_TIMEOUT, even while later frames
/// land; once it lands, nothing is overdue.
#[test]
fn an_export_frame_that_never_lands_times_out() {
    let start = Instant::now();
    let mut frames = Frames::<char>::default();
    frames.ask(10, 0, start);
    frames.ask(10, 0, start + Duration::from_secs(5));
    assert!(!frames.overdue(start + Duration::from_secs(9), CAPTURE_TIMEOUT));
    frames.land(1, 'b');
    assert!(frames.overdue(start + Duration::from_secs(11), CAPTURE_TIMEOUT), "frame 0 is still out");
    frames.land(0, 'a');
    assert!(!frames.overdue(start + Duration::from_secs(11), CAPTURE_TIMEOUT));
}

/// User 10-27 (audit 3a): trimming with LB moved both brackets onto the moved mark, so the [ faced the wrong way and
/// both ends never showed at once. Each bracket is sent alone, at its own mark (retail 0x82611600 / 0x82611694).
#[test]
#[ignore = "needs SKATE_REPLAY_UI_ROOT (the prepared replay-ui)"]
fn each_bracket_sits_on_its_own_mark() {
    let mut e = Editor::new(false);
    let mut screen = editor_screen(&e);
    let mut sent = (-1., -1.);
    send_marks(&mut screen, &e, &mut sent);
    let x = |screen: &Screen, bracket: &str| number(screen, &format!("mReplay.{bracket}"), "_x");
    let (start, end) = (x(&screen, "mReplayIn"), x(&screen, "mReplayOut"));
    assert!((end - start - 817.).abs() < 1., "on open, [ at the start and ] at the end: {start} {end}");
    // The out mark trimmed to the middle, the playhead riding it: the [ stays put.
    (e.out_mark, e.playhead) = (0.5, 0.5);
    send_marks(&mut screen, &e, &mut sent);
    assert_eq!(x(&screen, "mReplayIn"), start, "the [ stays at the start");
    assert!((x(&screen, "mReplayOut") - (start + 0.5 * 817.)).abs() < 1., "the ] at the middle");
    // Then the in mark, the playhead riding it: the ] stays.
    (e.in_mark, e.playhead) = (0.25, 0.25);
    send_marks(&mut screen, &e, &mut sent);
    assert!((x(&screen, "mReplayIn") - (start + 0.25 * 817.)).abs() < 1.);
    assert!((x(&screen, "mReplayOut") - (start + 0.5 * 817.)).abs() < 1., "the ] stays at the middle");
}

/// User 10-27 ("upload video should say render video", not retail): the export screen's title, start row and help
/// read as rendering, where its constructor set retail's upload and tag wording.
#[test]
#[ignore = "needs SKATE_REPLAY_UI_ROOT (the prepared replay-ui)"]
fn the_export_screen_says_render_video() {
    let answers = tags_answers();
    let borrowed: Vec<(&str, Json)> = answers.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    let mut tags = Screen::load(&ui_root().join("runtime/skatereel_tags.json"), &borrowed).unwrap();
    let retail = ["ID_SKATEREEL_TAG", "ID_REPLAY_RTW_TITLE", "ID_SKATEREEL_TAGS_HELP"];
    let before = texts(&tags);
    assert!(retail.iter().all(|id| before.contains(*id)), "the constructor's wording: {before:?}");
    render_wording(&mut tags).unwrap();
    let after = texts(&tags);
    assert!(retail.iter().all(|id| !after.contains(*id)), "{after:?}");
    assert!(after.contains(crate::replay::editor::RENDER_VIDEO) && after.contains(RENDER_HELP), "{after:?}");
    assert_eq!(MenuItem::Upload.text(), crate::replay::editor::RENDER_VIDEO, "the X menu's row");
}

/// Not retail, by request (user 10-27: clip a moment in an online game mode without leaving it): Back in an online
/// match saves the replay off the main thread, one at a time, and the event log hears how it went; the file loads
/// back. Too short a recording (under the Back gate's 150 frames) saves nothing.
#[test]
fn an_online_match_quick_save_writes_the_replay() {
    let dir = std::env::temp_dir().join(format!("s3r-quick-save-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let replay = recorded(200);
    let mut hud = ReplayHud::default();
    quick_save(&mut hud, &replay, ("University", None), Some(dir.clone()));
    assert!(hud.quick_saving, "writing");
    quick_save(&mut hud, &replay, ("University", None), Some(dir.clone()));
    let saved = (0..500)
        .find_map(|_| {
            std::thread::sleep(Duration::from_millis(10));
            hud.quick_saved.lock().unwrap().take()
        })
        .expect("written");
    let path = saved.as_ref().expect("saved").clone();
    let files = std::fs::read_dir(&dir).unwrap().filter_map(Result::ok).filter(|e| e.path().extension().is_some_and(|x| x == file::EXTENSION)).count();
    assert_eq!(files, 1, "a second press while it was writing saved nothing");
    assert!(file::load(&path).is_ok(), "it loads back");
    // Review I2: the event log shows a line as online_hud::format makes it (then `literal`): as written, no '#'.
    let shown = |saved| crate::online_hud::format(&Default::default(), quick_saved(saved), "");
    assert_eq!(shown(saved), "Replay saved to My Replays");
    assert_eq!(shown(Err(file::SaveError::SlotsFull)), "Replay not saved: My Replays is full");
    assert_eq!(shown(Err(file::SaveError::Io("disk".into()))), "The replay could not be saved");
    let _ = std::fs::remove_dir_all(&dir);
    let mut hud = ReplayHud::default();
    quick_save(&mut hud, &recorded(10), ("University", None), Some(dir.clone()));
    assert!(!hud.quick_saving && !dir.exists(), "under 150 frames: nothing");
}

/// Review I3: a map move under the open editor (an online host's move, any load) closes it, a render with it, and the
/// recording starts again on the new map: its old frames would play in the old map's places.
#[test]
fn a_map_move_closes_the_editor_and_starts_the_recording_again() {
    let mut world = hud_world(PathBuf::from("."));
    let (mut hud, mut replay) = (ReplayHud::default(), recorded(200));
    assert!(open(&mut hud, &mut replay, None, Path::new("."), false, None));
    world.insert_resource(hud);
    world.insert_resource(replay);
    press(&mut world, &[]);
    assert!(world.resource::<ReplayHud>().open(), "nothing moving: it stays");
    let mut transition = crate::map_transition::MapTransition::default();
    transition.request(crate::map_library::Entry { label: "University".into(), path: None });
    world.insert_resource(transition);
    press(&mut world, &[]);
    let replay = world.resource::<Replay>();
    assert!(!world.resource::<ReplayHud>().open() && !replay.active, "the move closed it");
    assert_eq!(replay.frames().len(), 0, "the old map's frames are gone");
}
