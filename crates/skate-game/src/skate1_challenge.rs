//! Skate 1's tutorial on its San Vanelona map (docs/superpowers/specs/2026-10-14-skate1-tutorial-retail-design.md):
//! the Skate 1 binding of the shared challenge core. Data from skate2port/old_challenges.py in
//! private/challenges-skate1. The first spawn on the map without saved progress runs retail's new-career frontend
//! (loaders, the two intro movies, the medical form, the customiser); then the career's challenges start in
//! tutorial_progress.sp's order, each run by its own decompiled disc scripts.
//!
//! This module registers the Skate 1 natives the scripts call, turns the skater's state into the events they wait
//! for, and hands the HUD states to skate1_screens.rs, which draws the retail movies.
use crate::{
    challenge::{
        LessonMenu, LessonRequest, Machine, Script,
        movie::{Movie, read_wav},
    },
    config::Config,
    map_transition::CurrentMap,
    physics::{GamePhysics, SkaterRuntime},
    skate_audio::StreamPcm,
};
use crate::{
    challenge::cutscene::Cutscene,
    physics::riders::Riders,
    replay::Replay,
    session_marker::SessionMarker,
    skate1_cast::People,
    skate1_missions::{AiCall, Facts as MissionFacts, Nis, Request},
    skate1_pros::ProGates,
    skate1_rider::{AiEvent, Skate1Ai},
    skate1_scenes::{REPLAY_FPS, SceneEnd, ScenePlay},
    skate1_screens::Skate1Screens,
};
use bevy::prelude::*;
use mlua::{Lua, Variadic};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use skate_core::physics::board::BodyId;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{Arc, Mutex},
};

const ROOT: &str = "private/challenges-skate1";
/// The land call arms the air record's countdown at 2 (0x8279f678 -> +3080); 0x8279e750 posts it when it runs out.
const AIR_COMPLETED_TICKS: u32 = 2;
/// cMsgSkaterPushed's cool-down after a report (0x826520e8), in ticks.
const PUSH_COOLDOWN: u32 = 60;
/// Air, grind and wipeout physical states (skate_core::player::state::PhysicalStateId).
const AIR_STATES: std::ops::RangeInclusive<u32> = 200..=202;
const GRIND_STATES: std::ops::RangeInclusive<u32> = 400..=405;
const WIPEOUT_STATE: u32 = 300;
/// PhysicalStateId::SlideGround (skate-core player/state.rs). INFERRED (P5 L7): a powerslide is this state.
const SLIDE_STATE: u32 = 101;
const PLAYER: i64 = 0;
/// XInput buttons as crate::customiser::Navigation reports them.
const PAD_START: u16 = 0x10;
const PAD_A: u16 = 0x1000;
const PAD_B: u16 = 0x2000;
const PAD_RB: u16 = 0x0200;
const PAD_Y: u16 = 0x8000;
/// The menu's pad buttons (d-pad up and down are 1 and 2) and their keys, as `capture` takes them.
const MENU_KEYS: [(u16, KeyCode); 6] = [
    (PAD_A, KeyCode::Enter),
    (PAD_B, KeyCode::Backspace),
    (PAD_START, KeyCode::Escape),
    (PAD_Y, KeyCode::KeyY),
    (1, KeyCode::ArrowUp),
    (2, KeyCode::ArrowDown),
];

// ---------------------------------------------------------------- data
#[derive(Deserialize)]
struct Tutorial {
    chain: Vec<String>,
    intros: Vec<String>,
    lessons: BTreeMap<String, LessonData>,
    places: Places,
    movies: BTreeMap<String, MovieData>,
    /// The missions and pro tutorials beside the lessons; the career needs their eGlobalType.
    #[serde(default)]
    missions: BTreeMap<String, Value>,
    #[serde(default)]
    pro_tutorials: BTreeMap<String, Value>,
    /// "bank:line:flags" -> the line's files (old_challenges.py speech_table); null = retail has no file (silent).
    #[serde(default)]
    speech_table: BTreeMap<String, Option<TableLine>>,
    /// A speech file's takes.
    #[serde(default)]
    speech_files: BTreeMap<String, Vec<Take>>,
}
#[derive(Deserialize)]
struct TableLine {
    files: Vec<String>,
    /// The vault play chance, % (tMCSpeechTuning +24, rolled by 0x822c6480; research 4.6).
    chance: f32,
}
#[derive(Deserialize)]
struct LessonData {
    #[serde(rename = "self")]
    fields: Value,
    steps: BTreeMap<String, BTreeMap<String, String>>,
}
#[derive(Deserialize)]
struct Places {
    locators: BTreeMap<String, Locator>,
    volumes: BTreeMap<String, Volume>,
}
#[derive(Deserialize)]
struct Locator {
    matrix: [[f32; 4]; 4],
    /// The child spawns of a locator the missions' own streams carry (old_challenges.py; None: BAM.loc only).
    #[serde(default)]
    slots: Option<Vec<[[f32; 4]; 4]>>,
}
#[derive(Deserialize, Clone, Copy)]
struct Volume {
    min: [f32; 3],
    max: [f32; 3],
}
impl Volume {
    fn contains(&self, p: Vec3) -> bool {
        (0..3).all(|k| self.min[k] <= p[k] && p[k] <= self.max[k])
    }
}
#[derive(Deserialize)]
struct Take {
    audio: String,
}
#[derive(Deserialize)]
struct MovieData {
    video: String,
    audio: String,
}

impl Tutorial {
    /// Cameraman line `n`'s takes (PlayCameramanSpeech / `m=N` -> 0x822ab198(speech manager, 31, n, 0); research 4.6),
    /// or None when retail has no file for it or its chance misses. `roll` = rand() / 32767.
    fn cameraman_line(&self, n: i64, roll: f32) -> Option<&[Take]> {
        self.line(&format!("31:{n}:0"), roll)
    }
    /// A speech table line "speaker:line:context" (a mission's Request::Speech, P5 L8), rolled the same way.
    fn line(&self, key: &str, roll: f32) -> Option<&[Take]> {
        let line = self.speech_table.get(key)?.as_ref()?;
        if roll * 100. > line.chance {
            return None;
        }
        self.speech_files.get(line.files.first()?).map(Vec::as_slice).filter(|t| !t.is_empty())
    }

    /// A challenge's vault fields: a lesson's, a mission's or a pro tutorial's.
    fn fields(&self, name: &str) -> Option<&Value> {
        match self.lessons.get(name) {
            Some(lesson) => Some(&lesson.fields),
            None => self.missions.get(name).or_else(|| self.pro_tutorials.get(name)).map(|c| &c["self"]),
        }
    }
    fn field<'a>(&'a self, name: &str, key: &str) -> Option<&'a Value> {
        self.fields(name)?.get(key)
    }
    fn position(&self, locator: &str) -> Option<Vec3> {
        self.places.locators.get(locator).map(|l| Vec3::new(l.matrix[3][0], l.matrix[3][1], l.matrix[3][2]))
    }
    /// A new career starts at fp_u2_tutorial01's Location (research 1.1 "New career", 0x8235dfd8).
    fn start(&self) -> Option<&str> {
        self.field(self.chain.first()?, "Location")?.as_str()
    }
    /// The eGlobalType of every challenge the tutorial loads.
    fn kinds(&self) -> BTreeMap<String, i64> {
        let lessons = self.lessons.iter().map(|(name, l)| (name, &l.fields));
        let others = self.missions.iter().chain(&self.pro_tutorials).map(|(name, c)| (name, &c["self"]));
        lessons.chain(others).filter_map(|(name, f)| Some((name.clone(), f.get("GlobalType")?.as_i64()?))).collect()
    }
}

// ---------------------------------------------------------------- career
/// One progression record of progress.sp, the career (mode 1; old_challenges.py progress.json; research 1.1 "Career
/// order"). tutorial_progress.sp belongs to mode 6, which skips the pre-action append.
#[derive(Deserialize, Clone)]
struct Record {
    name: String,
    /// Prerequisite records (+0x20).
    #[serde(default)]
    after: Vec<String>,
    /// Actions appended to the completing record's list when this one becomes available (+0xC0).
    #[serde(default)]
    pre_actions: Vec<String>,
    #[serde(default)]
    unlocks: Vec<String>,
    /// Completion actions in slot order (2e0, 300, 320 ...).
    #[serde(default)]
    actions: Vec<String>,
    /// +0x3C8 / +0x3D0 (INFERRED: points required / awarded, research 1.1).
    #[serde(default)]
    points_required: i64,
    #[serde(default)]
    points_awarded: i64,
}

/// A progression action (0x823fe948's letters, research 1.1).
#[derive(Debug, Clone, PartialEq)]
enum Action {
    /// `u=...`: a gear / character unlock. UNTRACED in S3R: logged, not applied.
    Unlock(String),
    /// `r=N`: ROP N (r=2 = the in-engine NIS_ROP_2002_shingo_S01_0).
    Rop(i64),
    /// `m=N`: cameraman line N (bank 31), once per career.
    Line(i64),
    /// `c=name`: a Sidekick incoming call.
    Call(String),
    /// `s=101:X`: the Skate 101 popup X.
    Skate101(String),
}
impl Action {
    fn parse(text: &str) -> Option<Self> {
        let (letter, value) = text.split_once('=')?;
        Some(match letter {
            "u" => Self::Unlock(value.into()),
            "r" => Self::Rop(value.parse().ok()?),
            "m" => Self::Line(value.parse().ok()?),
            "c" => Self::Call(value.into()),
            "s" => Self::Skate101(value.strip_prefix("101:")?.into()),
            _ => return None,
        })
    }
}

/// The Skate 1 career (settings/skate1-tutorial.json; Not retail (user 10-14): retail keeps it in the profile).
#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(default)]
struct Progress {
    /// The new-career frontend (intros, medical form, customiser) has run.
    frontend_done: bool,
    /// Records won (WinChallenge): out of the available set for good.
    won: BTreeSet<String>,
    points: i64,
    /// Cameraman `m=` lines already played (retail's played list, +720).
    lines_played: BTreeSet<i64>,
}

impl Progress {
    fn load(path: &std::path::Path) -> Self {
        crate::saves::load_json(path).ok().unwrap_or_default()
    }
    fn save(&self, path: &std::path::Path) {
        let result = crate::saves::write_atomic(path, &serde_json::to_vec_pretty(self).unwrap_or_default());
        if let Err(error) = result {
            warn!("Skate 1 tutorial: progress not saved to {}: {error}", path.display());
        }
    }
}

/// Availability, auto-start and completion over the progression records (pure; research 1.1).
struct Career {
    records: Vec<Record>,
    /// eGlobalType of every challenge the tutorial loads; records without one are outside the window.
    kinds: BTreeMap<String, i64>,
}
impl Career {
    fn new(records: Vec<Record>, kinds: BTreeMap<String, i64>) -> Self {
        Self { records, kinds }
    }
    /// Not won, every prerequisite won, and the points gate met (0x823f86f0), in record order.
    fn available(&self, progress: &Progress) -> Vec<&Record> {
        self.records
            .iter()
            .filter(|r| self.kinds.contains_key(&r.name) && !progress.won.contains(&r.name))
            .filter(|r| r.after.iter().all(|a| progress.won.contains(a)) && progress.points >= r.points_required)
            .collect()
    }
    /// The first available eGlobalType 4 challenge (0x823d2c08).
    fn auto_start(&self, progress: &Progress) -> Option<&str> {
        self.available(progress).into_iter().find(|r| self.kinds[&r.name] == 4).map(|r| r.name.as_str())
    }
    /// The end of a challenge. A win runs the points step (0x823f8c38) and returns the record's unlocks, then its
    /// slot actions, then the pre-actions of the records it made available, at most one `c` and one `m` in all
    /// (0x823fb850). No win: nothing changes, the challenge stays available.
    fn complete(&self, progress: &mut Progress, name: &str, won: bool) -> Vec<Action> {
        let Some(record) = self.records.iter().find(|r| r.name == name) else { return Vec::new() };
        if !won || progress.won.contains(name) {
            return Vec::new();
        }
        let before: BTreeSet<&str> = self.available(progress).iter().map(|r| r.name.as_str()).collect();
        progress.won.insert(name.to_string());
        progress.points += record.points_awarded;
        // UNTRACED: when the unlock list runs relative to the slots; unlocks have no presentation in S3R.
        let mut texts: Vec<&str> = record.unlocks.iter().chain(&record.actions).map(String::as_str).collect();
        for newly in self.available(progress).into_iter().filter(|r| !before.contains(r.name.as_str())) {
            for pre in &newly.pre_actions {
                let letter = &pre[..pre.find('=').unwrap_or(0)];
                if !(matches!(letter, "c" | "m") && texts.iter().any(|t| t.starts_with(&format!("{letter}=")))) {
                    texts.push(pre);
                }
            }
        }
        texts
            .into_iter()
            .filter_map(|t| {
                let action = Action::parse(t);
                if action.is_none() {
                    warn!("Skate 1 career: unknown action {t:?}");
                }
                action
            })
            .collect()
    }
}

// ---------------------------------------------------------------- frontend
/// A Skate 1 frontend screen (research 1.1 "Loading screens", FE 43); skate1_screens.rs draws `FeScreen::want`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Fe {
    LoadingWelcome,
    LoadingQuick,
    LoadingTips,
    CaCDetails,
    /// `s=101:X`: FE 71 Challenge101.
    Skate101(String),
    /// `c=name`: the incoming call (hud_action_overlay state 13); skate1_screens.rs plays its audio too.
    Call(String),
}

/// The flow's screen request and skate1_screens.rs's answers.
#[derive(Resource, Default)]
pub(crate) struct FeScreen {
    /// Set by the flow; None closes it (a loader then plays its outro over the game).
    pub(crate) want: Option<Fe>,
    /// The wanted screen's intro has played. Retail closes a loader when its load is done, with no minimum time and
    /// without waiting for the outro (research8 3, research9 3). UNTRACED: what queues that close; the port has
    /// nothing to load, so a loader stays up through its intro.
    pub(crate) intro_done: bool,
    /// The player confirmed CaCDetails.
    pub(crate) confirmed: bool,
    /// A Skate 101 popup or call was removed after its outro (codes 2 / 6, research9 2).
    pub(crate) closed: bool,
}

/// One presentation step, played to its end before the next.
#[derive(Debug, Clone, PartialEq)]
enum Item {
    Screen(Fe),
    Movie(String),
    /// CaC (FE 42). Not retail (user 10-14): the S3R customiser stands in.
    Customiser,
    Teleport(String),
    FrontendDone,
    /// `m=N`: a cameraman line; it plays on under whatever comes next (research8 2: no wait).
    Line(i64),
    /// `r=N`: an in-engine ROP scene, to its end (code 1).
    Scene(String),
    /// After a completion's list: the refresh's type-4 auto-start (0x823d2b28).
    AutoStart,
    /// A lesson's scene ended: the script hears niscomplete.
    NisComplete,
    /// A mission's NIS (P5 L8: EnterIntroOffer/Accept/DeclineNIS, or EnterReplayNIS with its replay frames); its end
    /// posts nischallengenotify (P3).
    MissionScene(String, Option<(i64, i64)>),
}
enum Playing {
    Screen(Fe),
    Movie(Movie),
    Customiser,
    Scene(ScenePlay),
}

/// The new-career frontend (research 1.2 steps 2-7). Not retail (user 10-14): it runs on the first spawn on the map
/// without saved progress (retail: a new career from the main menu); a save with wins never replays it. The move to
/// the start Location runs under the Tips loader, where retail loads the world.
fn frontend(progress: &Progress, intros: &[String], start: &str) -> Vec<Item> {
    if progress.frontend_done || !progress.won.is_empty() {
        return Vec::new();
    }
    let movie = |i: usize| intros.get(i).map(|m| Item::Movie(m.clone()));
    [
        Some(Item::Screen(Fe::LoadingWelcome)),
        movie(0),
        Some(Item::Screen(Fe::CaCDetails)),
        Some(Item::Screen(Fe::LoadingQuick)),
        Some(Item::Customiser),
        Some(Item::Screen(Fe::LoadingQuick)),
        movie(1),
        Some(Item::Teleport(start.into())),
        Some(Item::Screen(Fe::LoadingTips)),
        Some(Item::FrontendDone),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// The screen wanted is the one playing.
fn sync_screen(screen: &mut FeScreen, playing: Option<&Playing>) {
    let want = match playing {
        Some(Playing::Screen(fe)) => Some(fe.clone()),
        _ => None,
    };
    if screen.want != want {
        *screen = FeScreen { want, ..default() };
    }
}

/// A screen's end (research8 3, research9 2-3): CaCDetails on confirm, a Skate 101 popup or call when removed after
/// its outro, a loader once its intro played (the UNTRACED gate on `intro_done`).
fn screen_done(fe: &Fe, screen: &FeScreen) -> bool {
    match fe {
        Fe::CaCDetails => screen.confirmed,
        Fe::Skate101(_) | Fe::Call(_) => screen.closed,
        _ => screen.intro_done,
    }
}

/// A completion's actions as presentation items, in slot order (executor 0x823fe948, research8 2): `r` and `s` and
/// `c` wait for their end, `m` doesn't, `u` has nothing to present in S3R.
fn action_items(actions: Vec<Action>, progress: &mut Progress) -> Vec<Item> {
    actions
        .into_iter()
        .filter_map(|action| match action {
            // `u=unlock` is the availability refresh (RETAIL research8 2: 0x823fd838 x2, 0x823f9508, 0x823ff300), which
            // `Career::available` + `rebuild_signups` already do. Gear (`g:`) and characters (`c:`) have no unlock store
            // in S3R: logged, not applied.
            Action::Unlock(what) if what == "unlock" => None,
            Action::Unlock(what) => {
                info!("Skate 1 career: unlock {what} (not applied)");
                None
            }
            Action::Rop(n) => {
                let scene = rop_scene(n).map(|s| Item::Scene(s.into()));
                if scene.is_none() {
                    warn!("Skate 1 career: no ROP {n}");
                }
                scene
            }
            // Once per career: the played list (+720).
            Action::Line(n) => progress.lines_played.insert(n).then_some(Item::Line(n)),
            // Ruling (ledger): a call always starts; 0x823fcef8(..,1) is UNTRACED.
            Action::Call(name) => Some(Item::Screen(Fe::Call(name))),
            Action::Skate101(x) => Some(Item::Screen(Fe::Skate101(x))),
        })
        .collect()
}

/// The ROP table at 0x830a2ad0 (name, movie index, isMovie). ponytail: the window's one row; r=2 is in-engine.
fn rop_scene(n: i64) -> Option<&'static str> {
    (n == 2).then_some("NIS_ROP_2002_shingo_S01_0")
}

fn teleport(data: &Tutorial, skater: &mut SkaterRuntime, at: &str, slot: Option<i64>) {
    match locator_m4(data, at, slot) {
        Some(m) => {
            if let Err(error) = skater.travel_to(m) {
                warn!("Skate 1 tutorial: teleport to {at}: {error}");
            }
        }
        None => warn!("Skate 1 tutorial: no place {at}"),
    }
}

/// A locator's matrix as a travel target (w of the position row 0, as the teleports take it). RepositionAI's `slot`
/// (RETAIL S1 0x823f4a08 -> 0x823d8df0): a stream locator places it at child[slot], past the last child at child 0
/// (none: the locator) moved -5 m in z per slot (0x823d9088, online_map::locator_slot); a BAM.loc one is its matrix.
/// None: StreamTeleportLocalPlayer and the frontend's teleports, the locator itself (UNTRACED: their slot rule).
fn locator_m4(data: &Tutorial, at: &str, slot: Option<i64>) -> Option<[[f32; 4]; 4]> {
    let l = data.places.locators.get(at)?;
    let mut m = match (&l.slots, slot) {
        // ponytail: a negative slot (no script passes one) is slot 0, as online's.
        (Some(slots), Some(slot)) => crate::online_map::locator_slot(&l.matrix, slots, slot.max(0) as usize),
        _ => l.matrix,
    };
    m[3][3] = 0.;
    Some(m)
}

/// RepositionAI's arg 3 (RETAIL 0x827c07a0): a number is the slot, anything else the id's own.
fn reposition_slot(id: i64, arg: Option<&mlua::Value>) -> i64 {
    match arg {
        Some(mlua::Value::Integer(n)) => *n,
        Some(mlua::Value::Number(n)) => *n as i64,
        _ => id,
    }
}

// ---------------------------------------------------------------- sign-ups and the film menu
/// A sign-up box whose radius is <= 0 gets this one (0x823d10a8).
const SIGNUP_DEFAULT_RADIUS: f32 = 40.;
/// The player's probe (0x8283d700): x/z +-0.29 m around the root, from the root up to 1.74 m.
const PROBE_HALF_WIDTH: f32 = 0.29;
const PROBE_HEIGHT: f32 = 1.74;
/// The film menu closes itself after 5 s of HUD time (0x82369128).
const FILM_MENU_TIME: f32 = 5.;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Aabb {
    min: Vec3,
    max: Vec3,
}
impl Aabb {
    fn signup(at: Vec3, radius: f32) -> Self {
        let r = if radius <= 0. { SIGNUP_DEFAULT_RADIUS } else { radius };
        Self { min: at - r, max: at + r }
    }
    fn probe(root: Vec3) -> Self {
        let side = Vec3::new(PROBE_HALF_WIDTH, 0., PROBE_HALF_WIDTH);
        Self { min: root - side, max: root + side + Vec3::Y * PROBE_HEIGHT }
    }
    /// The volume set's per-hit test returns 1 (0x823dfd78): a plain overlap decides.
    fn overlaps(&self, other: &Self) -> bool {
        self.min.cmple(other.max).all() && other.min.cmple(self.max).all()
    }
}

/// Type-3 sign-ups (research 1.1 "Type 3": the manager at challenge manager +0xBBC0).
#[derive(Default)]
struct SignUps {
    /// The available type-3 challenges' boxes, in record order (0x823d4238).
    boxes: Vec<(String, Aabb)>,
    /// Last frame's overlapping boxes (+3232); a challenge ending doesn't clear it.
    inside: BTreeSet<String>,
    /// The challenge the CHALLENGE SIGNUP overlay offers (state 2); Y accepts it.
    offer: Option<String>,
}
impl SignUps {
    /// One frame (0x823d3d40): a box just entered is offered when no challenge runs (`idle`); leaving the offered box
    /// hides the overlay.
    fn update(&mut self, root: Vec3, idle: bool) {
        let probe = Aabb::probe(root);
        let hits: BTreeSet<String> =
            self.boxes.iter().filter(|(_, b)| b.overlaps(&probe)).map(|(name, _)| name.clone()).collect();
        let entered = self.boxes.iter().map(|(name, _)| name).find(|n| hits.contains(*n) && !self.inside.contains(*n));
        if let Some(name) = entered.filter(|_| idle) {
            self.offer = Some(name.clone());
        }
        if !idle || self.offer.as_ref().is_some_and(|o| !hits.contains(o)) {
            self.offer = None;
        }
        self.inside = hits;
    }
}

/// This frame's presses for the film menu.
#[derive(Default, Clone, Copy)]
struct Pad {
    rb: bool,
    y: bool,
}
/// The open film menu (HUD 15 / SelectFilm, research 2.9): the selected film and the seconds left.
struct FilmMenu {
    index: usize,
    left: f32,
}
/// One frame of the film menu: RB opens it when a film is available (0x8234f3b0) and then cycles the films, resetting
/// the timer (0x823692e8); Y starts the selected one; 5 s closes it (0x82369128). Returns the film to start.
/// Ruling: RB acts on its press; retail's code 50 is the release (AptRShift1Released).
fn film_menu(menu: &mut Option<FilmMenu>, films: &[String], pad: Pad, dt: f32) -> Option<String> {
    let Some(open) = menu.as_mut() else {
        if pad.rb && !films.is_empty() {
            *menu = Some(FilmMenu { index: 0, left: FILM_MENU_TIME });
        }
        return None;
    };
    if pad.y {
        let film = films.get(open.index).cloned();
        *menu = None;
        return film;
    }
    if pad.rb {
        open.index = (open.index + 1) % films.len().max(1);
        open.left = FILM_MENU_TIME;
    }
    open.left -= dt;
    if open.left <= 0. || films.is_empty() {
        *menu = None;
    }
    None
}

/// After a menu takes a press (Y accepting a sign-up or film, A skipping a scene), gameplay doesn't see that button
/// until it's released: still held on the next frame it would reach gameplay as a fresh press (Y S3R's dismount, A a
/// push: playtest run10). Returns whether to discard this frame's gameplay input.
fn hold(holding: &mut u16, down: u16) -> bool {
    *holding &= down;
    *holding != 0
}

// ---------------------------------------------------------------- natives
/// eSk8HUDState is retail (0x827afa70): a value is its name's position; the screens switch on these numbers.
const HUD_FREESKATE: i64 = 1;
/// MissionTutorialChallenge: the one state whose sub-state is an eSk8HUDTutorialState (lessons and pros).
const HUD_TUTORIAL: i64 = 19;
const HUD_STATES: [&str; 81] = [
    "Idle", "FreeSkate", "NIS", "MapScreenMain", "LogBookFront", "SpotBibleFront", "SkateShopClerk", "SkateShopLounge",
    "SkateShopStore", "SkateShopCoffeeTable", "SkateShopSponsers", "SkateShopMagazines", "Sidekick", "SidekickVideo",
    "ReadMessage", "MissionFilmChallenge", "MissionSpotChallenge", "MissionPhotoChallenge", "MissionSkateChallenge",
    "MissionTutorialChallenge", "Backpack", "TermsOfService", "Options", "EndChallengeOptions", "MedicalBill",
    "UnlockOverlay", "Replay", "PhotoSave", "ReplaySave", "OnlinePhotoSave", "OnlineFootageSave", "RatingMenu", "Cheats",
    "TrickTutorial", "MissionDeathRaceChallenge", "MissionSpotRaceChallenge", "MissionBestTrickChallenge",
    "MissionProChallenge", "MissionJamChallenge", "MissionDemoChallenge", "MissionXGamesMegarampChallenge",
    "MissionXGamesStreetSkatingChallenge", "CaC", "CaCDetails", "ExtrasMenu", "BonusVideo", "Credits", "MainMenu",
    "BootFlow", "LeaderBoard", "XGamesLeaderBoard", "MoviePlayer", "MoviePlayerROP", "MoviePlayerNoTransition",
    "LoadingTips", "LoadingController", "LoadingQuick", "LoadingBlank", "LoadingWelcome", "Intro", "HowToPlaySubscreen",
    "HowToPlay", "AdidasPromotion", "TransitMap", "Profile", "SignUpPoster", "Popup", "MissionFollowMeChallenge",
    "VirtualKeyboard", "ComingSoon", "PartyPlay", "Challenge101", "ChallengeInfo", "TutorialComplete", "PressA",
    "FreeSkateOption", "Teleporter", "JackedOverlay", "UnlockCoverageOverlay", "MainMenuOptions", "ESRB",
];
/// eSk8HUDTutorialState (RETAIL 0x827b66d8, P6 L-P2): a value is its name's position.
const TUTORIAL_STATES: [&str; 25] = [
    "Load", "SignUpPoster", "Active", "LeftArea", "EnterArea", "ShowController", "HideController", "UpdateController",
    "UpdateObjectives", "ShowObjectives", "HideObjectives", "Congratulations", "Failed", "TutorialComplete",
    "FailedObjectives", "ShowTimer", "HideTimer", "CompleteOverlay", "MarkObjectivesComplete", "ShowDistanceMeter",
    "HideDistanceMeter", "ShowIndicator", "HideIndicator", "ShowRetryDialog", "MessageGatePassed",
];
/// eSk8ControllerSequence (RETAIL 0x827b7050): a value is its name's position; skate1_screens' controller_label maps it.
const SEQUENCES: [&str; 13] = [
    "None", "LeftButton", "BottomButton", "LeftAndBottomButton", "LeftAnalog", "LeftAnalogQuick", "RightAnalog",
    "RightAnalogTrigger", "RightAnalogGesture", "ChristAir", "LeftTrigger", "RightTrigger", "BothTriggers",
];
/// eSk8ControllerGesture (RETAIL 0x827b7558); skate1_screens' gesture_name maps it (0x830a4060).
const GESTURES: [&str; 56] = [
    "None", "Flip360", "PopShuvit360", "Directions", "DPadUp", "FSNoseSlide", "HeelFlip", "KickFlip", "LBDPadLeft",
    "LBDPadRight", "LBHold", "Manual", "ManualToFlip", "Nollie", "Nollie360HardFlip", "Nollie360Shuvit",
    "NollieKickFlip", "NoseManual", "Ollie", "OllieHold", "FSPopShuvit", "PopShuvit", "Pumping", "PushAHold",
    "PushLight", "PushXHold", "TriggerDirections", "TriggerLeft", "TriggerRight", "TriggerSB", "ChristAir", "Spin",
    "Tweak", "360HardFlip", "360HeelFlip", "360InwardHeel", "360FSPopShuvit", "HardFlip", "InwardHeel",
    "VarialHeelFlip", "VarialKickFlip", "FSNolliePopShuvit", "Nollie360flip", "Nollie360HeelFlip", "Nollie360PopShuv",
    "NollieFS360PopShuv", "NollieHardFlip", "NollieHeel", "NollieInwardHeel", "NolliePopShuvit",
    "NollieVarialHeelFlip", "NollieVarialKickFlip", "Nollie360InwardHeelFlip", "TriggerLeftLarge",
    "TriggerRightLarge", "TriggerBothLarge",
];

/// What the scripts ask of the engine, in call order; [`Skate1Challenge::apply`] carries them out.
enum Call {
    Speech(i64),
    Hud(i64, i64),
    /// SetTutorialObjective(slot (1-based), label, arg).
    Objective(i64, String, Option<i64>),
    /// MarkTutorialObjectiveComplete(n) (1-based).
    ObjectiveDone(i64),
    Controller(i64, i64),
    /// RepositionAI (the locator's slot) / StreamTeleportLocalPlayer (None).
    Teleport(String, Option<i64>),
    Pause(bool),
    Nis(String),
    Complete,
    Indicators(bool),
    /// Set / RemoveChallengeVisualIndicatorByIndex(self, i): VisualIndicators row i (1-based).
    Indicator(i64, bool),
    /// PinAI / UnpinAI on the player.
    Pin(bool),
}

#[derive(Default, Clone)]
struct AirReport {
    height: f32,
    time: f32,
    grab: f32,
    spin: f32,
    tweak: bool,
}

#[derive(Default)]
struct Timer {
    id: i64,
    left: f32,
}

/// State the natives share with the engine for one lesson.
#[derive(Default)]
struct Bridge {
    out: Vec<Call>,
    timers: Vec<Timer>,
    next_id: i64,
    /// The HUD countdown (out-of-bounds) the left-area warning shows.
    hud_timer: Option<i64>,
    air: AirReport,
    grind: f32,
    missing: BTreeSet<String>,
    /// WinChallenge was called: the CompleteChallenge after it counts the challenge won.
    won: bool,
    /// ChallengeAsset.ChallengeAccepted ran (0x827c6c88 -> 0x823de668 sets record+16).
    accepted: bool,
}
impl Bridge {
    fn timer(&mut self, seconds: f64) -> i64 {
        self.next_id += 1;
        self.timers.push(Timer { id: self.next_id, left: seconds as f32 });
        self.next_id
    }
    fn reset(&mut self, id: i64, seconds: f64) {
        match self.timers.iter_mut().find(|t| t.id == id) {
            Some(t) => t.left = seconds as f32,
            None => self.timers.push(Timer { id, left: seconds as f32 }),
        }
    }
    /// FirstScorableId / TrickIndex: the scorable's catalog index (EScorableID lineage), -1 when it isn't in it.
    fn scorable(&self, name: &str) -> i64 {
        scorable_index(name)
    }
}

type Shared = Arc<Mutex<Bridge>>;
type Args = Variadic<mlua::Value>;

/// The Skate 1 tables the lesson scripts call (spec "Skate 1 functions in Rust"). A name a table lacks logs once and
/// returns nil.
fn natives(lua: &Lua, bridge: &Shared) -> mlua::Result<()> {
    macro_rules! native {
        ($table:expr, $name:literal, $args:ty, |$b:ident, $a:pat_param| $body:expr) => {{
            let shared = bridge.clone();
            $table.set(
                $name,
                lua.create_function(move |_, $a: $args| {
                    #[allow(unused_mut, unused_variables)]
                    let mut $b = shared.lock().unwrap();
                    Ok($body)
                })?,
            )?;
        }};
    }
    fn call(b: &mut Bridge, c: Call) {
        b.out.push(c);
    }
    let g = lua.globals();

    let challenge = lua.create_table()?;
    native!(challenge, "WinChallenge", Args, |b, _a| b.won = true);
    native!(challenge, "CompleteChallenge", Args, |b, _a| call(&mut b, Call::Complete));
    native!(challenge, "CheckIfTutorialMode", Args, |b, _a| 1);
    native!(challenge, "GetLogbookReplayFlag", Args, |b, _a| 0);
    native!(challenge, "SetAllChallengeVisualIndicators", Args, |b, _a| call(&mut b, Call::Indicators(true)));
    native!(challenge, "RemoveAllChallengeVisualIndicators", Args, |b, _a| call(&mut b, Call::Indicators(false)));
    // INFERRED (name; Community Spirit sets each round's rows by VisualIndicatorRoundCounts): one row of
    // SetAllChallengeVisualIndicators' set. UNTRACED: the C++ bodies.
    native!(challenge, "SetChallengeVisualIndicatorByIndex", (mlua::Value, i64), |b, a| call(&mut b, Call::Indicator(a.1, true)));
    native!(challenge, "RemoveChallengeVisualIndicatorByIndex", (mlua::Value, i64), |b, a| call(&mut b, Call::Indicator(a.1, false)));
    g.set("Challenge", challenge)?;

    let timer = lua.create_table()?;
    native!(timer, "AddCountdownTimer", f64, |b, t| b.timer(t));
    native!(timer, "AddHUDCountdownTimer", f64, |b, t| {
        let id = b.timer(t);
        b.hud_timer = Some(id);
        id
    });
    native!(timer, "ResetCountdownTimer", (Option<i64>, f64), |b, (id, t)| if let Some(id) = id {
        b.reset(id, t)
    });
    native!(timer, "RemoveCountdownTimer", Option<i64>, |b, id| b.timers.retain(|t| Some(t.id) != id));
    native!(timer, "RemoveHUDCountdownTimer", Option<i64>, |b, id| {
        b.timers.retain(|t| Some(t.id) != id);
        if b.hud_timer == id {
            b.hud_timer = None;
        }
    });
    g.set("ChallengeTimer", timer)?;

    let hud = lua.create_table()?;
    // RETAIL 0x827b93d8: the sub-state is -1 unless arg 2 is a number.
    native!(hud, "ChangeHUDState", (i64, Option<i64>), |b, (s, t)| call(&mut b, Call::Hud(s, t.unwrap_or(-1))));
    native!(hud, "SetTutorialObjective", (i64, String, Option<i64>), |b, (i, label, arg)| {
        call(&mut b, Call::Objective(i, label, arg))
    });
    native!(hud, "MarkTutorialObjectiveComplete", Option<i64>, |b, n| call(&mut b, Call::ObjectiveDone(n.unwrap_or(0))));
    // RETAIL 0x827b9b90: stores the sequence and gesture, queues (Tutorial, ShowController) and (Tutorial,
    // UpdateController) (Skate1Challenge::apply).
    native!(hud, "ShowController", (Option<i64>, Option<i64>), |b, (s, g)| {
        call(&mut b, Call::Controller(s.unwrap_or(0), g.unwrap_or(0)))
    });
    native!(hud, "SetMissionKey", Args, |b, _a| ());
    let names = |names: &[&str]| -> mlua::Result<mlua::Table> {
        let t = lua.create_table()?;
        for (i, n) in names.iter().enumerate() {
            t.set(*n, i as i64)?;
        }
        Ok(t)
    };
    hud.set("eSk8HUDState", names(&HUD_STATES)?)?;
    hud.set("eSk8HUDTutorialState", names(&TUTORIAL_STATES)?)?;
    hud.set("eSk8ControllerSequence", names(&SEQUENCES)?)?;
    hud.set("eSk8ControllerGesture", names(&GESTURES)?)?;
    g.set("HUD", hud)?;

    let speech = lua.create_table()?;
    native!(speech, "PlayCameramanSpeech", Option<i64>, |b, id| if let Some(id) = id {
        call(&mut b, Call::Speech(id))
    });
    // Only outside tutorial mode (lesson 05's pro); CheckIfTutorialMode says we are in it.
    native!(speech, "PlayNPCSpeechUsingAttribute", Args, |b, _a| ());
    g.set("ChallengeSpeech", speech)?;

    let nis = lua.create_table()?;
    native!(nis, "EnterExtendedNIS", (mlua::Value, Option<String>), |b, (_kind, name)| {
        call(&mut b, Call::Nis(name.unwrap_or_default()))
    });
    nis.set("eSceneType", names(&["SCENE_TYPE_TUTORIAL_INTRO"])?)?;
    g.set("NIS", nis)?;

    let trick = lua.create_table()?;
    native!(trick, "GetScorableName", Option<i64>, |b, id| id
        .and_then(|i| usize::try_from(i).ok())
        .and_then(|i| skate_core::scoring::catalog::IDENTIFIERS.get(i).map(|e| skate1_name(e.0).to_owned())));
    native!(trick, "GetCompletedAirHeight", Args, |b, _a| b.air.height);
    native!(trick, "GetCompletedAirTime", Args, |b, _a| b.air.time);
    native!(trick, "GetCompletedAirGrabTime", Args, |b, _a| b.air.grab);
    native!(trick, "GetCompletedAirDegreesOfSpin", Args, |b, _a| b.air.spin);
    native!(trick, "GetTweak", Args, |b, _a| b.air.tweak);
    native!(trick, "GetCompletedGrindDistance", Args, |b, _a| b.grind);
    g.set("Trick", trick)?;

    let boundary = lua.create_table()?;
    native!(boundary, "StartOOBEffect", Args, |b, _a| ());
    native!(boundary, "ResetOOBEffect", Args, |b, _a| b.hud_timer = None);
    g.set("ChallengeBoundary", boundary)?;

    let pause = lua.create_table()?;
    native!(pause, "PauseSimulation", Args, |b, _a| call(&mut b, Call::Pause(true)));
    native!(pause, "UnpauseSimulation", Args, |b, _a| call(&mut b, Call::Pause(false)));
    // ponytail: the Start menu stays available; Skate 1 hid its backpack during the poster only.
    native!(pause, "EnablePauseMenuAndBackpack", Args, |b, _a| ());
    native!(pause, "DisablePauseMenuAndBackpack", Args, |b, _a| ());
    g.set("Pause", pause)?;

    let ai = lua.create_table()?;
    native!(ai, "RepositionAI", (mlua::Value, Option<String>, Option<mlua::Value>), |b, (_who, at, slot)| if let Some(at) = at {
        call(&mut b, Call::Teleport(at, Some(reposition_slot(PLAYER, slot.as_ref()))))
    });
    native!(ai, "StreamTeleportLocalPlayer", Option<String>, |b, at| if let Some(at) = at {
        call(&mut b, Call::Teleport(at, None))
    });
    // The player only: other ids are AI riders (P4).
    native!(ai, "PinAI", Option<i64>, |b, id| if id == Some(PLAYER) {
        call(&mut b, Call::Pin(true))
    });
    native!(ai, "UnpinAI", Option<i64>, |b, id| if id == Some(PLAYER) {
        call(&mut b, Call::Pin(false))
    });
    g.set("AI", ai)?;

    let debug = lua.create_table()?;
    native!(debug, "DebugTextPermanent", Args, |b, _a| {
        b.next_id += 1;
        b.next_id
    });
    native!(debug, "DebugTextTimed", Args, |b, _a| ());
    native!(debug, "RemoveTextPermanent", Args, |b, _a| ());
    g.set("Debug", debug)?;

    let online = lua.create_table()?;
    native!(online, "GetLocalPlayerID", Args, |b, _a| PLAYER);
    g.set("Online", online)?;

    // Every lesson asset is preloaded, so the load screen never shows.
    let asset = lua.create_table()?;
    native!(asset, "ChallengeAccepted", Args, |b, _a| b.accepted = true);
    native!(asset, "IsChallengeAssetLoadComplete", Args, |b, _a| true);
    native!(asset, "ActivateChallengeAssetLoadScreen", Args, |b, _a| ());
    native!(asset, "DeactivateChallengeAssetLoadScreen", Args, |b, _a| ());
    g.set("ChallengeAsset", asset)?;

    let shared = bridge.clone();
    g.set(
        "__missing",
        lua.create_function(move |_, name: String| {
            if shared.lock().unwrap().missing.insert(name.clone()) {
                warn!("Skate 1 tutorial: {name} is not implemented; it does nothing");
            }
            Ok(())
        })?,
    )?;
    lua.load(
        r#"for _, name in ipairs({"Challenge", "ChallengeTimer", "HUD", "ChallengeSpeech", "NIS", "Trick",
            "ChallengeBoundary", "Pause", "AI", "Debug", "Online", "ChallengeAsset"}) do
          setmetatable(_G[name], {__index = function(_, key)
            return function() __missing(name .. "." .. key) end
          end})
        end"#,
    )
    .set_name("@skate1-natives")
    .exec()
}

/// Skate 1's GetScorableName spelling of an engine scorable. The lessons compare against ollie, kickflip, heelflip,
/// popshuvit and fspopshuvit, which Skate 3's catalog spells the same way (it is the same EScorableID lineage).
fn skate1_name(identifier: &str) -> &str {
    identifier
}

/// FirstScorableId / TrickIndex: the scorable's catalog index (EScorableID lineage), -1 when it isn't in it.
fn scorable_index(name: &str) -> i64 {
    skate_core::scoring::catalog::IDENTIFIERS.iter().position(|e| e.0 == name).map_or(-1, |i| i as i64)
}

/// Skate 1's eTrickType of an engine scorable (skate1_online's map over the definition's TrickType); 0 when unknown.
fn trick_type(scoring: &crate::scoring_runtime::Runtime, id: &str) -> i64 {
    scoring.data.definitions.iter().find(|d| d.identifier == id).map_or(0, |d| crate::skate1_online::skate1_trick_type(d.trick_type))
}

fn scorable_class(identifier: &str) -> Option<u32> {
    skate_core::scoring::catalog::IDENTIFIERS.iter().find(|(n, ..)| *n == identifier).map(|e| e.1)
}
/// Catalog class 2 = grabs; their directional variants (`_up`, `_down`, ...) are the tweaked ones.
const GRAB_CLASS: u32 = 2;
fn tweaked(identifier: &str) -> bool {
    ["_up", "_down", "_left", "_right"].iter().any(|s| identifier.ends_with(s))
}
/// The air's FirstScorableId: its first ollie/flip (catalog class 3 / 4), except that a Flip announced after an Air
/// launch (ollie, nollie) is the trick. Retail's is the air's carrier after conversion; the engine announces the ollie,
/// then the flip its flick converts it to (user 10-15 runs 10 / 12: a heelflip set or copied as an ollie, judged
/// wrong). INFERRED, as skate3_skate_game's / skate2_skate_game's first_scorable.
fn first_scorable(first: &mut Option<&'static str>, id: &'static str) {
    let kind = |id: &str| skate_core::scoring::catalog::IDENTIFIERS.iter().find(|e| e.0 == id).map(|e| e.2);
    let flip_after_launch = first.is_some_and(|f| kind(f) == Some(1)) && kind(id) == Some(2);
    if (first.is_none() && matches!(scorable_class(id), Some(3 | 4))) || flip_after_launch {
        *first = Some(id);
    }
}

/// The air's newly announced tricks to post now (RETAIL cMsgTrickComplete: one entry per completed trick, the air's
/// carrier after conversion). The engine announces the ollie / nollie (catalog kind 1), then the flip its flick
/// converts it to: the launch waits in `launch`, a flip replaces it, and touchdown posts what's left
/// ([`Observer::launch`]). User 10-15: posted at once, Manuals 101's flip out of the manual reached substate 4 as an
/// Air (ollie) first, the wrong trick there (speech 1704), and the lesson failed.
fn air_tricks(launch: &mut Option<&'static str>, announced: &[&'static str]) -> Vec<&'static str> {
    let kind = |id: &str| skate_core::scoring::catalog::IDENTIFIERS.iter().find(|e| e.0 == id).map(|e| e.2);
    let mut post = Vec::new();
    for &id in announced {
        match kind(id) {
            Some(1) => post.extend(launch.replace(id)),
            Some(2) => {
                *launch = None;
                post.push(id);
            }
            _ => post.push(id),
        }
    }
    post
}

// ---------------------------------------------------------------- engine -> events
#[derive(Default, Clone)]
struct Air {
    start: f32,
    peak: f32,
    report: AirReport,
    /// The first ollie/flip named in this air, else the first trick of any kind.
    first: Option<&'static str>,
    any: Option<&'static str>,
    wiped: bool,
    /// TrickReversed (research7 1): the profile stance is goofy, sampled when the trick starts.
    reversed: bool,
    /// Missions (P5 L13): the take-off point and the signed 180s turned.
    from: Vec3,
    turns: i64,
}

/// An event for the running lesson, with the trick data its Trick.* calls will read.
struct Event {
    name: &'static str,
    message: Value,
    air: Option<Air>,
    grind: Option<f32>,
}
impl Event {
    fn new(name: &'static str, message: Value) -> Self {
        Self { name, message, air: None, grind: None }
    }
}

#[derive(Default)]
struct Observer {
    air: Option<Air>,
    /// A landed air waiting out its countdown (ticks left).
    landing: Option<(u32, Air)>,
    grind: Option<f32>,
    manual: bool,
    last: Option<Vec3>,
    ragdoll: bool,
    push: PushReport,
    inside: BTreeSet<String>,
    /// Missions (P5 L7 / L13): the announced tricks already posted, the open sequence's start frame, a powerslide
    /// underway, and the player's last completed air.
    seen: usize,
    sequence: Option<i64>,
    slide: bool,
    completed: Option<crate::skate1_missions::Air>,
    /// The physics tick whose ended tricks (the scoring's `completed`) were posted.
    ended: u64,
    /// A landed sequence's score held back while a manual connects it ([`Observer::sequence`]).
    landed: Option<f32>,
    /// ChallengeManager.SetManualsAreConnectors(false): a manual no longer holds a landing back.
    split: bool,
    /// An announced ollie / nollie the air may still convert to a flip ([`air_tricks`]): posted at touchdown, ahead
    /// of the landing's events; a bail drops it (no land entry, as the air's other tricks would be in retail).
    launch: Option<&'static str>,
    /// AnimationInput::auto_pumps when last observed.
    pumps: Option<u32>,
}

/// Skate 1's eTrickType.Manual (Trick.eTrickType, 827aa308).
const MANUAL_TRICK: i64 = 8;

impl Observer {
    /// sequencecomplete as skate1_online's probe: (SequenceScore, StartTick = the replay frame it opened on, Wipedout)
    /// when one posts. `published` is the scoring's landed sequence this tick, `manual` a manual held.
    /// - A manual connects (user 10-15 run13): Manuals 101 (fp_u3_tutorial01) runs flip in, hold, flip out as one
    ///   sequence and fails on a sequencecomplete before the flip out, but the scoring publishes the landed air a few
    ///   ticks into the manual (its live reward is 0 at first, 82DAB2A8). A landing published while a manual is held
    ///   waits for the manual's end and adds to the next publish. INFERRED: SetManualsAreConnectors' default
    ///   (he_u2_photo01 turns it off over its scoring area and on again in a spot); off, a landing posts at once.
    /// - A bail ends an open sequence wiped, with no score.
    fn sequence(&mut self, published: Option<f32>, open: bool, manual: bool, bailed: bool, frame: i64) -> Option<(i64, i64, bool)> {
        if open && self.sequence.is_none() {
            self.sequence = Some(frame);
        }
        if let Some(score) = published {
            self.sequence.get_or_insert(frame);
            *self.landed.get_or_insert(0.) += score;
        }
        if self.landed.is_some() && !(manual && !self.split) && !open {
            let score = self.landed.take().unwrap_or(0.).round() as i64;
            return Some((score, self.sequence.take().unwrap_or(frame), false));
        }
        if bailed && self.sequence.is_some() {
            self.landed = None;
            return Some((0, self.sequence.take().unwrap_or(frame), true));
        }
        if !open && self.landed.is_none() {
            self.sequence = None;
        }
        None
    }
}

/// cMsgSkaterPushed (0x826520e8, research 5.2 #14): a press adds to the count; when the cool-down is out and the skater
/// is pushing, it reports the count, clears it and waits PUSH_COOLDOWN ticks. The two input bytes' meanings are
/// INFERRED from the message they feed.
#[derive(Default)]
struct PushReport {
    presses: i64,
    cooldown: u32,
    held: bool,
}
impl PushReport {
    fn tick(&mut self, pressed: bool, pushing: bool) -> Option<i64> {
        if pressed && !self.held {
            self.presses += 1;
        }
        self.held = pressed;
        if self.cooldown > 0 {
            self.cooldown -= 1;
            return None;
        }
        if !pushing {
            return None;
        }
        self.cooldown = PUSH_COOLDOWN;
        Some(std::mem::take(&mut self.presses))
    }
}

/// One tick of the trick controller's air record (research 5.2 #15 / #19):
/// - a bail finalises it at once, wiped (0x827a38c8 -> 0x8279e640); only the land call appends a trick entry, so no
///   trickcomplete1 (INFERRED);
/// - touchdown posts the air's trickcomplete1 and arms aircompleted AIR_COMPLETED_TICKS later;
/// - a trick starting (manual, grind, new air) flushes a pending landing at once (slot 13 -> 0x8279e6a0), and in that
///   tick AirCompleted is queued before the trick loop (0x8265154c).
fn land(o: &mut Observer, in_air: bool, bailed: bool, starts: bool, out: &mut Vec<Event>) {
    let post = |air: Air, out: &mut Vec<Event>| {
        let mut event = Event::new("aircompleted", json!({"PlayerID": PLAYER, "Wipedout": i64::from(air.wiped)}));
        event.air = Some(air);
        out.push(event);
    };
    if bailed {
        if let Some(mut air) = o.air.take().or_else(|| o.landing.take().map(|(_, a)| a)) {
            air.wiped = true;
            post(air, out);
        }
        return;
    }
    if let Some((left, _)) = o.landing.as_mut() {
        *left -= 1;
        if *left == 0 {
            post(o.landing.take().unwrap().1, out);
        }
    }
    let mut tricks = Vec::new();
    if !in_air && let Some(air) = o.air.take() {
        let reversed = i64::from(air.reversed);
        tricks.push(Event::new("trickcomplete1", json!({"PlayerID": PLAYER, "TrickReversed": reversed})));
        o.landing = Some((AIR_COMPLETED_TICKS, air));
    }
    if starts && let Some((_, air)) = o.landing.take() {
        post(air, out);
    }
    out.extend(tricks);
}

// ---------------------------------------------------------------- HUD state
/// The tutorial HUD data skate1_screens reads (Facts); skate1_screens draws every HUD state (preview 3 bugs 2 / 3).
#[derive(Default)]
struct Hud {
    /// SetTutorialObjective's rows (RETAIL 0x82340048: 88-byte rows, kind 3 = its label, +76 done, count = the last
    /// slot set): (text, done).
    objectives: Vec<(String, bool)>,
    /// ShowController's sequence and gesture (RETAIL hud data +584 / +588).
    controller: (i64, i64),
}

/// What the scripts ask of Skate1Screens' HUD manager, in call order (one FIFO in retail, 0x8234e720).
#[derive(Clone, Copy, Debug, PartialEq)]
enum ScreenCall {
    /// HUD.ChangeHUDState(state, sub).
    State(u8, i64),
    /// HUD.MarkTutorialObjectiveComplete(n): the manager's guard, (Tutorial, 18) and the objective post.
    Mark(i64),
}

#[derive(Default, Clone, Copy)]
struct Pressed {
    accept: bool,
    back: bool,
    skip: bool,
    up: bool,
    down: bool,
}

#[derive(Resource)]
pub(crate) struct Skate1Challenge {
    root: PathBuf,
    data: Option<Arc<Tutorial>>,
    strings: BTreeMap<String, String>,
    generation: Option<(u64, bool)>,
    queue: std::collections::VecDeque<Item>,
    playing: Option<Playing>,
    career: Option<Career>,
    progress: Progress,
    /// settings/skate1-tutorial.json. Not retail (user 10-14): retail keeps the career in the profile.
    settings: PathBuf,
    machine: Option<Machine>,
    /// The challenge the machine runs.
    running: Option<String>,
    signups: SignUps,
    films: Option<FilmMenu>,
    /// A sign-up or film the player just accepted, for `drive` to start.
    accepted: Option<String>,
    /// The menu's presses still down (`hold`).
    holding: u16,
    /// PinAI on the player: gameplay gets no actions (ControllerInput::pinned).
    pin: bool,
    /// ChallengeManager.SwitchCamera(id): the AI skater the camera shows (None: the player).
    watch: Option<i64>,
    lesson: usize,
    volumes: Vec<(String, Volume)>,
    bridge: Shared,
    events: Vec<Event>,
    next_frame: Vec<Event>,
    hud: Hud,
    observer: Observer,
    /// What [`drive`] last gave the scorer's ManualsAreConnectors (`Runtime::manuals_connect`).
    manuals_connect: bool,
    pause: bool,
    speech: Option<Entity>,
    takes: BTreeMap<String, usize>,
    /// Rolls the lines' play chances.
    rng: crate::skate1_rider::Rng,
    /// Audio Settings' Dialog volume, refreshed each frame by drive.
    dialog: PlaybackSettings,
    indicators: Vec<Entity>,
    /// The indicators set by row (Call::Indicator).
    indexed: BTreeMap<i64, Entity>,
    pressed: Pressed,
    /// The running mission's or pro tutorial's skate1_missions state (P5 L1); None for a lesson.
    missions: Option<crate::skate1_missions::Shared>,
    /// What skate1_missions posted this frame (missions.tick), delivered after the engine's events.
    mission_events: Vec<(String, Value)>,
    /// RETAIL 0x823de820 (challenge state 2): AssetLoadType 1 runs the start cleanup (0x823d82b0) at once; any other
    /// value waits for ChallengeAccepted (state 3 polls record+16).
    cleanup_at_accept: bool,
    /// The scene playing is a mission's NIS: its end posts nischallengenotify (P3).
    mission_scene: bool,
    /// AI.SelectCharacters ran. INFERRED (P5 L8): it names no id, so the competitor takes each id the script spawns.
    select: bool,
    /// The AI riders this mission spawned (Lua ids), and the volumes each is inside.
    ai_ids: BTreeSet<i64>,
    ai_inside: BTreeSet<(i64, String)>,
    /// The riders whose skatercreated has posted, and each rider's open sequence (Observer::sequence).
    ai_created: BTreeSet<i64>,
    ai_sequences: BTreeMap<i64, Observer>,
    /// SkateMission.GetPlayerName by Lua id, refreshed by `screens`. RETAIL: SKATEMission's (0x824622f8) and
    /// SkateMission's (0x824369c8) read the match's player record +0x20 (records 0x90 apart at [0x830fce80+8]), which
    /// 0x82319110 fills as 0x82319480 adds the player. Offline (neither [0x830978d0+0x195] nor mode [0x830978d0] == 2),
    /// a human (+0x18 == 1) gets "%s" of [0x830fce80+0x1c]+0x4800 (INFERRED: the signed-in profile's name; here
    /// skate2_contests::player_name), an AI (+0x18 == 2) its character's short name (skate1_rider::Skate1Ai::name).
    /// Run12: unset, the HUD showed no names by the S.K.A.T.E. letters.
    names: BTreeMap<i64, String>,
    /// P5 screens wiring: the HUD manager's calls for Skate1Screens, and whether it has shown any since the last clear.
    screen_states: Vec<ScreenCall>,
    screens_used: bool,
    /// EnterFilmReplay's autoplay playhead (recording seconds) while it plays ([`film_replay_step`]).
    film_replay: Option<f64>,
    /// Learn > Skate 1 Tutorial was picked: `drive` plays or resumes the tutorial next frame ([`Resume`]).
    resume: bool,
}

/// Bug 10-17 #23 (not retail, as the Skate 2 map's "Skate 2 Tutorial"): what Learn > Skate 1 Tutorial does.
#[derive(Debug, PartialEq)]
enum Resume {
    /// A challenge, the frontend or a scene is already going: nothing.
    Busy,
    /// The new-career frontend isn't done: queue it, as the map's load does.
    Frontend,
    /// The next tutorial lesson (the type-4 auto-start).
    AutoStart,
    /// Every lesson is won: play the chain's first again, keeping the career (Start Over is the erase).
    Replay(String),
}

impl Skate1Challenge {
    fn new(root: PathBuf, settings: PathBuf) -> Self {
        Self {
            root,
            data: None,
            strings: BTreeMap::new(),
            generation: None,
            queue: default(),
            playing: None,
            career: None,
            progress: Progress::default(),
            settings,
            machine: None,
            running: None,
            signups: SignUps::default(),
            films: None,
            accepted: None,
            holding: 0,
            pin: false,
            watch: None,
            lesson: 0,
            volumes: Vec::new(),
            bridge: Shared::default(),
            events: Vec::new(),
            next_frame: Vec::new(),
            hud: Hud::default(),
            observer: Observer::default(),
            manuals_connect: true,
            pause: false,
            speech: None,
            takes: BTreeMap::new(),
            rng: default(),
            dialog: PlaybackSettings::DESPAWN,
            indicators: Vec::new(),
            indexed: BTreeMap::new(),
            pressed: Pressed::default(),
            missions: None,
            mission_events: Vec::new(),
            cleanup_at_accept: false,
            mission_scene: false,
            select: false,
            ai_ids: BTreeSet::new(),
            ai_inside: BTreeSet::new(),
            ai_created: BTreeSet::new(),
            ai_sequences: BTreeMap::new(),
            names: BTreeMap::new(),
            screen_states: Vec::new(),
            screens_used: false,
            film_replay: None,
            resume: false,
        }
    }

    /// Free skate on the Skate 1 map: no challenge, nothing presenting.
    fn idle(&self) -> bool {
        self.data.is_some() && self.machine.is_none() && self.playing.is_none() && self.queue.is_empty()
    }

    /// The available type-3 challenges get boxes at their Location +- SignupRadius (0x823d4238).
    fn rebuild_signups(&mut self) {
        let (Some(data), Some(career)) = (&self.data, &self.career) else { return };
        self.signups.boxes = career
            .available(&self.progress)
            .into_iter()
            .filter(|r| career.kinds.get(&r.name) == Some(&3))
            .filter_map(|r| {
                let at = data.position(data.field(&r.name, "Location")?.as_str()?)?;
                let radius = data.field(&r.name, "SignupRadius").and_then(Value::as_f64).unwrap_or(0.);
                Some((r.name.clone(), Aabb::signup(at, radius as f32)))
            })
            .collect();
    }

    /// The film menu's list (0x823d0790): available ChallengeType 7 (film) records. ponytail: retail lists those
    /// with +0x3C0 == 0 before == 1 (INFERRED Skateboard Mag vs Thrasher); inside the window one film is available
    /// at a time.
    /// EnterExtendedNIS: the scene plays from the queue (it holds the game), then niscomplete.
    fn queue_nis(&mut self, name: String) {
        self.queue.push_back(Item::Scene(name));
        self.queue.push_back(Item::NisComplete);
    }

    /// What skate1_cast reads each frame (P4 interface).
    fn show_people(&self, people: &mut crate::skate1_cast::People) {
        people.on = self.career.is_some();
        people.available = self.career.as_ref().map_or_else(BTreeSet::new, |c| {
            c.available(&self.progress).into_iter().map(|r| r.name.clone()).collect()
        });
        people.running = self.running.clone().filter(|_| !self.cleanup_at_accept || self.bridge.lock().unwrap().accepted);
    }

    fn films_available(&self) -> Vec<String> {
        let (Some(data), Some(career)) = (&self.data, &self.career) else { return Vec::new() };
        career
            .available(&self.progress)
            .into_iter()
            .filter(|r| data.field(&r.name, "ChallengeType").and_then(Value::as_i64) == Some(7))
            .map(|r| r.name.clone())
            .collect()
    }

    /// A screen or movie owns the controller.
    fn modal(&self) -> bool {
        // skate1_screens' FE menus (the sign-up poster, the lesson end, ...) are modal through Skate1Screens::holds_menu.
        matches!(self.playing, Some(Playing::Movie(_) | Playing::Scene(_) | Playing::Screen(_)))
    }
    fn text(&self, label: &str) -> String {
        self.strings.get(label).cloned().unwrap_or_else(|| label.trim_start_matches('#').to_owned()).replace('^', "\n")
    }
    /// The running challenge's text (P6 L-P5): a lesson's, a mission's or a pro's.
    fn lesson_text(&self, key: &str) -> String {
        let label = self.running.as_deref().and_then(|n| self.data.as_ref()?.field(n, key)?.as_str().map(str::to_owned));
        label.map(|l| self.text(&l)).unwrap_or_default()
    }
}

pub(crate) struct Skate1ChallengePlugin;
impl Plugin for Skate1ChallengePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LessonMenu>()
            .init_resource::<FeScreen>()
            .add_systems(PostStartup, setup.after(crate::graphics_menu::PresentationSetup))
            .add_systems(
                PreUpdate,
                (
                    capture.after(crate::customiser::navigation).before(crate::challenge_map::interact),
                    // capture can't follow the poll: challenge_map -> core_menu -> MenuInput -> map_transition ->
                    // poll_controllers. The swallow has to, or the poll refills the gameplay pad.
                    swallow.after(crate::input::poll_controllers),
                ),
            )
            .add_systems(
                FixedUpdate,
                observe.after(crate::app::SimulationSet::Physics).run_if(crate::graphics_menu::gameplay_active),
            )
            // After Skate 2's run: it writes ControllerInput::pinned every frame on every map, so on the Skate 1 map
            // PinAI's write has to come last.
            .add_systems(
                Update,
                (drive, screens).chain().after(crate::app::FrameSet::Physics).after(crate::skate2_challenge::run),
            )
            .add_systems(Update, watch_camera.after(drive));
    }
}

impl Skate1Challenge {
    /// The challenge the machine runs (NIS.GetProInfo's mission, intro_overlay.rs).
    /// Learn > Skate 1 Tutorial: Start Over (core_menu). Not retail (user 10-15, the Skate 2 map's Start Over): the
    /// career is erased and `drive` reloads the tutorial, which ends what runs and queues the new-career frontend.
    pub(crate) fn start_over(&mut self) {
        self.progress = Progress::default();
        self.progress.save(&self.settings);
        self.generation = None;
        info!("Skate 1 tutorial: progress erased; starting over");
    }
    /// Learn > Skate 1 Tutorial: play or resume it (`drive` acts next frame, see [`Resume`]).
    pub(crate) fn resume(&mut self) {
        self.resume = true;
    }
    fn resume_plan(&self, data: &Tutorial) -> Resume {
        if self.machine.is_some() || !self.queue.is_empty() || self.playing.is_some() {
            return Resume::Busy;
        }
        // A save with wins but no frontend_done gets no frontend (`frontend`); it goes on to the lessons.
        if !frontend(&self.progress, &data.intros, data.start().unwrap_or_default()).is_empty() {
            return Resume::Frontend;
        }
        match self.career.as_ref().and_then(|c| c.auto_start(&self.progress)) {
            Some(_) => Resume::AutoStart,
            None => data.chain.first().map_or(Resume::Busy, |first| Resume::Replay(first.clone())),
        }
    }
    fn take_resume(&mut self, data: &Tutorial) -> Result<(), String> {
        if !std::mem::take(&mut self.resume) {
            return Ok(());
        }
        match self.resume_plan(data) {
            Resume::Busy => {
                info!("Skate 1 tutorial: already running");
                Ok(())
            }
            Resume::Frontend => {
                self.queue = frontend(&self.progress, &data.intros, data.start().unwrap_or_default()).into();
                Ok(())
            }
            Resume::AutoStart => self.auto_start(),
            Resume::Replay(first) => self.start_challenge(&first),
        }
    }
    /// The tutorial is loaded (the Skate 1 map, offline).
    pub(crate) fn available(&self) -> bool {
        self.data.is_some()
    }
    pub(crate) fn running(&self) -> Option<&str> {
        self.running.as_deref()
    }
    /// The available sign-ups and their Locations (`rebuild_signups`' boxes), for skate1_pros' markers.
    /// Off the Skate 1 map or online the data is gone and the boxes are last map's: none (review 10-15 I1).
    /// Each available sign-up's box centre; none off the Skate 1 map (review I1). An iterator: markers compare it
    /// every frame (review 10-15 minor 7).
    pub(crate) fn signup_places(&self) -> impl Iterator<Item = (&str, Vec3)> {
        self.signups.boxes.iter().filter(|_| self.data.is_some()).map(|(name, b)| (name.as_str(), (b.min + b.max) / 2.))
    }
}

/// ChallengeManager.SwitchCamera(id) (he_u1_skate01.lua's leader/follower attempts): the gameplay camera follows AI
/// skater `id` while he rides his S.K.A.T.E. turn, run by his own frame (Riders::watch, 0x82419688). Playtest run10:
/// the player's framing carried rigidly on his animation root looked bolted to his body.
fn watch_camera(
    state: Option<Res<Skate1Challenge>>,
    ai: Option<Res<crate::skate1_rider::Skate1Ai>>,
    riders: Option<ResMut<Riders>>,
    camera: Option<ResMut<crate::camera::CameraRuntime>>,
) {
    let (Some(mut riders), Some(mut camera)) = (riders, camera) else { return };
    let key = state.and_then(|s| s.watch).and_then(|id| ai.as_deref()?.key(id).map(str::to_owned));
    if riders.watch(key) {
        camera.reset_rig();
    }
}

/// P5 screens wiring: Skate1Screens::frame needs the World (its screens, Config, Skate1Hud), so this runs exclusive,
/// after drive. Only on the Skate 1 map (tutorial data loaded); a map change clears what a challenge left up.
fn screens(world: &mut World) {
    if !world.contains_resource::<Skate1Challenge>() || !world.contains_resource::<Skate1Screens>() {
        return;
    }
    world.resource_scope(|world, mut state: Mut<Skate1Challenge>| {
        let me = crate::skate2_contests::player_name(world);
        let ai = world.get_resource::<crate::skate1_rider::Skate1Ai>();
        let names = state.ai_ids.iter().filter_map(|&id| Some((id, ai?.name(id)?.to_owned()))).chain([(PLAYER, me)]);
        state.names = names.collect();
        world.resource_scope(|world, mut screens: Mut<Skate1Screens>| {
            // A frontend screen (the medical form, Skate 101, a call) takes the frame's presses (preview 10-14: the
            // medical form never heard A, so the frontend stopped there).
            // Bug 5: and a HUD FE menu (the film's end options, FE 23).
            if world.get_resource::<FeScreen>().is_some_and(|fe| fe.want.is_some()) || screens.holds_menu() {
                for pad in fe_pads(std::mem::take(&mut state.pressed)) {
                    screens.press(world, pad);
                }
            }
            let loaded = state.data.is_some();
            let clear = if loaded {
                state.screens_frame(&mut screens, |s, facts| s.frame(world, facts))
            } else {
                state.screen_states.clear();
                std::mem::take(&mut state.screens_used)
            };
            match (clear, loaded) {
                (true, true) => screens.clear(world),
                (true, false) => screens.unload(world),
                _ => {}
            }
        })
    });
}

/// research10 (RETAIL): A skips a scene whose type isn't CHALLENGE_WIN. Retail's other gates (FE state != 6, no net
/// session) always hold here: the career is mode 1 and the tutorial unloads online. Only a skip that took uses the
/// press: a held photo view's end options (FE 23) hear the rest (user 10-15 run18).
fn scene_press(pressed: &mut Pressed, play: &mut ScenePlay, cutscene: &crate::challenge::cutscene::Cutscene) {
    if pressed.accept && play.skip(cutscene) {
        pressed.accept = false;
    }
}

/// The frame's presses as the screens' pad buttons.
fn fe_pads(p: Pressed) -> Vec<crate::skate1_screens::Pad> {
    use crate::skate1_screens::Pad as P;
    [(p.accept, P::A), (p.back, P::B), (p.up, P::Up), (p.down, P::Down)].into_iter().filter(|(hit, _)| *hit).map(|(_, pad)| pad).collect()
}

fn setup(mut commands: Commands, config: Res<Config>, cameras: Query<Entity, With<IsDefaultUiCamera>>) {
    if cameras.single().is_err() {
        return;
    }
    let settings = crate::saves::settings_dir(&config.asset_root).join("skate1-tutorial.json");
    commands.insert_resource(Skate1Challenge::new(config.asset_root.join(ROOT), settings));
}

/// While a lesson screen or movie is up it takes the controller's menu presses, so the Start menu stays shut. In free
/// skate it takes the sign-up's Y and the film menu's RB / Y (research 1.1 "Type 3", 2.9).
/// A press the menu took never reaches gameplay: after the poll, while it's held.
fn swallow(
    state: Option<ResMut<Skate1Challenge>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut input: ResMut<crate::input::ControllerInput>,
) {
    let Some(mut state) = state else { return };
    let raw = input.raw_input().buttons;
    let down = MENU_KEYS.iter().filter(|&&(bit, key)| raw & bit != 0 || keys.pressed(key)).fold(0, |m, &(bit, _)| m | bit);
    if hold(&mut state.holding, down) {
        input.discard_gameplay();
    }
}

fn capture(
    state: Option<ResMut<Skate1Challenge>>,
    mut nav: ResMut<crate::customiser::Navigation>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    menu: Option<Res<crate::graphics_menu::Menu>>,
    time: Res<Time<Virtual>>,
    screens: Option<Res<Skate1Screens>>,
) {
    let Some(mut state) = state else { return };
    state.pressed = Pressed::default();
    if state.idle() && !menu.is_some_and(|m| m.open) {
        let pad = Pad { rb: nav.pressed & PAD_RB != 0, y: nav.pressed & PAD_Y != 0 || keys.just_pressed(KeyCode::KeyY) };
        let films = if pad.rb || state.films.is_some() { state.films_available() } else { Vec::new() };
        let accepted = if pad.y && state.signups.offer.is_some() {
            state.signups.offer.take()
        } else {
            film_menu(&mut state.films, &films, pad, time.delta_secs())
        };
        if accepted.is_some() {
            // S3R's Y also gets the skater off the board; Skate 1 has no dismount, so the press is the menu's
            // (swallow keeps it from gameplay until it's released).
            state.holding |= PAD_Y;
            nav.pressed &= !PAD_Y;
            keys.clear_just_pressed(KeyCode::KeyY);
            state.accepted = accepted;
        }
        return;
    }
    state.films = None;
    // Bug 5: a HUD FE menu (the film's end options) owns the controller too.
    if !state.modal() && !screens.is_some_and(|s| s.holds_menu()) {
        return;
    }
    let mut taken = 0;
    let mut take = |bits: u16, key: KeyCode| {
        let hit = nav.pressed & bits != 0 || keys.just_pressed(key);
        nav.pressed &= !bits;
        keys.clear_just_pressed(key);
        taken |= if hit { bits } else { 0 };
        hit
    };
    state.pressed = Pressed {
        accept: take(PAD_A, KeyCode::Enter),
        back: take(PAD_B, KeyCode::Backspace),
        skip: take(PAD_START, KeyCode::Escape),
        up: take(1, KeyCode::ArrowUp),
        down: take(2, KeyCode::ArrowDown),
    };
    state.holding |= taken;
}

/// Turns the skater's state into Skate 1 events (spec "Events from the engine"), each fixed tick of a lesson.
fn observe(
    state: Option<ResMut<Skate1Challenge>>,
    skater: Res<SkaterRuntime>,
    physics: Res<GamePhysics>,
    time: Res<Time>,
    replay: Res<Replay>,
) {
    let Some(mut state) = state else { return };
    if state.machine.is_none() {
        return;
    }
    let state = &mut *state;
    let mission = state.missions.is_some();
    let frame = replay_frame(&replay);
    let o = &mut state.observer;
    let dt = time.delta_secs();
    let deck = physics.board.part_transforms()[BodyId::Deck.index()].translation;
    let p = Vec3::new(deck.x, deck.y, deck.z);
    let physical = skater.player_state.current() as u32;
    let ragdoll = skater.skeleton_collision.is_ragdoll || physical == WIPEOUT_STATE;
    let mut out = Vec::new();
    let player = |extra: Value| {
        let mut m = json!({"PlayerID": PLAYER});
        if let (Value::Object(m), Value::Object(e)) = (&mut m, extra) {
            m.extend(e);
        }
        m
    };

    for (name, volume) in &state.volumes {
        let inside = volume.contains(p);
        if inside != o.inside.contains(name) {
            let event = if inside { "triggerentercollision" } else { "triggerexitcollision" };
            out.push(Event::new(event, player(json!({"VolumeID": name}))));
            if inside {
                o.inside.insert(name.clone());
            } else {
                o.inside.remove(name);
            }
        }
    }

    let bailed = ragdoll && !o.ragdoll;
    if bailed {
        out.push(Event::new("wipeout", player(json!({}))));
    }
    o.ragdoll = ragdoll;

    let in_air = AIR_STATES.contains(&physical) && !ragdoll;
    let manual = skater.animation_input.fields.balance != 0.;
    let grinding = GRIND_STATES.contains(&physical);
    let starts_air = in_air && o.air.is_none();
    let starts = starts_air || (manual && !o.manual) || (grinding && o.grind.is_none());
    let manual_ended = o.manual && !manual;
    o.manual = manual;
    // P5 L13: the player's completed air, for the missions' Trick.GetCompletedAir* (Facts.airs[0]).
    if (!in_air || bailed) && let Some(air) = &o.air {
        o.completed = Some(crate::skate1_missions::Air {
            scorable: air.first.or(air.any).map_or(-1, scorable_index),
            spins: air.turns,
            // UNTRACED: switch take-offs aren't tracked here (skate1_online's probe tracks them).
            switched: false,
            horizontal: Vec3::new(p.x - air.from.x, 0., p.z - air.from.z).length(),
        });
    }
    if mission && !in_air && let Some(id) = o.launch.take() && !bailed {
        let s = &skater.scoring;
        out.push(Event::new("trickcomplete", player(json!({"TrickType": trick_type(s, id), "TrickIndex": scorable_index(id)}))));
    }
    let from = out.len();
    land(o, in_air, bailed, starts, &mut out);
    if mission {
        // P5 L7 (INFERRED): Filming 101's aircompleted1 handler hears the same landing as aircompleted.
        let copies: Vec<Event> = out[from..]
            .iter()
            .filter(|e| e.name == "aircompleted")
            .map(|e| Event { name: "aircompleted1", message: e.message.clone(), air: e.air.clone(), grind: None })
            .collect();
        out.extend(copies);
    }
    if starts_air {
        o.air = Some(Air { start: p.y, peak: p.y, reversed: skater.animation.goofy(), from: p, ..default() });
    }
    if in_air && let Some(air) = o.air.as_mut() {
        air.report.time += dt;
        air.peak = air.peak.max(p.y);
        air.report.height = air.peak - air.start;
        air.report.spin = (skater.scoring.spin_turns.abs() * 180) as f32;
        air.turns = i64::from(skater.scoring.spin_turns);
        for id in skater.scoring.holding() {
            if scorable_class(id) == Some(GRAB_CLASS) {
                air.report.grab += dt;
                air.report.tweak |= tweaked(id);
            }
        }
        for &id in &skater.scoring.announced {
            air.any.get_or_insert(id);
            first_scorable(&mut air.first, id);
        }
    }

    if grinding {
        let step = o.last.map_or(0., |last| last.distance(p));
        *o.grind.get_or_insert(0.) += step;
    } else if let Some(distance) = o.grind.take() {
        // GrindLength: fp_u3_tutorial04 reads it (P5 L7); INFERRED metres, as GetCompletedGrindDistance.
        let mut event = Event::new("grindcompleted", player(json!({"Wipedout": i64::from(ragdoll), "GrindLength": distance})));
        event.grind = Some(distance);
        out.push(event);
    }
    o.last = Some(p);

    if skater.scoring.sequence_landed && !mission {
        out.push(Event::new("trickcomplete", player(json!({}))));
    }
    if mission {
        let s = &skater.scoring;
        // RETAIL cMsgTrickComplete (research 5.2 #19): one per completed trick with TrickIndex (the catalog index) and
        // TrickType (Skate 1's eTrickType), from the trick system's list of ended tricks (appended by 0x8279e358).
        // - A manual, grind, powerslide or revert posts when the scoring ends it. Run12: posted when announced, a held
        //   manual's reached Manuals 101 (fp_u3_tutorial01) in substate 3, where any trickcomplete fails, so "stay in
        //   the manual" and "flip out" failed at once. Ended before the air's tricks, as the manual a pop ends.
        // - INFERRED (skate1_online's probe): an air's tricks still post when announced, ahead of retail's land-time
        //   entry; that keeps the flip ahead of the aircompleted a manual starting in the touchdown tick flushes (land).
        let trick = |id| Event::new("trickcomplete", player(json!({"TrickType": trick_type(s, id), "TrickIndex": scorable_index(id)})));
        if o.ended != physics.ticks {
            o.ended = physics.ticks;
            out.extend(s.completed.iter().map(|&id| trick(id)));
        }
        if s.announced.len() < o.seen {
            o.seen = 0;
        }
        if s.in_air() {
            out.extend(air_tricks(&mut o.launch, &s.announced[o.seen..]).into_iter().map(trick));
        }
        o.seen = s.announced.len();
        // sequencecomplete (Observer::sequence). Wipedout is a bool here only (P5 L7).
        let manual = s.holding().any(|id| trick_type(s, id) == MANUAL_TRICK);
        let published = s.sequence_landed.then_some(s.sequence_score);
        if let Some((score, start, wiped)) = o.sequence(published, s.sequence_open(), manual, bailed, frame) {
            let message = json!({"SequenceScore": score, "StartTick": start, "Wipedout": wiped});
            out.push(Event::new("sequencecomplete", player(message)));
            // INFERRED (P5 L7): a landed sequence awards its score.
            if !wiped {
                out.push(Event::new("pointsawarded", player(json!({"Score": score}))));
            }
        }
        if manual_ended {
            out.push(Event::new("manualcompleted", player(json!({"Wipedout": i64::from(ragdoll)}))));
        }
        let sliding = physical == SLIDE_STATE;
        if o.slide && !sliding {
            out.push(Event::new("powerslidecompleted", player(json!({"Wipedout": i64::from(ragdoll)}))));
        }
        o.slide = sliding;
    }

    // The sim's push flags (published as state flags 56 / 55): bit 27 = the push asked for, bit 25 = pushing
    // (INFERRED pairing with retail's input byte / skater flag).
    let flags = skater.player_input.processed.flags_2468;
    if let Some(n) = o.push.tick(flags & 1 << 27 != 0, flags & 1 << 25 != 0) {
        out.push(Event::new("skaterpushed", player(json!({"PushAttempts": n}))));
    }
    // cMsgSkaterAutoPumped (0xEFC8C655, poster 0x826520e8; fp_u3_tutorial03 counts them). INFERRED source: the
    // NewAutoPump packet (crouching's new_auto_pump pulse); UNTRACED: the poster's own test. One post a frame.
    let pumps = skater.animation_input.auto_pumps;
    if o.pumps.is_some_and(|seen| seen != pumps) {
        out.push(Event::new("skaterautopumped", player(json!({}))));
    }
    o.pumps = Some(pumps);
    state.events.extend(out);
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn drive(
    mut commands: Commands,
    state: Option<ResMut<Skate1Challenge>>,
    mut menu: ResMut<LessonMenu>,
    map: Res<CurrentMap>,
    online: Option<Res<crate::multiplayer::Online>>,
    mut skater: ResMut<SkaterRuntime>,
    mut time: ResMut<Time<Virtual>>,
    mut screen: ResMut<FeScreen>,
    mut input: ResMut<crate::input::ControllerInput>,
    (mut cutscene, mut replay, graphics, mut people, mut ai, real, party): (
        ResMut<Cutscene>,
        ResMut<Replay>,
        Option<Res<crate::graphics_menu::Menu>>,
        ResMut<crate::skate1_cast::People>,
        ResMut<crate::skate1_rider::Skate1Ai>,
        Res<Time<Real>>,
        Option<Res<crate::party_play::PartyPlay>>,
    ),
    (mut customiser, mut models): (
        Option<ResMut<crate::customiser::Customiser>>,
        Option<ResMut<crate::custom_models::CustomModels>>,
    ),
    (mut gates, mut screens, riders, marker, voices): (
        ResMut<ProGates>,
        Option<ResMut<Skate1Screens>>,
        Option<Res<Riders>>,
        Option<Res<SessionMarker>>,
        Query<(), With<AudioPlayer<StreamPcm>>>,
    ),
    cameras: Query<Entity, With<IsDefaultUiCamera>>,
    (mut images, mut audio, server, mut materials): (
        ResMut<Assets<Image>>,
        ResMut<Assets<StreamPcm>>,
        Res<AssetServer>,
        ResMut<Assets<StandardMaterial>>,
    ),
) {
    let Some(mut state) = state else { return };
    let state = &mut *state;
    state.dialog = crate::music::one_shot(graphics.as_deref(), crate::music::DIALOG);
    // Joining or leaving a session reloads too: the tutorial is single player only, never online and never in Party
    // Play (user 11-02: Party Play on a Skate 1 map started the tutorial), as the Skate 2 tutorial's reload.
    let key = (map.generation, online.is_some_and(|o| o.0) || party.is_some_and(|p| p.active));
    if state.generation != Some(key) {
        if state.pin {
            input.pinned = false;
        }
        state.stop_scene(&mut cutscene, &mut replay);
        state.end(&mut commands, &mut menu, &mut images);
        sync_screen(&mut screen, None);
        state.generation = Some(key);
        state.data = None;
        state.career = None;
        if crate::skate1_hud::skate1_map(&map) && !key.1 {
            match load(&state.root) {
                Ok((data, strings, records)) => {
                    state.progress = Progress::load(&state.settings);
                    state.queue = frontend(&state.progress, &data.intros, data.start().unwrap_or_default()).into();
                    state.career = Some(Career::new(records, data.kinds()));
                    state.data = Some(Arc::new(data));
                    state.strings = strings;
                    state.signups = SignUps::default();
                    state.rebuild_signups();
                    // ROOT is two folders under the asset root; Skate1Ai joins its own folder.
                    if let Some(Err(error)) = state.root.ancestors().nth(2).map(|assets| ai.open(assets)) {
                        warn!("Skate 1 tutorial: {error}");
                    }
                    info!("Skate 1 tutorial ready ({})", state.root.display());
                    if state.queue.is_empty() {
                        if let Err(error) = state.auto_start() {
                            state.stop_scene(&mut cutscene, &mut replay);
                            state.fail(&error, &mut commands, &mut menu, &mut images);
                        }
                    }
                }
                Err(error) => {
                    if state.root.join("tutorial.json").exists() {
                        error!("Skate 1 tutorial unavailable: {error}");
                    }
                }
            }
        }
    }
    state.show_people(&mut people);
    // The mission's SetManualsAreConnectors (he_u2_photo01, he_u4_besttrick01 clear it outside their spots), set
    // again once none runs (a map change ends it above). Written on a change only, as online_scoring writes a
    // match's, so either stands; a request lands here the frame after carry_out takes it.
    let connect = state.machine.is_none() || !state.observer.split;
    if state.manuals_connect != connect || (skater.is_added() && !connect) {
        state.manuals_connect = connect;
        skater.scoring.manuals_connect = connect;
    }
    let Some(data) = state.data.clone() else { return };
    if let Err(error) = state.take_resume(&data) {
        state.stop_scene(&mut cutscene, &mut replay);
        state.fail(&error, &mut commands, &mut menu, &mut images);
    }

    // The frontend: each item plays to its end before the next.
    loop {
        let done = match &mut state.playing {
            None => true,
            Some(Playing::Screen(fe)) => screen_done(fe, &screen),
            Some(Playing::Scene(play)) => {
                // research10 (RETAIL): A skips a scene whose type isn't CHALLENGE_WIN. Retail's other gates (FE state
                // != 6, no net session) always hold here: the career is mode 1 and the tutorial unloads online.
                scene_press(&mut state.pressed, play, &cutscene);
                match play.tick(&cutscene, &mut replay) {
                    None => false,
                    // P3 (RETAIL poster 0x827cf080): a mission's NIS posts nischallengenotify 1 then niscomplete; a
                    // CHALLENGE_WIN scene posts 2 (photo view) and holds until NIS.KillNIS.
                    Some(end) if state.mission_scene => {
                        let complete = end == SceneEnd::Complete;
                        let id = if complete { 1 } else { 2 };
                        state.next_frame.push(Event::new("nischallengenotify", json!({"NotificationID": id})));
                        if complete {
                            state.next_frame.push(Event::new("niscomplete", json!({"PlayerID": PLAYER})));
                        }
                        complete
                    }
                    Some(_) => true,
                }
            }
            Some(Playing::Movie(movie)) => {
                // Not retail (user 10-14): A or Start skips an intro movie.
                let p = std::mem::take(&mut state.pressed);
                movie.advance(&mut images) || p.accept || p.skip
            }
            Some(Playing::Customiser) => !customiser.as_ref().is_some_and(|c| c.open),
        };
        if !done {
            break;
        }
        match state.playing.take() {
            Some(Playing::Movie(movie)) => movie.stop(&mut commands, &mut images),
            Some(Playing::Scene(play)) => {
                state.mission_scene = false;
                play.stop(&mut cutscene, &mut replay)
            }
            _ => {}
        }
        let Some(item) = state.queue.pop_front() else { break };
        match item {
            Item::Screen(fe) => {
                *screen = FeScreen { want: Some(fe.clone()), ..default() };
                state.playing = Some(Playing::Screen(fe));
            }
            Item::Movie(name) => {
                let started = data.movies.get(&name).ok_or_else(|| "not in tutorial.json".to_owned()).and_then(|m| {
                    let camera = cameras.single().map_err(|e| e.to_string())?;
                    Movie::start(
                        &mut commands,
                        &mut images,
                        &mut audio,
                        camera,
                        &state.root.join(&m.video),
                        Some(&state.root.join(&m.audio)),
                    )
                });
                match started {
                    Ok(movie) => state.playing = Some(Playing::Movie(movie)),
                    Err(error) => warn!("Skate 1 intro movie {name} skipped: {error}"),
                }
            }
            Item::Customiser => {
                if let Some(c) = customiser.as_deref_mut() {
                    if let Some(m) = models.as_deref_mut() {
                        m.request_stock();
                    }
                    c.begin();
                    state.playing = Some(Playing::Customiser);
                }
            }
            Item::Teleport(at) => teleport(&data, &mut skater, &at, None),
            Item::FrontendDone => {
                state.progress.frontend_done = true;
                state.progress.save(&state.settings);
                if let Err(error) = state.auto_start() {
                    state.stop_scene(&mut cutscene, &mut replay);
                    state.fail(&error, &mut commands, &mut menu, &mut images);
                }
            }
            Item::Line(n) => state.speak(n, &data, &mut commands, &mut audio),
            Item::NisComplete => state.next_frame.push(Event::new("niscomplete", json!({"PlayerID": PLAYER}))),
            // P3: the scene folder is ROOT/nis (skate1_scenes::SCENES).
            Item::Scene(name) => {
                state.playing = Some(Playing::Scene(ScenePlay::start(&mut cutscene, &state.root.join("nis"), &name)));
            }
            Item::AutoStart => {
                if let Err(error) = state.auto_start() {
                    state.stop_scene(&mut cutscene, &mut replay);
                    state.fail(&error, &mut commands, &mut menu, &mut images);
                }
            }
            Item::MissionScene(name, range) => {
                let root = state.root.join("nis");
                let play = match range {
                    None => ScenePlay::start(&mut cutscene, &root, &name),
                    Some((from, to)) => {
                        ScenePlay::start_replay(&mut cutscene, &mut replay, &root, &name, from as f64, to as f64)
                    }
                };
                state.mission_scene = true;
                state.playing = Some(Playing::Scene(play));
            }
        }
    }
    sync_screen(&mut screen, state.playing.as_ref());

    // Free skate: the sign-up boxes follow the skater; an accepted sign-up or film starts (manager slot 2, the
    // challenge manager's starter).
    let m = &skater.animated_skeleton.roots.animation_to_world;
    let root = Vec3::new(m[3][0], m[3][1], m[3][2]);
    let idle = state.idle();
    state.signups.update(root, idle);
    if let Some(name) = state.accepted.take() {
        if let Err(error) = state.start_challenge(&name) {
            state.stop_scene(&mut cutscene, &mut replay);
            state.fail(&error, &mut commands, &mut menu, &mut images);
        }
    }

    if state.machine.is_some() {
        menu.active = true;
        if state.missions.is_some() {
            let speaking = state.speech.is_some_and(|e| voices.contains(e));
            let frame = replay_frame(&replay);
            state.mission_frame(time.delta_secs(), &skater, &mut ai, riders.as_deref(), marker.as_deref(), frame, speaking);
        }
        let result = state.run(time.delta_secs(), &mut menu);
        let result = result.and_then(|ended| {
            state.apply(&mut commands, &mut skater, &mut audio, &server, &mut materials)?;
            state.carry_out(
                &data,
                &mut commands,
                &mut audio,
                &mut skater,
                (&mut *ai, &mut *gates, &mut *people, screens.as_deref_mut()),
                &mut cutscene,
                &mut replay,
            );
            Ok(ended)
        });
        match result {
            Ok(Some(_)) => {
                // CompleteChallenge (research 1.2 8d/8e, 12, 15a): the career takes the result, then the refresh
                // auto-starts the next type-4 challenge (0x823d2b28), the same one again after an abandon.
                let actions = state.completed();
                state.stop_scene(&mut cutscene, &mut replay);
                state.end(&mut commands, &mut menu, &mut images);
                state.rebuild_signups();
                state.queue_actions(actions);
            }
            Ok(None) => {}
            Err(error) => {
                state.stop_scene(&mut cutscene, &mut replay);
                state.fail(&error, &mut commands, &mut menu, &mut images)
            }
        }
    }

    // Bug 5: the film replay plays on real time (the simulation is paused); its end opens the film Options.
    if film_replay_step(&mut state.film_replay, &mut replay, real.delta_secs_f64())
        && let Some(screens) = screens.as_deref_mut()
    {
        screens.film_replay_finished();
    }

    // P6 L-P4: no challenge, no gates. The mission's riders and the cast's camera leave with it.
    if state.running.is_none() {
        gates.reset();
        people.photographing = false;
        if !state.ai_ids.is_empty() {
            ai.clear();
            state.ai_ids.clear();
        }
    }

    if input.pinned != state.pin {
        input.pinned = state.pin;
    }
    // A call is the action overlay over play (state 13); every other screen, movie and scene holds the game.
    let holds = match &state.playing {
        Some(Playing::Screen(fe)) => !matches!(fe, Fe::Call(_)),
        Some(Playing::Movie(_) | Playing::Scene(_)) => true,
        _ => false,
    };
    // UNTRACED: whether an FE push pauses the game. Fallback: an FE menu up (the sign-up poster, the lesson end, ...)
    // holds it, as the native poster did.
    if state.pause || holds || screens.as_ref().is_some_and(|s| s.holds_menu()) {
        time.pause();
    }
}

fn load(root: &std::path::Path) -> Result<(Tutorial, BTreeMap<String, String>, Vec<Record>), String> {
    let read = |name: &str| std::fs::read(root.join(name)).map_err(|e| format!("{name}: {e}"));
    let data: Tutorial = serde_json::from_slice(&read("tutorial.json")?).map_err(|e| format!("tutorial.json: {e}"))?;
    let strings = serde_json::from_slice(&read("strings.json")?).map_err(|e| format!("strings.json: {e}"))?;
    let records = serde_json::from_slice(&read("progress.json")?).map_err(|e| format!("progress.json: {e}"))?;
    if data.chain.is_empty() || data.start().and_then(|s| data.position(s)).is_none() {
        return Err("tutorial.json has no lesson chain or start place".into());
    }
    Ok((data, strings, records))
}

impl Skate1Challenge {
    /// With no challenge running, starts the first available eGlobalType 4 one (0x823d2c08).
    fn auto_start(&mut self) -> Result<(), String> {
        if self.machine.is_some() {
            return Ok(());
        }
        let Some(name) = self.career.as_ref().and_then(|c| c.auto_start(&self.progress)).map(str::to_owned) else {
            return Ok(());
        };
        self.start_challenge(&name)
    }

    /// Starts a challenge by name (manager slot 2, 0x823d2160).
    fn start_challenge(&mut self, name: &str) -> Result<(), String> {
        match self.data.as_ref().and_then(|d| d.chain.iter().position(|n| n == name)) {
            Some(index) => self.start_lesson(index),
            None => match self.data.as_ref().and_then(|d| d.missions.get(name).or_else(|| d.pro_tutorials.get(name))).cloned() {
                Some(mission) => self.start_mission(name, &mission),
                None => {
                    info!("Skate 1 tutorial: {name} is not in tutorial.json");
                    Ok(())
                }
            },
        }
    }

    fn start_lesson(&mut self, index: usize) -> Result<(), String> {
        let data = self.data.clone().ok_or("no tutorial data")?;
        let name = data.chain.get(index).ok_or("lesson index out of the chain")?;
        let lesson = data.lessons.get(name).ok_or_else(|| format!("{name}: not in tutorial.json"))?;
        let path = self.root.join(format!("scripts/{name}.lua"));
        let source = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let bridge = Shared::default();
        let script = Script {
            name,
            source: &source,
            steps: lesson.steps.clone(),
            initial: "initial",
            enter: "stateenter",
            exit: "stateexit",
        };
        let mut machine = Machine::new(script, &lesson.fields, |lua| natives(lua, &bridge))?;
        let mut volumes = Vec::new();
        for key in ["OTSTriggerBoundary", "OTSSpotVolumes"] {
            let names = match lesson.fields.get(key) {
                Some(Value::String(s)) => vec![s.clone()],
                Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect(),
                _ => Vec::new(),
            };
            volumes.extend(names.into_iter().filter_map(|n| data.places.volumes.get(&n).map(|v| (n, *v))));
        }
        self.begin(name, bridge, volumes);
        self.lesson = index;
        info!("Skate 1 tutorial: lesson {name}");
        machine.start()?;
        self.machine = Some(machine);
        Ok(())
    }

    /// P5 L1: a mission or pro tutorial on the shared core: the lead's natives, then skate1_missions' (its register fills
    /// only the names they lack).
    fn start_mission(&mut self, name: &str, mission: &Value) -> Result<(), String> {
        let data = self.data.clone().ok_or("no tutorial data")?;
        let path = self.root.join(format!("scripts/{name}.lua"));
        let source = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let fields = crate::skate1_missions::fields(&mission["self"]);
        let bridge = Shared::default();
        let missions: crate::skate1_missions::Shared =
            Arc::new(Mutex::new(crate::skate1_missions::Missions::new(&fields)));
        let script = Script {
            name,
            source: &source,
            steps: crate::skate1_missions::steps(mission),
            initial: "initial",
            enter: "stateenter",
            exit: "stateexit",
        };
        let mut machine = Machine::new(script, &fields, |lua| {
            natives(lua, &bridge)?;
            crate::skate1_missions::register(lua, &missions)
        })?;
        // UNTRACED key (P5 L1): any integer, never compared; the asset load is instant.
        machine.vars().set("kActivityID", 0).map_err(|e| format!("{name}: {e}"))?;
        self.begin(name, bridge, named_volumes(&fields, &data.places.volumes));
        self.missions = Some(missions);
        self.cleanup_at_accept = mission["self"]["AssetLoadType"].as_i64() != Some(1);
        info!("Skate 1 tutorial: mission {name}");
        machine.start()?;
        self.machine = Some(machine);
        Ok(())
    }

    /// What every challenge start resets (start_lesson / start_mission).
    fn begin(&mut self, name: &str, bridge: Shared, volumes: Vec<(String, Volume)>) {
        self.bridge = bridge;
        self.volumes = volumes;
        self.running = Some(name.to_owned());
        self.hud = Hud::default();
        self.observer = Observer::default();
        self.events.clear();
        self.next_frame.clear();
        self.mission_events.clear();
        self.pause = false;
        self.missions = None;
        self.cleanup_at_accept = false;
        self.select = false;
        self.ai_inside.clear();
        self.screen_states.clear();
    }

    /// CompleteChallenge: the career takes the result (0x827ae0a4); a WinChallenge before it counts the challenge
    /// won. Returns the completion actions.
    fn completed(&mut self) -> Vec<Action> {
        let won = std::mem::take(&mut self.bridge.lock().unwrap().won);
        let (Some(name), Some(career)) = (self.running.take(), self.career.as_ref()) else { return Vec::new() };
        let actions = career.complete(&mut self.progress, &name, won);
        self.progress.save(&self.settings);
        actions
    }

    /// research9 1 (RETAIL): a completion's list replaces whatever list was running; the refresh's auto-start runs after
    /// it (ruling, ledger). The played lines are saved.
    fn queue_actions(&mut self, actions: Vec<Action>) {
        self.queue = action_items(actions, &mut self.progress).into();
        self.queue.push_back(Item::AutoStart);
        self.progress.save(&self.settings);
    }

    /// Timers, held objectives, Start menu requests, screen answers and engine events, delivered to the lesson.
    /// Returns the step the lesson was in when it called CompleteChallenge.
    fn run(&mut self, dt: f32, menu: &mut LessonMenu) -> Result<Option<String>, String> {
        let mut events = std::mem::take(&mut self.next_frame);
        {
            let mut b = self.bridge.lock().unwrap();
            for t in &mut b.timers {
                t.left -= dt;
            }
            let (expired, live): (Vec<_>, Vec<_>) = std::mem::take(&mut b.timers).into_iter().partition(|t| t.left <= 0.);
            b.timers = live;
            for t in expired {
                events.push(Event::new("timerexpired", json!({"PlayerID": PLAYER, "TimerID": t.id})));
            }
        }
        if let Some(request) = menu.request.take() {
            let name = match request {
                LessonRequest::Restart => "restartchallenge",
                LessonRequest::Abandon => "abandonchallenge",
            };
            events.push(Event::new(name, json!({"PlayerID": PLAYER})));
        }
        events.append(&mut self.events);
        for event in events {
            {
                let mut b = self.bridge.lock().unwrap();
                if let Some(air) = &event.air {
                    b.air = air.report.clone();
                }
                if let Some(distance) = event.grind {
                    b.grind = distance;
                }
            }
            let mut message = event.message;
            if let Some(air) = &event.air {
                let first = air.first.or(air.any).map_or(-1, |n| self.bridge.lock().unwrap().scorable(n));
                message["FirstScorableId"] = json!(first);
            }
            let machine = self.machine.as_mut().ok_or("no lesson")?;
            machine.send(event.name, &message)?;
            if self.bridge.lock().unwrap().out.iter().any(|c| matches!(c, Call::Complete)) {
                return Ok(Some(machine.state().to_owned()));
            }
        }
        for (name, message) in std::mem::take(&mut self.mission_events) {
            let machine = self.machine.as_mut().ok_or("no lesson")?;
            machine.send(&name, &message)?;
            if self.bridge.lock().unwrap().out.iter().any(|c| matches!(c, Call::Complete)) {
                return Ok(Some(machine.state().to_owned()));
            }
        }
        Ok(None)
    }

    /// HUD.ChangeHUDState: every state, the tutorial window's (19) too, goes to Skate1Screens' HUD manager (P5 screens
    /// wiring, [`screens`]; preview 3 bugs 2 / 3: 19 used to be drawn by a native placeholder).
    fn hud_call(&mut self, state: i64, tutorial: i64) {
        match u8::try_from(state) {
            Ok(s) => self.screen_states.push(ScreenCall::State(s, tutorial)),
            Err(_) => warn!("Skate 1 tutorial: HUD state {state}/{tutorial}"),
        }
        if state == HUD_FREESKATE {
            self.hud = Hud::default();
        }
    }

    /// P5 screens wiring, one frame: the queued HUD states into the manager, `frame` (Skate1Screens::frame in the game,
    /// a no-asset step in tests) with [`Self::screen_facts`], and its answers back as script events next frame.
    /// Returns true when the HUD should be cleared (the challenge that used it ended).
    fn screens_frame(
        &mut self,
        screens: &mut Skate1Screens,
        frame: impl FnOnce(&mut Skate1Screens, &crate::skate1_screens::Facts) -> Vec<crate::skate1_screens::Event>,
    ) -> bool {
        for call in self.screen_states.drain(..) {
            match call {
                ScreenCall::State(state, sub) => screens.change_hud_state(state, sub),
                ScreenCall::Mark(n) => screens.mark_objective_complete(n),
            }
            self.screens_used = true;
        }
        if self.running.is_none() && std::mem::take(&mut self.screens_used) {
            return true;
        }
        // B6: a film / photo / FollowMe state's rows, built before the overlay's SetState reads them (rebuild_rows).
        if let Some(m) = &self.missions {
            m.lock().unwrap().rebuild_rows();
        }
        // Preview 4 (10-15): the type-3 offer's prompt was never shown.
        screens.sign_up_prompt(self.running.is_none() && self.signups.offer.is_some());
        // Free skate too: the minimap's offered-challenge icons (Facts.challenges).
        let events = frame(screens, &self.screen_facts());
        self.next_frame.extend(events.iter().filter_map(screen_event));
        false
    }

    /// Carries out what the scripts asked for.
    fn apply(
        &mut self,
        commands: &mut Commands,
        skater: &mut SkaterRuntime,
        audio: &mut Assets<StreamPcm>,
        server: &AssetServer,
        materials: &mut Assets<StandardMaterial>,
    ) -> Result<(), String> {
        let calls = std::mem::take(&mut self.bridge.lock().unwrap().out);
        let data = self.data.clone().ok_or("no tutorial data")?;
        for call in calls {
            match call {
                Call::Speech(id) => self.speak(id, &data, commands, audio),
                Call::Hud(state, tutorial) => self.hud_call(state, tutorial),
                Call::Objective(slot, label, arg) => self.set_objective(slot, &label, arg),
                Call::ObjectiveDone(n) => self.objective_done(n),
                Call::Controller(sequence, gesture) => self.show_controller(sequence, gesture),
                Call::Teleport(at, slot) => teleport(&data, skater, &at, slot),
                Call::Pause(on) => self.pause = on,
                Call::Nis(name) => self.queue_nis(name),
                Call::Complete => {}
                Call::Pin(on) => self.pin = on,
                Call::Indicators(on) => {
                    for e in self.indicators.drain(..) {
                        commands.entity(e).try_despawn();
                    }
                    if on {
                        let at = self.running.as_deref().map_or_else(Vec::new, |n| indicator_places(&data, n));
                        self.indicators = at
                            .into_iter()
                            .map(|t| crate::skate1_screens::spawn_indicator(commands, server, materials, t))
                            .collect();
                    }
                }
                Call::Indicator(i, on) => {
                    if let Some(e) = self.indexed.remove(&i) {
                        commands.entity(e).try_despawn();
                    }
                    let row = usize::try_from(i - 1).ok();
                    let at = self.running.as_deref().zip(row).and_then(|(n, row)| indicator_places(&data, n).into_iter().nth(row));
                    if let (true, Some(t)) = (on, at) {
                        self.indexed.insert(i, crate::skate1_screens::spawn_indicator(commands, server, materials, t));
                    }
                }
            }
        }
        Ok(())
    }

    /// HUD.SetTutorialObjective(slot, label, arg) (RETAIL 0x827b9a48 -> 0x82340048): row slot - 1 gets the label's
    /// text, formatted with arg when it is > 0, not done; the row count becomes slot.
    fn set_objective(&mut self, slot: i64, label: &str, arg: Option<i64>) {
        let Some(row) = usize::try_from(slot - 1).ok() else { return };
        let text = self.text(label);
        let text = match arg {
            Some(n) if n > 0 => text.replace("{0:d}", &n.to_string()),
            _ => text,
        };
        let rows = &mut self.hud.objectives;
        rows.resize(row + 1, (String::new(), false));
        rows[row] = (text, false);
    }

    /// HUD.MarkTutorialObjectiveComplete(n) (RETAIL 0x827b9c18): row n - 1 done, then the HUD manager's half. ponytail:
    /// the row is marked here even when the manager's guard (active type 32 / 24 / 25 / 29) skips the rest; no
    /// objective HUD shows then.
    fn objective_done(&mut self, n: i64) {
        if let Some(row) = usize::try_from(n - 1).ok().and_then(|i| self.hud.objectives.get_mut(i)) {
            row.1 = true;
        }
        self.screen_states.push(ScreenCall::Mark(n));
    }

    /// HUD.ShowController(sequence, gesture) (RETAIL 0x827b9b90): stores them, then queues (Tutorial, ShowController)
    /// and (Tutorial, UpdateController).
    fn show_controller(&mut self, sequence: i64, gesture: i64) {
        // 10-15 diagnostic (Pushing & Timing showed no controller in preview 4, not reproduced offline): once per call.
        // ponytail: remove once a preview log has settled it.
        info!("Skate 1 tutorial: ShowController sequence={sequence} gesture={gesture} running={:?}", self.running);
        self.hud.controller = (sequence, gesture);
        self.hud_call(HUD_TUTORIAL, 5);
        self.hud_call(HUD_TUTORIAL, 7);
    }

    /// A cameraman line: a lesson's PlayCameramanSpeech or a completion's `m=N` (0x823fce88), both bank 31 flags 0.
    /// ponytail: one xorshift for the rolls, the same sequence each launch; retail's CRT rand() can't be matched.
    fn speak(&mut self, n: i64, data: &Tutorial, commands: &mut Commands, audio: &mut Assets<StreamPcm>) {
        let roll = (self.rng.next() & 0x7FFF) as f32 / 32767.;
        match data.cameraman_line(n, roll) {
            Some(takes) => self.say(&format!("31:{n}:0"), takes, commands, audio),
            None if n != 0 => debug!("Skate 1 tutorial: cameraman line {n} silent or not rolled"),
            None => {}
        }
    }

    /// A speech table line on the one dialog channel (a mission's Request::Speech, P5 L8).
    fn say_line(&mut self, key: &str, data: &Tutorial, commands: &mut Commands, audio: &mut Assets<StreamPcm>) {
        let roll = (self.rng.next() & 0x7FFF) as f32 / 32767.;
        match data.line(key, roll) {
            Some(takes) => self.say(key, takes, commands, audio),
            None => debug!("Skate 1 mission: line {key} silent or not rolled"),
        }
    }

    /// One cameraman line at a time; a line's takes play in turn.
    fn say(&mut self, key: &str, takes: &[Take], commands: &mut Commands, audio: &mut Assets<StreamPcm>) {
        let n = self.takes.entry(key.to_owned()).or_insert(0);
        let take = &takes[*n % takes.len()];
        *n += 1;
        match read_wav(&self.root.join(&take.audio)) {
            Ok(pcm) => {
                if let Some(old) = self.speech.take() {
                    commands.entity(old).try_despawn();
                }
                self.speech = Some(commands.spawn((AudioPlayer(audio.add(pcm)), self.dialog)).id());
            }
            Err(error) => warn!("Skate 1 tutorial: line {key}: {error}"),
        }
    }

    fn fail(&mut self, error: &str, commands: &mut Commands, menu: &mut LessonMenu, images: &mut Assets<Image>) {
        error!("Skate 1 tutorial stopped: {error}");
        self.end(commands, menu, images);
    }

    /// Stops a scene the queue was playing (`end` drops it).
    fn stop_scene(&mut self, cutscene: &mut Cutscene, replay: &mut Replay) {
        if let Some(Playing::Scene(play)) = self.playing.take_if(|p| matches!(p, Playing::Scene(_))) {
            play.stop(cutscene, replay);
        }
        self.mission_scene = false;
    }

    /// Back to free skate: no lesson, no screens, simulation running.
    /// Leaves LessonMenu alone unless this tutorial was the one running (the Skate 2 tutorial shares it).
    fn end(&mut self, commands: &mut Commands, menu: &mut LessonMenu, images: &mut Assets<Image>) {
        let running = self.machine.is_some() || self.playing.is_some() || !self.queue.is_empty();
        self.queue.clear();
        if let Some(Playing::Movie(movie)) = self.playing.take() {
            movie.stop(commands, images);
        }
        self.machine = None;
        self.running = None;
        self.pin = false;
        self.watch = None;
        self.films = None;
        self.signups.offer = None;
        self.accepted = None;
        self.volumes.clear();
        self.events.clear();
        self.next_frame.clear();
        self.hud = Hud::default();
        self.pause = false;
        self.missions = None;
        self.mission_events.clear();
        self.cleanup_at_accept = false;
        self.mission_scene = false;
        self.select = false;
        self.ai_inside.clear();
        self.ai_created.clear();
        self.ai_sequences.clear();
        self.screen_states.clear();
        self.film_replay = None;
        for e in self.indicators.drain(..).chain(std::mem::take(&mut self.indexed).into_values()) {
            commands.entity(e).try_despawn();
        }
        if running {
            menu.active = false;
            menu.request = None;
        }
    }
}

// ---------------------------------------------------------------- missions and pro tutorials (P5 / P6 lead glue)
/// ChallengeReplay.GetLastFrameNumberWritten / a sequence's StartTick: the recording clock in 60 Hz frames (as
/// Skate 2's highlight clock).
fn replay_frame(replay: &Replay) -> i64 {
    (replay.now() * REPLAY_FPS).round() as i64
}

/// Bug 5 (preview 3): ChallengeReplay.EnterFilmReplay's autoplay, one frame. RETAIL: the native posts
/// cMsgEnterFilmReplay (0x3E580FCA, 0x827bc128); its handler 0x823f22d8 pushes the autoplay controller (0x823f1ed8,
/// update 0x823f20a0) on the replay player, which plays forward at speed 1 (+460 = 100 %: one 1/60 s frame per 1/60 s,
/// player update 0x825e5e28) on the presentation clock, so PauseSimulation doesn't hold it. At the recording's end
/// (cursor +20 >= end +28) the player stops (state 0) and the controller posts cMsgFilmReplayAutoplayFinished
/// (0xF5562738) and cMsgFilmReplayFinished once: no loop. True on that frame. Nothing recorded ends at once (retail:
/// the controller finds the player in state 0, INFERRED from the +49 guard in 0x823f1ed8).
/// UNTRACED, not ported: the replay camera cycling (every 14 s, 5 angles, [0x83170a44]+20980) and any input during
/// the autoplay (the 'replay' legend's Skip).
fn film_replay_step(at: &mut Option<f64>, replay: &mut Replay, dt: f64) -> bool {
    let Some(t) = at else { return false };
    let end = replay.now();
    *t = (*t + dt).min(end);
    if replay.scripted {
        replay.seek(*t);
        if *t < end {
            return false;
        }
    }
    *at = None;
    replay.playing = false;
    true
}

/// P5 L12: every `self` string or string array that names a volume (missions call them PhotoScoringBoundary,
/// PhotoChallengeBoundary, PhotoSpotVolumes, SpotVolumes, ScoringBoundary, FollowTheLeaderDestinationBoundary; the
/// pros OTSSpotVolumes), once each.
fn named_volumes(fields: &Value, volumes: &BTreeMap<String, Volume>) -> Vec<(String, Volume)> {
    let mut out: Vec<(String, Volume)> = Vec::new();
    for value in fields.as_object().into_iter().flat_map(|o| o.values()) {
        let names: Vec<&str> = match value {
            Value::String(s) => vec![s.as_str()],
            Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        for name in names {
            if let Some(v) = volumes.get(name).filter(|_| !out.iter().any(|(n, _)| n == name)) {
                out.push((name.to_owned(), *v));
            }
        }
    }
    out
}

/// P5 L7: the AI riders' landings and bails (Skate1Ai::drain) as their riders' events; a landing is that rider's
/// completed air (Facts.airs).
fn ai_events(drained: Vec<(i64, AiEvent)>, airs: &mut BTreeMap<i64, crate::skate1_missions::Air>) -> Vec<Event> {
    drained
        .into_iter()
        .map(|(id, event)| match event {
            AiEvent::Landed { trick, spins, switched, bail } => {
                // UNTRACED: the rider's horizontal air distance. Fallback: 0.
                let air = crate::skate1_missions::Air { scorable: i64::from(trick), spins: i64::from(spins), switched, horizontal: 0. };
                airs.insert(id, air);
                Event::new("aircompleted", json!({"PlayerID": id, "FirstScorableId": trick, "Wipedout": i64::from(bail)}))
            }
            AiEvent::Wipeout => Event::new("wipeout", json!({"PlayerID": id})),
        })
        .collect()
}

/// P5 L7: a skate1_screens answer as the script hears it. The objective mark and the indicator switch are the
/// engine's; a film pick starts that film (the lead's StartFilm).
fn screen_event(event: &crate::skate1_screens::Event) -> Option<Event> {
    use crate::skate1_screens::Event as S;
    let name = match event {
        S::Loaded => "hudloaded",
        S::Complete => "hudcomplete",
        S::Yes => "hudyes",
        S::No => "hudno",
        S::Continue => "hudcontinue",
        S::Retry => "hudretry",
        S::DisplayExit => "postchallengedisplayexit",
        S::ObjectiveComplete(_) | S::Indicator(_) | S::StartFilm(_) => return None,
    };
    Some(Event::new(name, json!({"PlayerID": PLAYER})))
}

impl Skate1Challenge {
    /// P5 L7 / L13: the mission's facts this frame, then its events after the engine's (observe's): the AI riders'
    /// volumes and landings, the dialog line ending, then what skate1_missions posts itself (missions.tick).
    #[allow(clippy::too_many_arguments)]
    fn mission_frame(
        &mut self,
        dt: f32,
        skater: &SkaterRuntime,
        ai: &mut Skate1Ai,
        riders: Option<&Riders>,
        marker: Option<&SessionMarker>,
        frame: i64,
        speaking: bool,
    ) {
        let Some(missions) = self.missions.clone() else { return };
        if self.speech.is_some() && !speaking {
            self.speech = None;
            self.events.push(Event::new("speechended", json!({"PlayerID": PLAYER})));
        }
        let s = &skater.scoring;
        let m = &skater.animated_skeleton.roots.animation_to_world;
        let physical = skater.player_state.current() as u32;
        let mut sequence_types = [0; 12];
        for &id in &s.announced {
            if let Some(n) = sequence_types.get_mut(trick_type(s, id) as usize) {
                *n += 1;
            }
        }
        let mut facts = MissionFacts {
            dt,
            player: std::array::from_fn(|r| std::array::from_fn(|c| m[r][c])),
            frame,
            cameraman_playing: speaking,
            in_air_or_grinding: AIR_STATES.contains(&physical) || GRIND_STATES.contains(&physical),
            in_sequence: s.sequence_open(),
            doing: s.holding().map(|id| trick_type(s, id)).collect(),
            sequence_types,
            player_regular: !skater.animation.goofy(),
            marker_sets: marker.map_or(0, |m| m.sets as u32),
            marker_uses: marker.map_or(0, |m| m.uses as u32),
            // UNTRACED (P5 L13): no_skate_zone, line_timer_on and bones have no engine source. Fallback: false / 0.
            ..default()
        };
        facts.positions.insert(PLAYER, [m[3][0], m[3][1], m[3][2]]);
        let launch = self.observer.air.as_ref().is_some_and(|a| a.first.or(a.any).is_some());
        facts.valid_launch.insert(PLAYER, launch);
        if let Some(air) = self.observer.completed {
            facts.airs.insert(PLAYER, air);
        }
        let mut riding = Vec::new();
        for &id in &self.ai_ids {
            facts.abd.insert(id, i64::from(ai.abd(id)));
            let Some(rider) = ai.key(id).and_then(|k| riders?.get(k)) else {
                self.ai_created.remove(&id);
                continue;
            };
            facts.positions.insert(id, rider.body().position.to_array());
            // INFERRED (user 10-15, Community Spirit waits for every competitor's): cMsgSkaterCreated once the
            // rider is in the world. UNTRACED: its retail poster (the AI skater's load end).
            if self.ai_created.insert(id) {
                riding.push(Event::new("skatercreated", json!({"PlayerID": id})));
            }
            // INFERRED: the rider's own scoring ends its sequences as the player's (retail's AI is a full cSkater;
            // physics/frame.rs scores riders). UNTRACED: a rider's manual connecting; never held back here.
            let (published, open) = rider.sequence();
            let sequence = self.ai_sequences.entry(id).or_default().sequence(published, open, false, rider.body().wiping_out, frame);
            if let Some((score, start, wiped)) = sequence {
                let message = json!({"PlayerID": id, "SequenceScore": score, "StartTick": start, "Wipedout": wiped});
                riding.push(Event::new("sequencecomplete", message));
            }
        }
        let mut events = riding;
        events.extend(self.ai_triggers(&facts.positions));
        events.extend(ai_events(ai.drain(), &mut facts.airs));
        self.events.extend(events);
        let posted = missions.lock().unwrap().tick(&facts);
        self.mission_events.extend(posted);
    }

    /// P5 L7: the AI riders' volume crossings (the player's are observe's).
    fn ai_triggers(&mut self, positions: &BTreeMap<i64, [f32; 3]>) -> Vec<Event> {
        let mut out = Vec::new();
        for (&id, p) in positions.iter().filter(|(id, _)| **id != PLAYER) {
            for (name, volume) in &self.volumes {
                let inside = volume.contains(Vec3::from_array(*p));
                let key = (id, name.clone());
                if inside != self.ai_inside.contains(&key) {
                    let event = if inside { "triggerentercollision" } else { "triggerexitcollision" };
                    out.push(Event::new(event, json!({"PlayerID": id, "VolumeID": name})));
                    if inside {
                        self.ai_inside.insert(key);
                    } else {
                        self.ai_inside.remove(&key);
                    }
                }
            }
        }
        out
    }

    /// P5 L8: this frame's mission requests (skate1_missions::drain). The skater, the dialog channel, the scenes and
    /// the replay are carried out here; everything else in [`Self::take_request`].
    #[allow(clippy::too_many_arguments)]
    fn carry_out(
        &mut self,
        data: &Tutorial,
        commands: &mut Commands,
        audio: &mut Assets<StreamPcm>,
        skater: &mut SkaterRuntime,
        (ai, gates, people, mut screens): (&mut Skate1Ai, &mut ProGates, &mut People, Option<&mut Skate1Screens>),
        cutscene: &mut Cutscene,
        replay: &mut Replay,
    ) {
        let Some(missions) = self.missions.clone() else { return };
        let requests = missions.lock().unwrap().drain();
        for request in requests {
            let Some(request) = self.take_request(request, data, ai, gates, people, screens.as_deref_mut()) else { continue };
            match request {
                Request::PlayerTo(m) => {
                    if let Err(error) = skater.travel_to(m) {
                        warn!("Skate 1 mission: ReturnPlayerToSessionMarker: {error}");
                    }
                }
                Request::Speech { speaker, line, context } => {
                    self.say_line(&format!("{speaker}:{line}:{context}"), data, commands, audio)
                }
                Request::StopCameraman => {
                    if let Some(e) = self.speech.take() {
                        commands.entity(e).try_despawn();
                    }
                }
                // L15: Trick.DisableWipeoutReset holds the sim's timed bail reset (s1-pros, physics/wipeout_states).
                Request::DisableWipeoutReset => {
                    let w = skater.wiping_out();
                    skater.wipeout_state.hold_reset(w);
                }
                Request::Nis(Nis::Kill) => self.stop_scene(cutscene, replay),
                // EnterFilmReplay: the recording from that frame, played by drive's film_replay_step to its end.
                Request::FilmReplay(frame) => {
                    if replay.script(frame as f64 / REPLAY_FPS) {
                        replay.playing = true;
                    }
                    self.film_replay = Some(replay.playhead().map_or(0., |(_, t)| t));
                }
                Request::RealTime => {
                    self.film_replay = None;
                    if replay.scripted {
                        replay.exit();
                    }
                }
                other => warn!("Skate 1 mission: {other:?} not carried out"),
            }
        }
    }

    /// P5 L8 / P6 L-P4: one mission request for the tutorial, the AI riders, the gates, the cast or the screens. The
    /// rest go back to [`Self::carry_out`].
    fn take_request(
        &mut self,
        request: Request,
        data: &Tutorial,
        ai: &mut Skate1Ai,
        gates: &mut ProGates,
        people: &mut People,
        screens: Option<&mut Skate1Screens>,
    ) -> Option<Request> {
        let name = self.running.clone().unwrap_or_default();
        match request {
            Request::Ai(id, call) => self.ai_call(id, call, &name, data, ai),
            Request::LoadPaths => {
                ai.load_paths(&name);
            }
            Request::UnloadPaths => ai.unload_paths(),
            Request::SelectCharacters => self.select = true,
            Request::FilterReset => ai.filter_reset(),
            Request::FilterPaths(round) => ai.filter_paths(round),
            Request::Nis(Nis::Offer(scene) | Nis::Accept(scene, false) | Nis::Decline(scene) | Nis::Flythrough(scene)) => {
                self.queue.push_back(Item::MissionScene(scene, None))
            }
            // Preview 4 (10-15): the pros' accept NIS ends on the asset loader (Nis::Accept). UNTRACED: how long
            // retail holds it (until EndTransition / the assets); fallback: the loader's intro, as the frontend's.
            // INFERRED: that it shows at all; 0x825aaa90 case 7's "assets not loaded" gate (+32/+33) is untraced.
            Request::Nis(Nis::Accept(scene, true)) => {
                self.queue.push_back(Item::MissionScene(scene, None));
                self.queue.push_back(Item::Screen(Fe::LoadingQuick));
            }
            Request::Nis(Nis::Replay { scene, from, to }) => self.queue.push_back(Item::MissionScene(scene, Some((from, to)))),
            Request::WinState(won) => {
                if let Some(screens) = screens {
                    screens.set_win_state(won);
                }
            }
            Request::Lose => self.bridge.lock().unwrap().won = false,
            Request::Photographing(on) => people.photographing = on,
            // skate1_screens' skate101: TriggerSkate101 opens n = 15 by its number (no name for it in 0x82144ae8).
            Request::Skate101(n) => self.queue.push_back(Item::Screen(Fe::Skate101(n.to_string()))),
            Request::Indicator { volume, kind } => {
                let aabb = self
                    .volumes
                    .iter()
                    .find(|(n, _)| *n == volume)
                    .map(|(_, v)| crate::skate1_online::Aabb { min: v.min, max: v.max });
                gates.set(volume, kind, aabb);
            }
            Request::ClearIndicators(kind) => gates.clear(kind),
            Request::ResetVolume(volume) => {
                self.observer.inside.remove(&volume);
            }
            // overnight-1015 C4: S.K.A.T.E.'s spectating (npc_leader_attempt / npc_followers_attempts).
            Request::SwitchCamera(id) => self.watch = (id != PLAYER).then_some(id),
            // UNTRACED (P5 L8): no engine side for these yet. Fallback: nothing.
            Request::ManualsAreConnectors(on) => self.observer.split = !on,
            Request::Camera(_)
            | Request::EndTransition
            | Request::ResetScoring(_)
            | Request::AmbientSkaters(_)
            | Request::AiChallenge(_) => {}
            other => return Some(other),
        }
        None
    }

    /// P5 L8: an AI rider native for Lua id `id` (Skate1Ai). The locators are the tutorial's places.
    fn ai_call(&mut self, id: i64, call: AiCall, name: &str, data: &Tutorial, ai: &mut Skate1Ai) {
        let at = |locator: &str, slot: i64| {
            let m = locator_m4(data, locator, Some(slot));
            if m.is_none() {
                warn!("Skate 1 mission: no place {locator}");
            }
            m
        };
        match call {
            AiCall::Spawn(locator) => {
                if self.select && ai.key(id).is_none() {
                    // band = ChallengeType 23 (P4 research 3.5 item 7).
                    let band = data.field(name, "ChallengeType").and_then(Value::as_i64) == Some(23);
                    ai.select(id, name, band);
                }
                // RETAIL 0x827c0340: the id is the slot (arg 3 unread).
                if let Some(m) = at(&locator, id) {
                    ai.spawn(id, m);
                    self.ai_ids.insert(id);
                }
            }
            AiCall::Reposition(locator, slot) => {
                if let Some(m) = at(&locator, slot) {
                    ai.reposition(id, m);
                }
            }
            AiCall::Unspawn => {
                ai.unspawn(id);
                self.ai_ids.remove(&id);
            }
            AiCall::Pin(on) => ai.pin(id, on),
            AiCall::Regular(regular) => ai.set_regular(id, regular),
            AiCall::StartPath { branching, avoid } => {
                ai.start_path(id, branching, avoid.into_iter().map(|t| t as i32).collect())
            }
            AiCall::FollowTrickPattern { trick, spins } => {
                ai.follow_trick_pattern(id, trick as i32, spins.unsigned_abs() as u32);
            }
            AiCall::ClearBehavior => ai.clear_behavior(id),
            AiCall::BandOff => ai.band_off(id),
        }
    }

    /// The screens' facts from the running challenge (P5 L11, P6 L-P6 / L-P7), or in free skate the offered sign-up's
    /// (its type and icon: the action overlay's state 14).
    // UNTRACED: the card's end texts and the film menu. Fallback: empty.
    fn screen_facts(&self) -> crate::skate1_screens::Facts {
        use crate::skate1_screens::{Facts, Objective};
        let mut facts = Facts {
            challenges: Some(
                self.signups
                    .boxes
                    .iter()
                    .map(|(k, b)| (k.clone(), [(b.min.x + b.max.x) / 2., (b.min.z + b.max.z) / 2.]))
                    .collect(),
            ),
            // INFERRED: he_u1_film01's slot is set once its record is won.
            film01_done: self.progress.won.contains("he_u1_film01"),
            ..default()
        };
        let kind = |name: &str| self.data.as_ref().and_then(|d| d.field(name, "ChallengeType")?.as_i64());
        let Some(name) = self.running.clone().or_else(|| self.signups.offer.clone()) else { return facts };
        facts.card.key = name.clone();
        facts.card.challenge_type = kind(&name);
        facts.card.icon = facts.card.challenge_type.map_or_else(String::new, |t| crate::skate1_screens::type_icon(t, &name));
        if self.running.is_none() {
            return facts;
        }
        facts.spawn_rows = self
            .data
            .as_ref()
            .and_then(|d| d.field(&name, "GlobalSpawn")?.as_array().map(|rows| !rows.is_empty()))
            .unwrap_or(false);
        // UNTRACED: SkateFacts.kind3 (the S.K.A.T.E. record +24 == 3). Fallback: false.
        facts.card.title = self.lesson_text("Title");
        facts.card.description = self.lesson_text("Description");
        facts.card.points =
            self.data.as_ref().and_then(|d| d.field(&name, "PointRequirement")?.as_i64()).unwrap_or(0);
        // Preview 3 bugs 2 / 3: the tutorial window's controller and objective rows (ShowController,
        // SetTutorialObjective: kind 3, the label as the title, no result, no checkbox; RETAIL 0x82340048).
        facts.controller = self.hud.controller.0;
        facts.gesture = self.hud.controller.1;
        facts.card.objectives = self
            .hud
            .objectives
            .iter()
            .map(|(title, done)| Objective { title: title.clone(), result: String::new(), done: *done, checkbox: false })
            .collect();
        // INFERRED: Mission.GetCurrentTime is the HUD countdown (AddHUDCountdownTimer; the left-area warning's).
        facts.time = {
            let b = self.bridge.lock().unwrap();
            b.hud_timer.and_then(|id| b.timers.iter().find(|t| t.id == id)).map(|t| t.left.max(0.))
        };
        let Some(missions) = &self.missions else { return facts };
        let m = missions.lock().unwrap();
        let text = |label: &str| self.strings.get(label).cloned().unwrap_or_default();
        // INFERRED: a pro tutorial that set tutorial rows shows those; a mission shows its objective holder's.
        if facts.card.objectives.is_empty() {
            facts.card.objectives = crate::skate1_missions::objectives(&m, &text)
                .into_iter()
                .map(|(title, result, done, checkbox)| Objective { title, result, done, checkbox })
                .collect();
        }
        // Mission.GetCurrentScore: the first Points row's count.
        facts.card.score = m.holder.rows.iter().find(|r| r.kind == 1).map_or(0, |r| r.current);
        facts.skate.won = m.won();
        // The players' names: Self::names. UNTRACED: the trick labels the S.K.A.T.E. HUD shows. Fallback: the catalog
        // identifiers.
        let trick = |i: i64| {
            usize::try_from(i)
                .ok()
                .and_then(|i| skate_core::scoring::catalog::IDENTIFIERS.get(i))
                .map_or_else(String::new, |e| e.0.to_owned())
        };
        facts.skate.view = m.skate_view(&|id| self.names.get(&id).cloned().unwrap_or_default(), &trick);
        // INFERRED: skate_view sorts by SetPlayerOrder, so the leader (order 1, `leader()`) is first.
        facts.skate.leader = 0;
        facts.distance = m.follow_distance();
        // Community Spirit: competitor i is Lua id i - 1 (its CompetitorID), named as Self::names.
        let c = &m.contest;
        facts.best = crate::skate1_screens::BestFacts {
            round: c.round,
            rounds: c.rounds,
            forfeit: c.forfeit,
            rows: self
                .names
                .iter()
                .map(|(&id, name)| {
                    let i = id + 1;
                    let (_, total, rank) = c.standings.get(&i).copied().unwrap_or_default();
                    let get = |t: &BTreeMap<i64, i64>| t.get(&i).copied().unwrap_or(0);
                    (name.clone(), get(&c.scores), get(&c.points), total, rank)
                })
                .collect(),
        };
        facts
    }
}

// ---------------------------------------------------------------- world markers
/// SetAllChallengeVisualIndicators (RETAIL 0x827aeae0 -> 0x823df6f0; research 2.0 "Visual indicators"): one indicator
/// per `VisualIndicators` row at its locator. Every in-window row is type 4, the obstacle indicator `oi000`, which isn't
/// lifted (only types 1-3 / 13 are, +2 m). RETAIL 0x826b7f10: only the locator's translation reaches the arrow; its
/// facing, scale and fade are skate1_screens::face_arrows' (10-15, p5/arrows.md).
/// Preview 3 bug 4: this drew a yellow torus on the ground, not the arrow.
fn indicator_places(data: &Tutorial, challenge: &str) -> Vec<Transform> {
    let rows = data.field(challenge, "VisualIndicators").and_then(Value::as_array).cloned().unwrap_or_default();
    rows.iter()
        .filter_map(|v| data.places.locators.get(v.as_str()?))
        .map(|l| Transform::from_translation(Vec3::new(l.matrix[3][0], l.matrix[3][1], l.matrix[3][2])))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bug 5 (preview 3): EnterFilmReplay froze on its first frame (nothing moved the playhead) and never ended. RETAIL
    /// (film_replay_step): speed 1 on real time to the recording's end, then finished once, no loop.
    #[test]
    fn the_film_replay_plays_to_its_end_once() {
        let mut replay = Replay::default();
        for i in 0..=600 {
            let pose = crate::presentation::Snapshot {
                root: Transform::from_xyz(i as f32, 0., 0.),
                bones: vec![Mat4::IDENTITY],
                camera: Transform::IDENTITY,
                fov: 1.,
            };
            replay.record(pose, 1. / 60., false);
        }
        let end = replay.now();
        assert!(replay.script(end - 2.));
        let mut at = replay.playhead().map(|(_, t)| t);
        let head = |r: &Replay| r.playhead().unwrap().1;
        let first = head(&replay);
        assert!(!film_replay_step(&mut at, &mut replay, 1.));
        assert!((head(&replay) - first - 1.).abs() < 1e-9, "one second on, at speed 1");
        assert!(!film_replay_step(&mut at, &mut replay, 0.9));
        assert!(film_replay_step(&mut at, &mut replay, 0.2), "the end");
        assert!(replay.scripted, "the last frame holds until cMsgEnterRealTime");
        assert!(!film_replay_step(&mut at, &mut replay, 1.), "once");
        // Nothing recorded: finished at once.
        let mut empty = Replay::default();
        assert!(!empty.script(0.));
        assert!(film_replay_step(&mut Some(0.), &mut empty, 0.));
    }

    /// Preview 10-14: the medical form never heard A. A frontend screen is modal and gets the frame's presses.
    #[test]
    fn a_frontend_screen_is_modal_and_hears_a() {
        let mut state = Skate1Challenge::new(PathBuf::new(), PathBuf::new());
        state.playing = Some(Playing::Screen(Fe::CaCDetails));
        assert!(state.modal());
        let pads = fe_pads(Pressed { accept: true, down: true, ..default() });
        assert_eq!(pads, [crate::skate1_screens::Pad::A, crate::skate1_screens::Pad::Down]);
    }

    /// The in-window records of progress.json (old_challenges.py, .sp recs 0-4, 100-108; research 1.1 / 1.2) and the
    /// eGlobalType of each challenge the tutorial loads.
    fn career() -> Career {
        let records = json!([
            {"name": "he_u1_film01", "after": ["fp_u2_tutorial05"], "unlocks": ["u=g:r_progressionstart"],
             "actions": ["u=unlock", "s=101:NewChallenge"]},
            {"name": "he_u1_skate01", "after": ["he_u1_film01"], "unlocks": ["u=c:IP_shingo"],
             "actions": ["r=2", "u=unlock", "m=314"], "points_awarded": 5},
            {"name": "he_u2_follow01", "actions": ["u=unlock", "s=101:SequenceScoring"], "points_required": 5,
             "points_awarded": 5},
            {"name": "he_u2_photo01", "unlocks": ["u=g:mag_he_a2_photo01"], "actions": ["u=unlock", "s=101:FilmMenu"],
             "points_required": 10, "points_awarded": 5},
            {"name": "he_u3_film01", "pre_actions": ["m=2905"], "actions": ["u=unlock", "c=shingo01", "s=101:TrainStation"],
             "points_required": 15, "points_awarded": 5},
            {"name": "he_u4_besttrick01", "actions": ["u=unlock", "c=shingo02", "m=2908"], "points_required": 20},
            {"name": "fp_u2_tutorial01"},
            {"name": "fp_u2_tutorial02", "after": ["fp_u2_tutorial01"]},
            {"name": "fp_u2_tutorial04", "after": ["fp_u2_tutorial02"]},
            {"name": "fp_u2_tutorial05", "after": ["fp_u2_tutorial04"]},
            {"name": "fp_u3_tutorial02", "after": ["he_u1_film01"], "unlocks": ["u=c:Paul_rodriguez"], "actions": ["u=unlock"]},
        ]);
        let kinds = [("fp_u2_tutorial01", 4), ("fp_u2_tutorial02", 4), ("fp_u2_tutorial04", 4), ("fp_u2_tutorial05", 4),
            ("he_u1_film01", 4), ("he_u1_skate01", 3), ("he_u2_follow01", 3), ("he_u2_photo01", 3), ("he_u3_film01", 0),
            ("fp_u3_tutorial02", 3)];
        Career::new(serde_json::from_value(records).unwrap(), kinds.iter().map(|&(n, k)| (n.to_string(), k)).collect())
    }
    fn names(records: Vec<&Record>) -> Vec<&str> {
        records.into_iter().map(|r| r.name.as_str()).collect()
    }

    #[test]
    fn the_lessons_auto_start_in_retail_order() {
        let (career, mut progress) = (career(), Progress::default());
        for name in ["fp_u2_tutorial01", "fp_u2_tutorial02", "fp_u2_tutorial04", "fp_u2_tutorial05", "he_u1_film01"] {
            assert_eq!(career.auto_start(&progress), Some(name));
            career.complete(&mut progress, name, true);
        }
        // S.H.I.N.G.O. is type 3: signed up at its marker, never auto-started.
        assert_eq!(career.auto_start(&progress), None);
    }

    #[test]
    fn rolling_rehab_waits_for_grinds() {
        let (career, mut progress) = (career(), Progress::default());
        for name in ["fp_u2_tutorial01", "fp_u2_tutorial02", "fp_u2_tutorial04"] {
            career.complete(&mut progress, name, true);
        }
        assert_eq!(names(career.available(&progress)), ["fp_u2_tutorial05"]);
    }

    #[test]
    fn shingo_follows_rolling_rehab_and_follow_waits_for_five_points() {
        let (career, mut progress) = (career(), Progress::default());
        for name in ["fp_u2_tutorial01", "fp_u2_tutorial02", "fp_u2_tutorial04", "fp_u2_tutorial05", "he_u1_film01"] {
            career.complete(&mut progress, name, true);
        }
        assert_eq!(names(career.available(&progress)), ["he_u1_skate01", "fp_u3_tutorial02"]);
        career.complete(&mut progress, "he_u1_skate01", true);
        assert_eq!(progress.points, 5);
        assert_eq!(names(career.available(&progress)), ["he_u2_follow01", "fp_u3_tutorial02"]);
    }

    #[test]
    fn a_completion_runs_its_actions_in_slot_order() {
        let (career, mut progress) = (career(), Progress::default());
        progress.won.extend(["fp_u2_tutorial05", "he_u1_film01"].map(String::from));
        assert_eq!(
            career.complete(&mut progress, "he_u1_skate01", true),
            [Action::Unlock("c:IP_shingo".into()), Action::Rop(2), Action::Unlock("unlock".into()), Action::Line(314)]
        );
    }

    #[test]
    fn photo_appends_filming_101s_pre_action() {
        let (career, mut progress) = (career(), Progress::default());
        progress.points = 10;
        let actions = career.complete(&mut progress, "he_u2_photo01", true);
        assert_eq!(actions[actions.len() - 2..], [Action::Skate101("FilmMenu".into()), Action::Line(2905)]);
    }

    #[test]
    fn a_decline_without_a_win_keeps_the_mission() {
        let (career, mut progress) = (career(), Progress::default());
        progress.won.extend(["fp_u2_tutorial05", "he_u1_film01"].map(String::from));
        assert_eq!(career.complete(&mut progress, "he_u1_skate01", false), []);
        assert_eq!(progress.points, 0);
        assert!(names(career.available(&progress)).contains(&"he_u1_skate01"));
    }

    #[test]
    fn an_old_or_hand_edited_save_loads_with_defaults() {
        let progress: Progress = serde_json::from_str(r#"{"won": ["fp_u2_tutorial01"], "unknown": 1}"#).unwrap();
        assert!(!progress.frontend_done && progress.won.contains("fp_u2_tutorial01") && progress.points == 0);
    }

    /// The tutorial state with `name` running and its WinChallenge called (`won`) or not; the save is a temp file.
    fn running(save: &str, name: &str, won: bool) -> Skate1Challenge {
        let settings = std::env::temp_dir().join(format!("skate1-tutorial-{save}-{}.json", std::process::id()));
        let mut state = Skate1Challenge::new(PathBuf::new(), settings);
        state.career = Some(career());
        state.running = Some(name.into());
        state.bridge.lock().unwrap().won = won;
        state
    }

    #[test]
    fn win_challenge_is_kept_for_complete_challenge() {
        let (lua, bridge) = (Lua::new(), Shared::default());
        natives(&lua, &bridge).unwrap();
        lua.load("Challenge.WinChallenge({}, 0)").exec().unwrap();
        assert!(bridge.lock().unwrap().won);
    }

    /// research 1.2 8e: a declined poster runs `failed` (WinChallenge + CompleteChallenge): done, the next one starts.
    #[test]
    fn declining_a_lesson_counts_it_done_and_starts_the_next() {
        let mut state = running("decline", "fp_u2_tutorial01", true);
        assert_eq!(state.completed(), []);
        assert_eq!(state.career.as_ref().unwrap().auto_start(&state.progress), Some("fp_u2_tutorial02"));
        assert!(Progress::load(&state.settings).won.contains("fp_u2_tutorial01"), "saved");
        std::fs::remove_file(&state.settings).ok();
    }

    /// `quit_tutorial` (Start menu Abandon) is CompleteChallenge alone: the type-4 auto-start runs the lesson again.
    #[test]
    fn abandoning_a_lesson_starts_it_again() {
        let mut state = running("abandon", "fp_u2_tutorial01", false);
        state.completed();
        assert_eq!(state.career.as_ref().unwrap().auto_start(&state.progress), Some("fp_u2_tutorial01"));
        std::fs::remove_file(&state.settings).ok();
    }

    /// research 1.2 15a: Shingo's decline is CompleteChallenge without a win; his marker stays and the win flag
    /// doesn't carry over to the next challenge.
    #[test]
    fn declining_shingo_leaves_its_marker() {
        let mut state = running("shingo", "he_u1_skate01", false);
        state.progress.won.extend(["fp_u2_tutorial05", "he_u1_film01"].map(String::from));
        state.completed();
        assert!(names(state.career.as_ref().unwrap().available(&state.progress)).contains(&"he_u1_skate01"));
        state.bridge.lock().unwrap().won = true;
        state.completed();
        assert!(!state.bridge.lock().unwrap().won && state.running.is_none());
        std::fs::remove_file(&state.settings).ok();
    }

    /// L4 (RETAIL eSk8HUDState 0x827afa70): the scripts' state names carry the retail numbers.
    #[test]
    fn hud_states_carry_their_retail_numbers() {
        let (lua, bridge) = (Lua::new(), Shared::default());
        natives(&lua, &bridge).unwrap();
        let get = |name: &str| lua.load(format!("return HUD.eSk8HUDState.{name}")).eval::<i64>().unwrap();
        assert_eq!([get("Idle"), get("FreeSkate"), get("MissionTutorialChallenge"), get("MissionFollowMeChallenge")], [0, 1, 19, 67]);
        assert_eq!(get("ESRB"), 80);
    }

    /// L5: RemoveHUDCountdownTimer drops the timer and the HUD's countdown with it.
    #[test]
    fn removing_the_hud_countdown_clears_the_hud_timer() {
        let (lua, bridge) = (Lua::new(), Shared::default());
        natives(&lua, &bridge).unwrap();
        lua.load("local id = ChallengeTimer.AddHUDCountdownTimer(30) ChallengeTimer.RemoveHUDCountdownTimer(id)").exec().unwrap();
        let b = bridge.lock().unwrap();
        assert!(b.timers.is_empty() && b.hud_timer.is_none());
    }

    /// User 10-15: a flick converts the ollie, which the engine announces first, so the flip is the air's
    /// FirstScorableId; a grab after the launch, or a second flip, isn't.
    #[test]
    fn a_flip_converted_from_an_ollie_is_the_first_scorable() {
        let first = |ids: &[&'static str]| {
            let mut first = None;
            ids.iter().for_each(|&id| first_scorable(&mut first, id));
            first
        };
        assert_eq!(first(&["ollie", "heelflip"]), Some("heelflip"));
        assert_eq!(first(&["nollie", "kickflip"]), Some("kickflip"));
        assert_eq!(first(&["heelflip", "kickflip"]), Some("heelflip"));
        assert_eq!(first(&["ollie", "bsgrab"]), Some("ollie"));
        assert_eq!(first(&["bsgrab", "ollie"]), Some("ollie"));
        assert_eq!(first(&["ollie"]), Some("ollie"));
    }

    /// L6: FirstScorableId / TrickIndex are catalog (IDENTIFIERS) indices, the same in every lesson and mission, and
    /// GetScorableName reads the catalog.
    #[test]
    fn scorable_ids_are_catalog_indices() {
        let (lua, bridge) = (Lua::new(), Shared::default());
        natives(&lua, &bridge).unwrap();
        let catalog = skate_core::scoring::catalog::IDENTIFIERS;
        let kickflip = catalog.iter().position(|e| e.0 == "kickflip").unwrap() as i64;
        assert_eq!(bridge.lock().unwrap().scorable("heelflip") as usize, catalog.iter().position(|e| e.0 == "heelflip").unwrap());
        assert_eq!(bridge.lock().unwrap().scorable("kickflip"), kickflip);
        assert_eq!(lua.load(format!("return Trick.GetScorableName({kickflip})")).eval::<String>().unwrap(), "kickflip");
    }

    /// L3 (s1-missions, RETAIL 0x827b93d8): ChangeHUDState without a sub-state passes -1, not sub-state 0 ("Load").
    #[test]
    fn change_hud_state_without_a_sub_state_passes_minus_one() {
        let (lua, bridge) = (Lua::new(), Shared::default());
        natives(&lua, &bridge).unwrap();
        lua.load("HUD.ChangeHUDState(19) HUD.ChangeHUDState(19, 2)").exec().unwrap();
        let out = &bridge.lock().unwrap().out;
        let huds: Vec<(i64, i64)> = out.iter().filter_map(|c| if let Call::Hud(s, t) = c { Some((*s, *t)) } else { None }).collect();
        assert_eq!(huds, [(19, -1), (19, 2)]);
    }

    /// PinAI / UnpinAI on the player (LocalPlayerID 0) hold the skater; other ids are AI riders (P4).
    #[test]
    fn pin_ai_on_the_player_pins_the_skater() {
        let (lua, bridge) = (Lua::new(), Shared::default());
        natives(&lua, &bridge).unwrap();
        lua.load("AI.PinAI(0) AI.PinAI(5) AI.UnpinAI(0)").exec().unwrap();
        let out = &bridge.lock().unwrap().out;
        let pins: Vec<bool> = out.iter().filter_map(|c| if let Call::Pin(on) = c { Some(*on) } else { None }).collect();
        assert_eq!(pins, [true, false]);
    }

    /// research 1.1 "Type 3": the box is the Location +- r on x, y and z (0x823d10a8); r <= 0 is 40.
    #[test]
    fn the_signup_box_is_location_plus_minus_signup_radius() {
        let at = Vec3::new(1., 2., 3.);
        assert_eq!(Aabb::signup(at, 10.), Aabb { min: Vec3::new(-9., -8., -7.), max: Vec3::new(11., 12., 13.) });
        assert_eq!(Aabb::signup(at, 0.), Aabb { min: at - 40., max: at + 40. });
    }

    /// 0x8283d700: x/z +-0.29 m around the root, y from the root up to +1.74 m.
    #[test]
    fn the_player_probe_is_0_29_wide_and_1_74_tall() {
        let probe = Aabb::probe(Vec3::ZERO);
        assert!(probe.overlaps(&Aabb::signup(Vec3::new(10.28, 0., 0.), 10.)));
        assert!(!probe.overlaps(&Aabb::signup(Vec3::new(10.3, 0., 0.), 10.)));
        assert!(probe.overlaps(&Aabb::signup(Vec3::new(0., 11.73, 0.), 10.)));
        assert!(!probe.overlaps(&Aabb::signup(Vec3::new(0., 11.75, 0.), 10.)));
        assert!(!probe.overlaps(&Aabb::signup(Vec3::new(0., -10.01, 0.), 10.)));
    }

    fn shingo_box() -> SignUps {
        SignUps { boxes: vec![("he_u1_skate01".into(), Aabb::signup(Vec3::new(100., 0., 0.), 10.))], ..default() }
    }

    #[test]
    fn entering_offers_and_leaving_hides() {
        let mut signups = shingo_box();
        signups.update(Vec3::ZERO, true);
        assert_eq!(signups.offer, None);
        signups.update(Vec3::new(95., 0., 0.), true);
        assert_eq!(signups.offer.as_deref(), Some("he_u1_skate01"));
        signups.update(Vec3::new(95., 0., 5.), true);
        assert_eq!(signups.offer.as_deref(), Some("he_u1_skate01"), "still inside");
        signups.update(Vec3::ZERO, true);
        assert_eq!(signups.offer, None);
    }

    #[test]
    fn no_offer_while_a_challenge_runs() {
        let mut signups = shingo_box();
        signups.update(Vec3::new(95., 0., 0.), false);
        assert_eq!(signups.offer, None);
    }

    /// The inside set isn't cleared when a challenge ends (INFERRED from 0x823d3d40): after a decline inside the box
    /// the overlay stays away until the next entry.
    #[test]
    fn the_inside_set_survives_a_decline() {
        let mut signups = shingo_box();
        signups.update(Vec3::new(95., 0., 0.), true);
        assert_eq!(signups.offer.take().as_deref(), Some("he_u1_skate01"), "Y accepts");
        signups.update(Vec3::new(95., 0., 0.), false);
        signups.update(Vec3::new(95., 0., 0.), true);
        assert_eq!(signups.offer, None, "declined, still inside");
        signups.update(Vec3::ZERO, true);
        signups.update(Vec3::new(95., 0., 0.), true);
        assert_eq!(signups.offer.as_deref(), Some("he_u1_skate01"), "entered again");
    }

    /// research 2.9 "Film menu": RB opens it only when a film is available (and nothing runs), RB cycles and resets
    /// the timer, Y starts the selected film, 5 s closes it.
    #[test]
    fn rb_opens_the_film_menu_only_with_a_film_available() {
        let mut menu = None;
        assert_eq!(film_menu(&mut menu, &[], Pad { rb: true, ..default() }, 0.), None);
        assert!(menu.is_none());
        film_menu(&mut menu, &["he_u3_film01".into()], Pad { rb: true, ..default() }, 0.);
        assert!(menu.is_some());
    }

    #[test]
    fn y_starts_the_selected_film() {
        let films = ["a".to_string(), "b".to_string()];
        let mut menu = None;
        film_menu(&mut menu, &films, Pad { rb: true, ..default() }, 0.);
        film_menu(&mut menu, &films, Pad { rb: true, ..default() }, 0.);
        assert_eq!(film_menu(&mut menu, &films, Pad { y: true, ..default() }, 0.).as_deref(), Some("b"));
        assert!(menu.is_none());
    }

    /// research9 2 (RETAIL): a Skate 101 popup ends when it's removed after its outro (code 2), not on PROCEED.
    #[test]
    fn skate101_waits_for_dismiss() {
        let fe = Fe::Skate101("NewChallenge".into());
        let mut screen = FeScreen { want: Some(fe.clone()), intro_done: true, confirmed: true, ..default() };
        assert!(!screen_done(&fe, &screen), "its intro or a PROCEED alone doesn't end it");
        screen.closed = true;
        assert!(screen_done(&fe, &screen));
    }

    /// `m=N` plays once per career (the played list, +720) and is saved with the career.
    #[test]
    fn a_cameraman_line_plays_once() {
        let mut progress = Progress::default();
        assert_eq!(action_items(vec![Action::Line(314)], &mut progress), [Item::Line(314)]);
        assert_eq!(action_items(vec![Action::Line(314)], &mut progress), []);
        let path = std::env::temp_dir().join(format!("skate1-lines-{}.json", std::process::id()));
        progress.save(&path);
        assert!(Progress::load(&path).lines_played.contains(&314));
        std::fs::remove_file(path).ok();
    }

    /// S.H.I.N.G.O.'s slots (research 1.2 16): r=2 is the in-engine NIS_ROP_2002_shingo_S01_0, then the line.
    #[test]
    fn rop_2_plays_its_scene() {
        let actions = vec![Action::Rop(2), Action::Unlock("unlock".into()), Action::Line(314)];
        let items = action_items(actions, &mut Progress::default());
        assert_eq!(items, [Item::Scene("NIS_ROP_2002_shingo_S01_0".into()), Item::Line(314)]);
    }

    /// Filming 101's slots (research 1.2 23): the call, then the popup; the call ends at its end (code 6).
    #[test]
    fn an_incoming_call_shows_and_plays() {
        let actions = vec![Action::Call("shingo01".into()), Action::Skate101("TrainStation".into())];
        let items = action_items(actions, &mut Progress::default());
        let call = Fe::Call("shingo01".into());
        assert_eq!(items, [Item::Screen(call.clone()), Item::Screen(Fe::Skate101("TrainStation".into()))]);
        assert!(!screen_done(&call, &FeScreen { want: Some(call.clone()), intro_done: true, ..default() }));
        assert!(screen_done(&call, &FeScreen { want: Some(call.clone()), closed: true, ..default() }));
    }

    /// `u=`: gear and characters have no unlock store in S3R (UNTRACED): logged, nothing presented or changed.
    #[test]
    fn unlocks_are_logged_not_applied() {
        let mut progress = Progress::default();
        let actions = vec![Action::Unlock("g:r_progressionstart".into()), Action::Unlock("c:IP_shingo".into())];
        assert_eq!(action_items(actions, &mut progress), []);
        assert!(progress.won.is_empty() && progress.points == 0 && progress.lines_played.is_empty());
    }

    /// research9 1 (RETAIL): a completion's list replaces the running one; the auto-start refresh follows the list.
    #[test]
    fn a_second_completion_replaces_the_running_list() {
        let mut state = running("replace", "he_u1_skate01", true);
        state.queue_actions(vec![Action::Skate101("NewChallenge".into()), Action::Skate101("FilmMenu".into())]);
        state.queue_actions(vec![Action::Line(314)]);
        assert_eq!(state.queue.iter().cloned().collect::<Vec<_>>(), [Item::Line(314), Item::AutoStart]);
        assert!(Progress::load(&state.settings).lines_played.contains(&314), "saved");
        std::fs::remove_file(&state.settings).ok();
    }

    /// User 10-15 run13 (Manuals 101): the kickflip's landing, published a few ticks into the manual it lands in
    /// (physics::trick_completeness::a_kickflip_lands_into_a_held_manual), posts no sequencecomplete until the flip
    /// out lands; that one carries both. Off (SetManualsAreConnectors(false)) or with no manual it posts at once.
    #[test]
    fn a_manual_connects_the_landings_either_side_of_it() {
        let mut o = Observer::default();
        assert_eq!(o.sequence(None, true, false, false, 10), None, "the flip in");
        assert_eq!(o.sequence(Some(100.), false, true, false, 20), None, "published in the manual");
        assert_eq!(o.sequence(None, true, true, false, 30), None, "held");
        assert_eq!(o.sequence(None, true, false, false, 40), None, "the flip out");
        assert_eq!(o.sequence(Some(250.), false, false, false, 50), Some((350, 10, false)), "one sequence");
        assert_eq!(o.sequence(Some(80.), false, false, false, 60), Some((80, 60, false)), "no manual: at once");
        o.split = true;
        assert_eq!(o.sequence(Some(90.), false, true, false, 70), Some((90, 70, false)), "manuals not connectors");
        o.split = false;
        o.sequence(Some(100.), false, true, false, 80);
        assert_eq!(o.sequence(None, false, true, true, 90), Some((0, 80, true)), "a bail in the manual: wiped");
        assert_eq!(o.sequence(None, false, false, false, 100), None, "and nothing left");
    }

    /// User 10-15 (Manuals 101's flip out): an ollie the flick converts posts only as the flip; one it doesn't waits.
    #[test]
    fn a_converted_ollie_posts_only_its_flip() {
        let mut launch = None;
        assert!(air_tricks(&mut launch, &["ollie"]).is_empty());
        assert_eq!(air_tricks(&mut launch, &["kickflip"]), ["kickflip"]);
        assert_eq!(launch, None);
        assert!(air_tricks(&mut launch, &["nollie"]).is_empty());
        assert_eq!(launch, Some("nollie"), "touchdown posts it");
    }

    fn land_names(o: &mut Observer, in_air: bool, bailed: bool, starts: bool) -> Vec<&'static str> {
        let mut out = Vec::new();
        land(o, in_air, bailed, starts, &mut out);
        out.iter().map(|e| e.name).collect()
    }

    /// research 5.2 #14 (0x826520e8): PushAttempts = presses since the last report; at most one report per 60 ticks.
    #[test]
    fn push_reports_presses_since_last_report_at_most_every_60_ticks() {
        let mut push = PushReport::default();
        assert_eq!(push.tick(true, true), Some(1));
        let quiet: Vec<_> = (1..=60).filter_map(|t| push.tick(t == 10 || t == 30, true)).collect();
        assert_eq!(quiet, Vec::<i64>::new(), "the cool-down holds the next report");
        assert_eq!(push.tick(false, true), Some(2));
        assert_eq!(push.tick(true, false), None, "no report while the skater isn't pushing");
    }

    /// research 5.2 #15: the land call arms a 2-tick countdown; the air's trick posts at touchdown, aircompleted 2 ticks
    /// later (3 when the land call trails the trick update; observe runs after physics, so 2).
    #[test]
    fn air_completed_posts_2_to_3_ticks_after_touchdown() {
        let mut o = Observer { air: Some(Air::default()), ..default() };
        assert_eq!(land_names(&mut o, false, false, false), ["trickcomplete1"]);
        assert_eq!(land_names(&mut o, false, false, false), Vec::<&str>::new());
        assert_eq!(land_names(&mut o, false, false, false), ["aircompleted"]);
    }

    /// research 5.2 #15: a trick starting (manual, grind, new air) flushes a pending landing at once (slot 13 -> 0x8279e6a0).
    #[test]
    fn a_manual_right_after_landing_flushes_air_completed() {
        let mut o = Observer { air: Some(Air::default()), ..default() };
        assert_eq!(land_names(&mut o, false, false, false), ["trickcomplete1"]);
        assert_eq!(land_names(&mut o, false, false, true), ["aircompleted"]);
        assert!(o.landing.is_none());
    }

    /// research 5.2 #19: in a tick where both fire, AirCompleted is queued before the trick loop (0x8265154c).
    #[test]
    fn a_same_tick_flush_posts_air_completed_first() {
        let mut o = Observer { air: Some(Air::default()), ..default() };
        assert_eq!(land_names(&mut o, false, false, true), ["aircompleted", "trickcomplete1"]);
    }

    /// research 5.2 #15: a bail finalises the air at once, wiped (0x827a38c8 -> 0x8279e640); only the land call appends
    /// a trick entry, so no trickcomplete1 (INFERRED).
    #[test]
    fn a_bail_finalises_the_air_at_once() {
        let mut o = Observer { air: Some(Air::default()), ..default() };
        let mut out = Vec::new();
        land(&mut o, false, true, false, &mut out);
        assert_eq!(out.iter().map(|e| e.name).collect::<Vec<_>>(), ["aircompleted"]);
        assert_eq!(out[0].message["Wipedout"], 1);
        assert!(o.air.is_none() && o.landing.is_none());
    }

    /// research7 1: TrickReversed = the profile stance is goofy, sampled when the trick starts.
    #[test]
    fn trick_reversed_is_the_goofy_stance() {
        for (goofy, want) in [(false, 0), (true, 1)] {
            let mut o = Observer { air: Some(Air { reversed: goofy, ..default() }), ..default() };
            let mut out = Vec::new();
            land(&mut o, false, false, false, &mut out);
            assert_eq!(out[0].message["TrickReversed"], want);
        }
    }

    /// A tutorial.json with only speech: 906 (25 %), 5100 (Grinds' career exit line), 1943 / 8195 null as P1 exports them.
    fn speech_data() -> Tutorial {
        serde_json::from_value(json!({
            "chain": [], "intros": [], "lessons": {}, "places": {"locators": {}, "volumes": {}}, "movies": {},
            "speech_table": {
                "31:906:0": {"files": ["C906_45_PushNeg"], "chance": 25.0},
                "31:5100:0": {"files": ["45_5100_Cam"], "chance": 100.0},
                "31:1943:0": null,
                "31:8195:0": null,
            },
            "speech_files": {
                "C906_45_PushNeg": [{"audio": "speech/C906_45_PushNeg_0.wav", "seconds": 1.7}],
                "45_5100_Cam": [{"audio": "speech/45_5100_Cam_0.wav", "seconds": 7.27}],
            },
        }))
        .unwrap()
    }

    /// research 4.6 (RETAIL): every line rolls its vault play chance (tMCSpeechTuning +24, 0x822c6480).
    #[test]
    fn a_line_plays_with_its_chance() {
        let data = speech_data();
        assert!(data.cameraman_line(906, 0.2).is_some());
        assert!(data.cameraman_line(906, 0.3).is_none());
    }

    /// research 4.6 (RETAIL): 1943 isn't in the cameraman table and 8195 (>= 5000) isn't in the NIS line table.
    #[test]
    fn silent_lines_play_nothing() {
        let data = speech_data();
        assert!(data.cameraman_line(1943, 0.).is_none());
        assert!(data.cameraman_line(8195, 0.).is_none());
    }

    /// research 4.4 (RETAIL): passing Grinds in the career plays 5100 (`45_5100_Cam`) as the lesson closes.
    #[test]
    fn grinds_career_exit_line_plays() {
        let data = speech_data();
        let takes = data.cameraman_line(5100, 1.).unwrap();
        assert_eq!(takes[0].audio, "speech/45_5100_Cam_0.wav");
    }

    /// P4 interface: skate1_cast stands the people of the available challenges and runs the running one's cleanup
    /// (research 4.1); off the map (no career) it releases the cast.
    #[test]
    fn people_see_the_career() {
        let mut state = running("people", "he_u1_film01", false);
        state.progress.won.insert("fp_u2_tutorial05".into());
        let mut people = crate::skate1_cast::People::default();
        state.show_people(&mut people);
        assert!(people.on && people.available.contains("he_u1_film01"));
        assert_eq!(people.running.as_deref(), Some("he_u1_film01"));
        state.career = None;
        state.show_people(&mut people);
        assert!(!people.on && people.available.is_empty());
    }

    /// P3: a lesson's EnterExtendedNIS (kind 14, TUTORIAL_INTRO) plays its scene to its end or an A skip, then
    /// answers niscomplete.
    #[test]
    fn a_lesson_nis_plays_its_scene_then_completes() {
        let mut state = running("nis", "fp_u2_tutorial01", false);
        state.queue_nis("NIS_TUT_Basics".into());
        assert_eq!(state.queue.iter().cloned().collect::<Vec<_>>(), [Item::Scene("NIS_TUT_Basics".into()), Item::NisComplete]);
    }

    #[test]
    fn a_press_the_menu_took_is_swallowed_until_released() {
        let mut holding = PAD_A | PAD_Y;
        assert!(hold(&mut holding, PAD_A), "A (a scene's skip) still held: gameplay must not see it");
        assert!(!hold(&mut holding, 0) && holding == 0, "released: done");
        assert!(!hold(&mut holding, PAD_A), "a new press after the release is gameplay's");
    }

    #[test]
    fn the_film_menu_closes_itself_after_5_seconds() {
        let films = ["he_u3_film01".to_string()];
        let mut menu = None;
        film_menu(&mut menu, &films, Pad { rb: true, ..default() }, 0.);
        film_menu(&mut menu, &films, Pad::default(), 4.9);
        assert!(menu.is_some());
        film_menu(&mut menu, &films, Pad { rb: true, ..default() }, 0.);
        film_menu(&mut menu, &films, Pad::default(), 4.9);
        assert!(menu.is_some(), "RB reset the timer");
        film_menu(&mut menu, &films, Pad::default(), 0.2);
        assert!(menu.is_none());
    }

    const INTROS: [&str; 2] = ["Game_Intro_1", "Game_Intro_3"];
    const START: &str = "suburbs_fp_u2_tutorial_01_challengelocator_01";

    /// research 1.2 steps 2-7: Welcome loader, intro 1, the medical form, Quick loader, CaC (the S3R customiser), Quick
    /// loader, intro 3, then the Tips loader over the move to tutorial01's Location.
    #[test]
    fn a_new_career_queues_retail_frontend() {
        let intros = INTROS.map(String::from);
        assert_eq!(
            frontend(&Progress::default(), &intros, START),
            [
                Item::Screen(Fe::LoadingWelcome),
                Item::Movie("Game_Intro_1".into()),
                Item::Screen(Fe::CaCDetails),
                Item::Screen(Fe::LoadingQuick),
                Item::Customiser,
                Item::Screen(Fe::LoadingQuick),
                Item::Movie("Game_Intro_3".into()),
                Item::Teleport(START.into()),
                Item::Screen(Fe::LoadingTips),
                Item::FrontendDone,
            ]
        );
    }

    #[test]
    fn a_finished_frontend_goes_straight_to_the_chain() {
        let progress = Progress { frontend_done: true, ..default() };
        assert!(frontend(&progress, &INTROS.map(String::from), START).is_empty());
    }

    /// A hand-edited save with lessons won but no `frontend_done` doesn't replay the intros over them.
    #[test]
    fn a_save_with_wins_skips_the_frontend() {
        let progress: Progress = serde_json::from_str(r#"{"won": ["fp_u2_tutorial01"]}"#).unwrap();
        assert!(frontend(&progress, &INTROS.map(String::from), START).is_empty());
    }

    /// Bug 10-17 #23: Learn > Skate 1 Tutorial plays or resumes; it never interrupts and never erases the career.
    #[test]
    fn the_tutorial_item_plays_or_resumes() {
        let data: Tutorial = serde_json::from_value(json!({
            "chain": ["fp_u2_tutorial01", "fp_u2_tutorial02"], "intros": INTROS, "movies": {}, "lessons": {},
            "places": {"locators": {START: {"matrix": [[1, 0, 0, 0], [0, 1, 0, 0], [0, 0, 1, 0], [0, 0, 0, 1]]}}, "volumes": {}},
        }))
        .unwrap();
        let mut state = running("resume", "x", false);
        state.running = None;
        // (b) a new career: the frontend.
        assert_eq!(state.resume_plan(&data), Resume::Frontend);
        // (a) the frontend (or a scene, or a challenge) already going: nothing.
        state.queue.push_back(Item::FrontendDone);
        assert_eq!(state.resume_plan(&data), Resume::Busy);
        state.queue.clear();
        // (c) the frontend done, lessons left: the next lesson.
        state.progress.frontend_done = true;
        assert_eq!(state.resume_plan(&data), Resume::AutoStart);
        assert_eq!(state.career.as_ref().unwrap().auto_start(&state.progress), Some("fp_u2_tutorial01"));
        // (d) every type-4 lesson won: the first again, with the wins kept.
        let lessons: Vec<String> = state.career.as_ref().unwrap().kinds.iter().filter(|(_, k)| **k == 4).map(|(n, _)| n.clone()).collect();
        state.progress.won.extend(lessons);
        let won = state.progress.won.len();
        assert_eq!(state.resume_plan(&data), Resume::Replay("fp_u2_tutorial01".into()));
        assert_eq!(state.progress.won.len(), won);
        // The flag is taken once; with none set nothing happens.
        state.progress.frontend_done = false;
        state.progress.won.clear();
        state.resume();
        state.take_resume(&data).unwrap();
        assert!(!state.resume && state.queue.front() == Some(&Item::Screen(Fe::LoadingWelcome)), "{:?}", state.queue);
        let queued = state.queue.len();
        state.take_resume(&data).unwrap();
        assert_eq!(state.queue.len(), queued);
    }

    /// User 10-15: Start Over erases the saved career and has `drive` reload, so the new-career frontend runs again.
    #[test]
    fn start_over_erases_the_career_and_reloads() {
        let settings = std::env::temp_dir().join(format!("skate1-tutorial-over-{}.json", std::process::id()));
        let mut state = Skate1Challenge::new(PathBuf::new(), settings.clone());
        (state.progress.frontend_done, state.progress.points, state.generation) = (true, 5, Some((3, false)));
        state.progress.won.insert("he_u1_film01".into());
        state.progress.save(&settings);
        state.start_over();
        let saved = Progress::load(&settings);
        let _ = std::fs::remove_file(&settings);
        assert_eq!((saved.frontend_done, saved.won.len(), saved.points, state.generation), (false, 0, 0, None));
        assert_eq!(frontend(&saved, &INTROS.map(String::from), START)[0], Item::Screen(Fe::LoadingWelcome));
    }

    /// Every trick name the lessons compare against is an engine scorable, spelled as Skate 1 returns it.
    #[test]
    fn every_lesson_trick_name_maps_from_an_engine_trick() {
        for name in ["ollie", "kickflip", "heelflip", "popshuvit", "fspopshuvit"] {
            let engine = skate_core::scoring::catalog::IDENTIFIERS.iter().find(|(n, ..)| skate1_name(n) == name);
            assert!(engine.is_some(), "{name}");
        }
    }

    #[test]
    fn tweaked_grabs_are_the_directional_variants() {
        assert_eq!(scorable_class("bsgrab_up"), Some(GRAB_CLASS));
        assert!(tweaked("bsgrab_up") && !tweaked("bsgrab"));
    }

    /// old_challenges.py's output (SKATE1_CHALLENGES_ROOT = private/challenges-skate1): every lesson loads with the
    /// Skate 1 natives and its initial step reaches the sign-up poster.
    #[test]
    #[ignore = "requires the prepared Skate 1 tutorial"]
    fn every_lesson_starts_and_shows_its_poster() {
        let root = PathBuf::from(std::env::var("SKATE1_CHALLENGES_ROOT").unwrap());
        let (data, _, _) = load(&root).unwrap();
        for name in &data.chain {
            let lesson = &data.lessons[name];
            let source = std::fs::read_to_string(root.join(format!("scripts/{name}.lua"))).unwrap();
            let bridge = Shared::default();
            let script = Script {
                name,
                source: &source,
                steps: lesson.steps.clone(),
                initial: "initial",
                enter: "stateenter",
                exit: "stateexit",
            };
            let mut m = Machine::new(script, &lesson.fields, |lua| natives(lua, &bridge)).unwrap();
            m.start().unwrap();
            m.send("hudloaded", &json!({"PlayerID": 0})).unwrap();
            let delay = bridge.lock().unwrap().timers[0].id;
            m.send("timerexpired", &json!({"PlayerID": 0, "TimerID": delay})).unwrap();
            let out = std::mem::take(&mut bridge.lock().unwrap().out);
            assert!(out.iter().any(|c| matches!(c, Call::Hud(_, 1))), "{name}: no sign-up poster");
            m.send("hudyes", &json!({"PlayerID": 0})).unwrap();
            m.send("niscomplete", &json!({"PlayerID": 0})).unwrap();
            assert_ne!(m.state(), "initial", "{name}");
        }
    }

    /// Every `Table.Name(` call and `Table.eEnum.Value` a mission or pro tutorial script names resolves in its Machine
    /// (the lead's natives, then skate1_missions'): an unbound one only reaches `__missing` when the handler runs.
    #[test]
    #[ignore = "requires the prepared Skate 1 tutorial"]
    fn every_mission_native_and_enum_is_bound() {
        let root = PathBuf::from(std::env::var("SKATE1_CHALLENGES_ROOT").unwrap());
        let (data, _, _) = load(&root).unwrap();
        let mut unbound = Vec::new();
        for (name, mission) in data.missions.iter().chain(&data.pro_tutorials) {
            let source = std::fs::read_to_string(root.join(format!("scripts/{name}.lua"))).unwrap();
            let fields = crate::skate1_missions::fields(&mission["self"]);
            let (bridge, missions) = (Shared::default(), Arc::new(Mutex::new(Missions::new(&fields))));
            let script = Script {
                name,
                source: &source,
                steps: crate::skate1_missions::steps(mission),
                initial: "initial",
                enter: "stateenter",
                exit: "stateexit",
            };
            let m = Machine::new(script, &fields, |lua| {
                natives(lua, &bridge)?;
                crate::skate1_missions::register(lua, &missions)
            })
            .unwrap();
            let check: mlua::Function = m.lua.load(r#"return function(src)
                local out, seen = {}, {}
                local function at(t, k) return type(t) == "table" and rawget(t, k) or nil end
                for t, k in src:gmatch("([%u][%w_]*)%.([%a_][%w_]*)%s*%(") do
                  local key = t .. "." .. k
                  if not seen[key] and type(at(_G[t], k)) ~= "function" then out[#out + 1] = key end
                  seen[key] = true
                end
                for t, e, k in src:gmatch("([%u][%w_]*)%.(e[%u][%w_]*)%.([%a_][%w_]*)") do
                  local key = t .. "." .. e .. "." .. k
                  if not seen[key] and at(at(_G[t], e), k) == nil then out[#out + 1] = key end
                  seen[key] = true
                end
                return out
              end"#).eval().unwrap();
            let names: Vec<String> = check.call(source.as_str()).unwrap();
            // RETAIL nil too: eSk8HUDFollowMeState (827b2f40) has no Accept / Decline; he_u2_follow01 passes nil.
            let retail_nil = ["HUD.eSk8HUDFollowMeState.Accept", "HUD.eSk8HUDFollowMeState.Decline"];
            unbound.extend(names.into_iter().filter(|n| !retail_nil.contains(&n.as_str())).map(|n| format!("{name}: {n}")));
        }
        assert!(unbound.is_empty(), "{}", unbound.join("
"));
    }

    // ---------------------------------------------------------------- P5 / P6 lead glue (s1-missions, 10-14)
    use crate::{
        skate1_cast::People,
        skate1_missions::{Air as MissionAir, Facts as MissionFacts, Missions},
        skate1_pros::ProGates,
        skate1_rider::{AiEvent, Skate1Ai},
        skate1_screens::Skate1Screens,
    };

    /// RETAIL 0x827b66d8: the 25 names in order; the pros' retry dialog is 23, the end screen 11.
    #[test]
    fn tutorial_states_are_retail() {
        for (i, n) in [(0, "Load"), (1, "SignUpPoster"), (3, "LeftArea"), (4, "EnterArea"), (11, "Congratulations"),
                       (14, "FailedObjectives"), (19, "ShowDistanceMeter"), (23, "ShowRetryDialog"), (24, "MessageGatePassed")] {
            assert_eq!(TUTORIAL_STATES[i], n);
        }
    }

    /// RETAIL 0x823de710 states 2-3: an AssetLoadType 0 challenge (S.H.I.N.G.O.) keeps its people through the offer
    /// NIS and runs its cleanup at ChallengeAccepted.
    #[test]
    fn asset_load_type_0_cleans_up_at_challenge_accepted() {
        let mut state = running("accept", "he_u1_skate01", false);
        state.cleanup_at_accept = true;
        let mut people = crate::skate1_cast::People::default();
        state.show_people(&mut people);
        assert_eq!(people.running, None, "Shingo stands through the offer NIS");
        let lua = Lua::new();
        natives(&lua, &state.bridge).unwrap();
        lua.load("ChallengeAsset.ChallengeAccepted({})").exec().unwrap();
        state.show_people(&mut people);
        assert_eq!(people.running.as_deref(), Some("he_u1_skate01"));
    }

    /// A tutorial.json with one lesson, one mission (he_x: a follow, ChallengeType 23, AssetLoadType 0) and P-Rod's
    /// pro tutorial.
    fn mission_data() -> Tutorial {
        serde_json::from_value(json!({
            "chain": ["fp_u2_tutorial01"], "intros": [], "movies": {},
            "lessons": {"fp_u2_tutorial01": {"self": {"Title": "ID_LESSON_TITLE"}, "steps": {}}},
            "places": {
                "locators": {"prod_loc": {"matrix": [[1, 0, 0, 0], [0, 1, 0, 0], [0, 0, 1, 0], [10, 0, 20, 1]]},
                             "ai_loc": {"matrix": [[1, 0, 0, 0], [0, 1, 0, 0], [0, 0, 1, 0], [5, 0, 5, 1]]}},
                "volumes": {"bound": {"min": [0, 0, 0], "max": [1, 1, 1]}, "gate_1": {"min": [2, 0, 2], "max": [4, 2, 4]}},
            },
            "missions": {"he_x": {
                "self": {"GlobalType": 3, "ChallengeType": 23, "AssetLoadType": 0, "PointRequirement": 500,
                         "FollowTheLeaderDestinationBoundary": "bound", "Name": "he_x"},
                "steps": {"initial": {"stateenter": "h_enter"}},
            }},
            "pro_tutorials": {"fp_u3_tutorial02": {"self": {
                "GlobalType": 3, "ChallengeType": 0, "AssetLoadType": 1, "Location": "prod_loc", "SignupRadius": 3.0,
                "Title": "ID_PRO_TUTORIAL_PROD_TITLE", "OTSSpotVolumes": ["gate_1"],
            }}},
        }))
        .unwrap()
    }

    /// Preview 4 (10-15): walking into a pro's box offered him with no prompt. The offer reaches the action overlay and
    /// its card carries the offered challenge's type icon (0x8243bb68).
    #[test]
    fn an_offer_shows_the_sign_up_prompt_with_its_icon() {
        let mut state = running("offer", "x", false);
        state.running = None;
        state.data = Some(Arc::new(mission_data()));
        state.signups.offer = Some("fp_u3_tutorial02".into());
        let mut screens = Skate1Screens::default();
        let mut card = None;
        state.screens_frame(&mut screens, |_, f| {
            card = Some(f.card.clone());
            Vec::new()
        });
        let card = card.unwrap();
        assert_eq!((card.key.as_str(), card.challenge_type, card.icon.as_str()), ("fp_u3_tutorial02", Some(0), "nopath_tutorial"));
        assert!(screens.sign_up_prompt_shown());
        state.signups.offer = None;
        state.screens_frame(&mut screens, |_, _| Vec::new());
        assert!(!screens.sign_up_prompt_shown());
    }

    /// L-P5: the card text comes from the running challenge, so a pro's poster shows the pro's title.
    #[test]
    fn card_text_reads_the_running_challenge() {
        let mut state = running("card", "fp_u3_tutorial02", false);
        state.data = Some(Arc::new(mission_data()));
        state.strings.insert("ID_PRO_TUTORIAL_PROD_TITLE".into(), "Paul Rodriguez".into());
        state.strings.insert("ID_LESSON_TITLE".into(), "Lesson".into());
        assert_eq!(state.lesson_text("Title"), "Paul Rodriguez");
        state.running = Some("fp_u2_tutorial01".into());
        assert_eq!(state.lesson_text("Title"), "Lesson");
    }

    /// P5 L12: a mission watches every `self` string or string array that names a volume.
    #[test]
    fn a_mission_watches_every_self_value_that_names_a_volume() {
        let data = mission_data();
        let fields = json!({"PhotoSpotVolumes": ["gate_1", "nope"], "ScoringBoundary": "bound", "Other": 3, "Name": "x"});
        let mut names: Vec<String> = named_volumes(&fields, &data.places.volumes).into_iter().map(|(n, _)| n).collect();
        names.sort();
        assert_eq!(names, ["bound", "gate_1"]);
    }

    /// P5 L1: a mission runs on its own Machine with the lead's natives plus skate1_missions', kActivityID set before
    /// the initial step and its volumes watched; AssetLoadType 0 waits for ChallengeAccepted (L-P3).
    #[test]
    fn a_mission_starts_with_the_mission_natives() {
        let root = std::env::temp_dir().join(format!("skate1-mission-{}", std::process::id()));
        std::fs::create_dir_all(root.join("scripts")).unwrap();
        std::fs::write(
            root.join("scripts/he_x.lua"),
            "function h_enter(self, message, vars) vars.id = vars.kActivityID vars.follow = FollowTheLeader ~= nil end",
        )
        .unwrap();
        let mut state = running("mission", "he_x", false);
        state.running = None;
        state.root = root;
        state.data = Some(Arc::new(mission_data()));
        state.start_challenge("he_x").unwrap();
        let m = state.machine.as_ref().expect("the mission runs");
        assert_eq!(m.vars().get::<i64>("id").unwrap(), 0);
        assert!(m.vars().get::<bool>("follow").unwrap());
        assert!(state.missions.is_some() && state.cleanup_at_accept);
        assert_eq!(state.running.as_deref(), Some("he_x"));
        assert_eq!(state.volumes.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["bound"]);
    }

    /// P5 L8 (+ P6 L-P4): the requests that touch only the tutorial, the AI, the gates, the cast and the screens are
    /// carried out here; the skater's teleport, speech, scenes and the replay are drive's.
    #[test]
    fn mission_requests_reach_their_owners() {
        use crate::skate1_missions::{Nis, Request};
        let mut state = running("requests", "fp_u3_tutorial02", true);
        let data = mission_data();
        state.volumes = named_volumes(&data.pro_tutorials["fp_u3_tutorial02"]["self"], &data.places.volumes);
        state.observer.inside.insert("gate_1".into());
        let (mut ai, mut gates, mut people, mut screens) =
            (Skate1Ai::default(), ProGates::default(), People::default(), Skate1Screens::default());
        let requests = vec![
            Request::Indicator { volume: "gate_1".into(), kind: 10 },
            Request::ResetVolume("gate_1".into()),
            Request::Lose,
            Request::Photographing(true),
            Request::Skate101(15),
            Request::Nis(Nis::Offer("NIS_A".into())),
            Request::Nis(Nis::Accept("NIS_C".into(), true)),
            Request::Nis(Nis::Replay { scene: "NIS_B".into(), from: 60, to: 180 }),
            Request::DisableWipeoutReset,
            Request::PlayerTo([[0.; 4]; 4]),
        ];
        let rest: Vec<Request> = requests
            .into_iter()
            .filter_map(|r| state.take_request(r, &data, &mut ai, &mut gates, &mut people, Some(&mut screens)))
            .collect();
        assert_eq!(rest, [Request::DisableWipeoutReset, Request::PlayerTo([[0.; 4]; 4])], "the skater's are carry_out's");
        assert!(!state.observer.inside.contains("gate_1"), "ResetVolume forgets the inside state");
        assert!(!state.bridge.lock().unwrap().won, "LoseChallenge");
        assert!(people.photographing);
        assert_eq!(state.queue.iter().cloned().collect::<Vec<_>>(), [
            Item::Screen(Fe::Skate101("15".into())),
            Item::MissionScene("NIS_A".into(), None),
            Item::MissionScene("NIS_C".into(), None),
            Item::Screen(Fe::LoadingQuick),
            Item::MissionScene("NIS_B".into(), Some((60, 180))),
        ]);
    }

    /// P5 L7: an AI rider's landing and bail reach the mission as that rider's aircompleted / wipeout, and the landing
    /// is the rider's completed air (Facts.airs).
    #[test]
    fn ai_landings_post_their_riders_events() {
        let mut airs = BTreeMap::new();
        let events = ai_events(
            vec![(2, AiEvent::Landed { trick: 7, spins: 2, switched: false, bail: true }), (3, AiEvent::Wipeout)],
            &mut airs,
        );
        let named: Vec<(&str, Value)> = events.into_iter().map(|e| (e.name, e.message)).collect();
        assert_eq!(named, [
            ("aircompleted", json!({"PlayerID": 2, "FirstScorableId": 7, "Wipedout": 1})),
            ("wipeout", json!({"PlayerID": 3})),
        ]);
        assert_eq!(airs[&2], MissionAir { scorable: 7, spins: 2, switched: false, horizontal: 0. });
    }

    /// P5 L7: the trigger volumes see the AI riders too (the player's crossings stay observe's).
    #[test]
    fn ai_riders_cross_volumes_too() {
        let mut state = running("ai-volumes", "he_x", false);
        state.volumes = named_volumes(&json!({"a": "gate_1"}), &mission_data().places.volumes);
        let at = |x: f32| BTreeMap::from([(0, [3., 1., 3.]), (2, [x, 1., 3.])]);
        let names = |e: Vec<Event>| e.into_iter().map(|e| (e.name, e.message)).collect::<Vec<_>>();
        let enter = json!({"PlayerID": 2, "VolumeID": "gate_1"});
        assert_eq!(names(state.ai_triggers(&at(3.))), [("triggerentercollision", enter.clone())]);
        assert!(state.ai_triggers(&at(3.)).is_empty());
        assert_eq!(names(state.ai_triggers(&at(9.))), [("triggerexitcollision", enter)]);
    }

    /// P5 L7: the screens' answers as the scripts hear them.
    /// User 10-15 run18 (he_u2_photo01, "you cannot press continue"): the photo replay (CHALLENGE_WIN) holds in the
    /// photo view under the end options (FE 23), and drive runs before the screens: the scene took the frame's
    /// presses, so the menu never heard A. A press the scene doesn't skip on stays for the screens.
    #[test]
    fn a_held_photo_scene_leaves_the_press_to_the_end_options() {
        let (mut cutscene, root) = (crate::challenge::cutscene::Cutscene::default(), std::env::temp_dir().join("s1-no-scenes"));
        let mut play = ScenePlay::start(&mut cutscene, &root, "he_u2_photo01");
        let mut pressed = Pressed { accept: true, down: true, ..Pressed::default() };
        scene_press(&mut pressed, &mut play, &cutscene);
        assert!(fe_pads(pressed) == [crate::skate1_screens::Pad::A, crate::skate1_screens::Pad::Down]);
    }

    #[test]
    fn screen_answers_map_to_script_events() {
        use crate::skate1_screens::Event as S;
        let names: Vec<_> = [S::Loaded, S::Complete, S::Yes, S::No, S::Continue, S::Retry, S::Indicator(true), S::StartFilm(0)]
            .iter()
            .map(|e| screen_event(e).map(|e| e.name))
            .collect();
        assert_eq!(names, [Some("hudloaded"), Some("hudcomplete"), Some("hudyes"), Some("hudno"), Some("hudcontinue"),
                           Some("hudretry"), None, None]);
    }

    /// overnight-1015 review I1: off the Skate 1 map (or online) the data is gone, so last map's boxes must not place
    /// sign-up markers.
    #[test]
    fn no_signup_places_without_the_tutorial_data() {
        let mut state = Skate1Challenge::new(PathBuf::new(), PathBuf::new());
        state.signups.boxes = vec![("fp_u3_tutorial02".into(), Aabb::signup(Vec3::new(10., 0., 20.), 3.))];
        assert!(state.signup_places().next().is_none());
    }

    /// P5 L11, P6 L-P6 / L-P7: the screens' facts from the running mission, the sign-up boxes and the follow distance.
    #[test]
    fn screen_facts_carry_the_card_the_icons_and_the_distance() {
        let mut state = running("facts", "he_x", false);
        state.data = Some(Arc::new(mission_data()));
        state.signups.boxes = vec![("fp_u3_tutorial02".into(), Aabb::signup(Vec3::new(10., 0., 20.), 3.))];
        let fields = crate::skate1_missions::fields(&mission_data().missions["he_x"]["self"]);
        let missions: crate::skate1_missions::Shared = Arc::new(Mutex::new(Missions::new(&fields)));
        let lua = Lua::new();
        natives(&lua, &state.bridge).unwrap();
        crate::skate1_missions::register(&lua, &missions).unwrap();
        lua.load("FollowTheLeader.SetWarningInfo({25, 6, 31, 0, 1, 24, 0, 1, 12, Count = 9})
                  FollowTheLeader.SetActorIDs(1) FollowTheLeader.StartDistanceUpdates()").exec().unwrap();
        missions.lock().unwrap().tick(&MissionFacts { positions: [(0, [0.; 3]), (1, [3., 4., 0.])].into(), ..default() });
        state.missions = Some(missions);
        let facts = state.screen_facts();
        assert_eq!(facts.challenges, Some(vec![("fp_u3_tutorial02".to_string(), [10., 20.])]));
        assert_eq!(facts.distance, Some((5., 6)));
        assert_eq!((facts.card.key.as_str(), facts.card.points), ("he_x", 500));
    }

    /// Run12: no names by the S.K.A.T.E. letters. SkateMission.GetPlayerName answers each player's name (RETAIL
    /// 0x82319110), in skate_view's SetPlayerOrder order: the leader first.
    #[test]
    fn the_skate_hud_names_the_player_and_the_ai() {
        let mut state = running("names", "he_x", false);
        state.data = Some(Arc::new(mission_data()));
        let fields = crate::skate1_missions::fields(&mission_data().missions["he_x"]["self"]);
        let missions: crate::skate1_missions::Shared = Arc::new(Mutex::new(Missions::new(&fields)));
        let lua = Lua::new();
        natives(&lua, &state.bridge).unwrap();
        crate::skate1_missions::register(&lua, &missions).unwrap();
        lua.load("Skate.SetPlayerOrder(1, 1) Skate.SetPlayerOrder(0, 2)").exec().unwrap();
        state.missions = Some(missions);
        state.names = [(0, "Me".to_owned()), (1, "SHINGO".to_owned())].into();
        assert_eq!(state.screen_facts().skate.view.names, ["SHINGO", "Me"]);
    }

    /// B6 (Rolling Rehab named no tricks): film Countdown's rows exist when the same frame's overlay SetState(1)
    /// UpdateObjectives reads them. RETAIL: the HUD state handler builds them (0x82368d30 case 8) before the overlay's
    /// SetState; the port rebuilt them on the next mission tick, after the screens had shown zero rows.
    #[test]
    fn a_film_countdown_hands_its_rows_to_the_same_frame() {
        let mut state = running("rehab", "he_x", false);
        state.data = Some(Arc::new(mission_data()));
        let fields = crate::skate1_missions::fields(&json!({"ChallengeType": 7, "RequiredTrickType": ["00000002", "00000004"],
            "Hash_769F1AAE200A496C": [1, 1], "Hash_3774A91CE72FE9BB": true, "Name": "he_x"}));
        let missions: crate::skate1_missions::Shared = Arc::new(Mutex::new(Missions::new(&fields)));
        let lua = Lua::new();
        natives(&lua, &state.bridge).unwrap();
        crate::skate1_missions::register(&lua, &missions).unwrap();
        state.missions = Some(missions);
        lua.load("HUD.ChangeHUDState(HUD.eSk8HUDState.MissionFilmChallenge, HUD.eSk8HUDFilmState.Countdown)").exec().unwrap();
        let calls = std::mem::take(&mut state.bridge.lock().unwrap().out);
        for call in calls {
            if let Call::Hud(s, sub) = call {
                state.hud_call(s, sub);
            }
        }
        let mut screens = Skate1Screens::default();
        let mut rows = 0;
        state.screens_frame(&mut screens, |_, facts| {
            rows = facts.card.objectives.len();
            Vec::new()
        });
        assert_eq!(rows, 3, "Flip, Grab, FilmTime");
    }

    /// P2/P3 retail unlock rules for the pros (progress.json `after`, eGlobalType 3): P-Rod reaches skate1_cast's
    /// available set once Filming 101 is won, and his sign-up box stands on his marker (Location +- SignupRadius).
    /// skate1_cast's own test stands him there (R1).
    #[test]
    fn an_unlocked_pro_reaches_the_cast_and_signs_up_at_his_marker() {
        let mut state = running("pro", "he_u1_film01", false);
        state.running = None;
        state.data = Some(Arc::new(mission_data()));
        state.progress.won.insert("fp_u2_tutorial05".into());
        let mut people = People::default();
        state.show_people(&mut people);
        assert!(!people.available.contains("fp_u3_tutorial02"), "locked until Filming 101 is won");
        state.progress.won.insert("he_u1_film01".into());
        state.show_people(&mut people);
        assert!(people.available.contains("fp_u3_tutorial02"));
        state.rebuild_signups();
        assert_eq!(state.signups.boxes, [("fp_u3_tutorial02".to_string(), Aabb::signup(Vec3::new(10., 0., 20.), 3.))]);
    }

    /// P5 screens wiring: a mission's HUD state reaches Skate1Screens' manager, and the overlay's hudloaded comes back
    /// to the script the frame after it opens. Needs s1-screens' #[cfg(test)] Skate1Screens::step_without_assets.
    #[test]
    fn a_mission_hud_state_reaches_the_screens_and_hudloaded_comes_back() {
        let mut state = running("screens", "he_x", false);
        let mut screens = Skate1Screens::default();
        state.hud_call(i64::from(crate::skate1_screens::FOLLOW), 0);
        assert_eq!(state.screen_states, [ScreenCall::State(crate::skate1_screens::FOLLOW, 0)]);
        let step = |s: &mut Skate1Screens, f: &crate::skate1_screens::Facts| s.step_without_assets(f);
        assert!(!state.screens_frame(&mut screens, step), "running: no clear");
        assert!(state.next_frame.is_empty(), "the overlay opens this frame");
        state.screens_frame(&mut screens, step);
        let names: Vec<&str> = state.next_frame.iter().map(|e| e.name).collect();
        assert_eq!(names, ["hudloaded"]);
        state.running = None;
        assert!(state.screens_frame(&mut screens, step), "the challenge ended: the HUD clears");
        assert!(!state.screens_frame(&mut screens, step), "once");
    }

    fn step(s: &mut Skate1Screens, f: &crate::skate1_screens::Facts) -> Vec<crate::skate1_screens::Event> {
        s.step_without_assets(f)
    }

    /// Preview 3 bug 2: the tutorial window (MissionTutorialChallenge, 19) was drawn by a native placeholder and never
    /// reached Skate1Screens, so hud_tutorial_challenge_overlay never loaded. Its Load now opens the overlay, and
    /// hudloaded comes once, from the screens.
    #[test]
    fn the_tutorial_window_goes_to_the_screens() {
        let mut state = running("tutorial-window", "fp_u2_tutorial01", false);
        let mut screens = Skate1Screens::default();
        state.hud_call(19, 0);
        assert_eq!(state.screen_states, [ScreenCall::State(19, 0)]);
        assert!(state.next_frame.is_empty(), "no native hudloaded");
        state.screens_frame(&mut screens, step);
        state.screens_frame(&mut screens, step);
        let names: Vec<&str> = state.next_frame.iter().map(|e| e.name).collect();
        assert_eq!(names, ["hudloaded"]);
    }

    /// Preview 3 bug 3 (RETAIL 0x827b9b90): ShowController stores the retail enums (0x827b7050 / 0x827b7558) and queues
    /// (19, ShowController) and (19, UpdateController); the screens read the sequence as the controller movie's label.
    #[test]
    fn show_controller_queues_the_controller_states_with_the_retail_sequence() {
        let (lua, bridge) = (Lua::new(), Shared::default());
        natives(&lua, &bridge).unwrap();
        lua.load("HUD.ShowController(HUD.eSk8ControllerSequence.RightAnalogGesture, HUD.eSk8ControllerGesture.Ollie)")
            .exec()
            .unwrap();
        let calls = std::mem::take(&mut bridge.lock().unwrap().out);
        let &[Call::Controller(sequence, gesture)] = &calls[..] else { panic!("one Controller call") };
        assert_eq!((sequence, gesture), (8, 18), "RightAnalogGesture, Ollie");
        let mut state = running("controller", "fp_u2_tutorial01", false);
        state.show_controller(sequence, gesture);
        assert_eq!(state.screen_states, [ScreenCall::State(19, 5), ScreenCall::State(19, 7)]);
        // Tutorial.GetControllerSequenceString: 8 is the controller movie's rightanalogGesture (skate1_screens parts.rs).
        assert_eq!(state.screen_facts().controller, 8);
        assert_eq!(state.screen_facts().gesture, 18, "Ollie: the gesture reaches GetControllerGestureString");
    }

    /// Preview 3 bug 2 (RETAIL 0x82340048 / 0x827b9c18): the lesson's objective rows reach the objective HUD, and the
    /// mark's hudcomplete comes from the manager's (19, 18) timer, once, 63 ticks after it applies.
    #[test]
    fn objective_rows_reach_the_screens_and_the_mark_answers_hudcomplete_once() {
        let mut state = running("objectives", "fp_u2_tutorial01", false);
        state.strings.insert("OBJ_A".into(), "Pop an ollie".into());
        state.strings.insert("OBJ_B".into(), "Do {0:d} grinds".into());
        state.set_objective(1, "OBJ_A", None);
        state.set_objective(2, "OBJ_B", Some(3));
        let titles = |s: &Skate1Challenge| {
            s.screen_facts().card.objectives.iter().map(|o| (o.title.clone(), o.done, o.checkbox)).collect::<Vec<_>>()
        };
        assert_eq!(titles(&state), [("Pop an ollie".into(), false, false), ("Do 3 grinds".into(), false, false)]);
        let mut screens = Skate1Screens::default();
        state.hud_call(19, 0);
        state.screens_frame(&mut screens, step);
        state.screens_frame(&mut screens, step);
        state.next_frame.clear();
        state.objective_done(1);
        assert_eq!(titles(&state)[0], ("Pop an ollie".into(), true, false));
        let mut at = Vec::new();
        for frame in 1..=200 {
            state.screens_frame(&mut screens, step);
            if state.next_frame.drain(..).any(|e| e.name == "hudcomplete") {
                at.push(frame);
            }
        }
        assert_eq!(at, [64], "the applying tick, then 63 more");
        // A later SetTutorialObjective(1) starts the rows again: count = slot.
        state.set_objective(1, "OBJ_A", None);
        assert_eq!(titles(&state), [("Pop an ollie".into(), false, false)]);
    }

    /// Preview 3 bug 4 (RETAIL 0x823df6f0, research 2.0): SetAllChallengeVisualIndicators places an oi000 arrow at each
    /// VisualIndicators locator, as authored (type 4: no lift), not a torus at its position.
    #[test]
    fn visual_indicators_stand_at_their_locators() {
        let data: Tutorial = serde_json::from_value(json!({
            "chain": [], "intros": [], "lessons": {}, "movies": {},
            "missions": {"m": {"self": {"VisualIndicators": ["v1", "missing"]}}},
            "places": {"volumes": {}, "locators": {"v1": {"matrix":
                [[0.0, 0.0, 1.0, 0.0], [0.0, 1.0, 0.0, 0.0], [-1.0, 0.0, 0.0, 0.0], [-515.65, 197.53, -1963.67, 1.0]]}}},
        }))
        .unwrap();
        let places = indicator_places(&data, "m");
        assert_eq!(places.len(), 1, "a row without a locator draws nothing");
        assert!(places[0].translation.abs_diff_eq(Vec3::new(-515.65, 197.53, -1963.67), 1e-3));
        assert_eq!(places[0].rotation, Quat::IDENTITY, "10-15 (RETAIL 0x826b7f10): only the translation goes on");
    }

    /// overnight-1015 C4: S.H.I.N.G.O.'s waiting and spawn locators share one point; retail places each skater on the
    /// locator's child for its slot (0x823d9088), so Shingo never stands inside the player.
    #[test]
    fn repositions_take_the_locators_child_slots() {
        let at = |x: f32, z: f32| [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [x, 195.0, z, 1.0]];
        let data: Tutorial = serde_json::from_value(json!({
            "chain": [], "intros": [], "lessons": {}, "movies": {},
            "places": {"volumes": {}, "locators": {
                "waiting": {"matrix": at(-425.5, -1955.8), "slots": [at(-423.6, -1965.2)]},
                "end": {"matrix": at(-426.3, -1997.4), "slots": []},
                "bam": {"matrix": at(1.0, 2.0)}}},
        }))
        .unwrap();
        let xz = |at: &str, slot: Option<i64>| {
            let m = locator_m4(&data, at, slot).unwrap();
            Vec2::new(m[3][0], m[3][2])
        };
        assert!(xz("waiting", Some(0)).abs_diff_eq(Vec2::new(-423.6, -1965.2), 1e-3), "the player: child 0");
        assert!(xz("waiting", Some(1)).abs_diff_eq(Vec2::new(-423.6, -1970.2), 1e-3), "Shingo: child 0, -5 m z");
        assert!(xz("end", Some(1)).abs_diff_eq(Vec2::new(-426.3, -2002.4), 1e-3), "no child: the locator, -5 m z");
        assert!(xz("bam", Some(3)).abs_diff_eq(Vec2::new(1.0, 2.0), 1e-3), "BAM.loc only: its matrix");
        assert!(xz("waiting", None).abs_diff_eq(Vec2::new(-425.5, -1955.8), 1e-3), "StreamTeleport: the locator");
        assert_eq!(reposition_slot(0, Some(&mlua::Value::Integer(1))), 1);
        assert_eq!(reposition_slot(1, None), 1, "no arg 3: the id's own");
    }
}
