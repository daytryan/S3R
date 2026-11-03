//! The career in the game (plan Task 21): open the save at startup, run the career script and the post-challenge
//! sequence on a career world, write `place` on arrivals, count play time, autosave. Not tested in play.
//!
//! Steps that need a screen this build does not draw yet (the reward summary, milestone and teammate pages, popup 306,
//! the sign-up poster, a state's presentation group) are closed at once with a log line, so the career script keeps
//! its retail order; `Not retail (story mode stage 1 build: screens pending)`.

use std::path::{Path, PathBuf};

use bevy::prelude::*;

use super::core::{CareerState, Core, Ready, Step};
use super::data::CareerData;
use super::mode::{self, GameMode, ModeExit};
use super::progression::Effect;
use super::rules::{Changed, Level, Outcome};
use super::save::{self, Best, Start};
use super::signup;
use super::stage::{self, Built, Holder};
use super::unlocks::{self, Store};
use super::CareerGate;
use crate::map_transition::{MapTransition, Requester, WorldChanged};

/// The glue started a career row (an accepted poster, after the move to its start): the row's mode port
/// (`skate3_tutorial::port_of`) takes it and answers with [`ChallengeResult`].
#[derive(Message, Clone, Debug, PartialEq, Eq)]
pub struct StartRow {
    pub row: String,
    /// The start needed a world load first (spec 12.6: the lesson opens its load screen only then).
    pub loaded_world: bool,
}

/// What a mode port sends when a career row ends (Tasks 28-33).
#[derive(Message, Clone, Debug, PartialEq)]
pub struct ChallengeResult {
    pub row: String,
    pub outcome: Outcome,
    pub score: i64,
}

#[derive(Resource)]
pub struct Career {
    pub state: Option<CareerState>,
    pub core: Core,
    pub store: Store,
    pub built: Built,
    pub holder: Holder,
    pub settings: PathBuf,
    /// Why there is no state this session (a damaged save), if so.
    pub off: Option<String>,
    /// The screen the career waits on, and the answer popups.rs writes back.
    pub shown: Option<Shown>,
    pub answer: Option<usize>,
    /// `career_runs` last frame, to send `ModeExit` once when it turns false.
    running: bool,
}

impl Career {
    /// The career of a launch: the save's start (save.rs `start`) and the unlock store.
    #[must_use]
    pub fn open(data: Option<std::sync::Arc<CareerData>>, settings: &Path) -> Self {
        let mut career = Self {
            state: None,
            core: Core::default(),
            store: Store::default(),
            built: Built::stage_1(),
            holder: Holder::default(),
            settings: settings.to_owned(),
            off: None,
            shown: None,
            answer: None,
            running: false,
        };
        let Some(data) = data else { return career };
        let boot = crate::boot::load_career(settings);
        let had_store = settings.join(unlocks::FILE).exists();
        career.store = Store::load(settings);
        match save::start(&boot, save::load(settings)) {
            Start::New => {
                // The boot's opening runs first; the career roots when it finishes (Task 34).
                let mut state = CareerState::new(data);
                state.new_career = true;
                career.state = Some(state);
                if !had_store {
                    career.store = Store::reset(false);
                }
            }
            Start::Upgrade { tutorial } => {
                // D4 (`Not retail (user 10-20)`): the opening ran before the career existed.
                let mut state = CareerState::new(data.clone());
                begin(&mut state, &mut career.core, tutorial.unwrap_or(false));
                career.state = Some(state);
                if !had_store {
                    // B.6: an existing install keeps everything it sees today.
                    career.store = Store::reset(true);
                }
            }
            Start::Resume(s) => {
                career.state = Some(save::into_state(s, data));
                if !had_store {
                    career.store = Store::reset(true);
                }
            }
            Start::Off(why) => {
                warn!("Skate 3 career off this session: {why}");
                career.off = Some(why);
            }
        }
        career
    }

    /// Writes the career save and the unlock store (the autosave).
    pub fn save(&self) {
        let Some(state) = &self.state else { return };
        if let Err(e) = save::write(&self.settings, &save::from_state(state)) {
            warn!("Skate 3 career save: {e}");
        }
        if let Err(e) = self.store.write(&self.settings) {
            warn!("Skate 3 unlocks save: {e}");
        }
    }
}

/// A new career's start (spec 11.3): `rules.root`, then the first auto sign-up in the world Coach Frank's answer
/// picks (0x827cf960): `tut_sksc_01` in the school, else `ftl_univ_02`.
pub fn begin(state: &mut CareerState, core: &mut Core, tutorial: bool) {
    let d = state.data.clone();
    for fired in state.rules.root(&d) {
        state.runner.queue(fired);
    }
    core.set_auto_signup(if tutorial { "tut_sksc_01" } else { "ftl_univ_02" });
}

/// The launch map: the career's `place` when it has one, else `default`.
#[must_use]
pub fn launch_map(career: &Career, asset_root: &Path, default: Option<&Path>) -> Option<PathBuf> {
    career
        .state
        .as_ref()
        .and_then(|s| s.place.as_ref())
        .filter(|p| !p.map.is_empty())
        .map(|p| asset_root.join(&p.map))
        .or_else(|| default.map(Path::to_owned))
}

/// Keeps a row's best score and level (its first Owned or Killed counts as the level).
pub fn keep_best(state: &mut CareerState, row: &str, outcome: Outcome, score: i64) {
    let level = match outcome {
        Outcome::Killed => Level::Killed as u32,
        Outcome::Owned => Level::Owned as u32,
        Outcome::Abandoned => return,
    };
    #[allow(clippy::cast_precision_loss)]
    let score = score as f64;
    let best = state.bests.entry(row.to_owned()).or_insert(Best { score, level });
    best.score = best.score.max(score);
    if level == Level::Killed as u32 {
        best.level = level;
    }
}

/// What a career screen is answering: a script popup (its reply goes back to the running graph) or a sequence step.
#[derive(Clone, Debug, PartialEq)]
pub enum Asking {
    /// `Notifications101.Show101`: closes only on A, sends `hudcomplete`. skate101.swf shows the description only (its
    /// header is hidden).
    Skate101 { description: String },
    /// `Popup.ShowYesNoPopup`: item 0 No (`hudno`), item 1 Yes (`hudyes`); B does nothing (spec 18.13).
    YesNo,
    Step(Step),
}

/// A screen the career is waiting on, as text and options for the popup stand-in (popups.rs).
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub asking: Asking,
    pub text: String,
    pub options: &'static [&'static str],
    pub cancelable: bool,
}

pub const OK: &[&str] = &["ID_COMMON_OK"];
pub const NO_YES: &[&str] = &["ID_COMMON_NO", "ID_COMMON_YES"];
pub const ACCEPT_DECLINE: &[&str] = &["ID_COMMON_ACCEPT", "ID_COMMON_DECLINE"];

fn literal(d: &CareerData, ids: &[&str]) -> String {
    let lines: Vec<&str> = ids.iter().map(|id| d.text(id)).filter(|t| !t.is_empty()).collect();
    format!("#{}", lines.join("  ").replace('^', " "))
}

/// The stand-in text of a step's screen; None for a step with no screen.
#[must_use]
pub fn shown(d: &CareerData, step: &Step) -> Option<Shown> {
    let ok = |text: String| Some(Shown { asking: Asking::Step(step.clone()), text, options: OK, cancelable: false });
    match step {
        Step::RewardSummary(s) => {
            let mut text = format!("#Challenge complete!  ${} cash  {} Board Sales", s.cash, s.boards);
            for item in &s.items {
                let title = d.rewards.get(item).map_or(item.as_str(), |r| d.text(&r.title));
                text.push_str(&format!("  Unlocked: {title}"));
            }
            ok(text)
        }
        Step::Milestones(n) => ok(format!("#Board Sales milestone reached! ({n})")),
        Step::Teammates(n) => ok(format!("#New teammate{} joined your team!", if *n == 1 { "" } else { "s" })),
        Step::OnlinePopup => ok(literal(d, &["ID_COMMON_WARNING_TITLE", "ID_MAP_VISIT_THE_ONLINEMAP"])),
        Step::Poster(row) => {
            let r = d.row(row)?;
            Some(Shown {
                asking: Asking::Step(step.clone()),
                text: literal(d, &[&r.title, &r.description]),
                options: ACCEPT_DECLINE,
                cancelable: true,
            })
        }
        Step::Present { .. } | Step::Autosave | Step::Teleport { .. } => None,
    }
}

/// `Popup.ShowYesNoPopup` on sk8popup: the title folded into the body (the movie has no title), No first, B ignored.
#[must_use]
pub fn yes_no(d: &CareerData, title: &str, description: &str) -> Shown {
    Shown { asking: Asking::YesNo, text: literal(d, &[title, description]), options: NO_YES, cancelable: false }
}

/// The glue's share of the script's effects: rewards to the unlock store, achievements, the feed, brand tiers; a
/// Skate 101 or yes/no popup comes back as the screen to show.
fn take(effects: Vec<Effect>, state: &mut CareerState, store: &mut Store) -> Option<Shown> {
    let d = state.data.clone();
    let mut screen = None;
    for effect in effects {
        match effect {
            Effect::Show101 { title, description } => {
                let text = literal(&d, &[&title, &description]);
                screen = Some(Shown { asking: Asking::Skate101 { description }, text, options: OK, cancelable: false });
            }
            Effect::YesNo { title, description } => screen = Some(yes_no(&d, &title, &description)),
            Effect::Reward { name, .. } => {
                store.grant_reward(&d, &name);
            }
            Effect::Achievement(name) if !state.rules.achievements.contains(&name) => state.rules.achievements.push(name),
            Effect::SkateFeed(message) => state.skatefeed.push(message),
            Effect::BrandTier(tier) if !state.brand_tiers.contains(&tier) => state.brand_tiers.push(tier),
            Effect::Teammate => state.teammates += 1,
            _ => {}
        }
    }
    screen
}

/// What the player answered on `asked` (an option index; a cancel is the last option).
fn answered(career: &mut Career, asked: Shown, answer: usize, current_map: &str) -> Option<signup::Launch> {
    let state = career.state.as_mut()?;
    let effects = match &asked.asking {
        Asking::Skate101 { .. } => career.core.reply("hudcomplete", state),
        Asking::YesNo => career.core.reply(if answer == 1 { "hudyes" } else { "hudno" }, state),
        Asking::Step(step) => {
            let mut launch = None;
            if let Step::Poster(row) = step
                && answer == 0
            {
                let d = state.data.clone();
                state.rules.started(&d, row);
                launch = signup::launch(&d, row, current_map);
            }
            let effects = career.core.done(step, state);
            career.shown = take(effects, state, &mut career.store);
            return launch;
        }
    };
    career.shown = take(effects, state, &mut career.store);
    None
}

fn world_now(d: &CareerData, map: &crate::map_transition::CurrentMap) -> Option<String> {
    map.path.as_ref().and_then(|p| mode::world_of(d, &p.to_string_lossy()))
}

#[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
fn drive(
    gate: Res<CareerGate>,
    mut career: ResMut<Career>,
    game_mode: Res<GameMode>,
    map: Res<crate::map_transition::CurrentMap>,
    config: Res<crate::config::Config>,
    mut transition: ResMut<MapTransition>,
    mut travel: ResMut<crate::teleport_menu::Travel>,
    mut skater: ResMut<crate::physics::SkaterRuntime>,
    net: Res<crate::multiplayer::Multiplayer>,
    party: Res<crate::party_play::PartyPlay>,
    matches: Res<crate::match_core::Match>,
    mut results: MessageReader<ChallengeResult>,
    (mut exits, mut starts): (MessageWriter<ModeExit>, MessageWriter<StartRow>),
    nav: Res<crate::customiser::Navigation>,
    lesson: Option<Res<super::lesson_run::LessonRun>>,
) {
    let career = &mut *career;
    let Some(d) = career.state.as_ref().map(|s| s.data.clone()) else { return };
    let world = world_now(&d, &map);
    let runs = mode::career_runs(gate.data.is_some(), *game_mode, net.active(), party.active, world.as_deref());
    if career.running && !runs {
        career.core.drop_pending();
        exits.write(ModeExit);
    }
    career.running = runs;
    if !runs {
        results.clear();
        return;
    }
    if let Some(state) = career.state.as_mut() {
        for r in results.read() {
            keep_best(state, &r.row, r.outcome, r.score);
            career.core.finished(&r.row, r.outcome, state);
        }
    }
    let here = map.path.as_ref().map_or_else(String::new, |p| p.to_string_lossy().into_owned());
    if let Some(answer) = career.answer.take()
        && let Some(asked) = career.shown.take()
        && let Some(launch) = answered(career, asked, answer, &here)
    {
        // The row's start (spec 6.2); its mode port starts it there (Tasks 28-33).
        go(&launch.map, launch.at, &config, &mut transition, &mut travel, &mut skater);
        info!("Skate 3 career: {} accepted", launch.row);
        starts.write(StartRow { loaded_world: launch.map.is_some(), row: launch.row });
    }
    let Some(state) = career.state.as_mut() else { return };
    if career.shown.is_some() {
        return;
    }
    let effects = career.core.pump(state);
    career.shown = take(effects, state, &mut career.store);
    if career.shown.is_some() {
        return;
    }
    let ready = Ready { challenge_running: matches.mode.is_some() || lesson.as_ref().is_some_and(|l| l.running()), transition_pending: transition.busy(), screen_open: false };
    // One step per frame at most, so a teleport's load is seen as pending before the next one.
    let Some(step) = career.core.next(&ready, state) else {
        if career.core.idle() {
            career.holder.end();
            // The sign-up manager (spec 6.2): inside a row's box, D-pad Up ('signup' context, 0x82D52F58).
            if !ready.challenge_running && !ready.transition_pending && nav.dpad & 1 != 0
                && let Some(world) = world.as_deref()
            {
                let built = &career.built;
                let boxes = signup::boxes(&state.rules, &d, world, &|r| built.hides(&d, r));
                let at = skater.animated_skeleton.roots.animation_to_world[3];
                let near = signup::in_range(&boxes, Vec3::new(at[0], at[1], at[2]));
                // shortcut: several rows in range take the first, not retail's picker (up to 4); add the picker
                // when two stage rows share a spot.
                let row = match signup::press(&near) {
                    Some(signup::Press::Begin(row)) => Some(row),
                    Some(signup::Press::Picker(rows)) => rows.into_iter().next(),
                    None => None,
                };
                if let Some(row) = row {
                    career.shown = shown(&d, &Step::Poster(row));
                }
            }
        }
        return;
    };
    if career.holder.hold(&career.built, &d, &step, &mut state.held) {
        info!("Skate 3 career: held for a later stage: {step:?}");
    } else if let Some(screen) = shown(&d, &step) {
        // Answered on a later frame (popups.rs); `done` waits for it.
        career.shown = Some(screen);
        return;
    } else {
        match &step {
            Step::Autosave => {}
            Step::Teleport { location, world } => {
                let target = d.worlds.get(world).cloned().flatten();
                match d.locators.get(location).filter(|_| target.is_some()) {
                    Some(l) => {
                        let target = target.filter(|t| !map.path.as_ref().is_some_and(|p| p.ends_with(t)));
                        go(&target, l.matrix, &config, &mut transition, &mut travel, &mut skater);
                    }
                    None => warn!("Skate 3 career: teleport to {location} in {world}: not in the export"),
                }
            }
            // `Not retail (story mode stage 1 build)`: the state's presentation group is the presenter's (Task 22),
            // not wired yet; the graph goes on as if it played.
            other => info!("Skate 3 career: {other:?} (not drawn in this build)"),
        }
    }
    if stage::notice_due(state.held.as_ref()) {
        info!("Skate 3 career: {}", stage::STOP_NOTICE);
        if let Some(h) = state.held.as_mut() {
            h.notice_shown = true;
        }
    }
    let autosave = step == Step::Autosave;
    let effects = career.core.done(&step, state);
    career.shown = take(effects, state, &mut career.store);
    if autosave {
        career.save();
    }
}

/// Moves the player to `at`: in place, or after loading `map` (asset-relative) when it is another one.
fn go(
    map: &Option<String>,
    at: [[f32; 4]; 4],
    config: &crate::config::Config,
    transition: &mut MapTransition,
    travel: &mut crate::teleport_menu::Travel,
    skater: &mut crate::physics::SkaterRuntime,
) {
    match map {
        None => {
            let _ = skater.travel_and_mark(at);
        }
        Some(map) => {
            travel.pending_matrix = Some(at);
            let label = Path::new(map).file_stem().map_or_else(String::new, |s| s.to_string_lossy().replace('_', " "));
            transition.request_by(crate::map_library::Entry { label, path: Some(config.asset_root.join(map)) }, Requester::Career);
        }
    }
}

/// A finished load the player or the career asked for writes `place` and autosaves (spec 6.0), and re-evaluates the
/// armed actions (0x82C1C178).
#[allow(clippy::needless_pass_by_value)]
fn arrivals(
    mut changed: MessageReader<WorldChanged>,
    mut career: ResMut<Career>,
    transition: Res<MapTransition>,
    map: Res<crate::map_transition::CurrentMap>,
    config: Res<crate::config::Config>,
) {
    if changed.read().count() == 0 {
        return;
    }
    let career = &mut *career;
    let Some(state) = career.state.as_mut() else { return };
    let Some(path) = map.path.as_ref() else { return };
    let relative = path.strip_prefix(&config.asset_root).unwrap_or(path).to_string_lossy().replace('\\', "/");
    let d = state.data.clone();
    if let Some(place) = mode::arrival(transition.requester(), &d, &relative, None) {
        state.place = Some(place);
        for fired in state.rules.reevaluate(&d, Changed::All) {
            state.runner.queue(fired);
        }
        career.save();
    }
}

/// The boot's opening finished on a new career (boot.rs `Command::Finish`). Spec 11.3: the world load into `schoolstart` (Yes) or `gamestart` (No), `rules.root`, then the first auto
/// sign-up, whose poster waits for the load (Core's transition gate).
pub fn start_new(state: &mut CareerState, core: &mut Core, tutorial: bool) {
    if !state.new_career {
        return;
    }
    let d = state.data.clone();
    let fe = if tutorial { &d.fe_locations.schoolstart } else { &d.fe_locations.gamestart };
    core.effects(vec![Effect::Teleport { location: fe.location.clone(), world: fe.world.clone() }], state);
    begin(state, core, tutorial);
}

/// A returning career's boot (spec 11.4): back to `place` when it is a career world's locator; nothing pending comes
/// back. shortcut: a map-library place (no locator) keeps the saved default map; load it here if players ask.
pub fn resume(state: &mut CareerState, core: &mut Core) {
    let Some(super::core::Place { world: Some(world), location: Some(location), .. }) = state.place.clone() else { return };
    core.effects(vec![Effect::Teleport { location, world }], state);
}

/// Skate 2's sign-up indicator data (skate2_arrows draws `signup:` locators with it).
const SIGNUP_INDICATORS: &str = "private/skate2-tutorial/indicator/signup.json";
/// Markers float 2 m up, as Skate 2's (its vault's `signup` row offset).
const MARKER_RISE: f32 = 2.;

/// Each active sign-up row's marker in this world, hidden while a challenge runs or a screen is up (spec 6.2).
/// `Not retail (story mode stage 1 build)`: drawn with Skate 2's sign-up indicator, not Skate 3's SignUpIndicator
/// ribbon types (Task 24 draws those).
#[must_use]
pub fn markers(career: &Career, world: Option<&str>, challenge_running: bool) -> Vec<(String, [[f32; 4]; 4])> {
    let (Some(state), Some(world)) = (career.state.as_ref(), world) else { return Vec::new() };
    if !career.running || challenge_running || career.shown.is_some() {
        return Vec::new();
    }
    let d = &state.data;
    signup::boxes(&state.rules, d, world, &|r| career.built.hides(d, r))
        .into_iter()
        .filter_map(|b| {
            let location = &d.row(&b.row)?.location;
            let mut m = d.locators.get(location)?.matrix;
            m[3][1] += MARKER_RISE;
            Some((format!("signup:{location}"), m))
        })
        .collect()
}

#[allow(clippy::needless_pass_by_value)]
fn draw_markers(
    career: Res<Career>,
    map: Res<crate::map_transition::CurrentMap>,
    matches: Res<crate::match_core::Match>,
    arrows: Option<ResMut<crate::skate2_arrows::Arrows>>,
    mut owned: Local<bool>,
) {
    let Some(mut arrows) = arrows else { return };
    let world = career.state.as_ref().and_then(|s| world_now(&s.data, &map));
    let wanted = markers(&career, world.as_deref(), matches.mode.is_some());
    // Only while the career owns the list: the Skate 2 modes write it on their own maps.
    if wanted.is_empty() && !*owned {
        return;
    }
    *owned = !wanted.is_empty();
    if !arrows.more.iter().any(|m| m == SIGNUP_INDICATORS) {
        arrows.more.push(SIGNUP_INDICATORS.into());
    }
    if arrows.wanted != wanted {
        arrows.wanted = wanted;
    }
}

/// Play time counts in the career and in Free Play (freeplay2 Q3).
#[allow(clippy::needless_pass_by_value)]
fn count_playtime(time: Res<Time>, mut career: ResMut<Career>) {
    if let Some(state) = career.state.as_mut() {
        state.playtime += time.delta_secs_f64();
    }
}

pub(super) fn build(app: &mut App, settings: &Path) {
    let data = app.world().resource::<CareerGate>().data.clone();
    let career = Career::open(data, settings);
    app.insert_resource(career)
        .init_resource::<GameMode>()
        .init_resource::<super::freeplay::FreePlay>()
        .init_resource::<super::cheats::Cheats>()
        .add_message::<ChallengeResult>()
        .add_message::<StartRow>()
        .add_message::<ModeExit>()
        .add_systems(
            Update,
            (count_playtime, arrivals, drive, draw_markers).chain().run_if(|career: Res<Career>| career.state.is_some()),
        );
}

#[cfg(test)]
mod tests {
    use super::super::data::Row;
    use super::*;
    use std::sync::Arc;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("s3career-glue-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn bests_keep_the_highest_score() {
        let mut s = CareerState::new(Arc::new(CareerData::default()));
        keep_best(&mut s, "r", Outcome::Owned, 500);
        keep_best(&mut s, "r", Outcome::Owned, 200);
        assert_eq!(s.bests["r"], Best { score: 500.0, level: Level::Owned as u32 });
        keep_best(&mut s, "r", Outcome::Killed, 100);
        assert_eq!(s.bests["r"], Best { score: 500.0, level: Level::Killed as u32 });
        keep_best(&mut s, "q", Outcome::Abandoned, 900);
        assert!(!s.bests.contains_key("q"));
    }

    #[test]
    fn the_launch_map_follows_place() {
        let dir = scratch("launch");
        let mut career = Career::open(None, &dir);
        let root = Path::new("C:/assets");
        assert_eq!(launch_map(&career, root, Some(Path::new("C:/assets/maps/Default.skate"))).unwrap(), Path::new("C:/assets/maps/Default.skate"));
        let mut s = CareerState::new(Arc::new(CareerData::default()));
        s.place = Some(super::super::core::Place { world: Some("dist_university".into()), map: "maps/University.skate".into(), location: None });
        career.state = Some(s);
        assert_eq!(launch_map(&career, root, None).unwrap(), root.join("maps/University.skate"));
    }

    #[test]
    fn a_new_install_starts_a_new_career_with_retails_reset() {
        let dir = scratch("new");
        let mut d = CareerData::default();
        d.rows.insert("tut_sksc_01".into(), Row::default());
        let career = Career::open(Some(Arc::new(d)), &dir);
        assert!(career.state.as_ref().unwrap().new_career);
        assert!(!career.store.everything, "a new career earns its items");
        assert!(career.store.earned_retail(unlocks::CHARACTERS, "shingo"));
    }

    #[test]
    fn a_returning_install_without_unlocks_keeps_everything() {
        let dir = scratch("upgrade");
        std::fs::write(dir.join("career.json"), r#"{"intro_done": true, "team_name": "t", "tutorial": true}"#).unwrap();
        let mut career = Career::open(Some(Arc::new(CareerData::default())), &dir);
        assert!(career.store.everything);
        assert_eq!(career.core.pending_signup(), Some("tut_sksc_01"));
        career.save();
        crate::saves::flush();
        assert!(dir.join(save::FILE).is_file());
        assert!(dir.join(unlocks::FILE).is_file());
    }

    #[test]
    fn coach_frank_yes_loads_the_school_then_signs_up_tut_sksc_01() {
        let mut d = CareerData::default();
        d.fe_locations.schoolstart = super::super::data::Place { world: "dist_skateschool".into(), location: "tele_world_to_school_dest_locator_01".into() };
        d.fe_locations.gamestart = super::super::data::Place { world: "dist_university".into(), location: "ftl_univ_02_challengelocator_01".into() };
        let d = Arc::new(d);
        for (tutorial, world, row) in [(true, "dist_skateschool", "tut_sksc_01"), (false, "dist_university", "ftl_univ_02")] {
            let mut s = CareerState::new(d.clone());
            s.new_career = true;
            let mut core = Core::default();
            start_new(&mut s, &mut core, tutorial);
            let Some(Step::Teleport { world: w, .. }) = core.next(&Ready::default(), &mut s) else { panic!("the load comes first") };
            assert_eq!(w, world);
            assert_eq!(core.pending_signup(), Some(row));
        }
        let mut s = CareerState::new(d);
        let mut core = Core::default();
        start_new(&mut s, &mut core, true);
        assert_eq!(core.pending_signup(), None, "a returning career never restarts");
    }

    #[test]
    fn a_returning_career_starts_at_place() {
        let mut s = CareerState::new(Arc::new(CareerData::default()));
        let mut core = Core::default();
        resume(&mut s, &mut core);
        assert!(core.next(&Ready::default(), &mut s).is_none(), "no place: where the boot left it");
        s.place = Some(super::super::core::Place { world: Some("dist_university".into()), map: "maps/University.skate".into(), location: Some("l".into()) });
        resume(&mut s, &mut core);
        let Some(Step::Teleport { world, .. }) = core.next(&Ready::default(), &mut s) else { panic!("back to the place") };
        assert_eq!(world, "dist_university");
        assert_eq!(core.pending_signup(), None, "nothing pending comes back");
    }

    #[test]
    fn markers_show_active_signup_rows_and_hide_in_a_challenge() {
        let mut d = CareerData::default();
        d.rows.insert(
            "ftl_univ_02".into(),
            Row { world: "dist_university".into(), location: "l".into(), global: signup::SIGNUP, ..Row::default() },
        );
        let mut matrix = [[0.0; 4]; 4];
        matrix[3] = [1.0, 2.0, 3.0, 1.0];
        d.locators.insert("l".into(), super::super::data::Locator { map: String::new(), matrix, slots: Vec::new() });
        let mut career = Career::open(None, &scratch("markers"));
        let mut s = CareerState::new(Arc::new(d));
        s.rules.active.push("ftl_univ_02".into());
        career.state = Some(s);
        assert!(markers(&career, Some("dist_university"), false).is_empty(), "not before the career runs");
        career.running = true;
        let m = markers(&career, Some("dist_university"), false);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].0, "signup:l");
        assert_eq!(m[0].1[3], [1.0, 4.0, 3.0, 1.0]);
        assert!(markers(&career, Some("dist_university"), true).is_empty(), "hidden in a challenge");
        assert!(markers(&career, Some("dist_skateschool"), false).is_empty(), "another world");
    }

    #[test]
    fn a_reward_reaches_the_unlock_store() {
        let mut d = CareerData::default();
        d.rewards.insert(
            "reda".into(),
            super::super::data::Reward {
                category: 1,
                item: "giovanni_reda".into(),
                title: String::new(),
                description: String::new(),
                icon: String::new(),
                raw: Vec::new(),
            },
        );
        let mut s = CareerState::new(Arc::new(d));
        let mut store = Store::default();
        take(vec![Effect::Reward { name: "reda".into(), milestone: false }, Effect::Achievement("a".into())], &mut s, &mut store);
        assert!(store.earned_retail(1, "giovanni_reda"));
        assert_eq!(s.rules.achievements, ["a"]);
    }
}
