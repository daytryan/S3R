//! Trick completeness: every reachable `EScorableID`, performed from raw controller samples and
//! proven by the scorer, not just by the graph.
//!
//! The older playback suites (`flip_playback.rs`, `grab_playback.rs`, ...) assert that the
//! MotionGraph *publishes* a name. A row here asserts the whole chain: raw `XboxState` ->
//! gesture recognizer -> ActionGraph -> MotionGraph -> physics -> `scoring_runtime`, and that the
//! scorer both **named** the trick (its identifier announced, or published by the graph's
//! scoring leaf) and **credited its record** (`ScoreHolder` repetition count for that id > 0,
//! which only `credit_trick` / `end_trick` raise).
//!
//! Recipes are retail's own inputs: scoops are read from the authored `.pat` files the recognizer
//! loads, buttons and triggers follow `docs/trick-input-recipes.md`.
//!
//! `TRICK_PROOF_LOG=<file>` appends one `SCORE_TRICK proof test=<fn> id=<id>` line per proven
//! row; `examples/trick_reachability.rs` reads those lines from `logs/` for its proof column.
use super::*;
use skate_core::animation::skeleton_input::name::encode;
use skate_core::input::xbox::XboxState;
use skate_core::scoring::catalog::{self, IDENTIFIERS};
use std::collections::BTreeMap;

pub(super) const A: u16 = 0x1000;
pub(super) const B: u16 = 0x2000;
pub(super) const X: u16 = 0x4000;
pub(super) const RB: u16 = 0x0200;

/// What a recipe sees each tick, after the spawn pin.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct View {
    pub tick: u32,
    pub airborne: bool,
    /// Airborne ticks so far (all airs).
    pub air_ticks: u32,
    pub category: u32,
}

/// How the run is staged before the controller takes over.
#[derive(Clone, Copy, Debug)]
pub(super) struct Stage {
    /// Forward speed given at tick 60 (m/s along the heading), the "rolling approach".
    pub speed: f32,
    /// Extra vertical velocity at `boost_tick` (default 123, just after the default pop), so a
    /// recipe gets the air time it needs.
    pub boost: f32,
    pub boost_tick: u32,
    /// Spawn on the course's lower floor at `[x, z]` facing `heading` radians from +z
    /// (towards +x), instead of the start box. Used for the rails, ledges and the half pipe.
    pub floor: Option<([f32; 2], f32)>,
    /// Score as an online x1 challenge does (`online_scoring::apply`): multiplier off, offline bit clear.
    pub online: bool,
    /// ManualsAreConnectors (`Runtime::manuals_connect`): clear as a challenge that turns it off, or as the scorer
    /// before 2026-10-25, which took +945 as clear.
    pub manuals_connect: bool,
}
impl Stage {
    pub const ROLL: Self = Self { speed: 8.0, boost: 0.0, boost_tick: 123, floor: None, online: false, manuals_connect: true };
    pub const fn pop(boost: f32) -> Self {
        Self { speed: 8.0, boost, boost_tick: 123, floor: None, online: false, manuals_connect: true }
    }
    pub const fn at(x: f32, z: f32, heading: f32, speed: f32) -> Self {
        Self { speed, boost: 0.0, boost_tick: 123, floor: Some(([x, z], heading)), online: false, manuals_connect: true }
    }
    pub const fn boosted(mut self, boost: f32, tick: u32) -> Self {
        self.boost = boost;
        self.boost_tick = tick;
        self
    }
    pub const fn online(mut self) -> Self {
        self.online = true;
        self
    }
    pub const fn manuals_connect(mut self, on: bool) -> Self {
        self.manuals_connect = on;
        self
    }
    fn heading(&self) -> f32 {
        self.floor.map_or(0.0, |(_, h)| h)
    }
}

#[derive(Default, Debug)]
pub(super) struct Proof {
    /// Session repetition counts by scorable id: only crediting raises them.
    pub credited: BTreeMap<usize, i8>,
    pub announced: Vec<&'static str>,
    /// Scorable names the MotionGraph's scoring leaves published (trick and grab slots).
    pub published: Vec<&'static str>,
    pub labels: Vec<String>,
    pub animations: Vec<String>,
    /// The tick each scorable was first announced / first credited (the Trick Analyzer's trigger vs the old one).
    pub announced_at: BTreeMap<&'static str, u32>,
    pub credited_at: BTreeMap<usize, u32>,
    /// Per tick: (tick, airborne, the scorer's held tricks, its ended ones, sequence landed, balance).
    pub ticks: Vec<(u32, bool, Vec<&'static str>, Vec<&'static str>, bool, f32)>,
    /// Every published sequence: (tick, the SequenceScore a challenge's Lua gets).
    pub sequences: Vec<(u32, i64)>,
}
impl Proof {
    pub fn id(identifier: &str) -> usize {
        IDENTIFIERS
            .iter()
            .position(|(name, _, _)| name.eq_ignore_ascii_case(identifier))
            .unwrap_or_else(|| panic!("{identifier} is not an EScorableID"))
    }
    /// Named by the scorer and credited with its record.
    pub fn proves(&self, identifier: &str) -> bool {
        let id = Self::id(identifier);
        let named = self.announced.iter().any(|n| n.eq_ignore_ascii_case(identifier))
            || self.published.iter().any(|n| n.eq_ignore_ascii_case(identifier));
        named && self.credited.get(&id).is_some_and(|n| *n > 0)
    }
    pub fn summary(&self) -> String {
        let credited: Vec<&str> = self.credited.keys().map(|id| IDENTIFIERS[*id].0).collect();
        format!(
            "credited={credited:?} announced={:?} published={:?} labels={:?}\n    animations={:?}",
            self.announced, self.published, self.labels, self.animations
        )
    }
}

/// Assets and graphs loaded once per test function and shared by its rows.
pub(super) struct Bench {
    root: std::path::PathBuf,
    graphs: crate::graph_runtime::StockGraphs,
    names: Vec<(skate_core::animation::output::attributes::AttributeName, &'static str)>,
}
impl Bench {
    pub fn load() -> Self {
        let root = std::path::PathBuf::from(
            std::env::var_os("SKATE3_ASSET_ROOT").expect("set SKATE3_ASSET_ROOT"),
        );
        let assets = skate_data::GameAssets::load(&root).unwrap();
        let graphs = crate::graph_runtime::StockGraphs::load(&root, &assets).unwrap();
        let names = IDENTIFIERS
            .iter()
            .map(|(name, _, _)| (encode(name.as_bytes()), *name))
            .collect();
        Self { root, graphs, names }
    }

    /// An authored gesture, as the recognizer loads it.
    pub fn scoop(&self, file: &str, name: &str) -> Vec<[f32; 2]> {
        self.scoops(file, name).swap_remove(0)
    }
    pub fn scoops(&self, file: &str, name: &str) -> Vec<Vec<[f32; 2]>> {
        let found: Vec<_> = skate_data::gesture_patterns::load(
            &self.root.join("private/stock/data/joystick").join(file),
        )
        .unwrap()
        .into_iter()
        .filter(|p| p.name == name)
        .map(|p| p.points)
        .collect();
        assert!(!found.is_empty(), "authored {name} pattern in {file}");
        found
    }

    pub fn perform(
        &self,
        stage: Stage,
        ticks: u32,
        mut pad: impl FnMut(&View) -> XboxState,
    ) -> Proof {
        let root = &self.root;
        let graphs = &self.graphs;
        let mut physics = match stage.floor {
            None => GamePhysics::load_with_terrain(root, ground::Terrain::Course).unwrap(),
            Some(([x, z], heading)) => {
                // A spot on the floor. `load_at` lands there as a game teleport does: the ground probe puts the
                // board origin 0.25 m above the face (0.15 + horizontal_spawn's 0.1), 0.16 m above its rest, so the
                // board drops onto the floor in the first ticks, well before the proof's tick 60.
                let at = [
                    [heading.cos(), 0., -heading.sin(), 0.],
                    [0., 1., 0., 0.],
                    [heading.sin(), 0., heading.cos(), 0.],
                    [x, ground::FLOOR_HEIGHT, z, 1.],
                ];
                GamePhysics::load_at(root, None, crate::difficulty::Difficulty::Easy, Some(at))
                    .unwrap()
            }
        };
        let mut skater = SkaterRuntime::load(root, graphs, &physics, "normal").unwrap();
        skater.scoring.multiplier_enabled = !stage.online;
        skater.scoring.offline = !stage.online;
        skater.scoring.manuals_connect = stage.manuals_connect;
        let mut camera = crate::camera::CameraRuntime::load(root).unwrap();
        let mut controller = crate::input::ControllerInput::default();
        let mut controls = PlayerControls::load(root).unwrap();
        let mut proof = Proof::default();
        let mut view = View::default();
        for step in 0..SPAWN_PIN_DELAY + ticks {
            let tick = step.saturating_sub(SPAWN_PIN_DELAY);
            if tick == 60 && stage.speed != 0.0 {
                let h = stage.heading();
                let v = Vector3::new(stage.speed * h.sin(), 0., stage.speed * h.cos());
                for body in physics.board.bodies_mut() {
                    body.rates.linear_velocity = v;
                }
                for body in skater.skeleton.bodies_mut() {
                    body.rates.linear_velocity = v;
                }
            }
            if tick == stage.boost_tick && stage.boost != 0.0 {
                for body in physics.board.bodies_mut() {
                    body.rates.linear_velocity.y += stage.boost;
                }
                for body in skater.skeleton.bodies_mut() {
                    body.rates.linear_velocity.y += stage.boost;
                }
            }
            view.tick = tick;
            view.category = skater.player_state.current().category();
            view.airborne = view.category == 200;
            let state = if step < SPAWN_PIN_DELAY {
                XboxState { buttons: 0, triggers: [0; 2], left: [0; 2], right: [0; 2] }
            } else {
                pad(&view)
            };
            controller.sample_raw_for_test(state);
            let mut actions = controller.player_actions();
            controls.update(
                &mut actions,
                physics.settings.step.simulation.time_step,
                physics.settings.input_magnitude_threshold,
                skater.player_input.physical.scoring.capabilities_204,
            );
            controls.publish_gestures(
                physics.animation_profile.physics_mode,
                skater.player_input.physical.state.state_16,
            );
            frame::advance(
                &mut physics,
                &mut skater,
                &mut controls,
                graphs,
                &mut actions,
                true,
                &mut camera,
            )
            .unwrap_or_else(|e| panic!("tick{tick}: {e}\n  {}", proof.summary()));
            if skater.player_state.current().category() == 200 {
                view.air_ticks += 1;
            }
            let motion = &skater.animation.motion;
            if let Some(name) = motion.animation.current_name.as_deref()
                && proof.animations.last().map(String::as_str) != Some(name)
            {
                proof.animations.push(name.to_owned());
            }
            let published = [
                motion.score_packet.trick_names.first,
                motion.score_packet.grab.map(|(name, _)| name),
            ];
            for name in published.into_iter().flatten() {
                if let Some((_, identifier)) = self.names.iter().find(|(e, _)| *e == name)
                    && !proof.published.contains(identifier)
                {
                    proof.published.push(identifier);
                }
            }
            if std::env::var_os("TRACE_TRICK").is_some() {
                eprintln!(
                    "t{tick} cat={} state={:?} air={} in={:?} anim={:?} packet={:?} grab={:?}",
                    view.category,
                    skater.player_state.current(),
                    view.air_ticks,
                    (state.buttons, state.triggers, state.left, state.right),
                    motion.animation.current_name,
                    published[0].and_then(|n| self.names.iter().find(|(e, _)| *e == n)).map(|x| x.1),
                    published[1].and_then(|n| self.names.iter().find(|(e, _)| *e == n)).map(|x| x.1),
                );
            }
            for identifier in &skater.scoring.announced {
                proof.announced.push(identifier);
                proof.announced_at.entry(identifier).or_insert(tick);
            }
            let s = &skater.scoring;
            proof.ticks.push((tick, view.airborne, s.holding().collect(), s.completed.clone(), s.sequence_landed,
                skater.animation_input.fields.balance));
            if s.sequence_landed {
                proof.sequences.push((tick, s.lua_sequence_score()));
            }
            let label = skater.scoring.hud_input().trick_name;
            if !label.is_empty() && proof.labels.last() != Some(&label) {
                proof.labels.push(label);
            }
            // Sampled every tick: an expired line banks and clears repetition (82DA3B38).
            let holder = &skater.scoring.session.holder;
            for id in 0..IDENTIFIERS.len() {
                if let Some(scorable) = catalog::metadata(id)
                    && let Some(count) = holder.repetition_count(scorable)
                    && count > 0
                {
                    let seen = proof.credited.entry(id).or_insert(0);
                    *seen = (*seen).max(count);
                    proof.credited_at.entry(id).or_insert(tick);
                }
            }
        }
        proof
    }
}

/// Raw stick sample for an authored coordinate. GameInputManager 82696030 negates the mapped Y
/// before matching, so the raw sample carries the opposite sign.
pub(super) fn raw(p: [f32; 2]) -> [i16; 2] {
    [(p[0] * 32767.) as i16, (-p[1] * 32767.) as i16]
}

/// The ordinary ground pop: crouch on the scoop's first coordinate, then walk the rest of its
/// coordinates three ticks apiece (`hold` keeps the last coordinate, which is what keeps
/// `<Trick>Hold` published).
pub(super) fn ground_flick(scoop: &[[f32; 2]], tick: u32, hold: bool) -> [i16; 2] {
    if (100..112).contains(&tick) {
        return raw(scoop[0]);
    }
    let Some(i) = tick.checked_sub(112).map(|t| t as usize / 3 + 1) else {
        return [0; 2];
    };
    if i < scoop.len() || hold || i == scoop.len() {
        raw(scoop[i.min(scoop.len() - 1)])
    } else {
        [0; 2]
    }
}

/// A scoop played one coordinate every `step` ticks starting at `start`; `None` outside it.
pub(super) fn sweep(scoop: &[[f32; 2]], start: u32, step: u32, at: u32) -> Option<[i16; 2]> {
    let i = at.checked_sub(start)? / step;
    scoop.get(i as usize).copied().map(raw)
}

pub(super) fn pad(right: [i16; 2]) -> XboxState {
    XboxState { buttons: 0, triggers: [0; 2], left: [0; 2], right }
}

/// Record a proven row for the reachability report.
pub(super) fn log_proof(test: &str, identifier: &str) {
    let Some(path) = std::env::var_os("TRICK_PROOF_LOG") else {
        return;
    };
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
    writeln!(file, "SCORE_TRICK proof test={test} id={}", Proof::id(identifier)).unwrap();
}

/// Run every row, report all failures together, and log the proven ones.
pub(super) fn check<R>(test: &str, rows: &[R], run: impl Fn(&R) -> (&'static str, Proof)) {
    let mut failed = Vec::new();
    for row in rows {
        let (target, proof) = run(row);
        if proof.proves(target) {
            eprintln!("PROVEN {target}");
            log_proof(test, target);
        } else {
            eprintln!("MISSING {target}: {}", proof.summary());
            failed.push(target);
        }
    }
    assert!(failed.is_empty(), "{test}: not proven: {failed:?}");
}

/// Not a proof: sweeps one recipe parameter and prints what each value credits, to find the
/// rows the proofs below hard-code. `SURVEY=<family>`.
#[test]
#[ignore = "survey"]
fn survey() {
    let bench = Bench::load();
    let family = std::env::var("SURVEY").unwrap_or_default();
    let show = |what: String, p: &Proof| {
        let credited: Vec<&str> = p.credited.keys().map(|id| IDENTIFIERS[*id].0).collect();
        eprintln!("SURVEY {what}: credited={credited:?} announced={:?}", p.announced);
    };
    let flips = ["Kickflip", "Heelflip", "N_Kickflip", "N_Heelflip", "PopShuvit", "FSPopShuvit", "N_PopShuvit", "N_FSPopShuvit"];
    match family.as_str() {
        "underflip" => {
            let under = bench.scoop("skater_air.pat", "L_F_Kickflip");
            for gesture in flips {
                let scoop = bench.scoop("skater.pat", gesture);
                for start in (0..48).step_by(4) {
                    let p = bench.perform(Stage::pop(10.0), 330, |v| {
                        let right = if v.airborne && v.air_ticks >= start {
                            sweep(&under, start, 3, v.air_ticks).unwrap_or(raw(scoop[scoop.len() - 1]))
                        } else {
                            ground_flick(&scoop, v.tick, true)
                        };
                        pad(right)
                    });
                    show(format!("{gesture} start={start}"), &p);
                }
            }
        }
        "dark" => {
            for gesture in &flips[..4] {
                let scoop = bench.scoop("skater.pat", gesture);
                for start in (0..48).step_by(4) {
                    let p = bench.perform(Stage::pop(10.0), 330, |v| XboxState {
                        buttons: if v.airborne && v.air_ticks >= start { RB } else { 0 },
                        ..pad(ground_flick(&scoop, v.tick, true))
                    });
                    show(format!("{gesture} rb_at={start}"), &p);
                }
            }
        }
        "late" => {
            let ollie = bench.scoop("skater.pat", "Ollie");
            for gesture in ["L_FS_Shuvit", "L_BS_Shuvit"] {
                for (variant, late) in bench.scoops("skater_air.pat", gesture).into_iter().enumerate() {
                    for step in [1, 2, 3, 4] {
                        let p = bench.perform(Stage::pop(9.0), 300, |v| {
                            pad(if v.airborne && v.air_ticks >= 10 {
                                sweep(&late, 10, step, v.air_ticks).unwrap_or([0; 2])
                            } else {
                                ground_flick(&ollie, v.tick, false)
                            })
                        });
                        show(format!("{gesture}#{variant} step={step}"), &p);
                    }
                }
            }
        }
        "grind" => {
            let ollie = bench.scoop("skater.pat", "Ollie");
            let speed = 5.0;
            for rail_x in [-7.0f32, 4.5] {
                for yaw in [0.0f32, 15., 30., 45., 60., 75., 90., -30., -60., -90.] {
                    for stick in [CENTRE, UP, DOWN] {
                        for early in [30u32, 24, 18, 12] {
                            let h = yaw.to_radians();
                            let pz = if yaw.abs() < 20. { 8.0 } else { 11.0 };
                            let lead = 6.0;
                            let (sx, sz) = (rail_x - lead * h.sin(), pz - lead * h.cos());
                            let arrive = 60 + (lead / speed * 60.0) as u32;
                            let pop = arrive - early;
                            let stage = Stage::at(sx, sz, h, speed).boosted(1.5, pop + 11);
                            let p = bench.perform(stage, 330, |v| {
                                if v.airborne {
                                    pad(stick)
                                } else {
                                    pad(ground_flick(&ollie, (v.tick + 112).saturating_sub(pop), false))
                                }
                            });
                            show(format!("rail={rail_x} yaw={yaw} stick={stick:?} early={early}"), &p);
                        }
                    }
                }
            }
        }
        "handplant" => {
            // The course half pipe: flat bottom x 10..14, copings at x 7.5 and 16.5.
            for heading in [90.0f32, -90.] {
                for speed in [5.0f32, 6.0, 7.0, 8.0, 9.0] {
                    for left in [CENTRE, LEFT, RIGHT] {
                        let stage = Stage::at(12.0, 0.0, heading.to_radians(), speed);
                        let p = bench.perform(stage, 330, |v| XboxState {
                            buttons: if v.tick >= 62 { RB } else { 0 },
                            triggers: [0; 2],
                            left: if v.tick >= 62 { left } else { CENTRE },
                            right: [0; 2],
                        });
                        show(format!("heading={heading} speed={speed} left={left:?}"), &p);
                    }
                }
            }
        }
        "nocomply" => {
            // `Push.xml`: a Square-group scoop while `Push.Left/Right.Default` is running.
            let ollie = bench.scoop("skater.pat", "Ollie");
            for button in [A, X] {
                for lead in [-12i32, -6, 0, 4, 8, 12, 16] {
                    for press in [4u32, 10, 30] {
                        let p = bench.perform(Stage::ROLL, 320, |v| {
                            let crouch = (150i32 + lead) as u32;
                            XboxState {
                                buttons: if (150..150 + press).contains(&v.tick) { button } else { 0 },
                                ..pad(ground_flick(&ollie, (v.tick + 100).saturating_sub(crouch), false))
                            }
                        });
                        show(format!("button={button:#x} lead={lead} press={press}"), &p);
                    }
                }
            }
        }
        other => panic!("unknown SURVEY {other:?}"),
    }
}

// ---------------------------------------------------------------------------------------------
// Flip tricks

/// One flick, released: the single of every authored ground scoop, tail and nose side.
/// The flip gate is air time (`docs/flip-ladder.md`), so the pop is boosted.
#[test]
#[ignore = "requires private stock graphs and animation assets"]
fn flicked_flips_credit_their_scorable() {
    let bench = Bench::load();
    let rows: &[(&str, &str, f32)] = &[
        ("Kickflip", "kickflip", 8.0),
        ("Heelflip", "heelflip", 8.0),
        ("PopShuvit", "popshuvit", 8.0),
        ("FSPopShuvit", "fspopshuvit", 8.0),
        ("VarialKickflip", "varialkickflip", 8.0),
        ("VarialHeelflip", "varialheelflip", 8.0),
        ("InwardHeelflip", "inwardheelflip", 8.0),
        ("Hardflip", "hardflip", 8.0),
        ("360PopShuvit", "360popshuvit", 8.0),
        ("FS360PopShuvit", "fs360popshuvit", 8.0),
        ("360Flip", "360flip", 8.0),
        ("Laserflip", "laserflip", 8.0),
        ("360Hardflip", "360hardflip", 8.0),
        ("360InwardHeelflip", "360inwardheelflip", 8.0),
        ("Nollie", "nollie", 8.0),
        ("N_Kickflip", "n_kickflip", 8.0),
        ("N_Heelflip", "n_heelflip", 8.0),
        ("N_PopShuvit", "n_popshuvit", 8.0),
        ("N_FSPopShuvit", "n_fspopshuvit", 8.0),
        ("N_VarialKickflip", "n_varialkickflip", 8.0),
        ("N_VarialHeelflip", "n_varialheelflip", 8.0),
        ("N_InwardHeelflip", "n_inwardheelflip", 8.0),
        ("N_Hardflip", "n_hardflip", 8.0),
        ("N_360PopShuvit", "n_360popshuvit", 8.0),
        ("N_FS360PopShuvit", "n_fs360popshuvit", 8.0),
        ("N_360Flip", "n_360flip", 8.0),
        ("N_Laserflip", "n_laserflip", 8.0),
        ("N_360Hardflip", "n_360hardflip", 8.0),
        ("N_360InwardHeelflip", "n_360inwardheelflip", 8.0),
    ];
    check("flicked_flips_credit_their_scorable", rows, |&(gesture, target, boost)| {
        let scoop = bench.scoop("skater.pat", gesture);
        let proof = bench.perform(Stage::pop(boost), 300, |v| pad(ground_flick(&scoop, v.tick, false)));
        (target, proof)
    });
}

/// The Trick Analyzer shows a trace on cMsgTrickBegun, the carrier's announcement (hud_meters.rs). A flip is
/// announced in the air, well before its record is credited at the landing (what the analyzer waited for before).
#[test]
#[ignore = "requires private stock graphs and animation assets"]
fn a_flip_is_announced_long_before_it_is_credited() {
    let bench = Bench::load();
    let scoop = bench.scoop("skater.pat", "Kickflip");
    let proof = bench.perform(Stage::pop(8.0), 300, |v| pad(ground_flick(&scoop, v.tick, false)));
    let (announced, credited) = (proof.announced_at["kickflip"], proof.credited_at[&Proof::id("kickflip")]);
    eprintln!("kickflip announced at tick {announced}, credited at tick {credited}");
    assert!(credited >= announced + 10, "{announced} vs {credited}: {}", proof.summary());
}

/// Held flicks walk the authored ladder; the pop decides where it stops (`End.Out.Out1` at
/// `TimeToLand < 0.525`, `Out2` at `< 0.8`), so each count is its own boost.
#[test]
#[ignore = "requires private stock graphs and animation assets"]
fn held_flips_credit_each_ladder_count() {
    let bench = Bench::load();
    let rows: &[(&str, &str, f32)] = &[
        ("Kickflip", "kickflip2", 3.0),
        ("Kickflip", "kickflip3", 5.5),
        ("Kickflip", "kickflip4", 8.0),
        ("Heelflip", "heelflip2", 3.0),
        ("Heelflip", "heelflip3", 6.5),
        ("Heelflip", "heelflip4", 10.0),
        ("N_Kickflip", "n_kickflip2", 3.0),
        ("N_Kickflip", "n_kickflip3", 5.5),
        ("N_Kickflip", "n_kickflip4", 8.0),
        ("N_Heelflip", "n_heelflip2", 3.0),
        ("N_Heelflip", "n_heelflip3", 6.5),
        ("N_Heelflip", "n_heelflip4", 10.0),
    ];
    check("held_flips_credit_each_ladder_count", rows, |&(gesture, target, boost)| {
        let scoop = bench.scoop("skater.pat", gesture);
        let proof = bench.perform(Stage::pop(boost), 320, |v| pad(ground_flick(&scoop, v.tick, true)));
        (target, proof)
    });
}

/// An ollie, then a `skater_air.pat` scoop once airborne: `air.xml`'s `Lateflip` state.
#[test]
#[ignore = "requires private stock graphs and animation assets"]
fn air_scoops_credit_the_late_flips() {
    let bench = Bench::load();
    let rows: &[(&str, &str)] = &[
        ("L_F_Kickflip", "latekickflip"),
        ("L_F_Heelflip", "lateheelflip"),
        ("L_B_Kickflip", "latebackfootkickflip"),
        ("L_B_Heelflip", "latebackfootheelflip"),
        ("L_FS_Shuvit", "latefsshuv"),
        ("L_BS_Shuvit", "latebsshuv"),
    ];
    check("air_scoops_credit_the_late_flips", rows, |&(gesture, target)| {
        let ollie = bench.scoop("skater.pat", "Ollie");
        let late = bench.scoop("skater_air.pat", gesture);
        let proof = bench.perform(Stage::pop(9.0), 300, |v| {
            let right = if v.airborne && v.air_ticks >= 10 {
                sweep(&late, 10, 4, v.air_ticks).unwrap_or([0; 2])
            } else {
                ground_flick(&ollie, v.tick, false)
            };
            pad(right)
        });
        (target, proof)
    });
}

/// A second scoop while the first flip is still running: `onboardgeneral.xml`'s `Underflips`
/// turns it into a `U_*` intent and `MonitorUnderflip` latches the request. The flip count the
/// underflip catches on is the ladder's, so the ladder rungs use the held-flip boosts.
#[test]
#[ignore = "requires private stock graphs and animation assets"]
fn a_second_scoop_mid_flip_credits_the_underflip() {
    let bench = Bench::load();
    let rows: &[(&str, bool, f32, &str)] = &[
        ("Kickflip", false, 8.0, "kickflip_underflip"),
        ("Kickflip", true, 3.0, "kickflip2_underflip"),
        ("Kickflip", true, 5.5, "kickflip3_underflip"),
        ("Kickflip", true, 8.0, "kickflip4_underflip"),
        ("Heelflip", false, 8.0, "heelflip_underflip"),
        ("Heelflip", true, 3.0, "heelflip2_underflip"),
        ("Heelflip", true, 6.5, "heelflip3_underflip"),
        ("Heelflip", true, 10.0, "heelflip4_underflip"),
        ("N_Kickflip", false, 8.0, "n_kickflip_underflip"),
        ("N_Kickflip", true, 3.0, "n_kickflip2_underflip"),
        ("N_Kickflip", true, 5.5, "n_kickflip3_underflip"),
        ("N_Kickflip", true, 8.0, "n_kickflip4_underflip"),
        ("N_Heelflip", false, 8.0, "n_heelflip_underflip"),
        ("N_Heelflip", true, 3.0, "n_heelflip2_underflip"),
        ("N_Heelflip", true, 6.5, "n_heelflip3_underflip"),
        ("N_Heelflip", true, 10.0, "n_heelflip4_underflip"),
        ("PopShuvit", false, 8.0, "popshuvit_underflip"),
        ("FSPopShuvit", false, 8.0, "fspopshuvit_underflip"),
        ("N_PopShuvit", false, 8.0, "n_popshuvit_underflip"),
        ("N_FSPopShuvit", false, 8.0, "n_fspopshuvit_underflip"),
    ];
    check("a_second_scoop_mid_flip_credits_the_underflip", rows, |&(gesture, hold, boost, target)| {
        let scoop = bench.scoop("skater.pat", gesture);
        let under = bench.scoop("skater_air.pat", "L_F_Kickflip");
        let proof = bench.perform(Stage::pop(boost), 320, |v| {
            let right = if v.airborne && v.air_ticks >= 8 {
                sweep(&under, 8, 3, v.air_ticks)
                    .unwrap_or(if hold { raw(scoop[scoop.len() - 1]) } else { [0; 2] })
            } else {
                ground_flick(&scoop, v.tick, hold)
            };
            pad(right)
        });
        (target, proof)
    });
}

/// RB armed before the pop, then the ordinary scoop: `gesture_mapping::select`'s dark-catch
/// column and `MonitorUnderflip`'s dark latch. Ladder counts are the held-flip boosts again.
#[test]
#[ignore = "requires private stock graphs and animation assets"]
fn a_flip_with_rb_held_credits_the_dark_catch() {
    let bench = Bench::load();
    let rows: &[(&str, bool, f32, &str)] = &[
        ("Kickflip", false, 12.0, "kickflip_darkcatch"),
        ("Kickflip", true, 3.0, "kickflip2_darkcatch"),
        ("Kickflip", true, 5.5, "kickflip3_darkcatch"),
        ("Kickflip", true, 8.0, "kickflip4_darkcatch"),
        ("Heelflip", false, 12.0, "heelflip_darkcatch"),
        ("Heelflip", true, 3.0, "heelflip2_darkcatch"),
        ("Heelflip", true, 6.5, "heelflip3_darkcatch"),
        ("Heelflip", true, 10.0, "heelflip4_darkcatch"),
        ("N_Kickflip", false, 12.0, "n_kickflip_darkcatch"),
        ("N_Kickflip", true, 3.0, "n_kickflip2_darkcatch"),
        ("N_Kickflip", true, 5.5, "n_kickflip3_darkcatch"),
        ("N_Kickflip", true, 8.0, "n_kickflip4_darkcatch"),
        ("N_Heelflip", false, 12.0, "n_heelflip_darkcatch"),
        ("N_Heelflip", true, 3.0, "n_heelflip2_darkcatch"),
        ("N_Heelflip", true, 6.5, "n_heelflip3_darkcatch"),
        ("N_Heelflip", true, 10.0, "n_heelflip4_darkcatch"),
        ("360Flip", false, 12.0, "360flip_darkcatch"),
        ("360Hardflip", false, 12.0, "360hardflip_darkcatch"),
        ("360InwardHeelflip", false, 12.0, "360inwardheelflip_darkcatch"),
        ("Laserflip", false, 12.0, "laserflip_darkcatch"),
        ("N_360Flip", false, 12.0, "n_360flip_darkcatch"),
        ("N_360Hardflip", false, 12.0, "n_360hardflip_darkcatch"),
        ("N_360InwardHeelflip", false, 12.0, "n_360inwardheelflip_darkcatch"),
        ("N_Laserflip", false, 12.0, "n_laserflip_darkcatch"),
    ];
    check("a_flip_with_rb_held_credits_the_dark_catch", rows, |&(gesture, hold, boost, target)| {
        let scoop = bench.scoop("skater.pat", gesture);
        let proof = bench.perform(Stage::pop(boost), 320, |v| XboxState {
            buttons: if v.tick >= 95 { RB } else { 0 },
            ..pad(ground_flick(&scoop, v.tick, hold))
        });
        (target, proof)
    });
}

/// `air.xml`'s `DarkLateflip` (a late scoop with RB held) goes dark; while still airborne and
/// dark, a second late scoop takes `DarkToLightflip`.
#[test]
#[ignore = "requires private stock graphs and animation assets"]
fn dark_late_flips_and_their_dark_to_light_outs() {
    let bench = Bench::load();
    let rows: &[(&str, Option<&str>, &str)] = &[
        ("L_F_Kickflip", None, "lateflip_darkcatch"),
        ("L_F_Kickflip", Some("L_F_Kickflip"), "darktolightkickflip"),
        ("L_F_Kickflip", Some("L_F_Heelflip"), "darktolightheelflip"),
        ("L_F_Kickflip", Some("L_BS_Shuvit"), "darktolightbsshuv"),
        ("L_F_Kickflip", Some("L_FS_Shuvit"), "darktolightfsshuv"),
    ];
    check("dark_late_flips_and_their_dark_to_light_outs", rows, |&(first, second, target)| {
        let ollie = bench.scoop("skater.pat", "Ollie");
        let a = bench.scoop("skater_air.pat", first);
        let b = second.map(|s| bench.scoop("skater_air.pat", s)).unwrap_or_default();
        let proof = bench.perform(Stage::pop(16.0), 360, |v| {
            let right = if !v.airborne {
                ground_flick(&ollie, v.tick, false)
            } else if let Some(r) = sweep(&a, 10, 4, v.air_ticks) {
                r
            } else {
                sweep(&b, 50, 4, v.air_ticks).unwrap_or([0; 2])
            };
            XboxState {
                buttons: if v.airborne && (6..40).contains(&v.air_ticks) { RB } else { 0 },
                ..pad(right)
            }
        });
        (target, proof)
    });
}

/// `air.xml`'s `NewOneFoot`: a push button pressed in the air with no grab held.
#[test]
#[ignore = "requires private stock graphs and animation assets"]
fn a_push_in_the_air_credits_the_one_foot_ollie() {
    let bench = Bench::load();
    let rows: &[(u16, &str)] = &[(A, "ollie_onefoot_front"), (X, "ollie_onefoot_back")];
    check("a_push_in_the_air_credits_the_one_foot_ollie", rows, |&(button, target)| {
        let ollie = bench.scoop("skater.pat", "Ollie");
        let proof = bench.perform(Stage::pop(9.0), 300, |v| XboxState {
            buttons: if v.airborne && (10..40).contains(&v.air_ticks) { button } else { 0 },
            ..pad(ground_flick(&ollie, v.tick, false))
        });
        (target, proof)
    });
}

// ---------------------------------------------------------------------------------------------
// Ground: ollie, manuals, powerslides, plants, the hippy jump, reverts, ground grabs

/// The ollie and the ground collector's flag slots: a held partial right stick is a manual
/// (`manual_playback.rs`), the left stick swung across then down is a powerslide
/// (`powerslide_playback.rs`, ID_LOADING_TIP_07_DESC).
#[test]
#[ignore = "requires private stock graphs and animation assets"]
fn the_ollie_manuals_and_powerslides_credit_their_scorable() {
    let bench = Bench::load();
    enum Row {
        Ollie,
        Manual(i16),
        Slide(i16),
    }
    let rows = [
        (Row::Ollie, "ollie"),
        (Row::Manual(22000), "tailmanual"),
        (Row::Manual(-22000), "nosemanual"),
        (Row::Slide(1), "fspowerslide"),
        (Row::Slide(-1), "bspowerslide"),
    ];
    check("the_ollie_manuals_and_powerslides_credit_their_scorable", &rows, |(row, target)| {
        let ollie = bench.scoop("skater.pat", "Ollie");
        let proof = bench.perform(Stage::ROLL, 320, |v| match row {
            Row::Ollie => pad(ground_flick(&ollie, v.tick, false)),
            Row::Manual(y) => pad(if (110..260).contains(&v.tick) { [0, *y] } else { [0; 2] }),
            Row::Slide(side) => XboxState {
                buttons: 0,
                triggers: [0; 2],
                left: match v.tick {
                    72..=83 => [side * 32767, 0],
                    96..=155 => [side * 22000, -32767],
                    _ => [0; 2],
                },
                right: [0; 2],
            },
        });
        (*target, proof)
    });
}

/// User 10-15 run13 (Skate 1's Manuals 101, fp_u3_tutorial01): a kickflip landed into a held manual. The lesson's
/// "stay in the manual" fails on any trickcomplete or sequencecomplete while it is held, and wants
/// Trick.IsDoingTrickType(Manual) (the scorer's held tricks) through its 0.5 s timer and the HUD's hudcomplete. The
/// scorer holds the manual and ends no trick, and the manual connects (82DAB2A8 with +945 set): the kickflip's
/// sequence stays open while the manual is held.
#[test]
#[ignore = "requires private stock graphs and animation assets"]
fn a_kickflip_lands_into_a_held_manual() {
    let bench = Bench::load();
    let scoop = bench.scoop("skater.pat", "Kickflip");
    let proof = bench.perform(Stage::pop(4.0), 520, |v| {
        pad(if v.tick < 140 { ground_flick(&scoop, v.tick, false) } else if v.tick < 460 { [0, 22000] } else { [0; 2] })
    });
    let trace: Vec<String> = proof.ticks.iter().filter(|t| t.0 >= 100).map(|t| format!("{t:?}")).collect();
    let trace = trace.join("\n");
    let manual = |held: &[&str]| held.iter().any(|h| h.contains("manual"));
    let land = proof.ticks.iter().position(|t| t.0 > 130 && !t.1 && proof.ticks.iter().any(|a| a.0 < t.0 && a.1)).expect("landed");
    let start = proof.ticks[land..].iter().position(|t| manual(&t.2)).map(|i| land + i).unwrap_or_else(|| panic!("no manual:\n{trace}"));
    let held = &proof.ticks[start..];
    let end = held.iter().position(|t| !manual(&t.2)).unwrap_or(held.len());
    assert!(proof.announced.contains(&"kickflip"), "{}", proof.summary());
    assert!(held[end - 1].0 >= 400, "the manual dropped at tick {}:\n{trace}", held[end - 1].0);
    let early: Vec<_> = held[..end].iter().filter(|t| !t.3.is_empty()).collect();
    assert!(early.is_empty(), "ended a trick while the manual was held: {early:?}\n{trace}");
    assert!(!held[..end].iter().any(|t| t.4), "published while the manual was held:\n{trace}");
}

/// User 10-24 (online Spot Battle on a Skate 3 map, "ours too low"). Retail's input +2788 bit 0x40000000 is the
/// player record's ManualsAreConnectors byte (8275F430; 824FA9A0 sets it, a challenge's spot volumes set it inside
/// them), and with it 82DAB2A8 holds a sequence open on fewer than three wheels: a kickflip landed into a manual,
/// the manual and a flip out of it are one sequence. At x1, as online, that sequence's SequenceScore is what a
/// one-sequence Spot Battle turn gets.
#[test]
#[ignore = "requires private stock graphs and animation assets"]
fn an_online_line_through_a_manual_is_one_sequence() {
    let bench = Bench::load();
    let scoop = bench.scoop("skater.pat", "Kickflip");
    let run = |connect: bool, flip_out: bool| {
        let proof = bench.perform(Stage::pop(4.0).online().manuals_connect(connect), 600, |v| {
            pad(match v.tick {
                0..140 => ground_flick(&scoop, v.tick, false),
                400.. if flip_out => ground_flick(&scoop, v.tick - 300, false),
                140..460 => [0, 22000],
                _ => [0; 2],
            })
        });
        eprintln!("ONLINE_LINE connect={connect} flip_out={flip_out} sequences={:?} announced={:?}", proof.sequences, proof.announced);
        proof.sequences
    };
    for flip_out in [false, true] {
        let connected = run(true, flip_out);
        assert_eq!(connected.len(), 1, "one sequence: {connected:?}");
        // +945 clear (the scorer before 2026-10-25; ots_dwtn_02_ol..): the kickflip's sequence closes 8 frames into
        // the manual, and a one-sequence turn scores the kickflip alone.
        let cleared = run(false, flip_out);
        assert!(cleared[0].1 < connected[0].1, "the kickflip alone: {cleared:?} vs {connected:?}");
    }
}

/// A grab trigger held fully (a ground grab needs exactly 1.0), then a push: `PushTrick`'s
/// bonelesses and fastplants. Both pushes with no trigger, then release: the hippy jump's charge.
#[test]
#[ignore = "requires private stock graphs and animation assets"]
fn ground_plants_and_the_hippy_jump_credit_their_scorable() {
    let bench = Bench::load();
    let rows: &[(&[usize], u16, &str)] = &[
        (&[0], A, "fsboneless"),
        (&[0], X, "fsfastplant"),
        (&[1], A, "bsboneless"),
        (&[1], X, "bsfastplant"),
        (&[], A | X, "hippyjump"),
        // `coffin.xml` on the ground: both ground grabs and both pushes; the ground collector's
        // slot 2 credits it (descriptor 60 with the grab bit, 82DAAF1C).
        (&[0, 1], A | X, "coffin"),
    ];
    check("ground_plants_and_the_hippy_jump_credit_their_scorable", rows, |&(hands, buttons, target)| {
        let proof = bench.perform(Stage::ROLL, 320, |v| {
            let mut triggers = [0u8; 2];
            if v.tick >= 120 {
                for &hand in hands {
                    triggers[hand] = 255;
                }
            }
            XboxState {
                buttons: if (150..190).contains(&v.tick) { buttons } else { 0 },
                triggers,
                left: [0; 2],
                right: [0; 2],
            }
        });
        (target, proof)
    });
}

/// `revert.xml`'s `Fs`/`Bs`, gated on the left-stick `SlideFs180`/`SlideBs180` of
/// `skaterls.pat`; credited by the ground collector's revert slot on State70's falling edge.
/// Entered from the pattern's deflected entry coordinate, one coordinate per tick
/// (`docs/trick-input-recipes.md`).
#[test]
#[ignore = "requires private stock graphs and animation assets"]
fn left_stick_slides_credit_both_reverts() {
    let bench = Bench::load();
    let rows: &[(&str, &str)] = &[("SlideFs180", "fsrevert"), ("SlideBs180", "bsrevert")];
    check("left_stick_slides_credit_both_reverts", rows, |&(gesture, target)| {
        let slide = bench.scoop("skaterls.pat", gesture);
        let proof = bench.perform(Stage::ROLL, 320, |v| {
            let left = if v.tick >= 120 {
                slide.get((v.tick - 120).saturating_sub(30) as usize).copied().map(raw).unwrap_or([0; 2])
            } else {
                [0; 2]
            };
            XboxState { buttons: 0, triggers: [0; 2], left, right: [0; 2] }
        });
        (target, proof)
    });
}

/// Not creditable in retail, by design: `GroundGrabs/*.xml` publish `gnd_*` only through
/// `ScoringGrabs`, whose ISkaterMotionGraph+232 (8258F9B8) sets the grab bit 0x02000000, and the
/// scorer's descriptor lookup (82DAC5D0..5FC) drops ids 67..69 whenever that bit is set. So the
/// graph names them while held, and no collector ever credits them. This pins that behaviour.
#[test]
#[ignore = "requires private stock graphs and animation assets"]
fn held_ground_grabs_are_named_but_never_credited() {
    let bench = Bench::load();
    for (hands, target) in [(&[0][..], "gnd_fsgrab"), (&[1], "gnd_bsgrab"), (&[0, 1], "gnd_dblgrab")] {
        let ollie = bench.scoop("skater.pat", "Ollie");
        let proof = bench.perform(Stage::ROLL, 320, |v| {
            let mut triggers = [0u8; 2];
            if v.tick >= 80 && v.tick < 200 {
                for &hand in hands {
                    triggers[hand] = 255;
                }
            }
            XboxState { triggers, ..pad(ground_flick(&ollie, v.tick.saturating_sub(60), false)) }
        });
        assert!(proof.published.contains(&target), "{target} published: {}", proof.summary());
        assert!(!proof.credited.contains_key(&Proof::id(target)), "{target} credited: {}", proof.summary());
        eprintln!("RETAIL-UNCREDITED {target}");
    }
}

// ---------------------------------------------------------------------------------------------
// Grabs

const UP: [i16; 2] = [0, 32767];
const DOWN: [i16; 2] = [0, -32767];
const LEFT: [i16; 2] = [-32767, 0];
const RIGHT: [i16; 2] = [32767, 0];
const CENTRE: [i16; 2] = [0, 0];
const fn diag(a: [i16; 2], b: [i16; 2]) -> [i16; 2] {
    [((a[0] as i32 + b[0] as i32) * 7 / 10) as i16, ((a[1] as i32 + b[1] as i32) * 7 / 10) as i16]
}
/// `select_grab_score`'s windows are not symmetric about the stick's axis: the up window is
/// 44..134 degrees, so a 45-degree diagonal lands in it on the right side but outside it on the
/// left. A right-hand tweak therefore has to lean further from the held direction.
const fn lean_right(held: [i16; 2]) -> [i16; 2] {
    [28377, if held[1] > 0 { 16384 } else { -16384 }]
}

/// One air grab: `stick` leads the trigger by `STICK_LEAD` ticks (`docs/trick-input-recipes.md`:
/// stick before trigger), then after `SETTLE` held ticks either the fingerflip `scoops` play or
/// the stick moves to `tweak`, and `buttons` go down.
#[derive(Clone, Copy)]
struct Grab {
    hands: &'static [usize],
    stick: [i16; 2],
    tweak: Option<[i16; 2]>,
    scoops: &'static [&'static str],
    buttons: u16,
    boost: f32,
    target: &'static str,
}
const fn grab(hands: &'static [usize], stick: [i16; 2], target: &'static str) -> Grab {
    Grab { hands, stick, tweak: None, scoops: &[], buttons: 0, boost: 13.0, target }
}
impl Grab {
    const fn tweak(mut self, to: [i16; 2]) -> Self {
        self.tweak = Some(to);
        self
    }
    const fn press(mut self, buttons: u16) -> Self {
        self.buttons = buttons;
        self
    }
    const fn scoop(mut self, scoops: &'static [&'static str], boost: f32) -> Self {
        self.scoops = scoops;
        self.boost = boost;
        self
    }
}

const STICK_LEAD: u32 = 6;
const SETTLE: u32 = 22;
const SCOOP_GAP: u32 = 30;

fn perform_grab(bench: &Bench, g: &Grab) -> Proof {
    let ollie = bench.scoop("skater.pat", "Ollie");
    // As grab_playback.rs: each fingerflip scoop is the authored variant starting nearest the
    // held (Y-negated) point, one coordinate every 3 ticks, then held through the gap.
    let mut from = [g.stick[0] as f32 / 32767., -(g.stick[1] as f32) / 32767.];
    let mut track: Vec<[f32; 2]> = Vec::new();
    for name in g.scoops {
        let mut variants = bench.scoops("skater_fingerflip.pat", name);
        let d = |p: [f32; 2]| (p[0] - from[0]).hypot(p[1] - from[1]);
        variants.sort_by(|a, b| d(a[0]).partial_cmp(&d(b[0])).unwrap());
        let chosen = variants.swap_remove(0);
        for p in &chosen {
            track.extend([*p; 3]);
        }
        from = *chosen.last().unwrap();
        track.extend(std::iter::repeat_n(from, SCOOP_GAP as usize));
    }
    bench.perform(Stage::pop(g.boost), 360, |v| {
        if !v.airborne {
            return pad(ground_flick(&ollie, v.tick, false));
        }
        let mut triggers = [0u8; 2];
        if v.air_ticks >= STICK_LEAD {
            for &hand in g.hands {
                triggers[hand] = 255;
            }
        }
        let right = if v.air_ticks < SETTLE {
            g.stick
        } else if !track.is_empty() {
            let i = (v.air_ticks - SETTLE) as usize;
            raw(*track.get(i).unwrap_or(track.last().unwrap()))
        } else {
            g.tweak.unwrap_or(g.stick)
        };
        XboxState {
            buttons: if v.air_ticks >= SETTLE { g.buttons } else { 0 },
            triggers,
            left: [0; 2],
            right,
        }
    })
}

/// Every air grab, its directional tweaks and its one-foot/no-foot additions.
#[test]
#[ignore = "requires private stock graphs and animation assets"]
fn air_grabs_and_their_tweaks_credit_their_scorable() {
    let bench = Bench::load();
    let rows = [
        grab(&[0], CENTRE, "fsgrab"),
        grab(&[0], CENTRE, "fsgrab_up").tweak(UP),
        grab(&[0], CENTRE, "fsgrab_down").tweak(DOWN),
        grab(&[0], CENTRE, "fsgrab_left").tweak(LEFT),
        grab(&[0], CENTRE, "fsgrab_right").tweak(RIGHT),
        grab(&[1], CENTRE, "bsgrab"),
        grab(&[1], CENTRE, "bsgrab_up").tweak(UP),
        grab(&[1], CENTRE, "bsgrab_down").tweak(DOWN),
        grab(&[1], CENTRE, "bsgrab_left").tweak(LEFT),
        grab(&[1], CENTRE, "bsgrab_right").tweak(RIGHT),
        grab(&[0, 1], CENTRE, "dblgrab"),
        grab(&[0], UP, "tailgrab"),
        grab(&[0], UP, "tailgrab_left").tweak(diag(UP, LEFT)),
        grab(&[0], UP, "tailgrab_right").tweak(lean_right(UP)),
        grab(&[1], UP, "seatbeltgrab"),
        grab(&[1], UP, "seatbeltgrab_left").tweak(diag(UP, LEFT)),
        grab(&[1], UP, "seatbeltgrab_right").tweak(lean_right(UP)),
        grab(&[1], DOWN, "nosegrab"),
        grab(&[1], DOWN, "nosegrab_left").tweak(diag(DOWN, LEFT)),
        grab(&[1], DOWN, "nosegrab_right").tweak(lean_right(DOWN)),
        grab(&[0], DOWN, "crailgrab"),
        grab(&[0], DOWN, "crailgrab_left").tweak(diag(DOWN, LEFT)),
        grab(&[0], DOWN, "crailgrab_right").tweak(lean_right(DOWN)),
        grab(&[0, 1], DOWN, "rocketair"),
        grab(&[0], CENTRE, "nofoot_air").press(B),
        grab(&[1], CENTRE, "christ_air").press(B),
        grab(&[0], CENTRE, "fs_onefoot_right").press(A),
        grab(&[0], CENTRE, "fs_backfoot").press(X),
        // The bs grab's feet are named from the other side: `FSGrab.OneFootAir.BSRightFoot`.
        grab(&[1], CENTRE, "bs_onefoot_behind").press(A),
        grab(&[1], CENTRE, "bs_onefoot_front").press(X),
        grab(&[0, 1], CENTRE, "superman").press(B),
        grab(&[0, 1], CENTRE, "coffin").press(A | X),
        grab(&[0], UP, "tailgrab_onefoot_behind").press(A),
        grab(&[0], UP, "tailgrab_onefoot_front").press(X),
        grab(&[0], UP, "tailgrab_airwalk").press(B),
        grab(&[1], DOWN, "nosegrab_onefoot_behind").press(A),
        grab(&[1], DOWN, "nosegrab_onefoot_front").press(X),
        grab(&[1], DOWN, "nosegrab_airwalk").press(B),
    ];
    check("air_grabs_and_their_tweaks_credit_their_scorable", &rows, |g| (g.target, perform_grab(&bench, g)));
}

/// Grab shove-its: the fingerflip scoops of `skater_fingerflip.pat`, thrown with the trigger
/// still held, including the grab-to-grab crossings and the stale/mute chain.
#[test]
#[ignore = "requires private stock graphs and animation assets"]
fn grab_fingerflips_credit_their_scorable() {
    let bench = Bench::load();
    let rows = [
        grab(&[0], CENTRE, "fsgrab_fingerflip").scoop(&["Fingerflip"], 13.0),
        grab(&[1], CENTRE, "bsgrab_fingerflip").scoop(&["Fingerflip"], 13.0),
        grab(&[0], UP, "tailgrab_fingerflip").scoop(&["Fingerflip"], 13.0),
        grab(&[1], DOWN, "nosegrab_fingerflip").scoop(&["Fingerflip"], 13.0),
        grab(&[0], CENTRE, "fsgrabvarial_fingerflip").scoop(&["BS_Varial"], 13.0),
        grab(&[1], CENTRE, "bsgrabvarial_fingerflip").scoop(&["BS_Varial"], 13.0),
        grab(&[1], UP, "seatbelttonose_fingerflip").scoop(&["BS_Varial"], 13.0),
        grab(&[0], DOWN, "crailtotail_fingerflip").scoop(&["BS_Varial"], 13.0),
        grab(&[0], CENTRE, "fsgrabtostalegrab_fingerflip").scoop(&["FS_Varial"], 13.0),
        grab(&[1], CENTRE, "bsgrabtomutegrab_fingerflip").scoop(&["FS_Varial"], 13.0),
        grab(&[0], CENTRE, "stalegrab").scoop(&["FS_Varial"], 13.0),
        grab(&[1], CENTRE, "mutegrab").scoop(&["FS_Varial"], 13.0),
        grab(&[0], CENTRE, "stalegrab_fingerflip").scoop(&["FS_Varial", "Fingerflip"], 20.0),
        grab(&[1], CENTRE, "mutegrab_fingerflip").scoop(&["FS_Varial", "Fingerflip"], 20.0),
        grab(&[0], CENTRE, "stalegrabtofsgrab_fingerflip").scoop(&["FS_Varial", "FS_Varial"], 20.0),
        grab(&[1], CENTRE, "mutegrabtobsgrab_fingerflip").scoop(&["FS_Varial", "FS_Varial"], 20.0),
    ];
    check("grab_fingerflips_credit_their_scorable", &rows, |g| (g.target, perform_grab(&bench, g)));
}
