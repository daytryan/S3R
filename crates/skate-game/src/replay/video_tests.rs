//! Item 16 (2026-10-11): the video-making extensions and their retail-default guard.
//!
//! `retail_default_snapshot`: a retail-only timeline (a version 2 save's timeline JSON, no extension in it) gives the
//! same camera, live playback, export frames, audio ticks and grade per frame as the code before item 16. The golden
//! was written by the pre-item-16 code (`SKATE_REPLAY_SNAPSHOT_WRITE=1`); never regenerate it to make a change pass.
//! Regenerated once, 10-27 (replay audit: the cameras made retail's): the Follow shot rides the game camera from its
//! key (the save migrated as hud.rs migrates it), so its rows after the key moved; every other row is unchanged.
use super::{
    cameras::{self, Subject},
    editor::Editor,
    export, file,
    grade::Grade,
};
use bevy::prelude::*;
use serde_json::{Value as Json, json};
use std::fmt::Write as _;

const TIMELINE_SECONDS: f32 = 20.;

fn shot(kind: usize, yaw: f32, pitch: f32, distance: f32, height: f32, lens: f32, eye: [f32; 3], look: [f32; 3]) -> Json {
    json!({"camera": {"kind": kind, "yaw": yaw, "pitch": pitch, "distance": distance, "height": height, "lens": lens, "eye": eye, "look": look}})
}
fn key(t: f32, soft: bool, cut: bool, value: Json) -> Json {
    json!({"t": t, "soft": soft, "cut": cut, "value": value})
}

/// A version 2 timeline as the pre-item-16 `file::timeline_json` wrote it: every retail channel used, four clips,
/// Follow / Adv Free with a cut / Tripod / Game, soft speed and brightness keys.
pub(crate) fn retail_timeline_v2() -> Json {
    let int = |v: i32| json!({"int": v});
    json!({
        "in": 0.05, "out": 0.95, "markers": [0.3, 0.55, 0.8],
        "keys": [
            [
                key(0., false, false, shot(0, 30., -10., 0.5, 0.2, 35., [0., 0., 0.], [0., 0., 0.])),
                key(0.3, false, false, shot(6, 0., 0., 0., 0., 24., [1., 2., 3.], [1., 2., 2.])),
                key(0.35, true, false, shot(6, 0., 0., 0., 0., 30., [1., 2., 3.], [1., 2., 2.])),
                key(0.42, true, true, shot(6, 0., 0., 0., 0., 50., [4., 2.5, 3.], [3., 2., 1.])),
                key(0.5, true, false, shot(6, 0., 0., 0., 0., 75., [6., 1., -2.], [5., 1., -3.])),
                key(0.55, false, false, shot(1, 0., 0., 0., 0.3, 40., [3., 2., 1.], [0., 0., 0.])),
                key(0.8, false, false, shot(2, 0., 0., 0., 0., 24., [0., 0., 0.], [0., 0., 0.])),
            ],
            [
                key(0., false, false, int(100)),
                key(0.3, false, false, int(50)),
                key(0.4, true, false, int(20)),
                key(0.55, false, false, int(100)),
                key(0.8, false, false, int(30)),
            ],
            [key(0.55, false, false, int(2)), key(0.8, false, false, int(1))],
            [key(0., false, false, int(40)), key(0.2, true, false, int(-20))],
            [key(0.3, false, false, int(14))],
            [key(0.55, false, false, int(50))],
            [key(0.8, false, false, json!({"tint": [100, 50, 25, 125]}))],
        ],
        "look": [false, false, false],
    })
}

/// The skater's root (heading 2t radians) and the recorded game camera at playhead `t`.
fn frame(t: f32) -> (Transform, Transform) {
    let root = Transform::from_xyz(t * 10., 0.5 * t, t * 3.).with_rotation(Quat::from_rotation_y(t * 2.));
    let recorded = Transform::from_xyz(t * 10., 2., t * 3. - 4.).looking_at(Vec3::new(t * 10., 1., t * 3.), Vec3::Y);
    (root, recorded)
}

/// Every per-frame output of the editor for `editor`, as text (6 decimals).
fn snapshot(editor: &Editor) -> String {
    let mut out = String::new();
    let v = |out: &mut String, values: &[f32]| {
        for x in values {
            let _ = write!(out, " {x:.6}");
        }
        out.push('\n');
    };
    // The camera and grade at 241 playheads.
    for i in 0..=240 {
        let t = i as f32 / 240.;
        let (root, recorded) = frame(t);
        let _ = write!(out, "cam {i}");
        match cameras::evaluate(editor, t, 0., Subject::new(&root, recorded), 1., t * TIMELINE_SECONDS) {
            None => v(&mut out, &[]),
            Some((tr, fov)) => {
                let r = tr.rotation;
                v(&mut out, &[tr.translation.x, tr.translation.y, tr.translation.z, r.x, r.y, r.z, r.w, fov.unwrap_or(-1.)]);
            }
        }
        let g = Grade::at(editor, t);
        let _ = write!(out, "grade {i} {}", editor.speed_at(t));
        v(&mut out, &[g.bcs.x, g.bcs.y, g.bcs.z, g.bcs.w, g.tint.x, g.tint.y, g.tint.z, g.tint.w]);
    }
    // Live playback from the in mark, 60 Hz.
    let mut live = editor.clone();
    live.playhead = live.in_mark;
    live.playing = true;
    let mut steps = 0;
    while live.play(1. / 60., TIMELINE_SECONDS) && steps < 20_000 {
        if steps % 15 == 0 {
            let _ = write!(out, "play {steps}");
            v(&mut out, &[live.playhead]);
        }
        steps += 1;
    }
    let _ = writeln!(out, "play end {steps} {:.6}", live.playhead);
    // The export: frame count and each frame's playhead at 30 and 60 fps, then the 60 Hz audio ticks' playhead and
    // time scale.
    let length = export::encoded_length(editor, TIMELINE_SECONDS);
    let _ = writeln!(out, "encoded {length:.6} {}", editor.encoded_seconds(TIMELINE_SECONDS));
    for fps in export::RATES {
        let frames = export::frame_count(length, fps);
        let _ = writeln!(out, "frames {fps} {frames}");
        for frame in (0..frames).step_by(7) {
            let _ = write!(out, "frame {fps} {frame}");
            v(&mut out, &[export::playhead_for_frame(editor, frame, fps, TIMELINE_SECONDS)]);
        }
    }
    for tick in (0..export::frame_count(length, 60)).step_by(11) {
        let playhead = export::playhead_for_frame(editor, tick, 60, TIMELINE_SECONDS);
        let _ = write!(out, "tick {tick}");
        v(&mut out, &[playhead, editor.speed_at(playhead).max(1) as f32 * 0.01]);
    }
    out
}

fn golden() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tests/fixtures/replay_retail_default.snap")
}

/// Brief part 3: with no extension used, every per-frame output matches the pre-item-16 code.
#[test]
fn retail_default_snapshot() {
    let mut editor = file::editor_from(&retail_timeline_v2()).expect("the v2 timeline loads");
    assert!(editor.legacy_follow, "a v2 timeline's Follow shot waits for its recording");
    cameras::migrate_follow(&mut editor, |t| Some(frame(t)));
    let empty = Editor::new(false);
    let now = format!("== retail timeline ==\n{}== empty timeline ==\n{}", snapshot(&editor), snapshot(&empty));
    if std::env::var_os("SKATE_REPLAY_SNAPSHOT_WRITE").is_some() {
        std::fs::write(golden(), &now).expect("write the golden");
        return;
    }
    let was = std::fs::read_to_string(golden()).expect("the golden exists (written before item 16)");
    let (was, now): (Vec<&str>, Vec<&str>) = (was.lines().collect(), now.lines().collect());
    assert_eq!(was.len(), now.len(), "line count");
    for (a, b) in was.iter().zip(&now) {
        let (wa, wb): (Vec<&str>, Vec<&str>) = (a.split(' ').collect(), b.split(' ').collect());
        assert_eq!(wa.len(), wb.len(), "{a} vs {b}");
        for (x, y) in wa.iter().zip(&wb) {
            match (x.parse::<f64>(), y.parse::<f64>()) {
                (Ok(x), Ok(y)) => assert!((x - y).abs() <= 2e-5, "{a}\n vs\n{b}"),
                _ => assert_eq!(x, y, "{a} vs {b}"),
            }
        }
    }
}
