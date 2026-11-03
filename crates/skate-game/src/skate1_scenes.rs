//! Skate 1's tutorial scenes (P1 `private/challenges-skate1/nis`): the lesson demos, ROP 2001/2002, Shingo's decline,
//! the he_u1_skate01 fly-through and the he_u2_photo01 win, played by the shared [`Cutscene`] on Skate 1's NIS rules
//! (nis-clock.md). skate1_challenge.rs holds a [`ScenePlay`] while one runs: it starts it from the NIS natives (or
//! progression's ROP r=2), ticks it each frame, shows its subtitle and posts the Lua events its [`SceneEnd`] names.
use crate::{challenge::cutscene::Cutscene, replay::Replay};
use bevy::prelude::*;
use serde::Deserialize;
use std::{collections::BTreeMap, path::Path};

/// The scenes' folder under the asset root (old_scenes1.py: tutorial.json, scene/camera, anim1, nis).
pub(crate) const SCENES: &str = "private/challenges-skate1/nis";
/// SCENE_TYPE_CHALLENGE_WIN (S1 enum 0x827bf360, RETAIL).
const CHALLENGE_WIN: u8 = 2;
/// NIS.EnterReplayNIS's frames per second: research9.md 5 (RETAIL, 0x82787638): L = (StopFrame - StartFrame) / 60,
/// fixed (not the replay's recorded rate). The binding's frame numbers are [`Replay::now`] x this.
pub(crate) const REPLAY_FPS: f64 = 60.;
/// The replay scene's shortest record duration (research9.md 5, RETAIL: 1.5 at 0x820d20ac).
const REPLAY_MIN_SECONDS: f64 = 1.5;

#[derive(Deserialize)]
struct Scenes {
    scenes: BTreeMap<String, Info>,
}
#[derive(Deserialize, Default)]
struct Info {
    /// The scene.bin row's type byte (P1 `kind`).
    #[serde(default)]
    kind: u8,
    #[serde(default)]
    subtitles: Vec<Cue>,
}
/// A subtitle line: P1 (old_challenges.py `subtitles1`) already walks the list in file order (research5.md 3: a cue
/// starts at max(its time, the previous start) and shows until the next record), so a lookup is exact.
#[derive(Deserialize, Clone)]
struct Cue {
    start: f32,
    end: f32,
    label: String,
}

/// How a scene ended, for the Lua events the binding posts (nis-clock.md 2, S1 RETAIL).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum SceneEnd {
    /// Poster 0x827cf080: `nischallengenotify` {NotificationID = 1 COMPLETED}, then `niscomplete`. UNTRACED (U1):
    /// the end transition before it; a cut here.
    Complete,
    /// A CHALLENGE_WIN scene at its take end: frozen there, `nischallengenotify` {NotificationID = 2
    /// NIS_FE_NOTIFY_ENTER_PHOTOVIEW} (enum 0x827bfa40); it holds until `NIS.KillNIS` ([`ScenePlay::stop`]).
    PhotoView,
}

/// A running Skate 1 scene.
pub(crate) struct ScenePlay {
    kind: u8,
    /// research10 (RETAIL): a scene the scene DB knows whose type byte isn't CHALLENGE_WIN.
    skippable: bool,
    /// A skip waits for the next update (byte +99, research10).
    skipped: bool,
    cues: Vec<Cue>,
    /// A replay scene's range (recording seconds).
    replay: Option<(f64, f64)>,
    done: bool,
}

impl ScenePlay {
    /// Start `scene` from `root`/tutorial.json. An unknown scene still starts: it ends (Complete) on its first tick.
    pub(crate) fn start(cutscene: &mut Cutscene, root: &Path, scene: &str) -> Self {
        let info = std::fs::read_to_string(root.join("tutorial.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<Scenes>(&text).ok())
            .and_then(|mut scenes| scenes.scenes.remove(scene));
        if info.is_none() {
            warn!("Skate 1 scene {scene}: not in {}", root.display());
        }
        // research10 (RETAIL, scene DB slot 5 0x8277fc40): an id that isn't found is not skippable.
        let skippable = info.as_ref().is_some_and(|i| i.kind != CHALLENGE_WIN);
        let info = info.unwrap_or_default();
        cutscene.start(root, scene);
        Self { kind: info.kind, skippable, skipped: false, cues: info.subtitles, replay: None, done: false }
    }

    /// NIS.EnterReplayNIS(scene, StartFrame, StopFrame): the replay plays StartFrame..StopFrame under the scene's
    /// camera. research9.md 5 (RETAIL, 0x827bfcc0 / 0x82787638): the frames are rounded, L = (stop - start) / 60, the
    /// record duration = max(L, 1.5 s), and it overwrites the take's duration (nis-clock.md 2: he_u2_photo01 is a
    /// byte +6 scene, scene.bin), so the camera runs over the range. The cap to the record duration applies only
    /// while the ICE controller is active (`+10560 > 0`, INFERRED a net session): never in the tutorial.
    /// INFERRED: the playhead is StartFrame + the scene's clock (its TIME_SCALE and FREEZE holds), so the take's end
    /// meets StopFrame. ponytail: with nothing recorded the camera watches the live skater (as Skate 2's highlight).
    pub(crate) fn start_replay(
        cutscene: &mut Cutscene,
        replay: &mut Replay,
        root: &Path,
        scene: &str,
        start_frame: f64,
        stop_frame: f64,
    ) -> Self {
        let (from, to) = (start_frame.round() / REPLAY_FPS, stop_frame.round() / REPLAY_FPS);
        let mut play = Self::start(cutscene, root, scene);
        cutscene.stretch((to - from).max(REPLAY_MIN_SECONDS) as f32);
        if replay.script(from) {
            play.replay = Some((from, to.max(from)));
        }
        play
    }

    /// A frame: a replay scene's playhead follows the scene's clock; the end is returned once. A scene that failed to
    /// load (or was stopped elsewhere) ends at once.
    pub(crate) fn tick(&mut self, cutscene: &Cutscene, replay: &mut Replay) -> Option<SceneEnd> {
        if self.done {
            return None;
        }
        if !cutscene.active() {
            self.done = true;
            return Some(SceneEnd::Complete);
        }
        if let Some((from, to)) = self.replay {
            replay.seek((from + f64::from(cutscene.time())).min(to));
        }
        // research10 (RETAIL): a skip ends the scene on the next update through the natural end's stop path
        // (0x8277eab8(this, 3) -> 0x8277e1d8), so the same completion fires (code 1 / niscomplete: INFERRED).
        // UNTRACED: whether reason 3 picks another exit transition (0x827912d0); the natural end's is used.
        if std::mem::take(&mut self.skipped) {
            self.done = true;
            return Some(SceneEnd::Complete);
        }
        if !cutscene.ended() {
            return None;
        }
        self.done = true;
        Some(if self.kind == CHALLENGE_WIN { SceneEnd::PhotoView } else { SceneEnd::Complete })
    }

    /// A (AptNext) pressed: research10 (RETAIL) skips a loaded, skippable scene once (byte +99); the scene ends on the
    /// next [`ScenePlay::tick`]. The binding checks the rest of retail's gate: FE state != 6 and no net session
    /// (streaming is always ready: the port loads a scene whole). True when the skip took.
    pub(crate) fn skip(&mut self, cutscene: &Cutscene) -> bool {
        let take = self.skippable && !self.done && cutscene.root().is_some();
        self.skipped |= take;
        self.skippable &= !take;
        take
    }

    /// The subtitle label showing now (strings.json key), on the scene's clock (nis-clock.md 3).
    pub(crate) fn subtitle(&self, cutscene: &Cutscene) -> Option<&str> {
        let t = cutscene.time();
        self.cues.iter().find(|c| c.start <= t && t < c.end).map(|c| c.label.as_str())
    }

    /// End the scene (after Complete, on KillNIS, or when the tutorial unloads): actors back, a replay closed.
    pub(crate) fn stop(self, cutscene: &mut Cutscene, replay: &mut Replay) {
        cutscene.stop();
        if self.replay.is_some() && replay.scripted {
            replay.exit();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scenes folder: `demo` (kind 14) on a TIME_SCALE 0.5 camera 3 s long (6 s real) with two subtitle lines;
    /// `win` (kind 2) on a 3 s camera at TIME_SCALE 1.
    fn scenes(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("challenges-skate1").join(format!("s1-scenes-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let camera = |ts: f32| format!(r#"{{"space":1,"samples":[[0,0,0,0,0,0,1,0,1,{ts}],[3,0,0,0,0,0,1,0,1,{ts}]]}}"#);
        std::fs::write(dir.join("half.json"), camera(0.5)).unwrap();
        std::fs::write(dir.join("one.json"), camera(1.)).unwrap();
        std::fs::write(
            dir.join("tutorial.json"),
            r#"{"scenes":{
            "demo":{"kind":14,"origin":[0,0,0],"tracks":[],"camera":"half.json",
              "subtitles":[{"start":0.5,"end":2.0,"label":"9697e270"},{"start":2.0,"end":2.5,"label":"97047576"}]},
            "win":{"kind":2,"origin":[0,0,0],"duration":3,"tracks":[],"camera":"one.json"}}}"#,
        )
        .unwrap();
        dir
    }

    /// nis-clock.md 2 (S1 RETAIL): a scene ends at its camera's end on its clock; the poster 0x827cf080's
    /// nischallengenotify(1) + niscomplete. Review Focus 5: reported once.
    #[test]
    fn a_scene_completes_at_its_cameras_end() {
        let dir = scenes("complete");
        let (mut cutscene, mut replay) = (Cutscene::default(), Replay::default());
        let mut play = ScenePlay::start(&mut cutscene, &dir, "demo");
        assert_eq!(play.tick(&cutscene, &mut replay), None, "not loaded yet");
        cutscene.run_for_test(5.9);
        assert_eq!(play.tick(&cutscene, &mut replay), None);
        cutscene.run_for_test(0.2);
        assert_eq!(play.tick(&cutscene, &mut replay), Some(SceneEnd::Complete));
        assert_eq!(play.tick(&cutscene, &mut replay), None, "the end is reported once");
        play.stop(&mut cutscene, &mut replay);
        assert!(!cutscene.active());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// nis-clock.md 2 (S1 RETAIL): a CHALLENGE_WIN scene (0x827bf360 type 2) freezes at its take end and notifies
    /// NIS_FE_NOTIFY_ENTER_PHOTOVIEW (2); it holds until KillNIS.
    #[test]
    fn a_photo_win_ends_in_the_photo_view_and_holds() {
        let dir = scenes("photo");
        let (mut cutscene, mut replay) = (Cutscene::default(), Replay::default());
        let mut play = ScenePlay::start(&mut cutscene, &dir, "win");
        cutscene.run_for_test(3.1);
        assert_eq!(play.tick(&cutscene, &mut replay), Some(SceneEnd::PhotoView));
        cutscene.run_for_test(5.);
        assert!(cutscene.active(), "held until KillNIS");
        play.stop(&mut cutscene, &mut replay);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Review Focus 1: an unknown scene never hangs the lesson.
    #[test]
    fn a_missing_scene_completes_at_once() {
        let dir = scenes("missing");
        let (mut cutscene, mut replay) = (Cutscene::default(), Replay::default());
        let mut play = ScenePlay::start(&mut cutscene, &dir, "nope");
        cutscene.run_for_test(1. / 60.);
        assert_eq!(play.tick(&cutscene, &mut replay), Some(SceneEnd::Complete));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// nis-clock.md 3 (RETAIL): cues fire on the scene's own (scaled) clock; a line shows until the next record.
    #[test]
    fn subtitles_follow_the_scene_clock() {
        let dir = scenes("subtitles");
        let mut cutscene = Cutscene::default();
        let play = ScenePlay::start(&mut cutscene, &dir, "demo");
        cutscene.run_for_test(0.5); // scene 0.25 s
        assert_eq!(play.subtitle(&cutscene), None);
        cutscene.run_for_test(1.); // 0.75 s
        assert_eq!(play.subtitle(&cutscene), Some("9697e270"));
        cutscene.run_for_test(3.); // 2.25 s
        assert_eq!(play.subtitle(&cutscene), Some("97047576"));
        cutscene.run_for_test(1.); // 2.75 s
        assert_eq!(play.subtitle(&cutscene), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// research10 (RETAIL): A skips a loaded scene whose scene.bin type isn't CHALLENGE_WIN, once; it ends on the next
    /// update as a natural end does. A photo win and an unknown scene can't be skipped.
    #[test]
    fn a_skips_a_scene_but_not_a_photo_win() {
        let dir = scenes("skip");
        let (mut cutscene, mut replay) = (Cutscene::default(), Replay::default());
        let mut play = ScenePlay::start(&mut cutscene, &dir, "demo");
        assert!(!play.skip(&cutscene), "not loaded yet");
        cutscene.run_for_test(1.);
        assert!(play.skip(&cutscene));
        assert!(!play.skip(&cutscene), "once");
        assert_eq!(play.tick(&cutscene, &mut replay), Some(SceneEnd::Complete));
        assert_eq!(play.tick(&cutscene, &mut replay), None);
        play.stop(&mut cutscene, &mut replay);
        let mut win = ScenePlay::start(&mut cutscene, &dir, "win");
        cutscene.run_for_test(1.);
        assert!(!win.skip(&cutscene), "kind 2 is not skippable");
        assert_eq!(win.tick(&cutscene, &mut replay), None);
        win.stop(&mut cutscene, &mut replay);
        let mut nope = ScenePlay::start(&mut cutscene, &dir, "nope");
        assert!(!nope.skip(&cutscene), "an unknown id is not skippable");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Ten seconds of recording at 60 Hz, the root at x = frame.
    fn recorded() -> Replay {
        let mut replay = Replay::default();
        for i in 0..600 {
            let pose = crate::presentation::Snapshot {
                root: Transform::from_xyz(i as f32, 0., 0.),
                bones: vec![Mat4::IDENTITY],
                camera: Transform::IDENTITY,
                fov: 1.,
            };
            replay.record(pose, 1. / 60., false);
        }
        replay
    }

    /// research9.md 5 (RETAIL): EnterReplayNIS frames are 60 a second; the take is stretched over the range (here 2 s
    /// from the camera's 3 s), the replay moving with the scene's clock up to StopFrame. Review Focus 2: KillNIS
    /// mid-scene closes the replay.
    #[test]
    fn a_replay_scene_moves_the_replay_with_its_clock() {
        let dir = scenes("replay");
        let (mut cutscene, mut replay) = (Cutscene::default(), recorded());
        let mut play = ScenePlay::start_replay(&mut cutscene, &mut replay, &dir, "win", 120., 240.);
        assert!(replay.scripted);
        for _ in 0..30 {
            cutscene.run_for_test(1. / 60.);
            assert_eq!(play.tick(&cutscene, &mut replay), None);
        }
        assert!((replay.position().0 - 2.5).abs() < 0.02, "half a second in: {:?}", replay.position());
        cutscene.run_for_test(1.4);
        assert_eq!(play.tick(&cutscene, &mut replay), None, "1.9 s of a 2 s take");
        cutscene.run_for_test(0.2);
        assert_eq!(play.tick(&cutscene, &mut replay), Some(SceneEnd::PhotoView));
        assert!((replay.position().0 - 4.).abs() < 0.02, "at StopFrame: {:?}", replay.position());
        play.stop(&mut cutscene, &mut replay);
        assert!(!replay.scripted && !replay.active, "KillNIS closes the replay");
        let play = ScenePlay::start_replay(&mut cutscene, &mut replay, &dir, "win", 120., 240.);
        cutscene.run_for_test(0.5);
        play.stop(&mut cutscene, &mut replay);
        cutscene.run_for_test(1. / 60.);
        assert!(!replay.scripted && !replay.active && !cutscene.active(), "KillNIS mid-scene closes the replay too");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// research9.md 5 (RETAIL): the record duration is at least 1.5 s, however short the range.
    #[test]
    fn a_short_replay_range_still_runs_a_second_and_a_half() {
        let dir = scenes("short");
        let (mut cutscene, mut replay) = (Cutscene::default(), recorded());
        let mut play = ScenePlay::start_replay(&mut cutscene, &mut replay, &dir, "win", 120., 150.);
        cutscene.run_for_test(1.4);
        assert_eq!(play.tick(&cutscene, &mut replay), None);
        assert!((replay.position().0 - 2.5).abs() < 0.02, "held at StopFrame: {:?}", replay.position());
        cutscene.run_for_test(0.2);
        assert_eq!(play.tick(&cutscene, &mut replay), Some(SceneEnd::PhotoView));
        play.stop(&mut cutscene, &mut replay);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// nis-clock.md 9 (RETAIL data + clock), on the port's 60 Hz clock (a py -3 replica of [`Clock`] on SP/s1out
    /// 10-14 gives these): the lesson demos at their cameras' TIME_SCALE; ROP 2001/2002, the decline and the
    /// fly-through at TIME_SCALE 1; he_u2_photo01 (without a replay range) = its 3.0 s take + eight CMD 4 holds of
    /// 10/30 s, its CMD 5 doing nothing. Each hold freezes 19 frames, not 20, as in retail: the hold is f32
    /// (0.33333334, fmuls 0x824bc384) counted down in f32 by an f32 dt (fsubs 0x8278393c; dt lfs 0x83170b14 in
    /// 0x8277dad8) ahead of the ICE tick, and 0.33333334 - 20 x 0.016666668 ends at -3.4e-8 on the 20th step
    /// (an f32 replica gives 5.5500174 s; an f64 one, 5.683).
    /// research10: all but he_u2_photo01 skippable.
    #[test]
    #[ignore = "requires SKATE3_ASSET_ROOT with private/challenges-skate1"]
    fn the_skate1_scenes_run_their_retail_lengths() {
        let root = std::path::PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap()).join(SCENES);
        let lengths = [
            ("Basic_Basics_Tutorial", 37.13),
            ("Basic_Timing_Tutorial", 38.2),
            ("Basic_Air_Tutorial", 36.93),
            ("Basic_Grinds_Tutorial", 27.18),
            ("NIS_ROP_2001_shingo_S01_0", 15.84),
            ("NIS_ROP_2002_shingo_S01_0", 13.68),
            ("NIS_IP_Shingo_SkateDecline_S01_0", 3.22),
            ("he_u1_skate01", 3.02),
            ("he_u2_photo01", 5.55),
        ];
        for (scene, seconds) in lengths {
            let real = crate::challenge::cutscene::real_length(&root, scene).unwrap();
            assert!((real - seconds).abs() < 0.03, "{scene}: {real}");
        }
        let scenes: Scenes = serde_json::from_str(&std::fs::read_to_string(root.join("tutorial.json")).unwrap()).unwrap();
        assert_eq!(scenes.scenes.len(), 9);
        let mut cutscene = Cutscene::default();
        for (name, info) in &scenes.scenes {
            assert_eq!(info.kind == CHALLENGE_WIN, name == "he_u2_photo01", "{name}");
            assert!(ScenePlay::start(&mut cutscene, &root, name).skippable == (name != "he_u2_photo01"), "{name}");
            let walked = info.subtitles.windows(2).all(|w| w[0].start <= w[1].start && w[0].end <= w[1].start + 1e-4);
            assert!(walked, "{name}: P1 walks the cues in file order (research5.md 3)");
        }
    }
}
