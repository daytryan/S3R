//! The Skate School lessons in the game (plan Tasks 25, 28, 29; spec 12): a career row whose port is
//! [`Port::Lesson`] (tut_sksc_01, 02, 06, 03, 04) runs retail's `tutorials` graph ([`Lesson`]) on the skater, its
//! objectives from the export's `rows/<row>.json` on [`skate3_objectives::Group`], and answers the career with
//! [`ChallengeResult`]. Driven as skate3_film drives a Film challenge (its `run` / `step` / `begin` / `end`), without
//! match_core: the lesson moves the skater itself. Not tested in play.
//!
//! One group per lesson: the export writes each objective record as a group of one; MakeObjectives(LocalPlayerID)
//! makes one group of all of them (a master's completion marks every other objective, stage1_school 3.1), under the
//! row's ObjectiveFlow. The skater natives the lessons' snippets call that [`Sense`] lacks are [`Answers`]. The
//! objectives' intro, complete and hint groups play on a [`Presenter`] whose [`LessonStage`] performs the input
//! restriction and the pin; their other events are logged.
//!
//! The graph's natives with no engine hook in this build are logged and skipped ([`Cmd`]: the HUD groups, the
//! indicators, HOM, the coach's speech, Progression's lock, the trick analyzer and manual meter). A presentation
//! group plays its NIS (event 5) through the cutscene player from the export's `scenes/tutorial.json`, else ends at
//! once. The end dialog 33 has no career screen yet: Continue is taken at once.
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use bevy::prelude::*;
use serde::Deserialize;
use serde_json::Value as Json;

use super::data::{CareerData, Event as PresEvent, Locator};
use super::glue::{Career, ChallengeResult, StartRow};
use super::mode::ModeExit;
use super::rules::Outcome;
use crate::{
    challenge::{LessonMenu, LessonRequest, cutscene::Cutscene},
    input::{ControllerInput, PublishedTickInput},
    map_transition::MapTransition,
    physics::{PlayerControls, SkaterRuntime},
    session_marker::SessionMarker,
    skate3_film::{self, Frame, Sense},
    skate3_modes::{self, ChallengeFade, Graph, Msg, Rt, Volume},
    skate3_objectives::{Group, GroupDef, ObjectiveDef, Out, Skater, Value},
    skate3_present::{Presenter, Stage, Wait},
    skate3_tutorial::{self, Cmd, Lesson, Port, Pres, St},
    tip_loader::LoadingHold,
};

/// Online.GetLocalPlayerID: single player.
const ME: usize = 0;
/// The objective group pointer the graph hears (tutorials.lua compares it only with its own).
const GROUP: u32 = 100;
/// PhysicalStateId category Air (GetStateCategory; skate3_film's AIR).
const AIR: u32 = 200;
/// Presentation event NIS (presentation.md 1.1; stage1_school 3: `NIS tut_sksc_01`).
const NIS: u32 = 5;
/// shortcut: a scene the cutscene player never reports ended is cut this long after its length; drop it once the
/// player is known to end every career scene.
const SCENE_SLACK: f64 = 2.;

// ---------------------------------------------------------------- data (rows/<row>.json, plan Task 6)
#[derive(Deserialize)]
struct RowFile {
    /// {"Objectives": [{PointRequirement, objectives}, ...]}.
    groups: Json,
    points_objective: ObjectiveDef,
    #[serde(default)]
    trigger_volumes: BTreeMap<String, Option<Volume>>,
    /// skatercollections: trick group -> its scorables.
    #[serde(default)]
    trick_group_members: HashMap<String, Vec<String>>,
    #[serde(default)]
    presentation: Presentation,
    /// ObjectiveFlow {type, count}.
    flow: Option<[u32; 2]>,
}

/// The row's IntroPresentationEvents / RestartPresentationEvents (Outro is empty for every lesson, stage1_school 3).
#[derive(Deserialize, Default)]
struct Presentation {
    #[serde(rename = "Intro", default)]
    intro: Vec<PresEvent>,
    #[serde(rename = "Restart", default)]
    restart: Vec<PresEvent>,
}

/// MakeObjectives' one group: every exported group's objectives in order.
/// The lessons have no PointRequirement (stage1_school 3), so no points objective.
fn lesson_group(groups: &Json) -> Result<GroupDef, String> {
    let mut objectives = Vec::new();
    for g in groups.get("Objectives").and_then(Json::as_array).ok_or("no Objectives group")? {
        for o in g.get("objectives").and_then(Json::as_array).ok_or("an objective group without objectives")? {
            objectives.push(serde_json::from_value::<Option<ObjectiveDef>>(o.clone()).map_err(|e| e.to_string())?);
        }
    }
    Ok(GroupDef { point_requirement: None, objectives })
}

/// self.VisualIndicators.Count. INFERRED: the row's own VisualIndicators are its export entries without an objective
/// (4 for tut_sksc_02, 6 for tut_sksc_04, as stage1_school 3 lists them); the rest are the objectives'.
fn own_indicators(d: &CareerData, id: &str) -> usize {
    d.indicators.get(id).map_or(0, |all| all.iter().filter(|i| i.objective.is_empty()).count())
}

/// RepositionAI's slot 0 of the StartLocation (online_map::locator_slot): the locator's first child, else itself.
fn start_of(l: &Locator) -> [[f32; 4]; 4] {
    let mut at = l.matrix;
    if let Some(slot) = l.slots.first().filter(|s| s.len() == 4 && s.iter().all(|r| r.len() == 4)) {
        for (row, values) in at.iter_mut().zip(slot) {
            row.copy_from_slice(values);
        }
    }
    at
}

fn outcome_of(o: skate3_tutorial::Outcome) -> Outcome {
    match o {
        skate3_tutorial::Outcome::Owned => Outcome::Owned,
        skate3_tutorial::Outcome::Abandoned => Outcome::Abandoned,
    }
}

// ---------------------------------------------------------------- the skater, as the lessons' snippets see it
/// What one physics tick adds to skate3_film's [`Frame`] for the lessons.
#[derive(Clone, Default)]
struct Extra {
    in_air: bool,
    /// The row's volumes the skater is in (instance ids), as the Frame's.
    inside: Vec<u64>,
    /// The scorables announced this tick.
    announced: Vec<&'static str>,
    /// The session marker's sets / uses so far.
    markers: (u64, u64),
    /// The right stick's Y, up positive (the tick's gameplay action 4).
    right_y: f32,
}

#[derive(Default)]
struct Tick {
    frame: Frame,
    extra: Extra,
}

/// The tick before, for the group's events.
#[derive(Default)]
struct Was {
    seen: bool,
    open: bool,
    wiping: bool,
    inside: Vec<u64>,
    markers: (u64, u64),
}

/// [`Sense`] plus the skater natives the lessons call that it lacks (plan Task 25's seven less the group's three).
struct Answers<'a> {
    sense: &'a mut Sense,
    in_air: bool,
    on_board: bool,
    speed: f32,
    right_y: f32,
}

impl Skater for Answers<'_> {
    fn native(&mut self, name: &str, args: &[Value]) -> Option<Value> {
        Some(match name {
            // INFERRED: on the board is not off it (ObjectiveLocomotion.IsOffBoard, category 500).
            "ObjectiveLocomotion.IsOnBoard" => Value::Bool(self.on_board),
            "ObjectiveLocomotion.IsInAir" => Value::Bool(self.in_air),
            // INFERRED: the board's speed (physical skateboard +0x80), the unit not traced; only a hint reads it.
            "ObjectiveLocomotion.GetSpeed" => Value::Num(self.speed),
            // INFERRED: the raw value is the conditioned stick (xbox.rs condition), -1 to 1; the lessons compare it with
            // -1.0 and 1.0 only.
            "Input.GetRawRightStickYValue" => Value::Num(self.right_y),
            _ => return self.sense.native(name, args),
        })
    }
    fn in_group(&self, group: &str, trick: &str) -> bool {
        self.sense.in_group(group, trick)
    }
}

/// One tick of the skater through the lesson's group: its messages. skate3_film::feed's order (sequencebegin, the
/// volumes entered, the tricks, sequencecomplete, wipeoutover, the marker, then the per-frame update 0x82c6f998), with
/// [`Answers`] for the skater; film's feed with no groups steps the [`Sense`].
fn feed(group: &mut Group, sense: &mut Sense, was: &mut Was, frame: Frame, x: &Extra) -> Vec<Out> {
    let (open, wiping, ended) = (frame.sequence_open(), frame.wiping(), frame.sequence_end(was.open));
    let (on_board, speed) = (!frame.off_board(), frame.speed());
    let mut none: [(u32, Group); 0] = [];
    let _ = skate3_film::feed(&mut none, sense, frame);
    let set = was.seen && x.markers.0 > was.markers.0;
    let used = was.seen && x.markers.1 > was.markers.1;
    let mut sk = Answers { sense, in_air: x.in_air, on_board, speed, right_y: x.right_y };
    let mut out = Vec::new();
    if open && !was.open {
        group.sequence_begin();
    }
    for &v in x.inside.iter().filter(|v| !was.inside.contains(v)) {
        out.extend(group.trigger_enter(v, &mut sk));
    }
    for t in &x.announced {
        group.trick(t);
    }
    if let Some((score, aborted)) = ended {
        out.extend(group.sequence_complete(score as i32, aborted, &mut sk));
    }
    if !wiping && was.wiping {
        group.wipeout_over();
    }
    if set {
        out.extend(group.marker(false, &mut sk));
    }
    if used {
        out.extend(group.marker(true, &mut sk));
    }
    out.extend(group.tick(&mut sk));
    *was = Was { seen: true, open, wiping, inside: x.inside.clone(), markers: x.markers };
    out
}

// ---------------------------------------------------------------- the objectives' groups
/// What the objectives' presentation groups do in this build: EnableInputRestriction / ClearInputRestrictions and
/// Pin / UnpinLocalPlayer reach the skater; every other event is logged and goes on at once (no engine hook yet, so a
/// coach's line does not hold its group).
#[derive(Default)]
struct LessonStage {
    /// In order: `Some(mask)` EnableInputRestriction, `None` ClearInputRestrictions; taken by the glue each frame.
    restrictions: Vec<Option<u32>>,
    /// The last Pin (true) / Unpin (false), until taken.
    pinned: Option<bool>,
    log: Vec<String>,
}

impl LessonStage {
    fn skip(&mut self, line: String) -> Wait {
        self.log.push(line);
        Wait::Done
    }
}

impl Stage for LessonStage {
    fn fade(&mut self, out: bool) -> Wait {
        self.skip(format!("fade out {out}"))
    }
    fn speech(&mut self, character: &str, id: u32, wait: bool) -> Wait {
        self.skip(format!("speech {character} {id} (wait {wait})"))
    }
    fn nis(&mut self, name: &str, option: &str) -> Wait {
        self.skip(format!("NIS {name} {option}"))
    }
    fn pin(&mut self, pinned: bool) -> Wait {
        self.pinned = Some(pinned);
        Wait::Done
    }
    fn teleport(&mut self, locator: &str) -> Wait {
        self.skip(format!("teleport {locator}"))
    }
    fn session_marker(&mut self, set: bool, enabled: bool) -> Wait {
        self.skip(format!("session marker (set {set}) enabled {enabled}"))
    }
    fn hud(&mut self, shown: bool) -> Wait {
        self.skip(format!("HUD shown {shown}"))
    }
    fn minimap(&mut self, shown: bool) -> Wait {
        self.skip(format!("minimap shown {shown}"))
    }
    fn hom_effect(&mut self, enabled: bool) -> Wait {
        self.skip(format!("HOM effect {enabled}"))
    }
    fn replay(&mut self) -> Wait {
        self.skip("replay".into())
    }
    fn callout(&mut self, text: Option<&str>) -> Wait {
        self.skip(format!("callout {text:?}"))
    }
    fn central(&mut self, text: &str) -> Wait {
        self.skip(format!("central message {text}"))
    }
    fn achievement(&mut self, name: &str) -> Wait {
        self.skip(format!("achievement {name}"))
    }
    fn trick_analyzer(&mut self, shown: bool) -> Wait {
        self.skip(format!("trick analyzer shown {shown}"))
    }
    fn manual_meter(&mut self, shown: bool) -> Wait {
        self.skip(format!("manual meter shown {shown}"))
    }
    fn sim_rate(&mut self, rate: Option<f32>) -> Wait {
        self.skip(format!("sim rate {rate:?}"))
    }
    fn minimal_scoring_hud(&mut self, on: bool) -> Wait {
        self.skip(format!("minimal scoring HUD {on}"))
    }
    fn input_restriction(&mut self, mask: Option<u32>) -> Wait {
        self.restrictions.push(mask);
        Wait::Done
    }
    fn fmv(&mut self, name: &str) -> Wait {
        self.skip(format!("FMV {name}"))
    }
    fn sfx(&mut self, a: &str, b: &str) -> Wait {
        self.skip(format!("SFX {a} {b}"))
    }
    fn icon(&mut self, add: bool, a: &str) -> Wait {
        self.skip(format!("icon {a} added {add}"))
    }
    fn map_tutorial(&mut self) -> Wait {
        self.skip("map tutorial".into())
    }
}

// ---------------------------------------------------------------- the glue
struct Play {
    id: String,
    g: Lesson,
    rt: Rt<St>,
    def: GroupDef,
    points: ObjectiveDef,
    /// ObjectiveFlow {type, count} (challenge_global_data).
    flow: [u32; 2],
    /// Objectives.MakeObjectives' group, until DeleteObjectiveGroup.
    group: Option<Group>,
    sense: Sense,
    was: Was,
    volumes: Vec<(u64, Volume)>,
    /// The StartLocation (RepositionAI's target) and whether the skater lands on its board (!SpawnOffboard).
    start: Option<[[f32; 4]; 4]>,
    on_board: bool,
    /// The last reposition carried out, and one waiting for the skater to be free.
    placed: u32,
    moving: Option<[[f32; 4]; 4]>,
    /// The export's scenes folder, and the Intro / Restart groups' NIS with its length.
    scenes: PathBuf,
    intro: Option<(String, f64)>,
    restart: Option<(String, f64)>,
    /// The group playing its scene, and when it is cut.
    shown: Option<(Pres, f64)>,
    /// The objectives' groups (StartPresentation 0x82dbaca0) and what they do here, and the last frame's time.
    presenter: Presenter,
    stage: LessonStage,
    last: f64,
    state: St,
}

/// The lesson the career started, if any.
#[derive(Resource, Default)]
pub(super) struct LessonRun {
    /// A lesson row accepted, waiting for its world load.
    pending: Option<String>,
    play: Option<Play>,
    probe: Vec<Tick>,
    volumes: Vec<(u64, Volume)>,
    live: bool,
    /// A group restricted or pinned the skater: both are released when the lesson ends.
    held: bool,
}

impl LessonRun {
    /// A lesson is starting or running (for the career's `Ready::challenge_running`).
    #[must_use]
    pub(super) fn running(&self) -> bool {
        self.play.is_some() || self.pending.is_some()
    }
}

/// The lesson row's graph and objectives (tutorials.lua `initial` entered).
fn begin(d: &CareerData, id: &str, now: f64) -> Result<Play, String> {
    let row = d.row(id).ok_or("not in career.json")?;
    let path = d.root.join("rows").join(format!("{id}.json"));
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let file: RowFile = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let def = lesson_group(&file.groups)?;
    let flow = file.flow.unwrap_or([0, 1]);
    // Built once here, so a snippet the runtime cannot read stops the start rather than MakeObjectives.
    Group::with_flow(ME, &def, &file.points_objective, flow)?;
    let scenes = d.root.join("scenes");
    let lengths: Option<Json> = std::fs::read(scenes.join("tutorial.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let scene = |events: &[PresEvent]| -> Option<(String, f64)> {
        let name = events.iter().find(|e| e.kind == NIS)?.args.first()?.clone();
        let length = lengths.as_ref()?.get("scenes")?.get(&name)?.get("duration")?.as_f64()?;
        Some((name, length))
    };
    let start = row.start.as_deref().and_then(|s| d.locators.get(s)).map(start_of);
    if start.is_none() {
        warn!("Skate 3 lesson {id}: StartLocation {:?} is not in the export; the skater stays where it is", row.start);
    }
    let mut g = Lesson::new(row.key, !file.presentation.intro.is_empty(), !file.presentation.restart.is_empty(), own_indicators(d, id));
    g.driven = true;
    let mut rt = Rt::new(St::Initial, 1, now);
    skate3_modes::start(&mut g, &mut rt);
    info!("Skate 3 lesson {id}: started ({} objectives)", def.objectives.len());
    Ok(Play {
        id: id.to_owned(),
        g,
        rt,
        points: file.points_objective,
        flow,
        def,
        group: None,
        sense: Sense::new(file.trick_group_members),
        was: Was::default(),
        volumes: file.trigger_volumes.iter().filter_map(|(k, v)| Some((k.parse().ok()?, v.clone()?))).collect(),
        start,
        on_board: !row.offboard,
        placed: 0,
        moving: None,
        intro: scene(&file.presentation.intro),
        restart: scene(&file.presentation.restart),
        scenes,
        shown: None,
        presenter: Presenter::default(),
        stage: LessonStage::default(),
        last: now,
        state: St::Initial,
    })
}

/// The graph's Objectives.* calls on the group; the rest logged (no engine hook in this build).
fn apply(play: &mut Play) {
    for c in std::mem::take(&mut play.g.cmds) {
        match c {
            Cmd::Make => match Group::with_flow(ME, &play.def, &play.points, play.flow) {
                Ok(g) => play.group = Some(g),
                Err(e) => warn!("Skate 3 lesson {}: {e}", play.id),
            },
            Cmd::Evaluate(on) => play.group.iter_mut().for_each(|g| g.enabled = on),
            Cmd::ResetAll => play.group.iter_mut().for_each(Group::reset),
            Cmd::Reset(i) => play.group.iter_mut().for_each(|g| g.reset_objective(i)),
            Cmd::Start => play.group.iter_mut().for_each(Group::activate_start),
            Cmd::Delete => play.group = None,
            other => info!("Skate 3 lesson {}: {other:?} (no engine hook in this build)", play.id),
        }
    }
}

/// Delivers what is due and applies the graph's calls until nothing is left (skate3_film's pump).
fn pump(play: &mut Play, now: f64) {
    for _ in 0..64 {
        apply(play);
        if play.rt.due().is_none_or(|d| d > now) {
            break;
        }
        skate3_modes::advance(&mut play.g, &mut play.rt, now);
    }
    apply(play);
}

/// ChallengePresentation.StartPresentation on a driven lesson: the group's NIS through the cutscene player, and
/// presentationeventgroupfinished when the scene ends; a group without one (or without the player) ends at once.
fn present(play: &mut Play, cutscene: Option<&mut Cutscene>, now: f64) {
    match (play.g.playing, play.shown) {
        (Some(p), None) => {
            let scene = match p {
                Pres::Intro => play.intro.clone(),
                Pres::Restart => play.restart.clone(),
                Pres::Outro => None,
            };
            match (scene, cutscene) {
                (Some((name, length)), Some(c)) => {
                    info!("Skate 3 lesson {}: {p:?} NIS {name}", play.id);
                    c.start(&play.scenes, &name);
                    play.shown = Some((p, now + length + SCENE_SLACK));
                }
                _ => {
                    skate3_modes::dispatch(&mut play.g, &mut play.rt, Msg::Presented);
                    pump(play, now);
                }
            }
        }
        (Some(p), Some((q, until))) if p == q => {
            if cutscene.as_deref().is_none_or(|c| c.ended() || !c.active()) || now >= until {
                if let Some(c) = cutscene {
                    c.stop();
                }
                play.shown = None;
                skate3_modes::dispatch(&mut play.g, &mut play.rt, Msg::Presented);
                pump(play, now);
            }
        }
        // RemoveAllPresentationGroups (complete), or another group took over.
        (_, Some(_)) => {
            if let Some(c) = cutscene {
                c.stop();
            }
            play.shown = None;
        }
        (None, None) => {}
    }
}

/// One frame of a running lesson.
fn step(play: &mut Play, now: f64, skater: Option<&mut SkaterRuntime>, cutscene: Option<&mut Cutscene>, menu: Option<&mut LessonMenu>, ticks: Vec<Tick>) {
    // The Start menu's Restart / Abandon Lesson: restartchallenge (L146-148) / abandonchallenge (L138-143).
    match menu.and_then(|m| m.request.take()) {
        Some(LessonRequest::Restart) => skate3_modes::dispatch(&mut play.g, &mut play.rt, Msg::Restart),
        Some(LessonRequest::Abandon) => play.g.abandon(&mut play.rt),
        None => {}
    }
    pump(play, now);
    present(play, cutscene, now);

    // This skater through the objectives.
    for Tick { frame, extra } in ticks {
        let Some(group) = play.group.as_mut() else { break };
        let outs = feed(group, &mut play.sense, &mut play.was, frame, &extra);
        for p in group.presentations() {
            let name = p.name.clone();
            if play.presenter.start(p) {
                info!("Skate 3 lesson {}: objective group {name}", play.id);
            }
        }
        for o in outs {
            match o {
                Out::AllComplete => play.rt.post(Msg::GroupAll { group: GROUP }),
                Out::Complete { index, .. } => info!("Skate 3 lesson {}: objective {index} complete", play.id),
                _ => {}
            }
        }
        pump(play, now);
    }

    let dt = (now - play.last) as f32;
    play.last = now;
    for name in play.presenter.tick(dt, &mut play.stage) {
        info!("Skate 3 lesson {}: objective group {name} ended", play.id);
    }
    for line in play.stage.log.drain(..) {
        info!("Skate 3 lesson {}: {line} (no engine hook in this build)", play.id);
    }

    // AI.RepositionAI(id, StartLocation, ...): the skater to the start at the fade's black, on foot for SpawnOffboard.
    if let Some((seq, _)) = play.rt.landed(ME)
        && play.placed != seq
    {
        play.placed = seq;
        play.moving = play.start;
    }
    if let Some(skater) = skater
        && let Some(at) = play.moving
        && !crate::match_core::teleport_blocked(skater)
    {
        play.moving = None;
        if let Err(e) = skater.travel_to_as(at, play.on_board) {
            warn!("Skate 3 lesson {}: teleport: {e}", play.id);
        }
    }

    // Dialog 33 (end_options L259-269). `Not retail (story mode stage 1 build: screens pending)`: the career has no
    // screen for it, so Continue (hudcontinue L278-280) is taken at once; Retry is the Start menu's Restart.
    if play.rt.dialog && play.rt.state == St::EndOptions {
        info!("Skate 3 lesson {}: dialog {} {}: Continue (not drawn in this build)", play.id, skate3_tutorial::END_DIALOG, skate3_tutorial::END_TITLE);
        skate3_modes::dispatch(&mut play.g, &mut play.rt, Msg::HudContinue);
        pump(play, now);
    }
    if play.rt.state != play.state {
        play.state = play.rt.state;
        info!("Skate 3 lesson {}: {}", play.id, <Lesson as Graph>::name(play.state));
    }
}

/// The lesson is over: the scene, the cover, the fade and the Start menu's entries released.
fn close(run: &mut LessonRun, cutscene: Option<&mut Cutscene>, hold: Option<&mut LoadingHold>, fade: Option<&mut ChallengeFade>, menu: Option<&mut LessonMenu>) {
    if let Some(play) = run.play.take() {
        info!("Skate 3 lesson {}: ended", play.id);
        if play.shown.is_some()
            && let Some(c) = cutscene
        {
            c.stop();
        }
    }
    (run.live, run.volumes) = (false, Vec::new());
    run.probe.clear();
    if let Some(h) = hold {
        h.0 = false;
    }
    if let Some(f) = fade {
        f.0 = 0.;
    }
    if let Some(m) = menu {
        (m.active, m.request) = (false, None);
    }
}

#[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
fn run(
    mut lesson: ResMut<LessonRun>,
    career: Res<Career>,
    time: Res<Time>,
    transition: Option<Res<MapTransition>>,
    mut skater: Option<ResMut<SkaterRuntime>>,
    mut cutscene: Option<ResMut<Cutscene>>,
    (mut hold, mut fade, mut menu): (Option<ResMut<LoadingHold>>, Option<ResMut<ChallengeFade>>, Option<ResMut<LessonMenu>>),
    (mut starts, mut exits, mut results): (MessageReader<StartRow>, MessageReader<ModeExit>, MessageWriter<ChallengeResult>),
    (mut controls, mut pad): (Option<ResMut<PlayerControls>>, Option<ResMut<ControllerInput>>),
) {
    let now = time.elapsed_secs_f64();
    let run = &mut *lesson;
    let data = career.state.as_ref().map(|s| s.data.clone());
    if exits.read().count() > 0 {
        run.pending = None;
        if run.play.is_some() {
            info!("Skate 3 lesson: the career stopped; the lesson ends without a result");
            close(run, cutscene.as_deref_mut(), hold.as_deref_mut(), fade.as_deref_mut(), menu.as_deref_mut());
        }
    }
    for s in starts.read() {
        // Another row started: the career has moved on, the lesson running is abandoned.
        if let Some(play) = run.play.as_mut()
            && play.g.done.is_none()
        {
            info!("Skate 3 lesson {}: {} started; abandoned", play.id, s.row);
            play.g.abandon(&mut play.rt);
        }
        let is_lesson = data.as_ref().and_then(|d| d.row(&s.row)).is_some_and(|r| skate3_tutorial::port_of(r) == Some(Port::Lesson));
        run.pending = is_lesson.then(|| s.row.clone());
    }

    if let Some(play) = run.play.as_mut() {
        step(play, now, skater.as_deref_mut(), cutscene.as_deref_mut(), menu.as_deref_mut(), std::mem::take(&mut run.probe));
        run.live = play.group.is_some();
        // The groups' EnableInputRestriction / ClearInputRestrictions and Pin / Unpin, on the local skater.
        for mask in play.stage.restrictions.drain(..) {
            run.held = true;
            if let Some(c) = controls.as_deref_mut() {
                match mask {
                    Some(bits) => c.restrict(bits),
                    None => c.clear_restrictions(),
                }
            }
        }
        if let (Some(pinned), Some(p)) = (play.stage.pinned.take(), pad.as_deref_mut()) {
            run.held = true;
            p.pinned = pinned;
        }
        // Challenge.OpenLoadingScreen / CloseLoadingScreen and RepositionAI's fade, as skate3_film shows them.
        if let Some(h) = hold.as_deref_mut() {
            h.0 = play.rt.covered && play.shown.is_none();
        }
        if let Some(f) = fade.as_deref_mut() {
            let alpha = if play.shown.is_some() { 0. } else { play.rt.fade_alpha(now) as f32 };
            if f.0 != alpha {
                f.0 = alpha;
            }
        }
    }

    // CompleteChallenge (complete, L118-135): the career's ChallengeResult. A lesson has no score.
    if let Some((id, outcome)) = run.play.as_ref().and_then(|p| Some((p.id.clone(), p.g.done?))) {
        results.write(ChallengeResult { row: id, outcome: outcome_of(outcome), score: 0 });
        close(run, cutscene.as_deref_mut(), hold.as_deref_mut(), fade.as_deref_mut(), menu.as_deref_mut());
    }

    // INFERRED (the teardown was not traced): a lesson that ends leaves no restriction or pin behind.
    if run.play.is_none() && run.held {
        run.held = false;
        if let Some(c) = controls.as_deref_mut() {
            c.clear_restrictions();
        }
        if let Some(p) = pad.as_deref_mut() {
            p.pinned = false;
        }
    }

    // The accepted lesson starts once any world load it needed is in (spec 12.6).
    if run.play.is_none()
        && let Some(id) = run.pending.clone()
        && !transition.as_ref().is_some_and(|t| t.busy())
    {
        run.pending = None;
        match data.as_deref().ok_or_else(|| "the career is off".to_owned()).and_then(|d| begin(d, &id, now)) {
            Ok(play) => {
                run.volumes = play.volumes.clone();
                if let Some(m) = menu.as_deref_mut() {
                    (m.active, m.request) = (true, None);
                }
                run.play = Some(play);
            }
            Err(e) => {
                warn!("Skate 3 lesson {id}: {e}; it ends Abandoned");
                results.write(ChallengeResult { row: id, outcome: Outcome::Abandoned, score: 0 });
            }
        }
    }
}

/// Every physics tick while the lesson's objectives exist: this skater's [`Tick`].
#[allow(clippy::needless_pass_by_value)]
fn probe(mut lesson: ResMut<LessonRun>, skater: Option<Res<SkaterRuntime>>, marker: Option<Res<SessionMarker>>, input: Option<Res<PublishedTickInput>>) {
    let LessonRun { probe, volumes, live, .. } = &mut *lesson;
    if !*live {
        probe.clear();
        return;
    }
    let Some(skater) = skater else { return };
    let at = skater.animated_skeleton.roots.animation_to_world[3];
    let at = Vec3::new(at[0], at[1], at[2]);
    let frame = skate3_film::frame_of(&skater, marker.as_deref(), volumes.as_slice(), None);
    let extra = Extra {
        in_air: skater.player_state.current().category() == AIR,
        inside: volumes.iter().filter(|(_, v)| v.contains(at)).map(|(id, _)| *id).collect(),
        announced: skater.scoring.announced.clone(),
        markers: marker.as_ref().map_or((0, 0), |m| (m.sets, m.uses)),
        right_y: input.map_or(0., |i| i.0.actions().values()[4]),
    };
    probe.push(Tick { frame, extra });
}

#[allow(clippy::needless_pass_by_value)]
fn on_career(career: Res<Career>) -> bool {
    career.state.is_some()
}

pub(super) fn build(app: &mut App) {
    app.init_resource::<LessonRun>()
        .add_systems(FixedUpdate, probe.after(crate::app::SimulationSet::Physics).run_if(on_career))
        .add_systems(Update, run.after(crate::app::FrameSet::Physics).run_if(on_career));
}

#[cfg(test)]
mod tests {
    use super::super::core::CareerState;
    use super::super::data::{Indicator, Row};
    use super::*;
    use bevy::ecs::message::Messages;
    use serde_json::json;
    use std::sync::Arc;

    /// A mode-1 objective evaluated each tick (trigger byte 12).
    fn objective(key: &str, obj: Json, hints: Vec<Json>) -> Json {
        let mut triggers = vec![0; 20];
        triggers[12] = 1;
        let hints: Vec<Json> = hints.into_iter().map(|t| json!({"src": 0, "tree": t})).collect();
        json!({"key": key, "activation": 1, "triggers": triggers, "fail_first": false, "master": false,
               "requires_highlight": false, "obj": {"src": 0, "tree": obj}, "points_valid": null, "failures": [],
               "hints": hints})
    }
    /// The export's shape: each objective record its own group.
    fn groups(objectives: Vec<Json>) -> Json {
        let each: Vec<Json> = objectives.into_iter().map(|o| json!({"PointRequirement": null, "objectives": [o]})).collect();
        json!({"Objectives": each})
    }
    fn points() -> Json {
        json!({"key": "global_points_objective", "activation": 0, "triggers": [], "fail_first": false, "master": false, "requires_highlight": false})
    }
    fn ret(e: Json) -> Json {
        json!([["return", [e]]])
    }
    fn native(name: &str) -> Json {
        json!(["native", name, []])
    }
    /// MakeObjectives, then ActivateChallengeStartObjectives.
    fn started(g: Json) -> Group {
        let points: ObjectiveDef = serde_json::from_value(points()).unwrap();
        let mut g = Group::new(ME, &lesson_group(&g).unwrap(), &points).unwrap();
        g.activate_start();
        g
    }
    fn run_ticks(g: &mut Group, n: usize) -> Vec<Vec<Out>> {
        let (mut sense, mut was) = (Sense::new(HashMap::new()), Was::default());
        (0..n).map(|_| feed(g, &mut sense, &mut was, Frame::default(), &Extra::default())).collect()
    }

    #[test]
    fn the_lessons_groups_make_one_group_with_their_hints() {
        let no = ret(json!(["bool", false]));
        let def = lesson_group(&groups(vec![objective("a", no.clone(), vec![]), objective("b", no, vec![ret(json!(["bool", true])); 2])])).unwrap();
        assert_eq!(def.objectives.len(), 2);
        assert_eq!(def.point_requirement, None);
        let b = def.objectives[1].as_ref().unwrap();
        assert_eq!((b.key.as_str(), b.hints.len(), b.failures.len()), ("b", 2, 0));
    }

    #[test]
    fn a_hint_shares_its_objectives_vars_and_changes_no_status() {
        let n = json!(["index", ["vars"], ["str", "n"]]);
        let obj = json!([["call", ["native", "InitVars", [["vars"], ["str", "n"], ["num", 0.0]]]], ["return", [["bin", ">=", n, ["num", 2.0]]]]]);
        let hint = json!([["set", [n], [["bin", "+", n, ["num", 1.0]]]], ["return", [["bool", true]]]]);
        let mut g = started(groups(vec![objective("o", obj, vec![hint])]));
        let outs = run_ticks(&mut g, 3);
        assert!(outs[0].is_empty() && outs[1].is_empty(), "the hint counts and posts nothing");
        assert_eq!(outs[2], [Out::Complete { index: 0, requires_highlight: false, points: false }, Out::AllComplete], "the objective read the hint's count");
        assert_eq!(g.status(0), crate::skate3_objectives::COMPLETE);
    }

    #[test]
    fn is_on_board_and_flow_status_answer_the_lessons_natives() {
        let status = json!(["bin", "==", ["native", "Flow.GetObjectiveStatus", [["num", 0.0]]], ["const", "Flow.eObjectiveStatus.Complete"]]);
        let mut g = started(groups(vec![objective("a", ret(native("ObjectiveLocomotion.IsOnBoard")), vec![]), objective("b", ret(status), vec![])]));
        let outs = run_ticks(&mut g, 2);
        assert_eq!(outs[0].last(), Some(&Out::AllComplete), "b reads a's status as it is");
        // Answered (not an error, whose default would complete it): the skater is on its board.
        let mut g = started(groups(vec![objective("c", ret(json!(["un", "not", native("ObjectiveLocomotion.IsOnBoard")])), vec![])]));
        assert!(run_ticks(&mut g, 3).iter().all(Vec::is_empty));
    }

    #[test]
    fn the_rows_own_indicators_are_the_ones_without_an_objective() {
        let ind = |objective: &str| Indicator {
            objective: objective.into(),
            kind: 0,
            locator: None,
            shape: String::new(),
            style: String::new(),
            spline: Vec::new(),
            flags: Vec::new(),
        };
        let mut d = CareerData::default();
        d.indicators.insert("tut_sksc_02".into(), vec![ind(""), ind(""), ind("tut_sksc_02_13")]);
        assert_eq!((own_indicators(&d, "tut_sksc_02"), own_indicators(&d, "tut_sksc_06")), (2, 0));
    }

    /// An objective group's EnableInputRestriction (51), Pin (6), Wait (4), ClearInputRestrictions (52) and Unpin (7)
    /// reach the skater in order, through the presenter; a callout is logged.
    #[test]
    fn an_objective_groups_restriction_and_pin_reach_the_skater() {
        use crate::skate3_present::{Event as Ev, Group as Pres, Presenter};
        let ev = |kind: u32, a: &str| Ev::from_args(kind, &[a.to_owned()], 0);
        let (mut presenter, mut stage) = (Presenter::default(), LessonStage::default());
        let events = vec![ev(51, "2672"), ev(6, ""), ev(4, "1"), ev(52, ""), ev(7, ""), ev(20, "")];
        presenter.start(Pres { name: "tut_sksc_06_4/intro".into(), events });
        assert!(presenter.tick(0.0, &mut stage).is_empty());
        assert_eq!((stage.restrictions.clone(), stage.pinned), (vec![Some(2672)], Some(true)), "until the wait ends");
        assert_eq!(presenter.tick(1.0, &mut stage), ["tut_sksc_06_4/intro"]);
        assert_eq!((stage.restrictions, stage.pinned), (vec![Some(2672), None], Some(false)));
        assert!(stage.log.iter().any(|l| l.starts_with("callout")), "{:?}", stage.log);
    }

    #[test]
    fn the_start_is_the_locators_first_slot() {
        let matrix = [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [1., 2., 3., 1.]];
        let slot = vec![vec![0., 0., -1., 0.], vec![0., 1., 0., 0.], vec![1., 0., 0., 0.], vec![4., 5., 6., 1.]];
        let l = Locator { map: String::new(), matrix, slots: vec![slot] };
        assert_eq!(start_of(&l), [[0., 0., -1., 0.], [0., 1., 0., 0.], [1., 0., 0., 0.], [4., 5., 6., 1.]]);
        assert_eq!(start_of(&Locator { slots: Vec::new(), ..l }), matrix);
    }

    /// Updates until `done`, keeping the career's results; the updates it took.
    fn until(app: &mut App, results: &mut Vec<ChallengeResult>, max: usize, done: impl Fn(&World, &[ChallengeResult]) -> bool) -> usize {
        for n in 0..max {
            if done(app.world(), results) {
                return n;
            }
            app.update();
            results.extend(app.world_mut().resource_mut::<Messages<ChallengeResult>>().drain());
        }
        panic!("not reached in {max} updates");
    }

    #[test]
    fn a_lesson_runs_to_owned_it_then_abandons_from_the_start_menu() {
        let dir = std::env::temp_dir().join(format!("s3career-lesson-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("rows")).unwrap();
        let (objectives, points) = (groups(vec![objective("tut_x_0", ret(native("ObjectiveLocomotion.IsOnBoard")), vec![])]), points());
        let row = json!({"groups": objectives, "points_objective": points, "flow": [0, 1], "presentation": {"Intro": [], "Restart": [], "Outro": []}});
        std::fs::write(dir.join("rows/tut_x.json"), row.to_string()).unwrap();
        let mut d = CareerData::default();
        d.rows.insert("tut_x".into(), Row { key: 7, graph: "tutorials".into(), ..Row::default() });
        d.root = dir.clone();
        let mut career = Career::open(None, &dir);
        career.state = Some(CareerState::new(Arc::new(d)));
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(std::time::Duration::from_millis(100)))
            .insert_resource(career)
            .init_resource::<LessonMenu>()
            .add_message::<StartRow>()
            .add_message::<ChallengeResult>()
            .add_message::<ModeExit>();
        build(&mut app);
        let mut results = Vec::new();
        let running = |w: &World| w.resource::<LessonRun>().play.as_ref().is_some_and(|p| p.rt.state == St::ChallengeRunning && p.group.as_ref().is_some_and(|g| g.enabled));

        app.world_mut().write_message(StartRow { row: "tut_x".into(), loaded_world: false });
        until(&mut app, &mut results, 100, |w, _| running(w));
        assert!(app.world().resource::<LessonMenu>().active, "the Start menu lists Restart / Abandon Lesson");
        // One tick on the board completes the objective: OwnedIt 3.5 s, then dialog 33's Continue.
        app.world_mut().resource_mut::<LessonRun>().probe.push(Tick::default());
        let took = until(&mut app, &mut results, 200, |_, r| !r.is_empty());
        assert!(took >= 34, "OwnedIt shows 3.5 s first ({took} updates)");
        assert_eq!(results, [ChallengeResult { row: "tut_x".into(), outcome: Outcome::Owned, score: 0 }]);
        assert!(app.world().resource::<LessonRun>().play.is_none());
        assert!(!app.world().resource::<LessonMenu>().active);

        results.clear();
        app.world_mut().write_message(StartRow { row: "tut_x".into(), loaded_world: false });
        until(&mut app, &mut results, 100, |w, _| running(w));
        app.world_mut().resource_mut::<LessonMenu>().request = Some(LessonRequest::Abandon);
        until(&mut app, &mut results, 10, |_, r| !r.is_empty());
        assert_eq!(results, [ChallengeResult { row: "tut_x".into(), outcome: Outcome::Abandoned, score: 0 }]);
        assert!(!app.world().resource::<LessonRun>().running());
    }
}
