//! Retail Skate 3's presentation event groups (spec 5, presentation 1.1): a group is a list of
//! `tChallengePresentationEvent`s {type, a, b, skip flags} played in order by the stepper 0x82dbade8 until one blocks.
//! StartPresentation 0x82dbaca0 queues groups behind the current one (stage1_traces 4.6). This is the pure runner;
//! a [`Stage`] performs each event, so nothing here draws, plays or teleports. Not tested in play.

use std::collections::BTreeSet;

use serde::Deserialize;

/// Groups StartPresentation queues behind the current one (the inline vector at +0xc74, 0x82dbad10-0x82dbad34).
pub const QUEUE: usize = 8;

/// The speech handler's timer: a `Speech` ends on its speech-done message or after this (float 0x821963e4,
/// stage1_traces 1.1).
pub const SPEECH_TIMEOUT: f32 = 10.0;

/// Ticks from RestoreSimRate until the rate is back to 60: the GSB's countdown +0x80 starts at frames + 1 = 2
/// (0x82857ec8) and runs down once a tick (0x8285a698, stage1_traces 5.2).
const RESTORE_TICKS: u8 = 2;

/// One presentation event (16 bytes in retail: {u32 type, string a, string b, u32 skip flags}, presentation 1.1).
/// The export writes `{kind, args: [a, b], flags}`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(from = "Raw")]
pub struct Event {
    pub kind: u32,
    pub a: String,
    pub b: String,
    pub flags: u32,
}

/// The export's form of an [`Event`].
#[derive(Deserialize)]
struct Raw {
    kind: u32,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    flags: u32,
}

impl From<Raw> for Event {
    fn from(raw: Raw) -> Self {
        Self::from_args(raw.kind, &raw.args, raw.flags)
    }
}

impl Event {
    /// An event from the export's `args`: `a` is `args[0]`, `b` is `args[1]`, a missing one is empty.
    #[must_use]
    pub fn from_args(kind: u32, args: &[String], flags: u32) -> Self {
        let arg = |i: usize| args.get(i).cloned().unwrap_or_default();
        Self { kind, a: arg(0), b: arg(1), flags }
    }
}

/// A named event list; its name is what `presentationeventgroupfinished` carries.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub name: String,
    pub events: Vec<Event>,
}

/// What an event asks of the runner before the next one plays.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Wait {
    /// Go on at once.
    Done,
    /// Hold the next event this long; zero or less goes on at once.
    Seconds(f32),
    /// Hold the next event until [`Presenter::signal`] sends this.
    Until(Signal),
}

/// What ends a [`Wait::Until`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    /// A line finished: (character, line id), the pair a `Speech` stored (+0xa24, +0xa1c; 0x8257c588).
    SpeechDone(u32, u32),
    NisEnded,
    ReplayEnded,
    FmvEnded,
    TeleportFinished,
}

/// Performs the events stage 1 uses (ids of presentation 1.1 / evtypes.txt). Each returns what the runner waits for.
pub trait Stage {
    /// 0 FadeIn (`out` false) / 1 FadeOut.
    fn fade(&mut self, out: bool) -> Wait;
    /// 2 Speech (`wait`) / 3 SpeechNoWait: `character` is the name in `a` (the stage resolves it, 0x8257c418), `id` is
    /// `atoi(b)`. A waiting line returns `Until(SpeechDone(character, id))`; the runner also ends it after
    /// [`SPEECH_TIMEOUT`]. A SpeechNoWait goes on at once whatever this returns (0x8257c528).
    fn speech(&mut self, character: &str, id: u32, wait: bool) -> Wait;
    /// 4 Wait: `a` seconds.
    fn wait(&mut self, seconds: f32) -> Wait {
        Wait::Seconds(seconds)
    }
    /// 5 NIS: the scene in `a`; `b` is its option (`True`, `f1`, presentation 2.4).
    fn nis(&mut self, name: &str, option: &str) -> Wait;
    /// 6 PinLocalPlayer / 7 UnpinLocalPlayer.
    fn pin(&mut self, pinned: bool) -> Wait;
    /// 8 TeleportLocalPlayer: the locator in `a`.
    fn teleport(&mut self, locator: &str) -> Wait;
    /// 11 EnableSetSessionMarker, 12 DisableSetSessionMarker, 13 EnableUseSessionMarker, 14 DisableUseSessionMarker
    /// (`set` is true for 11-12).
    fn session_marker(&mut self, set: bool, enabled: bool) -> Wait;
    /// 9 ShowHUD / 10 HideHUD.
    fn hud(&mut self, shown: bool) -> Wait;
    /// 15 ShowMiniMap / 16 HideMiniMap.
    fn minimap(&mut self, shown: bool) -> Wait;
    /// 17 EnableHOMEffect / 18 DisableHOMEffect.
    fn hom_effect(&mut self, enabled: bool) -> Wait;
    /// 19 Replay.
    fn replay(&mut self) -> Wait;
    /// 20 ShowTutorialCalloutHUD (the string id in `b`; `a` is empty in every row) / 21 HideTutorialCalloutHUD (None).
    /// INFERRED (from the rows' arguments, events_rows.txt): the text is `b`.
    fn callout(&mut self, text: Option<&str>) -> Wait;
    /// 23 CentralMessage: the string id in `a`.
    fn central(&mut self, text: &str) -> Wait;
    /// 28 GrantAchievement: `a`.
    fn achievement(&mut self, name: &str) -> Wait;
    /// 37 ShowTrickAnalyzer / 38 HideTrickAnalyzer.
    fn trick_analyzer(&mut self, shown: bool) -> Wait;
    /// 41 ShowManualMeter / 42 HideManualMeter.
    fn manual_meter(&mut self, shown: bool) -> Wait;
    /// The simulation speed: `Some(n / 60)` for SetSimRate(n), `Some(0.0)` for PauseSim, `None` back to normal.
    fn sim_rate(&mut self, rate: Option<f32>) -> Wait;
    /// 49 ActivateMinimalScoringHUD / 50 DeactivateMinimalScoringHUD.
    fn minimal_scoring_hud(&mut self, on: bool) -> Wait;
    /// 51 EnableInputRestriction: `Some(mask)`, which the stage ORs into the local player's mask (vt+0x6c 0x8256e4c0,
    /// stage1_traces 2.1); 52 ClearInputRestrictions: `None` clears it.
    fn input_restriction(&mut self, mask: Option<u32>) -> Wait;
    /// 64 FMV: the movie in `a`.
    fn fmv(&mut self, name: &str) -> Wait;
    /// 65 SFX (the speech handler's third event, stage1_traces 1.1): its `a` and `b`.
    fn sfx(&mut self, a: &str, b: &str) -> Wait;
    /// 66 AddIcon (`add`) / 67 RemoveIcon: the map icon named in `a`.
    fn icon(&mut self, add: bool, a: &str) -> Wait;
    /// 68 ShowMapTutorial.
    fn map_tutorial(&mut self) -> Wait;
}

/// Whether [`Presenter`] hands `kind` to the [`Stage`]; any other kind logs once and counts as [`Wait::Done`].
#[must_use]
pub fn supported(kind: u32) -> bool {
    matches!(kind, 0..=21 | 23 | 28 | 37 | 38 | 41 | 42 | 45..=52 | 64..=68)
}

/// What the current event is holding for.
#[derive(Clone, Copy, Debug)]
struct Blocked {
    until: Option<Signal>,
    left: f32,
}

/// The presenter: one current group, up to [`QUEUE`] behind it.
#[derive(Default)]
pub struct Presenter {
    current: Option<Group>,
    /// The next event of `current` (+0x58).
    next: usize,
    queue: Vec<Group>,
    blocked: Option<Blocked>,
    /// The director's skip mask (+0xc).
    mask: u32,
    /// Ticks until RestoreSimRate puts the rate back (0 = none pending).
    restore_in: u8,
    warned: BTreeSet<u32>,
}

impl Presenter {
    /// StartPresentation 0x82dbaca0: while busy, a group that is current with an empty queue or is the last queued is
    /// ignored, a ninth queued one is dropped. Returns whether `group` was started or queued. INFERRED (start has no
    /// stage): an idle start's first events play on the next [`Presenter::tick`], where retail steps at once.
    /// INFERRED (retail compares group pointers): groups are the same when their names are.
    pub fn start(&mut self, group: Group) -> bool {
        if self.current.is_none() {
            self.current = Some(group);
            self.next = 0;
            self.blocked = None;
            return true;
        }
        let repeat = self.queue.last().or(self.current.as_ref()).is_some_and(|other| other.name == group.name);
        if repeat || self.queue.len() >= QUEUE {
            return false;
        }
        self.queue.push(group);
        true
    }

    /// Advances by `dt` seconds and plays events until one blocks; returns the names of the groups that finished, in
    /// order (each is a `presentationeventgroupfinished`, post 0x23836ca7 at 0x82dbaffc).
    pub fn tick(&mut self, dt: f32, stage: &mut dyn Stage) -> Vec<String> {
        if self.restore_in > 0 {
            self.restore_in -= 1;
            if self.restore_in == 0 {
                stage.sim_rate(None);
            }
        }
        if let Some(blocked) = &mut self.blocked {
            blocked.left -= dt;
            if blocked.left > 0.0 {
                return Vec::new();
            }
            self.blocked = None;
        }
        let mut finished = Vec::new();
        while let Some(group) = &self.current {
            let Some(event) = group.events.get(self.next).cloned() else {
                finished.push(group.name.clone());
                self.current = if self.queue.is_empty() { None } else { Some(self.queue.remove(0)) };
                self.next = 0;
                continue;
            };
            self.next += 1;
            // 0x82dbade8: skipped when the director's mask meets the event's flags.
            if self.mask & event.flags != 0 {
                continue;
            }
            let timeout = if event.kind == 2 { SPEECH_TIMEOUT } else { f32::INFINITY };
            self.blocked = match self.play(&event, stage) {
                Wait::Done => None,
                Wait::Seconds(left) => (left > 0.0).then_some(Blocked { until: None, left }),
                Wait::Until(signal) => Some(Blocked { until: Some(signal), left: timeout }),
            };
            if self.blocked.is_some() {
                break;
            }
        }
        finished
    }

    /// Ends the current event's wait when it is waiting for `signal`; the next event plays on the next tick.
    pub fn signal(&mut self, signal: Signal) {
        if self.blocked.is_some_and(|blocked| blocked.until == Some(signal)) {
            self.blocked = None;
        }
    }

    /// The director's skip mask (+0xc): an event is skipped when `mask & flags != 0` (0x82dbade8).
    pub fn skip_mask(&mut self, mask: u32) {
        self.mask = mask;
    }

    /// Whether a group is playing (+0xc6c).
    #[must_use]
    pub fn busy(&self) -> bool {
        self.current.is_some()
    }

    /// `RemoveAllPresentationGroups`: drops the current group and the queue, posting nothing. A pending
    /// RestoreSimRate still lands (it is the GSB's countdown, not the presenter's).
    pub fn clear(&mut self) {
        self.current = None;
        self.next = 0;
        self.queue.clear();
        self.blocked = None;
    }

    fn play(&mut self, event: &Event, stage: &mut dyn Stage) -> Wait {
        let (kind, a, b) = (event.kind, event.a.as_str(), event.b.as_str());
        match kind {
            0 | 1 => stage.fade(kind == 1),
            2 => stage.speech(a, atoi(b), true),
            3 => {
                stage.speech(a, atoi(b), false);
                Wait::Done
            }
            4 => stage.wait(atof(a)),
            5 => stage.nis(a, b),
            6 | 7 => stage.pin(kind == 6),
            8 => stage.teleport(a),
            9 | 10 => stage.hud(kind == 9),
            11..=14 => stage.session_marker(kind <= 12, kind % 2 == 1),
            15 | 16 => stage.minimap(kind == 15),
            17 | 18 => stage.hom_effect(kind == 17),
            19 => stage.replay(),
            20 => stage.callout(Some(b)),
            21 => stage.callout(None),
            23 => stage.central(a),
            28 => stage.achievement(a),
            37 | 38 => stage.trick_analyzer(kind == 37),
            41 | 42 => stage.manual_meter(kind == 41),
            // INFERRED (PauseSim goes through 0x826db498, not the rate): a pause is rate 0, an unpause rate 1.
            45 => stage.sim_rate(Some(0.0)),
            46 => stage.sim_rate(None),
            // SetSimRate(N): +0x14 = 1/N (0x8282a2f8); the GSB then ticks at N Hz under a fixed 1/60 step, so the sim
            // runs at N/60 (stage1_traces 5.1; the speed reading is INFERRED, 5.3). A later SetSimRate's posts keep
            // the rate, so a pending restore is dropped.
            47 => {
                self.restore_in = 0;
                stage.sim_rate(Some(atof(a) / 60.0))
            }
            // RestoreSimRate parses no argument (the lessons' '60' is ignored, 0x8256e648); the rate returns two
            // ticks later (stage1_traces 5.2).
            48 => {
                self.restore_in = RESTORE_TICKS;
                Wait::Done
            }
            49 | 50 => stage.minimal_scoring_hud(kind == 49),
            51 => stage.input_restriction(Some(atoi(a))),
            52 => stage.input_restriction(None),
            64 => stage.fmv(a),
            65 => stage.sfx(a, b),
            66 | 67 => stage.icon(kind == 66, a),
            68 => stage.map_tutorial(),
            _ => {
                if self.warned.insert(kind) {
                    bevy::log::warn!("Skate 3 presentation: event kind {kind} is not supported; it is skipped");
                }
                Wait::Done
            }
        }
    }
}

/// atoi (0x82f4dec0): a number that does not parse is 0. INFERRED: no trailing text appears in the rows.
fn atoi(s: &str) -> u32 {
    s.trim().parse().unwrap_or(0)
}

/// The float parse of SetSimRate / Wait (0x82f4dc50): a number that does not parse is 0.
fn atof(s: &str) -> f32 {
    s.trim().parse().unwrap_or(0.0)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The character every recorded line resolves to.
    const CUZ: u32 = 35;

    /// Records each call; waits as the Bevy stage would (speech, NIS, replay, FMV and teleports until their signal).
    #[derive(Default)]
    pub(crate) struct Rec {
        log: Vec<String>,
    }

    impl Rec {
        fn rec(&mut self, line: String, wait: Wait) -> Wait {
            self.log.push(line);
            wait
        }

        fn take(&mut self) -> Vec<String> {
            std::mem::take(&mut self.log)
        }
    }

    impl Stage for Rec {
        fn fade(&mut self, out: bool) -> Wait {
            self.rec(format!("fade {out}"), Wait::Done)
        }
        fn speech(&mut self, character: &str, id: u32, wait: bool) -> Wait {
            self.rec(format!("speech {character} {id} {wait}"), Wait::Until(Signal::SpeechDone(CUZ, id)))
        }
        fn wait(&mut self, seconds: f32) -> Wait {
            self.rec(format!("wait {seconds}"), Wait::Seconds(seconds))
        }
        fn nis(&mut self, name: &str, option: &str) -> Wait {
            self.rec(format!("nis {name} {option}"), Wait::Until(Signal::NisEnded))
        }
        fn pin(&mut self, pinned: bool) -> Wait {
            self.rec(format!("pin {pinned}"), Wait::Done)
        }
        fn teleport(&mut self, locator: &str) -> Wait {
            self.rec(format!("teleport {locator}"), Wait::Until(Signal::TeleportFinished))
        }
        fn session_marker(&mut self, set: bool, enabled: bool) -> Wait {
            self.rec(format!("marker {set} {enabled}"), Wait::Done)
        }
        fn hud(&mut self, shown: bool) -> Wait {
            self.rec(format!("hud {shown}"), Wait::Done)
        }
        fn minimap(&mut self, shown: bool) -> Wait {
            self.rec(format!("minimap {shown}"), Wait::Done)
        }
        fn hom_effect(&mut self, enabled: bool) -> Wait {
            self.rec(format!("hom {enabled}"), Wait::Done)
        }
        fn replay(&mut self) -> Wait {
            self.rec("replay".into(), Wait::Until(Signal::ReplayEnded))
        }
        fn callout(&mut self, text: Option<&str>) -> Wait {
            self.rec(format!("callout {text:?}"), Wait::Done)
        }
        fn central(&mut self, text: &str) -> Wait {
            self.rec(format!("central {text}"), Wait::Done)
        }
        fn achievement(&mut self, name: &str) -> Wait {
            self.rec(format!("achievement {name}"), Wait::Done)
        }
        fn trick_analyzer(&mut self, shown: bool) -> Wait {
            self.rec(format!("analyzer {shown}"), Wait::Done)
        }
        fn manual_meter(&mut self, shown: bool) -> Wait {
            self.rec(format!("manual {shown}"), Wait::Done)
        }
        fn sim_rate(&mut self, rate: Option<f32>) -> Wait {
            self.rec(format!("sim {rate:?}"), Wait::Done)
        }
        fn minimal_scoring_hud(&mut self, on: bool) -> Wait {
            self.rec(format!("minimal {on}"), Wait::Done)
        }
        fn input_restriction(&mut self, mask: Option<u32>) -> Wait {
            self.rec(format!("input {mask:?}"), Wait::Done)
        }
        fn fmv(&mut self, name: &str) -> Wait {
            self.rec(format!("fmv {name}"), Wait::Until(Signal::FmvEnded))
        }
        fn sfx(&mut self, a: &str, b: &str) -> Wait {
            self.rec(format!("sfx {a} {b}"), Wait::Done)
        }
        fn icon(&mut self, add: bool, a: &str) -> Wait {
            self.rec(format!("icon {add} {a}"), Wait::Done)
        }
        fn map_tutorial(&mut self) -> Wait {
            self.rec("map tutorial".into(), Wait::Done)
        }
    }

    fn ev(kind: u32, a: &str, b: &str) -> Event {
        Event { kind, a: a.into(), b: b.into(), flags: 0 }
    }

    fn group(name: &str, events: Vec<Event>) -> Group {
        Group { name: name.into(), events }
    }

    /// A group that holds for one second.
    fn held(name: &str) -> Group {
        group(name, vec![ev(4, "1", "")])
    }

    #[test]
    fn events_run_in_order_and_wait() {
        let (mut p, mut s) = (Presenter::default(), Rec::default());
        let events = vec![
            ev(3, "CameraMan", "8798"),
            ev(4, "1.5", ""),
            ev(9, "", ""),
            ev(2, "Cuz", "5093"),
            ev(10, "", ""),
            ev(2, "Cuz", "5094"),
            ev(15, "", ""),
        ];
        assert!(p.start(group("intro", events)));
        assert!(p.tick(0.0, &mut s).is_empty());
        assert_eq!(s.take(), ["speech CameraMan 8798 false", "wait 1.5"], "SpeechNoWait goes on at once");
        p.tick(1.0, &mut s);
        assert!(s.take().is_empty(), "the wait holds for 1.5 s");
        p.tick(0.5, &mut s);
        assert_eq!(s.take(), ["hud true", "speech Cuz 5093 true"]);
        p.tick(5.0, &mut s);
        p.signal(Signal::SpeechDone(CUZ, 1));
        p.signal(Signal::NisEnded);
        p.tick(1.0, &mut s);
        assert!(s.take().is_empty(), "only its own speech-done ends a line");
        p.signal(Signal::SpeechDone(CUZ, 5093));
        p.tick(0.0, &mut s);
        assert_eq!(s.take(), ["hud false", "speech Cuz 5094 true"]);
        p.tick(9.0, &mut s);
        assert!(s.take().is_empty());
        assert_eq!(p.tick(1.0, &mut s), ["intro"], "a line ends after 10 s without its speech-done");
        assert_eq!(s.take(), ["minimap true"]);
        assert!(!p.busy());
    }

    #[test]
    fn the_presenter_queues_eight_and_drops_the_ninth() {
        let (mut p, mut s) = (Presenter::default(), Rec::default());
        assert!(p.start(held("g0")));
        for i in 1..=8 {
            assert!(p.start(held(&format!("g{i}"))), "g{i}");
        }
        assert!(!p.start(held("g9")));
        let mut finished = Vec::new();
        for _ in 0..20 {
            finished.extend(p.tick(1.0, &mut s));
        }
        assert_eq!(finished, (0..=8).map(|i| format!("g{i}")).collect::<Vec<_>>());
    }

    #[test]
    fn a_current_group_with_an_empty_queue_is_not_requeued() {
        let mut p = Presenter::default();
        assert!(p.start(held("a")));
        assert!(!p.start(held("a")));
        assert!(p.start(held("b")));
        assert!(p.start(held("a")), "with a queue, the current group may queue again");
    }

    #[test]
    fn the_last_queued_group_is_not_queued_twice() {
        let mut p = Presenter::default();
        assert!(p.start(held("a")));
        assert!(p.start(held("b")));
        assert!(!p.start(held("b")));
        assert!(p.start(held("c")));
        assert!(p.start(held("b")), "b is no longer the last queued");
    }

    #[test]
    fn skip_flags_skip_events() {
        let (mut p, mut s) = (Presenter::default(), Rec::default());
        p.skip_mask(2);
        let flagged = |kind, flags| Event { flags, ..ev(kind, "", "") };
        p.start(group("g", vec![flagged(9, 2), flagged(15, 1), flagged(10, 6)]));
        assert_eq!(p.tick(0.0, &mut s), ["g"]);
        assert_eq!(s.take(), ["minimap true"]);
    }

    #[test]
    fn a_finished_group_reports_its_name() {
        let (mut p, mut s) = (Presenter::default(), Rec::default());
        p.start(group("OwnedIt", vec![ev(9, "", "")]));
        assert_eq!(p.tick(0.0, &mut s), ["OwnedIt"]);
        assert!(!p.busy());
        p.start(group("empty", Vec::new()));
        assert_eq!(p.tick(0.0, &mut s), ["empty"]);
        p.start(held("a"));
        p.start(group("b", vec![ev(10, "", "")]));
        assert!(p.tick(0.0, &mut s).is_empty());
        assert_eq!(p.tick(1.0, &mut s), ["a", "b"], "the next queued group starts in the same tick");
    }

    #[test]
    fn clear_drops_everything() {
        let (mut p, mut s) = (Presenter::default(), Rec::default());
        p.start(held("a"));
        p.start(group("b", vec![ev(9, "", "")]));
        p.tick(0.0, &mut s);
        s.take();
        p.clear();
        assert!(!p.busy());
        assert!(p.tick(5.0, &mut s).is_empty());
        assert!(s.take().is_empty());
        assert!(p.start(held("a")));
    }

    #[test]
    fn set_sim_rate_slows_by_n_over_60_and_restore_returns_two_ticks_later() {
        let (mut p, mut s) = (Presenter::default(), Rec::default());
        p.start(group("g", vec![ev(47, "30", ""), ev(47, "45", ""), ev(48, "60", "")]));
        p.tick(0.0, &mut s);
        assert_eq!(s.take(), ["sim Some(0.5)", "sim Some(0.75)"]);
        p.tick(0.0, &mut s);
        assert!(s.take().is_empty(), "one tick after RestoreSimRate the rate is still set");
        p.tick(0.0, &mut s);
        assert_eq!(s.take(), ["sim None"]);
        p.tick(0.0, &mut s);
        assert!(s.take().is_empty());
    }

    #[test]
    fn exactly_the_supported_kinds_reach_the_stage() {
        for kind in 0..80 {
            let (mut p, mut s) = (Presenter::default(), Rec::default());
            p.start(group("g", vec![ev(kind, "1", "1")]));
            for _ in 0..3 {
                p.tick(0.0, &mut s);
            }
            assert_eq!(!s.log.is_empty(), supported(kind), "kind {kind}: {:?}", s.log);
        }
    }

    #[test]
    fn events_read_the_exports_args() {
        let e: Event = serde_json::from_str(r#"{"kind": 3, "args": ["CameraMan", "8798"], "flags": 4}"#).unwrap();
        assert_eq!(e, Event { kind: 3, a: "CameraMan".into(), b: "8798".into(), flags: 4 });
        assert_eq!(Event::from_args(9, &[], 0), ev(9, "", ""));
    }

    /// Every event-shaped object (`kind` and `args`) under `value`, as `(where, kind)`.
    fn kinds(value: &serde_json::Value, at: &str, out: &mut Vec<(String, u64)>) {
        match value {
            serde_json::Value::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    kinds(item, &format!("{at}[{i}]"), out);
                }
            }
            serde_json::Value::Object(map) => {
                let kind = map.get("kind").and_then(serde_json::Value::as_u64);
                if let (Some(kind), Some(serde_json::Value::Array(_))) = (kind, map.get("args")) {
                    out.push((at.to_owned(), kind));
                }
                for (key, item) in map {
                    kinds(item, &format!("{at}.{key}"), out);
                }
            }
            _ => {}
        }
    }

    #[test]
    #[ignore = "requires the skate3-career export (SKATE3_CAREER_DIR)"]
    fn every_stage_one_event_kind_is_supported() {
        let dir = std::path::PathBuf::from(std::env::var_os("SKATE3_CAREER_DIR").expect("set SKATE3_CAREER_DIR"));
        let read = |path: std::path::PathBuf| -> serde_json::Value {
            let bytes = std::fs::read(&path).unwrap_or_else(|why| panic!("{}: {why}", path.display()));
            serde_json::from_slice(&bytes).unwrap()
        };
        let mut found = Vec::new();
        // The progression states (every stategraph), and each stage-1 row's groups and objectives (Task 6).
        kinds(&read(dir.join("career.json"))["stategraphs"], "stategraphs", &mut found);
        for entry in std::fs::read_dir(dir.join("rows")).expect("rows/") {
            let path = entry.unwrap().path();
            let at = path.file_name().unwrap().to_string_lossy().into_owned();
            kinds(&read(path), &at, &mut found);
        }
        assert!(!found.is_empty(), "no events in the export");
        let missing: Vec<_> = found.iter().filter(|(_, kind)| !u32::try_from(*kind).is_ok_and(supported)).collect();
        assert!(missing.is_empty(), "{} of {} events unsupported: {missing:#?}", missing.len(), found.len());
    }
}
