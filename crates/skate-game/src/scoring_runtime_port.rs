//! The Skate 2 port's tricks in the engine's scorer (Task 17). Not tested in play.
//!
//! While Skate 2 physics drives the skater the engine's sim (and its `advance`) does not run: `skate2_physics::step`
//! hands each port frame's `Tricks` to [`Runtime::apply_port_frame`] instead. The port has already scored the tricks;
//! this only publishes them where the engine's readers look (the trick HUDs, the modes' trick reads, the line meter),
//! so the Skate 3 trick HUD shows Skate 2's tricks and Skate 2's scores.

use skate2_physics_protocol::Tricks;
use skate_core::scoring::catalog::IDENTIFIERS;

use super::{Runtime, compose_trick_name};

/// The engine's `spin_turns` for the port's `spins` over the same rotation: `spin_turns = SPIN_SIGN * spins`.
///
/// Step 1 (row 14) fixes it with `the_ports_spin_has_the_engines_sign`: the real sim, scripted to spin, and each air
/// tick's skater root (`pose::animation_to_world`, the rows the engine's own spin tracker reads) fed through the
/// engine's `SpinTracker`. Provisional until that run: both trackers come from the same code (the port's
/// `player_spin_tracker` and 82DA8BE0) and measure the yaw of the same rows in the same world (the Task 16 review
/// confirmed the handedness), so +1. The run's result is written here and in the Task 17 report.
pub(super) const SPIN_SIGN: i32 = 1;

impl Runtime {
    /// One port frame's tricks on this scorer, in place of `advance`: the tick's trick events by the engine's
    /// identifiers (`IDENTIFIERS[id].0`), the port's live score and line meter and multiplier, and the display's name.
    ///
    /// Not native Skate 2 (user 10-20): Skate 2's own trick display is not drawn; the Skate 3 trick HUD shows the
    /// port's tricks through this scorer. The name is composed with Skate 3's records and the port's spin and flip as
    /// Skate 2's display reads them (`spins < 0` is a backside spin when not switch); the protocol carries no stance, so
    /// no Switch, Fakie or Cab is named.
    pub fn apply_port_frame(&mut self, t: &Tricks) {
        let identifier = |id: &u16| IDENTIFIERS.get(usize::from(*id)).map(|entry| entry.0);
        self.announced = t.begun.iter().filter_map(identifier).collect();
        self.completed = t.ended.iter().filter_map(|(id, _)| identifier(id)).collect();
        self.new_trick = !t.begun.is_empty();
        if let Some(&id) = t.begun.last().or(t.converted.last()) {
            self.base_trick = Some(usize::from(id));
        }
        // A changed spin or flip renames the trick in place, as 825E4F40's dirty byte does; not on the tick the
        // sequence ends, where the engine zeroes them without an event.
        let turned = (t.spins, t.flips) != (self.published_spin_turns, self.published_flip_direction);
        self.modified_trick = !self.new_trick && (!t.converted.is_empty() || turned && !t.sequence_completed);
        (self.published_spin_turns, self.published_flip_direction) = (t.spins, t.flips);
        self.spin_turns = SPIN_SIGN * t.spins;
        // A bail (or a line the port cut short) closes the display; a landing banks the score and leaves the names to
        // fade, as `advance` publishes them.
        let bailed = t.interrupted || t.sequence_completed && !t.completed_successfully;
        self.close_tricks = bailed;
        self.sequence_landed = t.sequence_completed && t.completed_successfully;
        self.sequence_active = (self.sequence_active || t.sequence_begun || !t.active.is_empty()) && !t.sequence_completed;
        self.sequence_score = t.line_score as f32;
        // trickdisplay2 shows the sequence before the multiplier and the line total beside it (user 10-22: the
        // multiplier never showed, the line total was never set).
        self.port_sequence_score = Some(t.sequence_score as f32);
        self.session.holder.snapshot.line = t.line_score as f32;
        self.port_line_capacity = Some(t.line_max_points as f32);
        self.session.line.points = t.line_points as f32;
        // `online_scoring` sets `multiplier_enabled` from the match every frame; the shown multiplier follows it. A
        // tick with no HUD frame carries the protocol's default 0, which no multiplier is.
        let multiplier = if self.multiplier_enabled { t.multiplier.max(1.0) } else { 1.0 };
        self.session.combo.multiplier = multiplier;
        self.multiplier_changed = (multiplier != self.published_multiplier).then_some(multiplier);
        self.published_multiplier = multiplier;
        if self.close_tricks {
            self.base_trick = None;
        }
        self.trick_name = compose_trick_name(self.named(), t.spins, t.flips, false, false, false, false).name;
    }

    /// Ends the line (a teleport under the switch, a hand-over between the physics): no score, trick or meter is
    /// left in this scorer; the challenge's settings carry over, as in [`Runtime::fresh`].
    pub fn end_line(&mut self) {
        *self = self.fresh();
    }
}

#[cfg(test)]
mod tests {
    use skate2_physics_protocol::Tricks;
    use skate_core::scoring::catalog::IDENTIFIERS;

    use super::*;

    fn runtime() -> Runtime {
        let root = std::path::PathBuf::from(std::env::var_os("SKATE3_ASSET_ROOT").expect("set SKATE3_ASSET_ROOT"));
        let data = skate_data::collections::Collections::load(&root).unwrap();
        Runtime::load(&data).unwrap()
    }

    #[test]
    #[ignore = "requires owned collections (SKATE3_ASSET_ROOT)"]
    fn every_port_id_lands_as_the_engines_identifier() {
        let mut r = runtime();
        for id in 0..=268u16 {
            r.apply_port_frame(&Tricks { begun: vec![id], ..Tricks::default() });
            assert_eq!(r.announced.last(), Some(&IDENTIFIERS[usize::from(id)].0), "port id {id}");
            assert!(r.new_trick, "port id {id}");
        }
        for (id, name) in [(0u16, "fspowerslide"), (2, "nosemanual"), (268, "bs_lip_nose")] {
            r.apply_port_frame(&Tricks { begun: vec![id], ..Tricks::default() });
            assert_eq!(r.announced, [name]);
        }
    }

    #[test]
    #[ignore = "requires owned collections (SKATE3_ASSET_ROOT)"]
    fn a_scripted_line_scores_what_the_port_scores() {
        let mut r = runtime();
        let frames = [
            Tricks { begun: vec![2], ..Tricks::default() },
            Tricks { ended: vec![(2, 300)], line_score: 300, ..Tricks::default() },
            Tricks { begun: vec![0], line_score: 300, ..Tricks::default() },
            Tricks {
                ended: vec![(0, 450)],
                line_score: 1250,
                multiplier: 2.5,
                sequence_completed: true,
                completed_successfully: true,
                ..Tricks::default()
            },
        ];
        // `completed` is per tick, as `advance` publishes it; a reader (skate1_challenge) collects it.
        let mut completed = Vec::new();
        for frame in &frames {
            r.apply_port_frame(frame);
            completed.extend(r.completed.iter().copied());
        }
        assert_eq!(r.sequence_score, 1250.0);
        assert_eq!(completed, ["nosemanual", "fspowerslide"]);
        assert!(r.sequence_landed);
        assert!(!r.close_tricks, "a landing leaves the names to fade");
        assert!(!r.sequence_open());
        assert_eq!(r.multiplier_changed, Some(2.5));
        assert_eq!(r.hud_input().multiplier, 2.5);
    }

    /// User 10-22: "the score multiplier doesn't show". The trick display's line score (SetState 11,
    /// UpdateLineDisplay, which draws the multiplier with it) reads the line total, which the port's frames never
    /// set, so under the switch the line display never changed.
    #[test]
    #[ignore = "requires owned collections (SKATE3_ASSET_ROOT)"]
    fn the_line_display_gets_the_ports_line() {
        let mut r = runtime();
        r.multiplier_enabled = true;
        r.apply_port_frame(&Tricks {
            ended: vec![(96, 100)],
            line_score: 100,
            sequence_score: 60,
            line_points: 85,
            line_max_points: 400,
            multiplier: 1.5,
            ..Tricks::default()
        });
        let input = r.hud_input();
        assert_eq!(input.line_score, 100);
        assert_eq!(input.sequence_score, 60, "the display's score is the port's, before the multiplier");
        assert_eq!(r.sequence_score, 100.0, "the modes still read the line's total");
        assert_eq!((input.multiplier, input.line_points, input.line_capacity), (1.5, 85.0, 400.0));
        r.apply_port_frame(&Tricks { line_score: 0, line_max_points: 400, multiplier: 1.0, interrupted: true, ..Tricks::default() });
        assert_eq!(r.hud_input().line_score, 0, "a bail empties the line");
    }

    /// The spike's run (its pushes, ollie at 6.0 s, kickflip at 8.0 s, powerslide at 10.6 s) as pads for the real host.
    fn spike(tick: u32) -> skate2_physics_protocol::Pad {
        use skate2_physics_protocol::Pad;
        let s = |sec: f32| (sec * 60.0) as u32;
        let right = |[x, y]: [f32; 2]| {
            let magnitude = x.hypot(y);
            let raw = 0.25 + magnitude / f32::from_bits(0x3fb6_db6e);
            let axis = |v: f32| (v / magnitude * raw * 32768.0).round().clamp(-32768.0, 32767.0) as i16;
            Pad { right: [axis(x), axis(-y)], ..Pad::default() }
        };
        let flick = |[antic, point]: [[f32; 2]; 2], start: u32| {
            if (start..start + 2).contains(&tick) {
                Some(right(antic))
            } else if (start + 3..start + 5).contains(&tick) {
                Some(right(point))
            } else {
                None
            }
        };
        if [1.0, 1.9, 2.8, 3.7, 9.0, 9.7].iter().any(|&at| (s(at)..s(at) + 12).contains(&tick)) {
            return Pad { buttons: 0x1000, ..Pad::default() };
        }
        let ollie = [[-0.245_714, 0.942_857], [0.371_429, -0.908_571]];
        let kickflip = [[-0.028_571, 0.691_429], [0.908_571, -0.417_143]];
        if let Some(pad) = flick(ollie, s(6.0)).or_else(|| flick(kickflip, s(8.0))) {
            return pad;
        }
        if (s(10.6)..s(11.8)).contains(&tick) {
            return Pad { right: [i16::MAX, 0], ..Pad::default() };
        }
        Pad::default()
    }

    /// User 10-22: "tricks show up sometimes but the score multiplier doesn't show". The real host's spike run through
    /// this scorer into Skate 2's own trick display, as `skate2_hud::advance` drives it each tick.
    #[test]
    #[ignore = "needs SKATE3_ASSET_ROOT, S2HOST_TEST_DISC and SKATE2_HUD_ROOT"]
    fn the_real_hosts_tricks_reach_the_skate2_display() {
        use crate::apt_vm::Value;
        let disc = std::path::PathBuf::from(std::env::var_os("S2HOST_TEST_DISC").expect("set S2HOST_TEST_DISC"));
        let hud_root = std::path::PathBuf::from(std::env::var_os("SKATE2_HUD_ROOT").expect("set SKATE2_HUD_ROOT"));
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(hud_root.join("runtime/trickdisplay.json")).unwrap()).unwrap();
        let mut r = runtime();
        let mut hud = crate::hud_runtime::Runtime::load(&json, r.hud_input()).unwrap();
        let mut host = skate2_physics_host::Host::boot(&disc, &skate2_physics_protocol::Boot::default()).unwrap();
        host.warm_up().unwrap();
        let member = |hud: &crate::hud_runtime::Runtime, path: &[&str]| {
            path.iter().fold(Value::Object(hud.vm.global), |at, key| match at {
                Value::Object(object) => hud.vm.get(object, key),
                _ => Value::Undefined,
            })
        };
        let screen = ["trickdisplay2", "mScreen"];
        let shown = |hud: &crate::hud_runtime::Runtime, rest: &[&str]| {
            let path = [&screen[..], rest].concat();
            match member(hud, &path) {
                Value::Object(id) => {
                    let frame = hud.bindings.movie.instances.get(&id).map(|i| i.frame);
                    (frame, hud.vm.get(id, "_visible") == Value::Bool(true))
                }
                _ => (None, false),
            }
        };
        let mut begun = Vec::new();
        let mut refreshed = Vec::new();
        let mut multiplier_shown = Vec::new();
        for tick in 0..900 {
            let frame = host.tick(spike(tick)).unwrap();
            r.apply_port_frame(&frame.tricks);
            hud.update_skate2(r.hud_input(), r.new_trick, r.modified_trick, r.close_tricks).unwrap();
            let t = &frame.tricks;
            if !t.begun.is_empty() || !t.ended.is_empty() || t.sequence_completed || t.interrupted {
                println!(
                    "tick {tick}: begun {:?} ended {:?} line {} seq {} points {}/{} x{} done {} ok {} cut {}  name {:?}  \
                     trick0 {:?}  multiplier_mc {:?}  linescore {:?}",
                    t.begun, t.ended, t.line_score, t.sequence_score, t.line_points, t.line_max_points, t.multiplier,
                    t.sequence_completed, t.completed_successfully, t.interrupted, r.trick_name,
                    shown(&hud, &["mcTrickAnim0"]), shown(&hud, &["multiplier_mc"]),
                    shown(&hud, &["mcScoreModes", "mcLinescore"]),
                );
            }
            begun.extend(t.begun.iter().map(|&id| (tick, id)));
            if r.new_trick {
                refreshed.push(tick);
            }
            if t.multiplier > 1.0 {
                multiplier_shown.push((tick, t.multiplier, shown(&hud, &["multiplier_mc"])));
            }
        }
        println!("begun {begun:?}\nrefreshed {refreshed:?}\nmultiplier {multiplier_shown:?}");
        assert!(!begun.is_empty(), "the spike run began no trick");
        let mut begun_ticks: Vec<u32> = begun.iter().map(|(tick, _)| *tick).collect();
        begun_ticks.dedup();
        assert_eq!(refreshed, begun_ticks, "a begun trick never refreshed the name");
    }

    #[test]
    #[ignore = "requires owned collections (SKATE3_ASSET_ROOT)"]
    fn the_line_meter_is_skate_2s() {
        let mut r = runtime();
        let data_capacity = r.data.line_capacity;
        assert_eq!(r.hud_input().line_capacity, data_capacity, "no port frame: the data's meter");
        r.apply_port_frame(&Tricks { line_points: 1200, line_max_points: 5000, ..Tricks::default() });
        let input = r.hud_input();
        assert_eq!(input.line_capacity, 5000.0);
        assert_eq!(input.line_points, 1200.0);
    }

    #[test]
    #[ignore = "requires owned collections (SKATE3_ASSET_ROOT)"]
    fn end_line_clears_it() {
        let mut r = runtime();
        r.apply_port_frame(&Tricks { begun: vec![2], line_score: 300, line_max_points: 5000, ..Tricks::default() });
        assert_eq!(r.sequence_score, 300.0);
        r.end_line();
        assert_eq!(r.sequence_score, 0.0);
        assert!(r.announced.is_empty());
        assert_eq!(r.hud_input().line_capacity, r.data.line_capacity, "the Skate 2 meter goes with the line");
    }

    #[test]
    #[ignore = "requires owned collections (SKATE3_ASSET_ROOT)"]
    fn a_bail_closes_the_display() {
        let mut r = runtime();
        r.apply_port_frame(&Tricks { begun: vec![2], line_score: 300, ..Tricks::default() });
        assert!(r.hud_input().trick_name.len() > 1, "a name: {:?}", r.hud_input().trick_name);
        r.apply_port_frame(&Tricks { sequence_completed: true, ..Tricks::default() });
        assert!(r.close_tricks && !r.sequence_landed);
    }

    /// The right stick at one of the spike's pattern points (the vendored host tests' `support::right_point`).
    fn right_point(point: [f32; 2]) -> skate2_physics_protocol::Pad {
        let magnitude = point[1].mul_add(point[1], point[0] * point[0]).sqrt();
        let axis = |value: f32| -> i16 {
            let raw_radius = 0.25 + magnitude / f32::from_bits(0x3fb6_db6e);
            ((value / magnitude * raw_radius * 32768.0).round()).clamp(-32768.0, 32767.0) as i16
        };
        skate2_physics_protocol::Pad { right: [axis(point[0]), axis(point[1])], ..Default::default() }
    }

    /// The spike's run up to its ollie (pushes at 1.0, 1.9, 2.8 and 3.7 s, the ollie's flick at 6.0 s), with the
    /// left stick held full left from the flick for 1.5 s: Skate 2 turns the skater in the air with the left stick.
    fn spin_script(tick: u32) -> skate2_physics_protocol::Pad {
        let pushes = [60, 114, 168, 222].iter().any(|at| (*at..at + 12).contains(&tick));
        let mut pad = match tick {
            360..=361 => right_point([-0.245_714, 0.942_857]),
            363..=364 => right_point([0.371_429, -0.908_571]),
            _ if pushes => skate2_physics_protocol::Pad { buttons: 0x1000, ..Default::default() },
            _ => skate2_physics_protocol::Pad::default(),
        };
        if (363..453).contains(&tick) {
            pad.left = [i16::MIN, 0];
        }
        pad
    }

    /// Step 1 (row 14): the sign of the port's `spins` against the engine's `spin_turns` for the same rotation.
    #[test]
    #[ignore = "needs S2HOST_TEST_DISC (the stage output: the Skate 2 physics data)"]
    fn the_ports_spin_has_the_engines_sign() {
        use skate_core::scoring::air::{SpinTracker, half_turns};
        let root = std::env::var_os("S2HOST_TEST_DISC").expect("set S2HOST_TEST_DISC to the stage output");
        let boot = skate2_physics_protocol::Boot::default();
        let mut host = skate2_physics_host::Host::boot(std::path::Path::new(&root), &boot).expect("the port boots");
        let rows = |frame: &skate2_physics_protocol::Frame| {
            let m = crate::skate2_physics::pose::animation_to_world(frame);
            super::super::frame_rows([m[1][0], m[1][1], m[1][2]], [m[2][0], m[2][1], m[2][2]])
        };
        let (mut tracker, mut degrees, mut spins, mut air, mut begun) = (None, 0.0f32, 0i32, 0u32, Vec::new());
        for tick in 0..600u32 {
            let frame = host.tick(spin_script(tick)).expect("a tick runs");
            begun.extend(frame.tricks.begun.iter().copied());
            // The engine's spin tracker reads the rider's root rows while the Air collector runs (category 2).
            if frame.category == 2 {
                air += 1;
                let r = rows(&frame);
                let t = tracker.get_or_insert_with(|| SpinTracker::new(r, r));
                degrees = t.update(true, &r, &r, [0.0, 1.0, 0.0, 0.0]);
            }
            if frame.tricks.spins != 0 {
                spins = frame.tricks.spins;
            }
        }
        // 80 degrees: the engine data's [0x63C] allowance (`spin_tests`).
        let engine = half_turns(degrees, 80.0);
        println!("air ticks {air}; begun {begun:?}; port spins {spins}; engine {degrees:.1} degrees = {engine} half-turns");
        assert!(air > 0 && spins != 0 && engine != 0, "the script must spin: air {air}, spins {spins}, engine {engine}");
        assert_eq!(engine.signum(), SPIN_SIGN * spins.signum(), "write the sign this run found into SPIN_SIGN");
        assert_eq!(engine.abs(), spins.abs(), "the same half-turns");
    }
}
