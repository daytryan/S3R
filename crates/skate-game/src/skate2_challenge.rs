//! Skate 2's tutorial on New_San_Vanelona (docs/superpowers/specs/2026-09-30-skate2-tutorial-design.md), run by the
//! disc's own progression triggers and lesson step machines on the shared challenge core. The data comes from
//! skate2port/old_tutorial.py (`private/skate2-tutorial`); what was inferred rather than read is listed in
//! skate2port/tutorial_recovery.md.
//!
//! Flow: on the first NSV load the intro movies and Create-a-Skater play, then progressioncore's "Start State
//! Machine" runs. It plays the opening NIS, asks "New to skate?", and executes Meet Slappy. A lesson's win starts its
//! progression triggers, which activate the next lessons. An active lesson starts from a prompt at its start locator,
//! or from Learn > Skate 2 Tutorial.
//!
//! The two photo challenges (Atiba's rail and puzzle shots) run the disc's spot-photo template. Their trick is
//! judged when its sequence lands (the objective's PointsValid). The highlight reel replays it from the engine's
//! replay recording, under the photo scene's camera, snapping the photos the player then picks from.
//!
//! ponytail: what stands in for parts still to come:
//! - The HUD is Bevy UI text, not the Skate 2 tutorial Flash movies.
//! - Guide markers are Skate 2's neon outline indicators (skate2_arrows.rs, from the disc's spline shapes).
//! - The lessons' DMOs (dmos.json) are physics/props.rs props drawn with Skate 3's models of the same template; a
//!   template Skate 3 has no movable model of (picnic tables, wood panels, oil drums) stays absent.
//! - The photo pick is Skate 2's photoselect with the highlighted screenshot (taken during the reel) drawn over it as
//!   Bevy UI, not inside its strip; the chosen photo isn't kept.
use crate::{
    skate2_cast::Cast,
    skate2_screens::{Central, Popup, TutorialScreens, View, countdown_text, timer_text},
    core_menu_runtime::Key,
    challenge::{
        Condition, Machine, Script,
        cutscene::Cutscene,
        movie::{Movie, read_wav},
    },
    config::Config,
    map_transition::CurrentMap,
    physics::{GamePhysics, SkaterRuntime, riders::Riders},
    replay::Replay,
    skate2_rider::{Driver, Player},
    skate_audio::StreamPcm,
};
use bevy::{
    prelude::*,
    render::view::screenshot::{Screenshot, ScreenshotCaptured},
};
use mlua::{FromLuaMulti, IntoLuaMulti, Lua, Table, Value as LuaValue, Variadic};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Instant,
};

const DIR: &str = "private/skate2-tutorial";
const MAP: &str = "New_San_Vanelona";
const START_MACHINE: &str = "Start State Machine";
const MEET_SLAPPY: &str = "m_1a_01_goto_slappys";
const FLIPTRICKS: &str = "m_1a_03_tutorial_fliptricks";
const FOLLOW_SLAPPY: &str = "m_1a_07_follow_slappy";
/// The tutorial's lessons (progression.lua): Meet Slappy's win activates Flipping Wood 101; its win opens No Mugshots
/// Please, Two On The Floor and The Art of Pumping together; No Mugshots' opens Just get up there, whose opens Follow
/// Slappy. Retail runs the open ones in any order; this order only picks which one Learn resumes and whose sign-up
/// prompt wins where two start locators meet. Follow Slappy is the last (user 10-13): its win activates the sponsor-me
/// missions, which are not lessons here, so nothing follows it.
const LESSONS: [&str; 7] = [
    MEET_SLAPPY,
    FLIPTRICKS,
    "m_1a_05_photo_rail",
    "m_1a_04_tutorial_manuals",
    "m_1a_06_photo_puzzle",
    "m_2a_01_tutorial_slappy",
    FOLLOW_SLAPPY,
];
/// GlobalSpawnCharacterInfo words [2..4] (the livingworld_entities Model key) -> cast key.
/// ponytail: the photo challenges' photographer key is taken to be Atiba (they spawn him at the photo spot and speak
/// as AudioCharacterOverride 38, his bank).
const SPAWN_CHARACTERS: [((u64, u64), &str); 3] = [
    ((429363999, 446713631), "slappy"),
    ((1131089035, 1165832375), "giovanni_reda"),
    ((2295803380, 1138059089), "attiba_jefferson"),
];
/// Progression spawn messages -> cast key.
const PROS: [(&str, &str); 3] = [("Atiba", "attiba_jefferson"), ("Carroll", "mike_carroll"), ("Koston", "eric_koston")];
/// The NIS after the photo puzzle that takes the pros away.
const PROS_LEAVE: &str = "03_Pros_Leave";
/// ChallengeSpeech attributes are [speaker, line << 16]; speaker -> speech bank.
const SPEAKERS: [(i64, &str); 3] = [(29, "Slappy"), (38, "Atiba"), (40, "CameraMan")];
/// The highlight clock counts 60 Hz frames (GetLastFrameNumberWritten, GetVisualGameTick).
const FRAME_HZ: f64 = 60.;
/// Objective conditions count frames (`objcounter >= 90`), and the disc ran them at 30 per second.
const EVALUATION_HZ: f64 = 30.;
/// ponytail: speeds for IsMoving/IsStoppped, chosen by eye.
const MOVING: f32 = 0.5;
const STOPPED: f32 = 0.3;
const CENTRAL_SECONDS: f64 = 3.;
/// A teleport counts as finished after at least the minimum, or at the latest once the timeout passes.
const TELEPORT_MIN: f64 = 0.25;
const TELEPORT_TIMEOUT: f64 = 3.;
/// Natives the handlers call that have nothing to do in the port.
const NO_OPS: &[&str] = &[
    "HUD2.SetupObjectiveHUDData",
    "HUD2.SetActiveGroup",
    "Objectives.DeleteObjectiveGroup",
    "HOM.EnableHOMVisuals",
    "Effects.StartEffect",
    "ChallengeAsset.ActivateChallengeAssetLoadScreen",
    "ChallengeAsset.DeactivateChallengeAssetLoadScreen",
    "ChallengeAsset.PreloadChallengeAssetsByName",
    "DMOs.ReleaseDMOVolumeTester",
    "Progression.RevertChallengeByTitle",
    // The spot-photo template's: online, security, session marker, timed and scored film modes, the AI pin.
    "Service.ActivateSecurityServiceForChallengeReplay",
    "Service.DeactivateSecurityServiceForChallengeReplay",
    "SessionMarker.SetSessionMarker",
    "Teleport.TeleportLocalPlayerToSessionMarker",
    "HUD2.CreatePreFilmSetupHUD",
    "HUD2.DeletePreFilmSetupHUD",
    "HUD2.DPOSetIcon",
    "HUD2.DPOSetParams",
    "HUD2.SetABDGestureVisible",
    "HUD2.ShowTrickDisplayContext",
    "HUD2.ABDHUDClearList",
    "HUD2.ABDHUDRefreshFilmTrickList",
    "AI.EnterAiBasedChallenge",
    "AI.FilterReset",
    "AI.UnloadPaths",
    "AI.PinAI",
    "AI.UnpinAI",
    "FrontEnd.Results_SetNumPlayers",
    "FrontEnd.SetEndChallengeOption",
    "Objectives.AddLastUniqueTrickToList",
    "Objectives.ChangeObjectivePointsRequirement",
    "Objectives.FailAllObjectiveStatus",
    "Objectives.SetFractionNumerator",
    "ObjectivePoints.AwardSequencePointsImmediately",
    "ChallengeTimer.ResetCountdownTimer",
    // The race template's (FollowMe, single player): race stats, markers, debug text, results rows, the camera.
    "AI.SelectCharacters",
    "AI.FilterPaths",
    "AI.StartLapPaths",
    "AI.ClearBehavior",
    "ChallengeManager.SetManualsAreConnectors",
    "ChallengeManager.SwitchChallengeCamera",
    // ponytail: Slappy has no follow-me lines (309/310) on the disc.
    "ChallengeSpeech.PlayFollowMeWarningSpeech",
    "ChallengeSpeech.PlayNPCSpeech",
    "ChallengeSpeech.PlaySkaterSpeech",
    "ChallengeTimer.PauseTimer",
    "ChallengeTimer.RemoveCountupTimer",
    "Debug.DebugTextPermanent",
    "Debug.DebugTextTimed",
    "Debug.RemoveTextPermanent",
    "Effects.ResetEffect",
    "FrontEnd.Results_SetActiveResultsMenu",
    "FrontEnd.Results_SetColTitle",
    "FrontEnd.Results_SetPlayerData",
    "FrontEnd.Results_SetPlayerStatus",
    "FrontEnd.Results_SetPlayerTime",
    "FrontEnd.Results_SetResultsMenuType",
    "HUD2.LeaderboardHUDSetVariantString",
    "NIS.ClearPreloadedNIS",
    "NIS.ResetNISPlayerIndexes",
    "NIS.SubmitNISPlayerIndex",
    "Objectives.ForceObjectiveComplete",
    "Race.AddPlayerToRace",
    "Race.FinishPlayer",
    "Race.RegisterPlayerTimer",
    "Race.ResetRaceStats",
    "Race.SetPlayerProgress",
    "Race.StopRaceStats",
    "VisualIndicators.AddFollowIndicator",
    "VisualIndicators.AddTriggerIndicator",
    "VisualIndicators.RemoveByType",
    "VisualIndicators.RemoveFollowIndicator",
    "VisualIndicators.RemoveTriggerIndicator",
];
/// Turns every global table into one whose missing functions log once and return nil (spec: an unimplemented
/// native never stops a lesson). Undefined globals become such tables too.
const STUBS: &str = r#"
local function missing(name)
  return function() __missing(name) return nil end
end
function __stub(name, t)
  return setmetatable(t or {}, {__index = function(t, k)
    local f = missing(name .. "." .. tostring(k))
    rawset(t, k, f)
    return f
  end})
end
for _, name in ipairs(__namespaces) do _G[name] = __stub(name, rawget(_G, name)) end
for _, name in ipairs(__no_ops) do
  local ns, f = string.match(name, "([%w_]+)%.([%w_]+)")
  if rawget(_G, ns) == nil then _G[ns] = __stub(ns) end
  rawset(_G[ns], f, function() end)
end
setmetatable(_G, {__index = function(g, k) local t = __stub(k) rawset(g, k, t) return t end})
HUD2.eHUDComponent = {ObjectiveHUD = "ObjectiveHUD", CentralHUD = "CentralHUD", TutorialInstructionHUD = "TutorialInstructionHUD",
  CountdownHUD = "CountdownHUD", TimerHUD = "TimerHUD", PhotoTreatmentHUD = "PhotoTreatmentHUD", ABDHUD = "ABDHUD", DPOHUD = "DPOHUD",
  DistanceMeter = "DistanceMeter", LeaderboardHUD = "LeaderboardHUD", TrickDisplayHUD = "TrickDisplayHUD"}
HUD2.eMessageColour = {White = "White"}
HUD2.eMessageAudioHook = {Success = "Success", Fail = "Fail"}
HUD2.eDPOIcon = {SpotPhoto = "SpotPhoto"}
Effects.eChallengeEffect = {FadeIn = "FadeIn", FadeOut = "FadeOut"}
Transition.eChallengeTransitionType = {ToGameplayLoadScreen = "ToGameplayLoadScreen", Normal = "Normal"}
-- ponytail: ChallengeType 38 is the photo challenges' (the spot-photo template's); the other types only need to differ.
Challenge.eChallengeTypes = {SpotPhoto = 38, SpotFilm = -1, AnywhereFilm = -2}
Challenge.eChallengeFlow = {Lose = 0, Win = 1, Kill = 2, Exit = 3}
Challenge.GameMode = {SinglePlayer = 0, NoProfileFreeSkate = 1}
FrontEnd.eSk8BEEndMissionOptions = {Continue_Retry = 0, Retry_Quit = 1, Unknown = 2, Continue = 3, Previous = 4, Retry = 5}
FrontEnd.eResultGroupType = {FinalResults = 0, RoundResults = 1}
FrontEnd.eResultsLBType = {BestTrickNonElimInt = 0, DeathRace = 1}
-- ponytail: FollowMe is RaceType 2 (m_1a_07_follow_slappy's); the other race types only need to differ.
Race.eRaceType = {FollowMe = 2, TrickForTime = -1}
VisualIndicators.eChallengeIndicators = {RaceGates = 0, RaceFinish = 1, FollowMeTarget = 2}
Broadcasters.Ports = {PresThread = 0}
function cMsgEnterRealTime() end
"#;

// ---- Data (tutorial.json) ----

#[derive(Deserialize)]
struct Data {
    frontend: Vec<Value>,
    progression: ProgressionData,
    challenges: BTreeMap<String, ChallengeData>,
    placement: Placement,
    scenes: BTreeMap<String, Scene>,
    movies: BTreeMap<String, MovieData>,
    speech: BTreeMap<String, Speaker>,
    trick_groups: BTreeMap<String, Vec<String>>,
    /// Grind.DidSpecificGrind ids (decimal) -> the scorables they name.
    #[serde(default)]
    grinds: BTreeMap<String, Vec<String>>,
    /// The competitors' laps by lesson, one per competitor: [seconds, x, y, z, motion code] samples.
    #[serde(default)]
    ai_paths: BTreeMap<String, Vec<Vec<[f32; 5]>>>,
    /// round2 T5: the lessons' aipaths.big recordings whole, for the riders' drivers (skate2_rider.rs).
    #[serde(default)]
    ai_segments: BTreeMap<String, Vec<crate::skate2_rider::Path>>,
    /// round2 T5 tricks: Skate 2's scorable rows by trick index (lead, AI recipe) and the competitors' AI profiles.
    #[serde(default)]
    ai_tricks: Vec<Option<crate::skate2_rider::Trick>>,
    #[serde(default)]
    ai_profiles: BTreeMap<String, crate::skate2_rider::Profile>,
    #[serde(skip)]
    strings: BTreeMap<String, String>,
    #[serde(skip)]
    root: PathBuf,
    #[serde(skip)]
    progression_lua: String,
}
#[derive(Deserialize)]
struct ProgressionData {
    triggers: BTreeMap<String, TriggerData>,
    messages: BTreeMap<String, MessageData>,
    entries: BTreeMap<String, EntryData>,
}
#[derive(Deserialize)]
struct TriggerData {
    initial: String,
    states: BTreeMap<String, BTreeMap<String, String>>,
}
#[derive(Deserialize)]
struct MessageData {
    #[serde(rename = "type")]
    kind: Value,
    properties: Vec<Value>,
    /// The properties by vault name (FileID, TitleHALID, ...), ints as numbers.
    #[serde(default)]
    named: BTreeMap<String, Value>,
    /// The vault message id (Hash_6931057318D2132E): type 1 branches on it.
    #[serde(default)]
    id: Option<u64>,
}
#[derive(Deserialize)]
struct EntryData {
    triggers: Vec<String>,
    /// [trigger, mode] binding rows in vault order (old_tutorial.py entry_bindings).
    #[serde(default)]
    bindings: Vec<(String, u8)>,
}
impl EntryData {
    /// The triggers CompleteChallenge starts for `outcome`. An install without bindings: its trigger list on a win.
    fn fired(&self, outcome: Outcome) -> Vec<String> {
        if self.bindings.is_empty() {
            return if outcome == Outcome::Win { self.triggers.clone() } else { Vec::new() };
        }
        self.bindings.iter().filter(|(_, mode)| outcome.fires(*mode)).map(|(t, _)| t.clone()).collect()
    }
    /// The triggers a lesson's sign-up starts: kind 5 (0x824d1668) fires binding modes 8, 10 and 11 (0x824d12d8, jump
    /// table 0x824cd104), Follow Slappy's `_accept`. INFERRED: kind 5 = sign-up accepted (challenge flow state 6,
    /// 0x82b78ae8, by its callers' "signup" / "Entry Fee" strings).
    fn signed_up(&self) -> Vec<String> {
        self.bindings.iter().filter(|(_, mode)| matches!(mode, 8 | 10 | 11)).map(|(t, _)| t.clone()).collect()
    }
}

/// The event a lesson records before CompleteChallenge (Win/Lose/AbandonChallenge).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Outcome {
    Win,
    Lose,
    Abandon,
}
impl Outcome {
    /// Which binding modes (Hash_20C0) the outcome starts: 0x824cd1b8 (round2-research2 Q5).
    fn fires(self, mode: u8) -> bool {
        matches!((self, mode), (Outcome::Win, 0 | 3 | 5) | (Outcome::Lose, 1 | 3 | 4 | 5) | (Outcome::Abandon, 2 | 4 | 5))
    }
}
#[derive(Deserialize)]
struct ChallengeData {
    #[serde(rename = "self")]
    fields: Value,
    steps: BTreeMap<String, BTreeMap<String, String>>,
    /// Vault rows: [`ObjectiveData`], and `self.Objectives` for the handlers.
    objectives: Vec<Value>,
    /// The hudminimap marker (old_tutorial.py minimap_icon); none in an install from before it was exported.
    #[serde(default)]
    icon: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ObjectiveData {
    name: String,
    definition: Definition,
    #[serde(default)]
    intro_events: Vec<Event>,
    #[serde(default)]
    complete_events: Vec<Event>,
    #[serde(default)]
    visual_indicators: Vec<Indicator>,
    #[serde(default)]
    hints: Vec<HintData>,
    /// [source, ...]: the objective completes only while this holds too (a landed sequence).
    #[serde(default)]
    points_valid: Value,
    #[serde(default)]
    requires_highlight: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct HintData {
    name: String,
    definition: Definition,
    #[serde(default)]
    events: Vec<Event>,
}
#[derive(Deserialize)]
struct Definition {
    #[serde(default)]
    source: String,
    #[serde(default)]
    text: String,
    /// The disc's definition row: [source, ?, ?, text, the text's {0:...} argument].
    #[serde(default)]
    words: Vec<Value>,
}
/// A presentation event: `kind` is the disc's event type; tutorial_recovery.md says what each one was named from.
#[derive(Deserialize, Clone, Debug)]
struct Event {
    #[serde(rename = "type")]
    kind: u32,
    #[serde(default)]
    a: String,
    #[serde(default)]
    b: String,
}
#[derive(Deserialize)]
struct Indicator {
    locator: String,
}
#[derive(Deserialize)]
struct Placement {
    locators: BTreeMap<String, [[f32; 4]; 4]>,
    volumes: BTreeMap<String, Volume>,
    /// A locator's sub-locations (tLocationDescData, old_tutorial_dmos.py): GetSpawnLocation's rows.
    #[serde(default)]
    spawn_points: BTreeMap<String, Vec<[[f32; 4]; 4]>>,
}
/// Every trigger volume on the disc has an identity matrix, so its min/max is a world box.
#[derive(Deserialize)]
struct Volume {
    min: [f32; 3],
    max: [f32; 3],
}
impl Volume {
    fn contains(&self, p: Vec3) -> bool {
        (0..3).all(|i| self.min[i] <= p[i] && p[i] <= self.max[i])
    }
}
#[derive(Deserialize)]
struct Scene {
    duration: f64,
    audio: Vec<SceneAudio>,
    subtitles: Vec<Cue>,
    /// A photo scene's snaps (its camera's FREEZE keys).
    #[serde(default)]
    photos: usize,
    /// The scene's character tracks (tutorial.json, as challenge/cutscene.rs reads them).
    #[serde(default)]
    tracks: Vec<SceneTrack>,
}
#[derive(Deserialize)]
struct SceneTrack {
    character: String,
}
impl Scene {
    /// RenderMan::UpdateVisibleTypes (0x82839be0): a scene with characters draws no gameplay skaters, so the player is
    /// seen only as its own current_player track. Every tutorial scene with tracks is such a scene (kind 19/26).
    fn hides_player(&self) -> bool {
        !self.tracks.is_empty() && self.tracks.iter().all(|t| t.character != "current_player")
    }
}
#[derive(Deserialize)]
struct SceneAudio {
    audio: String,
}
#[derive(Deserialize, Clone)]
struct Cue {
    start: f64,
    end: f64,
    label: String,
}
#[derive(Deserialize)]
struct MovieData {
    video: String,
    audio: Option<String>,
    subtitles: Vec<Cue>,
}
#[derive(Deserialize)]
struct Speaker {
    /// Lines cut from the disc (CameraMan 936) are null.
    lines: BTreeMap<String, Option<Line>>,
}
#[derive(Deserialize)]
struct Line {
    audio: String,
    seconds: f64,
}

impl Data {
    fn load(root: PathBuf) -> Result<Self, String> {
        let read = |name: &str| std::fs::read_to_string(root.join(name)).map_err(|e| format!("{}: {e}", root.join(name).display()));
        let mut data: Self = serde_json::from_str(&read("tutorial.json")?).map_err(|e| format!("tutorial.json: {e}"))?;
        data.strings = serde_json::from_str(&read("strings.json")?).map_err(|e| format!("strings.json: {e}"))?;
        data.progression_lua = read("scripts/progression.lua")?;
        // The camera's whoosh/zoom bed (the take's AUDIO_POST_ID) beside the speech: older exports list only the
        // speech, so a nis/<scene>_post.wav found next to it is played too.
        for (name, scene) in &mut data.scenes {
            let post = format!("nis/{name}_post.wav");
            if scene.audio.len() == 1 && root.join(&post).is_file() {
                scene.audio.push(SceneAudio { audio: post });
            }
        }
        data.root = root;
        Ok(data)
    }
    /// A string-table label as display text (`^` is the Flash movies' line break); the label itself when missing.
    fn text(&self, label: &str) -> String {
        self.strings.get(label).map_or_else(|| label.to_owned(), |s| s.replace('^', "\n"))
    }
    fn locator(&self, name: &str) -> Option<[[f32; 4]; 4]> {
        self.placement.locators.get(name).copied()
    }
    /// GetSpawnLocation (s2.pe 0x82879950): the locator's sub-location `index`; past the last, the first (with none,
    /// the locator) moved 5 m along world -z per index. RepositionAI's index defaults to the skater's id (0x826DC118).
    fn spawn_point(&self, name: &str, index: i64) -> Option<[[f32; 4]; 4]> {
        let subs = self.placement.spawn_points.get(name).map_or(&[][..], Vec::as_slice);
        if let Some(&m) = usize::try_from(index).ok().and_then(|i| subs.get(i)) {
            return Some(m);
        }
        let mut m = subs.first().copied().or_else(|| self.locator(name))?;
        m[3][2] -= 5. * index as f32;
        Some(m)
    }
    fn description(&self, lesson: &str) -> String {
        self.challenges.get(lesson).and_then(|c| c.fields["Description"].as_str()).map(|d| self.text(d)).unwrap_or_default()
    }
    /// A lesson's tutorial movie and its sign-up poster label (none where the label is ID_COMMON_NONE).
    fn poster_movie(&self, lesson: &str) -> Option<(String, String)> {
        let c = self.challenges.get(lesson)?;
        let label = c.fields["TutorialMovieLabel"].as_str().filter(|l| *l != "ID_COMMON_NONE")?;
        Some((c.fields["TutorialMovie"].as_str()?.to_owned(), self.text(label)))
    }
    fn title(&self, lesson: &str) -> String {
        self.challenges.get(lesson).and_then(|c| c.fields["Title"].as_str()).map_or_else(|| lesson.to_owned(), |t| self.text(t))
    }
}

/// The challenge's DMOResetInfo volumes (`[_, _, "DIST_Projects|<volume>|0x<id>", ...]`): its DMOs inside them go back
/// to their authored poses (physics/props.rs; only staged, active ones move).
fn reset_dmos(world: &mut World, data: &Data, lesson: &str) {
    let info = data.challenges.get(lesson).and_then(|c| c.fields["DMOResetInfo"].as_array());
    for entry in info.into_iter().flatten() {
        let name = entry.get(2).and_then(Value::as_str).and_then(|p| p.split('|').nth(1)).unwrap_or_default();
        match (data.placement.volumes.get(name), world.get_resource_mut::<GamePhysics>()) {
            (Some(volume), Some(mut physics)) => physics.reset_props(|p| volume.contains(p)),
            _ => debug!("Skate 2 tutorial: {lesson} DMO reset volume {name} not found"),
        }
    }
}

fn position(matrix: &[[f32; 4]; 4]) -> Vec3 {
    Vec3::new(matrix[3][0], matrix[3][1], matrix[3][2])
}

/// Retail's sign-up volume (0x82b790d0 -> 0x82acebb8): an axis-aligned box, `half` metres on every axis, around the
/// lesson's `Location`; a half of 0 or less is 40.
fn in_signup_box(location: Vec3, half: f32, at: Vec3) -> bool {
    let half = if half > 0. { half } else { 40. };
    (at - location).abs().max_element() <= half
}

/// The vault's `signup` indicator row offset (Hash_4374C0E6F4CB45ED = (0, 2, 0)).
const SIGNUP_RISE: f32 = 2.;
/// signup.json (old_indicators.py): each lesson's sign-up indicator, keyed `signup:<Location>`.
const SIGNUP_INDICATORS: &str = "private/skate2-tutorial/indicator/signup.json";

/// Each open, unwon lesson's sign-up indicator: `signup:<Location>` (signup.json's key) and its matrix, 2 m up.
/// UNTRACED (UNKNOWN #6): drawn at any distance; retail's ±250 m box (Hash_2C5FB57A89E8CDE2) is unread.
fn signup_indicators(data: &Data, p: &Progress) -> Vec<(String, [[f32; 4]; 4])> {
    LESSONS
        .into_iter()
        .filter(|l| p.active(l) && !p.complete(l))
        .filter_map(|l| {
            let location = data.challenges.get(l)?.fields["Location"].as_str()?;
            let mut m = data.locator(location)?;
            m[3][1] += SIGNUP_RISE;
            Some((format!("signup:{location}"), m))
        })
        .collect()
}
/// The minimap's lesson markers (icon, world x, z): each active lesson (0x824ca998) but ChallengeType 44 (0x824a8ba0)
/// at its Location, none at x = y = 0 (0x82472d74; an empty Location is that), with its base icon (round2-research2 Q4).
fn minimap_markers(data: &Data, active: &[String]) -> Vec<(String, [f32; 2])> {
    active
        .iter()
        .filter_map(|l| {
            let c = data.challenges.get(l).filter(|c| c.fields["ChallengeType"].as_i64() != Some(44))?;
            let m = data.locator(c.fields["Location"].as_str()?).filter(|m| m[3][0] != 0. || m[3][1] != 0.)?;
            Some((c.icon.clone()?, [m[3][0], m[3][2]]))
        })
        .collect()
}

/// `"DIST_Projects|m_1a_01_goto_slappys_push_exit_vol01"` -> the volume's name.
fn volume_name(path: &str) -> &str {
    path.rsplit('|').next().unwrap_or(path)
}

// ---- Progress (settings/skate2-tutorial.json) ----

#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(default)]
struct Progress {
    frontend_done: bool,
    skipped: bool,
    completed: Vec<String>,
    active: Vec<String>,
    /// ChallengeSave "Points Earned" by challenge.
    scores: BTreeMap<String, i64>,
    /// DynamicHull states the handlers set (SetupDynamicHullState, 0x82b76ff0), by hull: retail saves them with the
    /// career (cChallengeDynamicHullManager::Data, 0x82b77f60). Unset = the vault default (dmos.json `hulls`).
    hulls: BTreeMap<String, String>,
}
impl Progress {
    fn load(path: &Path) -> Option<Self> {
        crate::saves::load_json(path).ok()
    }
    fn save(&self, path: &Path) {
        let result = crate::saves::write_atomic(path, &serde_json::to_vec_pretty(self).unwrap_or_default());
        if let Err(error) = result {
            warn!("Skate 2 tutorial: progress not saved to {}: {error}", path.display());
        }
    }
    fn complete(&self, c: &str) -> bool {
        self.completed.iter().any(|x| x == c)
    }
    fn active(&self, c: &str) -> bool {
        self.active.iter().any(|x| x == c)
    }
    fn activate(&mut self, c: &str) {
        if !self.active(c) && !self.complete(c) {
            self.active.push(c.to_owned());
        }
    }
    fn next_lesson(&self) -> Option<&'static str> {
        LESSONS.into_iter().find(|l| self.active(l) && !self.complete(l))
    }
}

/// The DynamicHull states props.rs shows (round2 10-14); None = no hull at all. Retail loads a hull only when its load
/// mask (slappys: 4) shares a bit with the mode's flags (0x823a8be8), and only career (game type 1) sets 4
/// (0x824abf58): online (8), party (16) and the other offline types (2) never show the gate. INFERRED: the port's free
/// skate (tutorial exited, `skipped`) is such a mode.
fn career_hulls(tutorial: bool, progress: &Progress) -> Option<&BTreeMap<String, String>> {
    (tutorial && !progress.skipped).then_some(&progress.hulls) // Not retail (user 10-14): retail has no exit to free skate
}

// ---- What the natives see and ask for ----

/// The player as the objective natives see it, refreshed before each evaluation.
#[derive(Default)]
struct Sense {
    position: Vec3,
    on_board: bool,
    speed: f32,
    wiping: bool,
    wipe_seconds: f32,
    tricks_frame: Vec<&'static str>,
    tricks_objective: BTreeSet<&'static str>,
    landed: bool,
    holding: Vec<&'static str>,
    nose_seconds: f32,
    tail_seconds: f32,
    marker_set: bool,
    marker_used: bool,
    inside: BTreeSet<String>,
    entered: BTreeSet<String>,
    completed: usize,
    /// A points-gated objective (PointsValid) is being evaluated: volumes and tricks count over its sequence.
    scoped: bool,
    /// Volumes the skater was in, and tricks, since the current (or just landed) sequence began.
    visited: BTreeSet<String>,
    tricks_sequence: BTreeSet<&'static str>,
    grind_metres: f32,
    /// The last landed sequence's score.
    points: f32,
    /// The running lesson's DMO props (physics/props.rs): instance id -> the volumes holding its centre.
    dmos: Vec<(u64, BTreeSet<String>)>,
    /// Holding a DMO (OffBoardPushing 502).
    grabbing: bool,
}

enum Request {
    Message(String),
    Execute(String),
    Lesson(&'static str, Value),
    Teleport(String),
    Timer(i64, f64),
    Central(String, Central),
    Present(i64),
    HideInstruction,
    Outcome(Outcome),
    Complete,
    Speech(Event),
    RemoveTimer(i64),
    /// Pause.PauseSimulation (false) / UnpauseSimulation (true).
    Simulation(bool),
    Reel,
    Photos,
    KillNis,
    /// AI.SpawnAIAtLocation (a locator) / AI.UnspawnAI (None) of a competitor.
    Ai(i64, Option<String>),
    StartPath(i64),
    /// AI.RepositionAI of a competitor (the player's is a Teleport).
    Reposition(i64, String),
    /// FrontEnd.OpenContinueRetryDialog(won).
    ContinueRetry(bool),
    /// ChallengeReplay.ResetReplayBuffer: a race heat restart drops the recording.
    ResetReplay,
}

/// A competitor: spawned at a locator, then skating its lap of the lesson's ai_paths on game time.
#[derive(Default)]
struct Ai {
    locator: String,
    /// When the lap started (virtual seconds), and the lift putting the lap's heights on the ground.
    path: Option<(f64, f32)>,
    /// A photo subject's lap loops; a race leader's route ends.
    loops: bool,
    /// The trigger volumes it is in.
    inside: BTreeSet<String>,
    /// round2 T5: its rider (physics/riders.rs) and the cast key it is drawn under, when the lesson has recordings.
    key: Option<String>,
    /// Its PathController, from StartPath.
    driver: Option<Driver>,
}
impl Ai {
    /// Where it is: its rider when it rides as a full skater, else [`Ai::at`].
    fn position(&self, world: &World, data: &Data, lesson: &str, id: i64, now: f64) -> Option<Vec3> {
        let rider = self.key.as_deref().and_then(|k| world.get_resource::<Riders>()?.get(k).map(|r| r.body().position));
        rider.or_else(|| self.at(data, lesson, id, now).map(|t| t.translation))
    }
    /// Where it is: riding its lap, else standing at its locator.
    fn at(&self, data: &Data, lesson: &str, id: i64, now: f64) -> Option<Transform> {
        match (self.path, lap(data, lesson, id)) {
            (Some((start, lift)), Some(lap)) => ride(lap, (now - start) as f32, lift, self.loops),
            _ => data.spawn_point(&self.locator, id).map(|m| Transform::from_translation(position(&m))),
        }
    }
}

/// A highlight (HighlightReel.AddHighlightUsingDefinition): its photo scene and replay window, in seconds of the
/// replay recording.
#[derive(Clone)]
struct Highlight {
    scene: String,
    from: f64,
    to: f64,
}

/// State the Lua natives read and write. Locked only around single native calls and by Rust between Lua calls,
/// never across one.
#[derive(Default)]
struct Shared {
    progress: Progress,
    dirty: bool,
    requests: Vec<Request>,
    sense: Sense,
    evaluating: bool,
    objective_hud: bool,
    pause_disabled: bool,
    /// SessionMarker.DisableSetSessionMarker in force (the spot-photo lessons), published to the marker's list.
    marker_set_disabled: bool,
    indicators: Vec<String>,
    groups: BTreeMap<i64, Vec<Event>>,
    next_id: i64,
    reset_all: bool,
    reset: Vec<usize>,
    fraction: Option<f64>,
    unlocked: Vec<String>,
    warned: BTreeSet<String>,
    /// The replay recording's clock, in seconds.
    clock: f64,
    objective_count: usize,
    highlight_count: usize,
    highlights: Vec<Highlight>,
    results_title: String,
    /// Virtual seconds, for the count-up timers (id -> start).
    now: f64,
    countups: BTreeMap<i64, f64>,
    follow: crate::challenge::follow::Follow,
    /// HUD2.SetupDistanceHUDData (limit, metres), shown while the DistanceMeter is.
    distance: Option<(f64, f64)>,
    distance_shown: bool,
    /// HUD2.StartCountdown's timer (the CountdownHUD).
    countdown: Option<i64>,
    /// HUD2.StartTimer's timer, shown while the TimerHUD is (StartTimer shows it).
    hud_timer: Option<i64>,
    timer_shown: bool,
}
impl Shared {
    fn clear_lesson(&mut self) {
        (self.evaluating, self.objective_hud, self.pause_disabled, self.reset_all, self.fraction) = (false, false, false, false, None);
        self.marker_set_disabled = false;
        self.indicators.clear();
        self.groups.clear();
        self.reset.clear();
        self.sense.tricks_objective.clear();
        self.highlights.clear();
        self.results_title.clear();
        self.countups.clear();
        self.follow = default();
        (self.distance, self.distance_shown, self.countdown, self.hud_timer, self.timer_shown) = (None, false, None, None, false);
    }
    fn frame(&self) -> i64 {
        (self.clock * FRAME_HZ).round() as i64
    }
}

fn number(v: Option<&LuaValue>) -> Option<f64> {
    match v? {
        LuaValue::Integer(i) => Some(*i as f64),
        LuaValue::Number(n) => Some(*n),
        _ => None,
    }
}

fn bind<A, R, F>(lua: &Lua, table: &Table, shared: &Arc<Mutex<Shared>>, name: &str, f: F) -> mlua::Result<()>
where
    A: FromLuaMulti,
    R: IntoLuaMulti,
    F: Fn(&mut Shared, A) -> R + Send + 'static,
{
    let shared = shared.clone();
    table.set(name, lua.create_function(move |_, args: A| Ok(f(&mut shared.lock().unwrap(), args)))?)
}

fn namespace(lua: &Lua, name: &str) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    lua.globals().set(name, t.clone())?;
    Ok(t)
}

/// Registers the stub fallback over `namespaces`; call after the binding's own tables exist.
fn stubs(lua: &Lua, shared: &Arc<Mutex<Shared>>, label: &str, namespaces: &[&str]) -> mlua::Result<()> {
    let s = shared.clone();
    let label = label.to_owned();
    lua.globals().set(
        "__missing",
        lua.create_function(move |_, name: String| {
            if s.lock().unwrap().warned.insert(name.clone()) {
                warn!("Skate 2 tutorial ({label}): {name} is not implemented; it returns nil");
            }
            Ok(())
        })?,
    )?;
    lua.globals().set("__namespaces", lua.create_sequence_from(namespaces.iter().copied())?)?;
    lua.globals().set("__no_ops", lua.create_sequence_from(NO_OPS.iter().copied())?)?;
    lua.load(STUBS).set_name("@stubs").exec()
}

fn progression_natives(lua: &Lua, shared: &Arc<Mutex<Shared>>) -> mlua::Result<()> {
    let p = namespace(lua, "Progression")?;
    bind(lua, &p, shared, "IsChallengeCompleteByTitle", |s, t: String| s.progress.complete(&t))?;
    bind(lua, &p, shared, "IsChallengeActiveByTitle", |s, t: String| s.progress.active(&t))?;
    bind(lua, &p, shared, "DeactivateChallengeByTitle", |s, t: String| {
        s.progress.active.retain(|a| *a != t);
        s.dirty = true;
    })?;
    for name in ["ActivateChallenge", "ActivateChallengeByTitle"] {
        bind(lua, &p, shared, name, |s, t: String| {
            s.progress.activate(&t);
            s.dirty = true;
        })?;
    }
    bind(lua, &p, shared, "ExecuteChallengeByTitle", |s, t: String| s.requests.push(Request::Execute(t)))?;
    bind(lua, &p, shared, "SendMessage", |s, name: String| s.requests.push(Request::Message(name)))?;
    // ponytail: the HOM path settings belong to challenges outside the tutorial.
    bind(lua, &p, shared, "GetMessageSettingBool", |_, _: LuaValue| false)?;
    let unlock = namespace(lua, "Unlock")?;
    bind(lua, &unlock, shared, "UnlockCharacterForFreeskate", |s, name: String| s.unlocked.push(name))?;
    // The reminders are the camera man's lines (4004 "Rmd_Atiba").
    let speech = namespace(lua, "ChallengeSpeech")?;
    bind(lua, &speech, shared, "PlayProgressionReminder", |s, line: i64| {
        s.requests.push(Request::Speech(Event { kind: 3, a: "CameraMan".into(), b: line.to_string() }))
    })?;
    let teleport = namespace(lua, "Teleport")?;
    bind(lua, &teleport, shared, "TeleportLocalPlayerToLocationName", |s, locator: String| s.requests.push(Request::Teleport(locator)))?;
    // Round2 10-14: the hull's state, for props.rs (Slappy's gate). INFERRED: not saved by itself (the triggers end
    // TerminateWithoutSave); the next career save carries it. TestFor/Filter/AllowNonEssential: not in the tutorial.
    let hull = namespace(lua, "DynamicHull")?;
    bind(lua, &hull, shared, "SetupDynamicHullState", |s, (name, state): (String, String)| {
        s.progress.hulls.insert(name, state);
    })?;
    stubs(lua, shared, "progression", &["Progression", "Unlock", "ChallengeSpeech", "Teleport", "DynamicHull"])
}

fn lesson_natives(lua: &Lua, shared: &Arc<Mutex<Shared>>, data: &Arc<Data>) -> mlua::Result<()> {
    let online = namespace(lua, "Online")?;
    bind(lua, &online, shared, "GetLocalPlayerID", |_, ()| 0)?;

    let objectives = namespace(lua, "Objectives")?;
    bind(lua, &objectives, shared, "MakeObjectives", |_, _: LuaValue| 1)?;
    bind(lua, &objectives, shared, "SetEvaluationStatus", |s, (_, on): (LuaValue, bool)| s.evaluating = on)?;
    bind(lua, &objectives, shared, "ResetObjectives", |s, _: LuaValue| s.reset_all = true)?;
    bind(lua, &objectives, shared, "ResetObjective", |s, (_, i): (LuaValue, usize)| s.reset.push(i))?;
    bind(lua, &objectives, shared, "GetAccumulatedPoints", |s, _: LuaValue| s.sense.points.round() as i64)?;
    bind(lua, &objectives, shared, "GetCurrentNumCompletedObjectives", |s, _: LuaValue| s.sense.completed)?;
    bind(lua, &objectives, shared, "GetTotalNumObjectives", |s, _: LuaValue| s.objective_count)?;
    bind(lua, &objectives, shared, "GetNumObjectivesRequiresHighlight", |s, _: LuaValue| s.highlight_count)?;
    bind(lua, &objectives, shared, "GetFractionDenominator", |_, _: LuaValue| 1)?;
    bind(lua, &objectives, shared, "IsTrackedUniqueTrickGroup", |_, ()| false)?;
    bind(lua, &objectives, shared, "GetObjectiveFailureHALText", |_, _: LuaValue| "")?;
    // The definition GetPhotoNISCount reads back: the highlight's photo scene.
    bind(lua, &objectives, shared, "GetObjectiveNISPlaybackDefinition", |s, _: LuaValue| {
        s.highlights.last().map(|h| h.scene.clone()).unwrap_or_default()
    })?;
    let points = namespace(lua, "ObjectivePoints")?;
    bind(lua, &points, shared, "IsSequenceNewlyCompletePoints", |s, ()| s.sense.landed)?;

    let hud = namespace(lua, "HUD2")?;
    // The group's "hash" is its name: the handlers only compare it with GetGroupHash.
    bind(lua, &hud, shared, "CreateGroup", |s, name: String| s.requests.push(Request::Lesson("hudloaded", json!({"GroupHash": name}))))?;
    bind(lua, &hud, shared, "GetGroupHash", |_, name: String| name)?;
    bind(lua, &hud, shared, "Show", |s, (_, component, on): (LuaValue, String, bool)| match component.as_str() {
        "ObjectiveHUD" => s.objective_hud = on,
        "DistanceMeter" => s.distance_shown = on,
        "TimerHUD" => s.timer_shown = on,
        "CountdownHUD" if !on => s.countdown = None,
        _ => {}
    })?;
    bind(lua, &hud, shared, "SetupDistanceHUDData", |s, (_, limit, metres): (LuaValue, f64, f64)| s.distance = Some((limit, metres)))?;
    // Each points its component's data at the timer and marks it shown (0x826E1450 / 0x826E1518).
    bind(lua, &hud, shared, "StartCountdown", |s, (_, id): (LuaValue, LuaValue)| s.countdown = number(Some(&id)).map(|id| id as i64))?;
    bind(lua, &hud, shared, "StartTimer", |s, (_, id): (LuaValue, LuaValue)| {
        s.hud_timer = number(Some(&id)).map(|id| id as i64);
        s.timer_shown = true;
    })?;
    for (name, kind) in [("ShowChallengeCompleteMessage", Central::Complete), ("ShowChallengeFailedMessage", Central::Failed)] {
        bind(lua, &hud, shared, name, move |s, label: Option<String>| s.requests.extend(label.map(|l| Request::Central(l, kind))))?;
    }
    bind(lua, &hud, shared, "DeleteGroup", |s, _: LuaValue| {
        s.objective_hud = false;
        (s.distance_shown, s.countdown, s.timer_shown) = (false, None, false);
        s.requests.push(Request::HideInstruction);
    })?;
    // (label, colour, 4 format args, audio hook): the won lessons pass eMessageAudioHook.Success.
    bind(lua, &hud, shared, "ShowCentralMessage", |s, args: Variadic<LuaValue>| {
        let Some(LuaValue::String(label)) = args.first() else { return };
        let kind = match args.get(6).and_then(|h| h.as_string()).map(|h| h.to_string_lossy()).as_deref() {
            Some("Success") => Central::Complete,
            Some("Fail") => Central::Failed,
            _ => Central::Plain,
        };
        s.requests.push(Request::Central(label.to_string_lossy(), kind));
    })?;
    // ponytail: retail shows these in hudcentral's mcModifiedMessage with no audio hook; they share the plain
    // message here (its DEFAULT hook's central_message_default is silent in Skate 2's own data).
    bind(lua, &hud, shared, "ShowModifiedCentralMessage", |s, label: String| s.requests.push(Request::Central(label, Central::Plain)))?;

    let pause = namespace(lua, "Pause")?;
    bind(lua, &pause, shared, "DisablePauseMenuAndBackpack", |s, ()| s.pause_disabled = true)?;
    bind(lua, &pause, shared, "EnablePauseMenuAndBackpack", |s, ()| s.pause_disabled = false)?;
    let session_marker = namespace(lua, "SessionMarker")?;
    bind(lua, &session_marker, shared, "DisableSetSessionMarker", |s, ()| s.marker_set_disabled = true)?;
    bind(lua, &session_marker, shared, "EnableSetSessionMarker", |s, ()| s.marker_set_disabled = false)?;
    bind(lua, &pause, shared, "PauseSimulation", |s, ()| s.requests.push(Request::Simulation(false)))?;
    bind(lua, &pause, shared, "UnpauseSimulation", |s, ()| s.requests.push(Request::Simulation(true)))?;

    let indicators = namespace(lua, "VisualIndicators")?;
    bind(lua, &indicators, shared, "CreateGroup", |_, ()| 1)?;
    bind(lua, &indicators, shared, "AddIndicator", |s, (_, row): (LuaValue, Table)| {
        if let Ok(locator) = row.get::<String>("locator") {
            s.indicators.push(locator);
        }
    })?;
    bind(lua, &indicators, shared, "RemoveIndicator", |s, (_, row): (LuaValue, Table)| {
        if let Ok(locator) = row.get::<String>("locator") {
            s.indicators.retain(|l| *l != locator);
        }
    })?;
    bind(lua, &indicators, shared, "DeleteGroup", |s, _: LuaValue| s.indicators.clear())?;
    bind(lua, &indicators, shared, "ClearAllIndicators", |s, _: LuaValue| s.indicators.clear())?;

    let presentation = namespace(lua, "ChallengePresentation")?;
    bind(lua, &presentation, shared, "AddPresentationGroup", |s, _: LuaValue| {
        s.next_id += 1;
        s.groups.insert(s.next_id, Vec::new());
        s.next_id
    })?;
    bind(lua, &presentation, shared, "AddPresentationEvent", |s, (group, row): (i64, Table)| {
        let event = Event {
            kind: row.get("type").unwrap_or(0),
            a: row.get::<Option<String>>("a").ok().flatten().unwrap_or_default(),
            b: row.get::<Option<String>>("b").ok().flatten().unwrap_or_default(),
        };
        s.groups.entry(group).or_default().push(event);
    })?;
    bind(lua, &presentation, shared, "RemoveAllPresentationGroups", |s, _: LuaValue| s.groups.clear())?;
    bind(lua, &presentation, shared, "StartPresentation", |s, group: i64| s.requests.push(Request::Present(group)))?;

    let ai = namespace(lua, "AI")?;
    let d = data.clone();
    bind(lua, &ai, shared, "SkaterDistanceFromLocation", move |s, (_, locator): (LuaValue, String)| {
        d.locator(&locator).map_or(f64::MAX, |m| position(&m).distance(s.sense.position) as f64)
    })?;
    // The player (id 0, Online.GetLocalPlayerID) teleports; a competitor moves to the locator, off its path.
    bind(lua, &ai, shared, "RepositionAI", |s, (id, locator): (Option<i64>, String)| {
        s.requests.push(match id.unwrap_or(0) {
            0 => Request::Teleport(locator),
            id => Request::Reposition(id, locator),
        })
    })?;
    // The race template's path count (one per competitor here too).
    bind(lua, &ai, shared, "LoadRaceHeatPaths", |_, _: LuaValue| 1)?;
    bind(lua, &ai, shared, "GetPathCount", |_, ()| 1)?;
    // The competitors: FilmAIID i is self.Competitors[i - 1], skating ai_paths[lesson][i - 1] (the paths are
    // tutorial.json's; LoadPaths has nothing to load). ponytail: PinAI holds no one: a lap only starts on StartPath.
    bind(lua, &ai, shared, "LoadPaths", |_, _: LuaValue| 0)?;
    bind(lua, &ai, shared, "SpawnAIAtLocation", |s, (id, locator): (i64, String)| s.requests.push(Request::Ai(id, Some(locator))))?;
    bind(lua, &ai, shared, "UnspawnAI", |s, id: i64| s.requests.push(Request::Ai(id, None)))?;
    bind(lua, &ai, shared, "StartPath", |s, id: i64| s.requests.push(Request::StartPath(id)))?;
    let entity = namespace(lua, "Entity")?;
    bind(lua, &entity, shared, "DoesSkaterExist", |_, _: LuaValue| true)?;

    let challenge = namespace(lua, "Challenge")?;
    bind(lua, &challenge, shared, "WinChallenge", |s, _: LuaValue| s.requests.push(Request::Outcome(Outcome::Win)))?;
    bind(lua, &challenge, shared, "LoseChallenge", |s, _: LuaValue| s.requests.push(Request::Outcome(Outcome::Lose)))?;
    bind(lua, &challenge, shared, "AbandonChallenge", |s, _: LuaValue| s.requests.push(Request::Outcome(Outcome::Abandon)))?;
    bind(lua, &challenge, shared, "CompleteChallenge", |s, _: LuaValue| s.requests.push(Request::Complete))?;
    bind(lua, &challenge, shared, "GetChallengeType", |_, this: Table| this.get::<i64>("ChallengeType").unwrap_or(0))?;
    bind(lua, &challenge, shared, "IsReplayingChallenge", |_, ()| false)?;
    // Challenge.GameMode.SinglePlayer
    bind(lua, &challenge, shared, "GetGameMode", |_, ()| 0)?;
    let race = namespace(lua, "Race")?;
    // ponytail: the results add it to an unfinished competitor's time, which the port doesn't show.
    bind(lua, &race, shared, "GetEstimatedTimeToFinish", |_, _: LuaValue| 0)?;
    let follow = namespace(lua, "FollowTheLeader")?;
    bind(lua, &follow, shared, "SetActorIDs", |s, leader: i64| s.follow.leader = leader)?;
    // [min, max, warning speaker, line, followee, recovery speaker, line, delay, repeat]
    bind(lua, &follow, shared, "SetWarningInfo", |s, row: Table| {
        s.follow.info = row.sequence_values::<f64>().map(|v| v.unwrap_or(0.)).collect();
    })?;
    for (i, name) in crate::challenge::follow::GETTERS.into_iter().enumerate() {
        bind(lua, &follow, shared, name, move |s, ()| s.follow.info.get(i).copied().unwrap_or(0.))?;
    }
    bind(lua, &follow, shared, "RegisterDistanceCallback", |s, (_, metres): (LuaValue, f64)| {
        s.next_id += 1;
        s.follow.callbacks.push((s.next_id, metres, true));
        s.next_id
    })?;
    bind(lua, &follow, shared, "UnregisterDistanceCallback", |s, id: LuaValue| {
        if let Some(id) = number(Some(&id)) {
            s.follow.callbacks.retain(|c| c.0 != id as i64);
        }
    })?;
    bind(lua, &follow, shared, "StartDistanceUpdates", |s, ()| s.follow.updating = true)?;
    bind(lua, &follow, shared, "StopDistanceUpdates", |s, ()| s.follow.updating = false)?;
    bind(lua, &follow, shared, "GetDistance", |s, ()| s.follow.distance.round() as i64)?;
    let save = namespace(lua, "ChallengeSave")?;
    bind(lua, &save, shared, "GetSaveData", |s, this: Table| {
        this.get::<String>("Name").ok().and_then(|n| s.progress.scores.get(&n).copied()).unwrap_or(0)
    })?;
    bind(lua, &save, shared, "SetSaveData", |s, (this, _, value): (Table, LuaValue, f64)| {
        if let Ok(name) = this.get::<String>("Name") {
            s.progress.scores.insert(name, value.round() as i64);
            s.dirty = true;
        }
    })?;
    let speech = namespace(lua, "ChallengeSpeech")?;
    for name in ["PlayNPCSpeechUsingOverrideAttribute", "PlayNPCSpeechUsingAttribute"] {
        bind(lua, &speech, shared, name, |s, attribute: LuaValue| {
            let LuaValue::Table(attribute) = attribute else { return };
            let (who, line) = (attribute.get::<i64>(1).unwrap_or(0), attribute.get::<i64>(2).unwrap_or(0) >> 16);
            match SPEAKERS.iter().find(|(id, _)| *id == who) {
                Some((_, bank)) if line != 0 => s.requests.push(Request::Speech(Event { kind: 3, a: (*bank).into(), b: line.to_string() })),
                Some(_) => {}
                None => debug!("Skate 2 tutorial: no speech bank for speaker {who} (line {line})"),
            }
        })?;
    }
    let replay = namespace(lua, "ChallengeReplay")?;
    bind(lua, &replay, shared, "GetLastFrameNumberWritten", |s, ()| s.frame())?;
    // The race template restarts a heat with it: retail drops the DataMan ring (Replay Editor plan, Task 3).
    bind(lua, &replay, shared, "ResetReplayBuffer", |s, _: LuaValue| s.requests.push(Request::ResetReplay))?;
    let time = namespace(lua, "Time")?;
    bind(lua, &time, shared, "GetVisualGameTick", |s, ()| s.frame())?;
    let reel = namespace(lua, "HighlightReel")?;
    bind(lua, &reel, shared, "CreateHighlightGroup", |_, _: LuaValue| 1)?;
    // (group, definition, killed, end frame[, sequence start frame, _, _, seconds before, seconds after])
    bind(lua, &reel, shared, "AddHighlightUsingDefinition", |s, args: Variadic<LuaValue>| {
        let scene = match (args.get(1), args.get(2)) {
            (Some(LuaValue::Table(d)), Some(LuaValue::Boolean(true))) => d.get::<String>(21),
            (Some(LuaValue::Table(d)), _) => d.get::<String>(5),
            _ => Ok(String::new()),
        };
        let end = number(args.get(3)).unwrap_or(s.clock * FRAME_HZ) / FRAME_HZ;
        let (before, after) = (number(args.get(7)).unwrap_or(0.5), number(args.get(8)).unwrap_or(0.5));
        let begin = number(args.get(4)).map_or(end - 3., |f| f / FRAME_HZ);
        s.highlights.push(Highlight { scene: scene.unwrap_or_default(), from: begin - before, to: end + after });
        s.highlights.len()
    })?;
    bind(lua, &reel, shared, "PlayHighlightReel", |s, _: LuaValue| s.requests.push(Request::Reel))?;
    for name in ["RemoveAllHighlights", "DestroyHighlightGroup"] {
        bind(lua, &reel, shared, name, |s, _: LuaValue| s.highlights.clear())?;
    }
    let nis = namespace(lua, "NIS")?;
    let d = data.clone();
    bind(lua, &nis, shared, "GetPhotoNISCount", move |_, scene: Option<String>| {
        scene.and_then(|scene| d.scenes.get(&scene)).map_or(0, |s| s.photos)
    })?;
    bind(lua, &nis, shared, "KillNIS", |s, ()| s.requests.push(Request::KillNis))?;
    let frontend = namespace(lua, "FrontEnd")?;
    bind(lua, &frontend, shared, "OpenPhotoSelection", |s, _: LuaValue| s.requests.push(Request::Photos))?;
    bind(lua, &frontend, shared, "Results_SetMainTitle", |s, label: String| s.results_title = label)?;
    bind(lua, &frontend, shared, "OpenContinueRetryDialog", |s, won: Option<bool>| s.requests.push(Request::ContinueRetry(won.unwrap_or(false))))?;
    let asset = namespace(lua, "ChallengeAsset")?;
    bind(lua, &asset, shared, "IsChallengeAssetLoadComplete", |_, _: LuaValue| true)?;
    let timer = namespace(lua, "ChallengeTimer")?;
    bind(lua, &timer, shared, "AddCountdownTimer", |s, seconds: f64| {
        s.next_id += 1;
        s.requests.push(Request::Timer(s.next_id, seconds));
        s.next_id
    })?;
    bind(lua, &timer, shared, "AddHUDCountdownTimer", |s, seconds: f64| {
        s.next_id += 1;
        s.requests.push(Request::Timer(s.next_id, seconds));
        s.next_id
    })?;
    bind(lua, &timer, shared, "RemoveCountdownTimer", |s, id: LuaValue| {
        if let Some(id) = number(Some(&id)) {
            s.requests.push(Request::RemoveTimer(id as i64));
        }
    })?;
    bind(lua, &timer, shared, "RemoveHUDCountdownTimer", |s, id: LuaValue| {
        if let Some(id) = number(Some(&id)) {
            s.requests.push(Request::RemoveTimer(id as i64));
        }
    })?;
    bind(lua, &timer, shared, "AddCountupTimer", |s, ()| {
        s.next_id += 1;
        s.countups.insert(s.next_id, s.now);
        s.next_id
    })?;
    bind(lua, &timer, shared, "GetCountupTime", |s, id: LuaValue| {
        number(Some(&id)).and_then(|id| s.countups.get(&(id as i64))).map_or(0., |start| s.now - start)
    })?;
    // ponytail: only bonus time reads it, and the tutorial's challenges give none.
    bind(lua, &timer, shared, "GetCountdownTime", |_, _: LuaValue| 0)?;

    // Objective conditions.
    let locomotion = namespace(lua, "ObjectiveLocomotion")?;
    bind(lua, &locomotion, shared, "IsOnBoard", |s, ()| s.sense.on_board)?;
    bind(lua, &locomotion, shared, "IsOffBoard", |s, ()| !s.sense.on_board)?;
    bind(lua, &locomotion, shared, "IsMoving", |s, ()| s.sense.speed > MOVING)?;
    bind(lua, &locomotion, shared, "IsStoppped", |s, ()| s.sense.speed < STOPPED)?;
    bind(lua, &locomotion, shared, "SequenceCompleted", |s, ()| s.sense.landed)?;
    let trigger = namespace(lua, "ObjectiveTrigger")?;
    bind(lua, &trigger, shared, "EnteredVolume", |s, (_, path): (LuaValue, String)| {
        let entered = if s.sense.scoped { &s.sense.visited } else { &s.sense.entered };
        entered.contains(volume_name(&path))
    })?;
    bind(lua, &trigger, shared, "IsInVolume", |s, (_, path): (LuaValue, String)| s.sense.inside.contains(volume_name(&path)))?;
    let tricks = namespace(lua, "Tricks")?;
    bind(lua, &tricks, shared, "DidSpecificTrick", |s, name: String| s.sense.tricks().contains(name.as_str()))?;
    bind(lua, &tricks, shared, "DidSpecificTrickThisFrame", |s, name: String| s.sense.tricks_frame.contains(&name.as_str()))?;
    let d = data.clone();
    bind(lua, &tricks, shared, "DidSpecificTrickGroup", move |s, group: String| {
        d.trick_groups.get(&group).is_some_and(|g| g.iter().any(|t| s.sense.tricks().contains(t.as_str())))
    })?;
    let grind = namespace(lua, "Grind")?;
    let d = data.clone();
    bind(lua, &grind, shared, "DidSpecificGrind", move |s, id: LuaValue| {
        let key = match id {
            LuaValue::Integer(i) => i.to_string(),
            LuaValue::Number(n) => (n as i64).to_string(),
            _ => String::new(),
        };
        // ponytail: an id the data doesn't name stands for any grind.
        let names = d.grinds.get(&key).or_else(|| d.trick_groups.get("grinds"));
        names.is_some_and(|g| g.iter().any(|t| s.sense.tricks().contains(t.as_str())))
    })?;
    bind(lua, &grind, shared, "GetTotalGrindLengthInSequence", |s, ()| s.sense.grind_metres)?;
    let d = data.clone();
    bind(lua, &tricks, shared, "DidSpecificTrickGroupThisFrame", move |s, group: String| {
        d.trick_groups.get(&group).is_some_and(|g| g.iter().any(|t| s.sense.tricks_frame.contains(&t.as_str())))
    })?;
    let wipeout = namespace(lua, "Wipeout")?;
    bind(lua, &wipeout, shared, "IsWipingOut", |s, ()| s.sense.wiping)?;
    let hom = namespace(lua, "ObjectiveHOM")?;
    bind(lua, &hom, shared, "GetWipeoutDuration", |s, ()| s.sense.wipe_seconds)?;
    let manual = namespace(lua, "Manual")?;
    bind(lua, &manual, shared, "IsManualing", |s, ()| s.sense.holding.iter().any(|h| *h == "nosemanual" || *h == "tailmanual"))?;
    bind(lua, &manual, shared, "NoseManualTime", |s, ()| s.sense.nose_seconds)?;
    bind(lua, &manual, shared, "TailManualTime", |s, ()| s.sense.tail_seconds)?;
    let marker = namespace(lua, "ObjectiveSessionMarker")?;
    bind(lua, &marker, shared, "JustSetSessionMarker", |s, ()| s.sense.marker_set)?;
    bind(lua, &marker, shared, "JustUsedSessionMarker", |s, ()| s.sense.marker_used)?;
    let info = namespace(lua, "ObjectiveInfo")?;
    bind(lua, &info, shared, "GetTotalNumCompletedObjectives", |s, _: LuaValue| s.sense.completed)?;
    let game_info = namespace(lua, "ObjectiveGameInfo")?;
    bind(lua, &game_info, shared, "SetFractionValue", |s, value: f64| s.fraction = Some(value))?;
    // The lesson's DMO props (physics/props.rs, dmos.json). A tester is its DMO's instance id, which is past i64 and
    // reaches Lua as a float; the handle is that float. A DMO with no prop (no Skate 3 model) is never ready and stays
    // "in" every volume, as before the props: Meet Slappy's exit volume still completes its objective.
    // ponytail: the prop's centre stands for the DMO's origin in the volume test.
    let dmos = namespace(lua, "DMOs")?;
    bind(lua, &dmos, shared, "AddDMOVolumeTester", |_, id: LuaValue| number(Some(&id)).unwrap_or(0.))?;
    fn tester(s: &Shared, handle: Option<f64>) -> Option<&BTreeSet<String>> {
        s.sense.dmos.iter().find(|(id, _)| Some(*id as f64) == handle).map(|(_, inside)| inside)
    }
    bind(lua, &dmos, shared, "IsDMOInVolume", |s, (handle, _, path): (LuaValue, LuaValue, String)| {
        tester(s, number(Some(&handle))).is_none_or(|inside| inside.contains(volume_name(&path)))
    })?;
    bind(lua, &dmos, shared, "IsDMOTesterReady", |s, handle: LuaValue| tester(s, number(Some(&handle))).is_some())?;
    bind(lua, &dmos, shared, "IsGrabbingDMO", |s, ()| s.sense.grabbing)?;

    stubs(
        lua,
        shared,
        "lesson",
        &[
            "Online", "Objectives", "HUD2", "Pause", "VisualIndicators", "ChallengePresentation", "AI", "Challenge",
            "ChallengeAsset", "ChallengeTimer", "ObjectiveLocomotion", "ObjectiveTrigger", "Tricks", "Wipeout",
            "ObjectiveHOM", "Manual", "ObjectiveSessionMarker", "SessionMarker", "ObjectiveInfo", "ObjectiveGameInfo", "DMOs",
            "ObjectivePoints", "Entity", "ChallengeSave", "ChallengeSpeech", "ChallengeReplay", "Time", "HighlightReel",
            "NIS", "FrontEnd", "Grind", "Race", "FollowTheLeader",
        ],
    )
}

impl Sense {
    /// Tricks an objective sees: the sequence's for a points-gated one, else everything since it began.
    fn tricks(&self) -> &BTreeSet<&'static str> {
        if self.scoped { &self.tricks_sequence } else { &self.tricks_objective }
    }
}

// ---- A running lesson ----

struct Hint {
    condition: Condition,
    events: Vec<Event>,
    was: bool,
}
struct Objective {
    text: String,
    condition: Condition,
    hints: Vec<Hint>,
    intro: Vec<Event>,
    complete: Vec<Event>,
    indicators: Vec<String>,
    done: bool,
    /// PointsValid: the objective completes only on an evaluation where this holds too.
    points: Option<Condition>,
    highlight: bool,
}
struct Lesson {
    name: String,
    machine: Machine,
    objectives: Vec<Objective>,
    /// ObjectiveFlow `[1, 2]` (pumping): objective 0 counts the others and ends the lesson, while the others run in
    /// order. `[1, 1]`: every objective runs in order.
    master: Option<usize>,
    order: Vec<usize>,
    /// Index into `order` of the objective being evaluated; None until evaluation first starts.
    position: Option<usize>,
    intro: Vec<Event>,
    intro_played: bool,
    finished: bool,
    outcome: Option<Outcome>,
}
impl Lesson {
    fn new(data: &Arc<Data>, name: &str, shared: &Arc<Mutex<Shared>>) -> Result<Self, String> {
        let c = data.challenges.get(name).ok_or_else(|| format!("no lesson {name} in tutorial.json"))?;
        let path = data.root.join("scripts").join(format!("{name}.lua"));
        let source = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut fields = c.fields.clone();
        // The handlers read `.Count` of these unguarded; the disc leaves some null.
        for key in ["VisualIndicators", "OutroPresentationEvents", "IntroPresentationEvents"] {
            if fields[key].is_null() {
                fields[key] = json!([]);
            }
        }
        if fields["Objectives"].is_null() {
            fields["Objectives"] = Value::Array(c.objectives.clone());
        }
        let script = Script { name, source: &source, steps: c.steps.clone(), initial: "initial", enter: "stateenter", exit: "stateexit" };
        let machine = Machine::new(script, &fields, |lua| lesson_natives(lua, shared, data))?;
        // The spot-photo template adds to vars.StopFrame before anything in it sets it (the reel would never start).
        machine.vars().set("StopFrame", 0).map_err(|e| e.to_string())?;
        let lua = &machine.lua;
        let mut objectives = Vec::new();
        for raw in &c.objectives {
            let o: ObjectiveData = serde_json::from_value(raw.clone()).map_err(|e| format!("{name}: an objective: {e}"))?;
            // An objective and its hints share one vars table (the hints read what the definition initialises).
            let vars = lua.create_table().map_err(|e| e.to_string())?;
            let hints = o
                .hints
                .iter()
                .map(|h| {
                    let condition = Condition::compile_with(lua, &h.name, &h.definition.source, vars.clone())?;
                    Ok(Hint { condition, events: h.events.clone(), was: false })
                })
                .collect::<Result<_, String>>()?;
            objectives.push(Objective {
                text: fill(&data.text(&o.definition.text), o.definition.words.get(4).and_then(Value::as_str).unwrap_or("")),
                condition: Condition::compile_with(lua, &o.name, &o.definition.source, vars)?,
                hints,
                intro: o.intro_events.clone(),
                complete: o.complete_events.clone(),
                indicators: o.visual_indicators.iter().map(|v| v.locator.clone()).collect(),
                done: false,
                // Most objectives carry a no-op `return true`: not points-gated (their hints, Push/Turn, fire on edges).
                points: match o.points_valid[0].as_str().filter(|s| {
                    !matches!(s.split_whitespace().collect::<Vec<_>>()[..], [] | ["do", "return", "true;", "end"])
                }) {
                    Some(source) => {
                        let vars = lua.create_table().map_err(|e| e.to_string())?;
                        Some(Condition::compile_with(lua, &format!("{}_points", o.name), source, vars)?)
                    }
                    None => None,
                },
                highlight: o.requires_highlight,
            });
        }
        {
            let mut s = shared.lock().unwrap();
            s.objective_count = objectives.len();
            s.highlight_count = objectives.iter().filter(|o| o.highlight).count();
        }
        let master = (c.fields["ObjectiveFlow"][1].as_i64() == Some(2) && objectives.len() > 1).then_some(0);
        let order = (0..objectives.len()).filter(|i| Some(*i) != master).collect();
        // The spot-photo template plays its intro itself (nis_overview).
        let intro = if source.contains("self.IntroPresentationEvents") {
            Vec::new()
        } else {
            serde_json::from_value(fields["IntroPresentationEvents"].clone()).unwrap_or_default()
        };
        Ok(Self {
            name: name.to_owned(),
            machine,
            objectives,
            master,
            order,
            position: None,
            intro,
            intro_played: false,
            finished: false,
            outcome: None,
        })
    }
    fn current(&self) -> Option<usize> {
        self.position.and_then(|p| self.order.get(p).copied())
    }
    fn reset(&mut self, i: usize) -> Result<(), String> {
        let Some(o) = self.objectives.get_mut(i) else { return Ok(()) };
        o.condition.reset()?;
        o.done = false;
        o.hints.iter_mut().for_each(|h| h.was = false);
        Ok(())
    }
}

// ---- Presentation ----

enum Item {
    Event(Event),
    Movie(String),
    Customiser,
    /// The frontend is over: save that, then start this progression trigger.
    FrontendDone(String),
    /// An event for the running lesson, once everything queued before it has played.
    Lesson(&'static str, Value),
    /// An event for every running progression trigger.
    Triggers(&'static str),
    /// A popup; option `yes_at` answers `yes`, any other (or B) `no`: to the progression triggers, or to the
    /// running lesson.
    Dialog { popup: Popup, yes: &'static str, no: Option<&'static str>, yes_at: usize, lesson: bool },
    Start(String),
    Reel,
    Photos,
    /// A lesson's sign-up poster opening (true) or accepted (false), each at a white bloom.
    Poster(&'static str, bool),
    /// "Show Unlocks Screen" (type 1, id 10): FE state 10's UnlockOverlay with the characters this win unlocked.
    Unlocks(Vec<String>),
    /// A type 5 message: FE state 79's Skate 101 overlay, the skater pinned.
    Skate101 { title: String, text: String },
}
enum Playing {
    Wait(f64),
    /// `length` real seconds from `start` (its clock runs at its TIME_SCALE: nis-clock.md); subtitles on its own time.
    Scene { start: Instant, length: f64, cues: Vec<Cue>, cutscene: bool, hides_player: bool },
    Teleport { from: f64 },
    Movie { movie: Movie, cues: Vec<Cue>, start: Instant },
    Customiser,
    Dialog { yes: &'static str, no: Option<&'static str>, yes_at: usize, lesson: bool },
    /// A highlight reel: the replay at `at` (recording seconds) under its photo scene, `left` real seconds of it to go,
    /// `clock` the real time it last ran.
    Reel { at: f64, left: f64, clock: f64 },
    /// Presentation event 0x13: the last seconds of play replayed under a highlight scene's camera, from `at` to `to`
    /// (recording seconds), `clock` the real time it last ran.
    Highlight { at: f64, to: f64, clock: f64 },
    /// FrontEnd.OpenPhotoSelection: the player picks one of the reel's photos (`forward`: the last step was "next").
    Photos { index: usize, forward: bool },
    /// A cutscene's flash rising to white; the cutscene's event is next in the queue.
    Flash,
    /// The sign-up poster, gameplay held: A accepts, B declines, X plays the lesson's tutorial movie.
    SignUp(&'static str),
    /// FE state 10, UnlockOverlay up: A posts cMsgHUDContinue once `ready` (the state's +80 flag, Step 1a).
    Unlocks { ready: bool },
    /// FE state 79, Skate101 up: A posts cMsgHUDContinue.
    Skate101,
}

/// NISManager's BloomToWhite transition around a cutscene (0x826f3b88 -> 0x82666840): the screen fade
/// ([`crate::retail_render::ScreenFade`]) ramps linearly over a second of real time each way, and the cut happens at
/// white.
struct Flash {
    rising: bool,
    start: Instant,
}
const FLASH_SECONDS: f32 = 1.;
impl Flash {
    fn level(&self) -> f32 {
        let t = (self.start.elapsed().as_secs_f32() / FLASH_SECONDS).min(1.);
        if self.rising { t } else { 1. - t }
    }
    /// A ramp toward white (`rising`) or back, from wherever `from` had got to.
    fn ramp(from: Option<&Flash>, rising: bool) -> Flash {
        let level = from.map_or(if rising { 0. } else { 1. }, Flash::level);
        let done = if rising { level } else { 1. - level };
        let start = Instant::now();
        Flash { rising, start: start.checked_sub(std::time::Duration::from_secs_f32(done * FLASH_SECONDS)).unwrap_or(start) }
    }
    fn white(&self) -> bool {
        self.rising && self.level() >= 1.
    }
}
enum Delivery {
    Lesson(&'static str, Value),
    Triggers(&'static str),
    StartTrigger(String),
    StartLesson(String),
}

#[derive(Default)]
struct Hud {
    objective: Option<String>,
    instruction: Option<String>,
    /// The instruction's gesture animation (presentation event 0x14's first argument).
    gesture: String,
    /// "Challenge Complete!" over the lesson it is for, until a time.
    central: Option<(String, f64)>,
    central_title: String,
    central_kind: Central,
    /// A starting lesson's title and description, until a time.
    card: Option<(String, String, f64)>,
    subtitle: Option<String>,
    dialog: Option<Popup>,
    prompt: Option<String>,
    /// The lesson whose sign-up poster is up.
    signup: Option<&'static str>,
    /// UnlockOverlay's characters (their text labels) while it is up.
    unlocks: Option<Vec<String>>,
    /// Skate101's (title, text) while it is up.
    skate101: Option<(String, String)>,
}

/// What the pause menu shows: Learn > Skate 2 Tutorial when `available`; `busy` keeps the menu shut (movies,
/// dialogs, lesson steps that disable pausing). While the tutorial runs it also holds the shared
/// `challenge::LessonMenu`, whose Abandon is the spec's EXIT TUTORIAL.
#[derive(Resource, Default)]
pub(crate) struct TutorialMenu {
    pub available: bool,
    pub busy: bool,
}
#[derive(Message, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TutorialCommand {
    Start,
    /// Erase progress, then Start: the frontend movies play again.
    StartOver,
    Exit,
}

/// Tricks and manual time collected on fixed ticks between two evaluations.
#[derive(Resource, Default)]
struct TrickLog {
    tricks: Vec<&'static str>,
    landed: bool,
    holding: Vec<&'static str>,
    nose: f32,
    tail: f32,
    /// The trick sequence: running now; begun since the last evaluation; its tricks, metres ground and (once
    /// landed) score.
    sequence: bool,
    begun: bool,
    sequence_tricks: Vec<&'static str>,
    grind: f32,
    points: f32,
}

/// The reel's photos (screenshots at half size).
#[derive(Resource, Default)]
struct Shots(Vec<Handle<Image>>);

#[derive(Resource, Default)]
struct Tutorial {
    /// The player's skater is hidden for a scene without it (show_player).
    player_hidden: bool,
    /// skate2_arrows' list is this tutorial's ([`write_guides`]).
    owns_arrows: bool,
    cache: Option<Arc<Data>>,
    data: Option<Arc<Data>>,
    loaded: Option<(u64, bool)>,
    settings: PathBuf,
    shared: Arc<Mutex<Shared>>,
    triggers: Vec<Machine>,
    lesson: Option<Lesson>,
    queue: VecDeque<Item>,
    playing: Option<Playing>,
    pending: VecDeque<Delivery>,
    hud: Hud,
    audio: Vec<Entity>,
    timers: Vec<(i64, f64)>,
    /// Timers the lesson removed, with the seconds they had left: a HUD still showing one holds there.
    stopped: Vec<(i64, f64)>,
    lesson_teleport: Option<f64>,
    now: f64,
    next_evaluation: f64,
    last_evaluation: f64,
    last_inside: BTreeSet<String>,
    markers: Option<(u64, u64)>,
    paused_time: bool,
    /// Locator of the minimap waypoint (event 0x1E): a minimap marker only, as on the disc.
    waypoint: Option<String>,
    /// Presentation event 6 (cMsgPinPlayer) holds the skater in place until event 7.
    pinned: bool,
    /// Pros placed by progression spawn messages (type 8), by cast key -> locator.
    pros: BTreeMap<String, String>,
    screens: TutorialScreens,
    /// This binding set `challenge::LessonMenu::active` (shared with the Skate 1 port).
    holds_lesson_menu: bool,
    /// Pause.PauseSimulation paused virtual time.
    sim_paused: bool,
    /// The lesson's competitors by AI id.
    ai: BTreeMap<i64, Ai>,
    /// The cutscene flash, until it has fallen back.
    flash: Option<Flash>,
}

pub(crate) struct Skate2TutorialPlugin;
impl Plugin for Skate2TutorialPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Tutorial>()
            .init_resource::<TutorialMenu>()
            .init_resource::<crate::skate_audio::CutsceneHold>()
            .init_resource::<crate::challenge::LessonMenu>()
            .init_resource::<TrickLog>()
            .init_resource::<Shots>()
            .add_message::<TutorialCommand>()
            .add_systems(PostStartup, setup_ui)
            .add_systems(FixedUpdate, collect.after(crate::app::SimulationSet::Physics))
            .add_systems(
                FixedUpdate,
                drive.after(crate::app::SimulationSet::Physics).before(crate::physics::riders::advance),
            )
            .add_systems(Update, run.after(crate::app::FrameSet::Physics));
    }
}

fn collect(mut log: ResMut<TrickLog>, skater: Res<SkaterRuntime>, time: Res<Time>, tutorial: Res<Tutorial>) {
    if tutorial.lesson.is_none() {
        return;
    }
    let s = &skater.scoring;
    if s.sequence_open() && !log.sequence {
        (log.begun, log.grind) = (true, 0.);
        log.sequence_tricks.clear();
    }
    log.sequence = s.sequence_open();
    log.tricks.extend(s.announced.iter().copied());
    log.sequence_tricks.extend(s.announced.iter().copied());
    log.landed |= s.sequence_landed;
    if s.sequence_landed {
        log.points = s.sequence_score;
    }
    log.holding = s.holding().collect();
    let dt = time.delta_secs();
    log.nose = if log.holding.contains(&"nosemanual") { log.nose + dt } else { 0. };
    log.tail = if log.holding.contains(&"tailmanual") { log.tail + dt } else { 0. };
    // Grind length: the distance ridden while a grind is held.
    let grinds = tutorial.data.as_ref().and_then(|d| d.trick_groups.get("grinds"));
    if grinds.is_some_and(|g| log.holding.iter().any(|h| g.iter().any(|n| n == h))) {
        let v = skater.player_input.physical.skateboard.vector_80.map(f32::from_bits);
        log.grind += Vec3::new(v[0], v[1], v[2]).length() * dt;
    }
}

pub(crate) fn run(world: &mut World) {
    world.resource_scope(|world, mut tutorial: Mut<Tutorial>| {
        tutorial.reload(world);
        let mut commands: Vec<_> = world.resource_mut::<bevy::ecs::message::Messages<TutorialCommand>>().drain().collect();
        if tutorial.holds_lesson_menu {
            use crate::challenge::{LessonMenu, LessonRequest};
            match world.get_resource_mut::<LessonMenu>().and_then(|mut m| m.request.take()) {
                Some(LessonRequest::Abandon) => commands.push(TutorialCommand::Exit),
                Some(LessonRequest::Restart) => tutorial.pending.push_back(Delivery::Lesson("restartchallenge", Value::Null)),
                None => {}
            }
        }
        if tutorial.data.is_some() {
            for command in commands {
                tutorial.command(world, command);
            }
            tutorial.frame(world);
        }
        // G8: the running lesson's DMOs are solid, grabbable and drawn; everyone else's are not. So are the career's
        // DynamicHull rows (Slappy's gate while "slappys" is Locked).
        let lesson = tutorial.lesson.as_ref().map(|l| l.name.to_string());
        if let Some(mut physics) = world.get_resource_mut::<GamePhysics>() {
            let shared = tutorial.shared.lock().unwrap();
            physics.stage_props(lesson.as_deref(), career_hulls(tutorial.data.is_some(), &shared.progress));
        }
        tutorial.publish(world);
    });
    world.flush();
}

impl Tutorial {
    fn reload(&mut self, world: &mut World) {
        let map = world.resource::<CurrentMap>();
        // Joining or leaving a session reloads too: the tutorial is single player only, never online, and never in a
        // Party Play session, whose modes draw their own arrows (user 11-01: Meet Slappy's sign-up took S.K.A.T.E.'s).
        // Retail's party game runs no career (its mode mask never loads the career's hulls, career_hulls).
        let key = (map.generation, crate::multiplayer::engaged(world) || crate::party_play::in_session(world));
        if self.loaded == Some(key) {
            return;
        }
        let on_map = map.path.as_deref().and_then(Path::file_stem).is_some_and(|s| s == MAP);
        self.stop(world);
        self.loaded = Some(key);
        self.data = None;
        if !on_map || key.1 {
            return;
        }
        let asset_root = world.resource::<Config>().asset_root.clone();
        if self.cache.is_none() {
            match Data::load(asset_root.join(DIR)) {
                Ok(data) => self.cache = Some(Arc::new(data)),
                Err(error) => {
                    info!("Skate 2 tutorial unavailable ({error}). Prepare it with skate2port/old_tutorial.py");
                    return;
                }
            }
        }
        self.data = self.cache.clone();
        self.settings = crate::saves::settings_dir(&asset_root).join("skate2-tutorial.json");  // Not retail (user 10-13)
        let progress = Progress::load(&self.settings).unwrap_or_default();
        let (skipped, frontend_done) = (progress.skipped, progress.frontend_done);
        self.shared.lock().unwrap().progress = progress;
        if skipped {
            return;
        }
        if frontend_done {
            // Retail runs it on every load: it re-locks what the tutorial hasn't unlocked and, before Meet Slappy is
            // won, replays the opening and its question.
            self.pending.push_back(Delivery::StartTrigger(START_MACHINE.into()));
        } else {
            self.queue_frontend();
        }
    }

    fn queue_frontend(&mut self) {
        let Some(data) = self.data.clone() else { return };
        for step in &data.frontend {
            if let Some(movie) = step["movie"].as_str() {
                self.queue.push_back(Item::Movie(movie.to_owned()));
            } else if step["customiser"].as_bool() == Some(true) {
                self.queue.push_back(Item::Customiser);
            } else if let Some(trigger) = step["trigger"].as_str() {
                self.queue.push_back(Item::FrontendDone(trigger.to_owned()));
            }
        }
    }

    /// Ends everything running; progress stays.
    /// Shows or hides the player's skater (a scene without it, [`Scene::hides_player`]); once per change.
    fn show_player(&mut self, world: &mut World, show: bool) {
        if self.player_hidden == show {
            self.player_hidden = !show;
            let to = if show { Visibility::Inherited } else { Visibility::Hidden };
            for mut v in world.query_filtered::<&mut Visibility, With<crate::world::PlayerRoot>>().iter_mut(world) {
                *v = to;
            }
        }
    }
    fn stop(&mut self, world: &mut World) {
        self.show_player(world, true);
        if let Some(Playing::Movie { movie, .. }) = self.playing.take() {
            world.resource_scope(|world, mut images: Mut<Assets<Image>>| movie.stop(&mut world.commands(), &mut images));
        }
        if std::mem::take(&mut self.paused_time) | std::mem::take(&mut self.sim_paused) {
            world.resource_mut::<Time<Virtual>>().unpause();
        }
        self.stop_audio(world);
        // Only the tutorial's own scene: a map load resets it while another mode's intro may be starting.
        if self.data.as_ref().is_some_and(|d| world.resource::<Cutscene>().plays_from(&d.root)) {
            world.resource_mut::<Cutscene>().stop();
        }
        if self.flash.take().is_some() {
            if let Some(mut fade) = world.get_resource_mut::<crate::retail_render::ScreenFade>() {
                fade.0 = 0.;
            }
        }
        close_replay(world);
        self.lesson = None;
        self.triggers.clear();
        self.queue.clear();
        self.pending.clear();
        self.timers.clear();
        self.stopped.clear();
        self.lesson_teleport = None;
        self.waypoint = None;
        self.pinned = false;
        self.pros.clear();
        self.ai.clear();
        self.screens.clear(world);
        self.hud = Hud::default();
        self.shared.lock().unwrap().clear_lesson();
    }

    fn stop_audio(&mut self, world: &mut World) {
        for entity in self.audio.drain(..) {
            if let Ok(e) = world.get_entity_mut(entity) {
                e.despawn();
            }
        }
    }

    fn save(&self) {
        let mut shared = self.shared.lock().unwrap();
        shared.dirty = false;
        shared.progress.save(&self.settings);
    }

    fn command(&mut self, world: &mut World, command: TutorialCommand) {
        match command {
            TutorialCommand::Exit => {
                self.stop(world);
                self.shared.lock().unwrap().progress.skipped = true;
                self.save();
                info!("Skate 2 tutorial: exited to free skate; Learn > Skate 2 Tutorial resumes it");
            }
            TutorialCommand::StartOver => {
                self.stop(world);
                self.shared.lock().unwrap().progress = Progress::default();
                self.save();
                info!("Skate 2 tutorial: progress erased; starting over");
                self.command(world, TutorialCommand::Start);
            }
            TutorialCommand::Start => {
                if self.lesson.is_some() || self.playing.is_some() || !self.queue.is_empty() {
                    return;
                }
                let (frontend_done, meet_slappy_won, next) = {
                    let mut shared = self.shared.lock().unwrap();
                    shared.progress.skipped = false;
                    let p = &shared.progress;
                    (p.frontend_done, p.complete(MEET_SLAPPY), p.next_lesson())
                };
                if !frontend_done {
                    self.queue_frontend();
                } else if !meet_slappy_won {
                    self.pending.push_back(Delivery::StartTrigger(START_MACHINE.into()));
                } else if let Some(next) = next {
                    self.shared.lock().unwrap().requests.push(Request::Execute(next.into()));
                } else {
                    // Everything is done: replay from Meet Slappy.
                    let mut shared = self.shared.lock().unwrap();
                    shared.progress.completed.clear();
                    shared.progress.active.clear();
                    drop(shared);
                    self.pending.push_back(Delivery::StartTrigger(START_MACHINE.into()));
                }
                self.save();
            }
        }
    }

    fn frame(&mut self, world: &mut World) {
        self.now = world.resource::<Time<Virtual>>().elapsed_secs_f64();
        {
            let mut shared = self.shared.lock().unwrap();
            (shared.clock, shared.now) = (world.resource::<Replay>().now(), self.now);
        }
        // The flash's screen fade, every frame (menus too); a finished fall ends it.
        let fade = self.flash.as_ref().map_or(0., Flash::level);
        if self.flash.as_ref().is_some_and(|f| !f.rising && fade <= 0.) {
            self.flash = None;
        }
        if let Some(mut screen) = world.get_resource_mut::<crate::retail_render::ScreenFade>() {
            screen.set_if_neq(crate::retail_render::ScreenFade(fade));
        }
        // A reel or photo pick outlives no lesson (one that failed or ended mid-way): back to gameplay.
        if self.lesson.is_none() {
            if matches!(self.playing, Some(Playing::Reel { .. } | Playing::Photos { .. })) {
                self.playing = None;
                world.resource_mut::<Cutscene>().stop();
            }
            // A highlight plays out (the lesson may end under it) and hands the replay back itself.
            if !matches!(self.playing, Some(Playing::Highlight { .. })) {
                close_replay(world);
            }
            if std::mem::take(&mut self.sim_paused) {
                world.resource_mut::<Time<Virtual>>().unpause();
            }
        }
        // graphics_menu::interact (PreUpdate) unpauses the game each frame its menu is closed: ours holds here.
        if self.sim_paused || self.paused_time {
            world.resource_mut::<Time<Virtual>>().pause();
        }
        if world.get_resource::<crate::graphics_menu::Menu>().is_some_and(|m| m.open) {
            return;
        }
        self.input(world);
        let now = self.now;
        let due: Vec<i64> = self.timers.iter().filter(|(_, at)| *at <= now).map(|(id, _)| *id).collect();
        self.timers.retain(|(_, at)| *at > now);
        for id in due {
            self.pending.push_back(Delivery::Lesson("timerexpired", json!({"TimerID": id})));
        }
        if let Some(from) = self.lesson_teleport {
            if teleport_done(world, now - from) {
                self.lesson_teleport = None;
                self.pending.push_back(Delivery::Lesson("teleporttransitionfinished", Value::Null));
                self.pending.push_back(Delivery::Lesson("skaterteleportcomplete", json!({"PlayerID": 0})));
            }
        }
        if self.hud.central.as_ref().is_some_and(|(_, until)| *until <= now) {
            self.hud.central = None;
        }
        if self.hud.card.as_ref().is_some_and(|(_, _, until)| *until <= now) {
            self.hud.card = None;
        }
        self.settle(world);
        self.pump(world);
        if now >= self.next_evaluation {
            self.next_evaluation = now + 1. / EVALUATION_HZ;
            self.evaluate(world);
            self.settle(world);
            self.pump(world);
        }
        self.prompt(world);
        if std::mem::take(&mut self.shared.lock().unwrap().dirty) {
            self.save();
        }
    }

    fn input(&mut self, world: &mut World) {
        let keys = world.resource::<ButtonInput<KeyCode>>();
        let nav = world.get_resource::<crate::customiser::Navigation>().map_or(0, |n| n.pressed);
        let accept = keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::Space) || nav & 0x1000 != 0;
        let back = keys.just_pressed(KeyCode::Backspace) || nav & 0x2000 != 0;
        let up = keys.just_pressed(KeyCode::ArrowUp) || keys.just_pressed(KeyCode::ArrowLeft) || nav & (1 | 4) != 0;
        let down = keys.just_pressed(KeyCode::ArrowDown) || keys.just_pressed(KeyCode::ArrowRight) || nav & (2 | 8) != 0;
        let (sign_up, x) = (
            sign_up_pressed(keys, world.get_resource::<crate::customiser::Navigation>()),
            keys.just_pressed(KeyCode::KeyX) || nav & 0x4000 != 0,
        );
        match &mut self.playing {
            Some(Playing::Dialog { yes, no, yes_at, lesson }) => {
                let (yes, no, yes_at, lesson) = (*yes, *no, *yes_at, *lesson);
                let key = [(accept, Key::Accept), (back, Key::Back), (up, Key::Up), (down, Key::Down)].into_iter().find(|(on, _)| *on);
                // The popup screen picks with its own highlight; without it A is yes and B no.
                let answer = if self.screens.popup_shown() {
                    match key.and_then(|(_, key)| self.screens.popup_input(world, key)) {
                        Some(Some(i)) if i == yes_at => Some(yes),
                        Some(_) => no,
                        None => None,
                    }
                } else if accept {
                    Some(yes)
                } else if back {
                    no
                } else {
                    None
                };
                if let Some(answer) = answer {
                    self.playing = None;
                    self.hud.dialog = None;
                    if std::mem::take(&mut self.paused_time) {
                        world.resource_mut::<Time<Virtual>>().unpause();
                    }
                    self.pending.push_back(if lesson { Delivery::Lesson(answer, Value::Null) } else { Delivery::Triggers(answer) });
                }
            }
            // State 10's A (0x82528280) acts once +80 is set: cMsgHUDContinue to progression, then the pop.
            Some(Playing::Unlocks { ready }) => {
                *ready |= self.screens.unlocks_ready(world);
                if accept && *ready {
                    self.playing = None;
                    self.hud.unlocks = None;
                    if std::mem::take(&mut self.paused_time) {
                        world.resource_mut::<Time<Virtual>>().unpause();
                    }
                    self.pending.push_back(Delivery::Triggers("cMsgHUDContinue"));
                }
            }
            // State 79's A (0x8257c438): cMsgHUDContinue, then the pop; its Exit (0x8257c3a0) unpins.
            Some(Playing::Skate101) if accept => {
                self.playing = None;
                self.hud.skate101 = None;
                self.pinned = false;
                self.pending.push_back(Delivery::Triggers("cMsgHUDContinue"));
            }
            Some(Playing::Photos { index, forward }) => {
                let count = world.resource::<Shots>().0.len().max(1);
                *index = if up { (*index + count - 1) % count } else if down { (*index + 1) % count } else { *index % count };
                if up || down {
                    *forward = down;
                }
                // ponytail: B (retry) is the port's; Skate 2's photoselect only steps and picks.
                let answer = if accept { Some("hudcontinue") } else if back { Some("hudretry") } else { None };
                if accept {
                    self.screens.pick_photo(world);
                }
                if let Some(answer) = answer {
                    self.pending.push_back(Delivery::Lesson(answer, Value::Null));
                    self.end_playing(world);
                }
            }
            // A/Enter skips movies and cutscenes.
            Some(Playing::Movie { .. }) if accept => self.end_playing(world),
            Some(Playing::Scene { cutscene: true, length, .. }) if accept => {
                *length = 0.;
                self.stop_audio(world);
            }
            Some(Playing::SignUp(lesson)) => {
                let lesson = *lesson;
                let movie = x.then(|| self.data.as_ref()?.poster_movie(lesson)).flatten();
                if accept {
                    self.playing = None;
                    self.queue.push_front(Item::Poster(lesson, false));
                } else if back {
                    self.playing = None;
                    self.hud.signup = None;
                    if std::mem::take(&mut self.paused_time) {
                        world.resource_mut::<Time<Virtual>>().unpause();
                    }
                } else if let Some((movie, _)) = movie {
                    // The movie, then the poster again.
                    self.playing = None;
                    self.hud.signup = None;
                    self.queue.push_front(Item::Poster(lesson, true));
                    self.queue.push_front(Item::Movie(movie));
                }
            }
            // D-pad Up at a lesson's start signs up: its poster.
            None if sign_up && self.lesson.is_none() && self.queue.is_empty() => {
                let start = self.hud.prompt.is_some().then(|| self.prompted(world)).flatten();
                if let Some(lesson) = start {
                    self.hud.prompt = None;
                    self.queue.push_back(Item::Poster(lesson, true));
                }
            }
            _ => {}
        }
    }

    /// The open lesson whose sign-up box (around its `Location`) the skater is in.
    fn prompted(&self, world: &World) -> Option<&'static str> {
        let data = self.data.as_ref()?;
        let at = skater_position(world);
        let shared = self.shared.lock().unwrap();
        let p = &shared.progress;
        LESSONS.into_iter().filter(|l| p.active(l) && !p.complete(l)).find(|l| {
            let Some(c) = data.challenges.get(*l) else { return false };
            let half = c.fields["Hash_8022BA64CB8E431D"].as_f64().unwrap_or(0.) as f32;
            c.fields["Location"].as_str().and_then(|s| data.locator(s)).is_some_and(|m| in_signup_box(position(&m), half, at))
        })
    }

    /// The photo reel or the photo pick is up: the shots are being taken or chosen.
    fn photographing(&self) -> bool {
        matches!(self.playing, Some(Playing::Reel { .. } | Playing::Photos { .. }))
    }

    /// The guide arrows to draw: the indicators the lesson and its objective name and, between lessons, the open
    /// lessons' sign-up indicators. None in a photo: Not retail (user 10-13, bug 6), what retail's reel hides is UNTRACED.
    fn guides(&self, indicators: &[String]) -> Vec<(String, [[f32; 4]; 4])> {
        let Some(data) = self.data.as_ref().filter(|_| !self.photographing()) else { return Vec::new() };
        let mut wanted: Vec<_> = indicators.iter().filter_map(|l| Some((l.clone(), data.locator(l)?))).collect();
        if self.lesson.is_none() {
            wanted.extend(signup_indicators(data, &self.shared.lock().unwrap().progress));
        }
        wanted
    }

    fn prompt(&mut self, world: &World) {
        let idle = self.lesson.is_none() && self.playing.is_none() && self.queue.is_empty() && self.triggers.is_empty();
        let skipped = self.shared.lock().unwrap().progress.skipped;
        self.hud.prompt = (idle && !skipped)
            .then(|| self.prompted(world))
            .flatten()
            .and_then(|l| Some(self.data.as_ref()?.title(l)));
    }

    /// Runs requests and deliveries until both are empty.
    fn settle(&mut self, world: &mut World) {
        for _ in 0..1000 {
            let requests = std::mem::take(&mut self.shared.lock().unwrap().requests);
            let had = !requests.is_empty();
            for request in requests {
                self.request(world, request);
            }
            match self.pending.pop_front() {
                Some(delivery) => self.deliver(world, delivery),
                None if !had => return,
                None => {}
            }
        }
        error!("Skate 2 tutorial: the handlers keep asking for more; stopping");
        self.stop(world);
    }

    fn request(&mut self, world: &mut World, request: Request) {
        let Some(data) = self.data.clone() else { return };
        match request {
            Request::Message(name) => self.message(&data, &name),
            Request::Execute(name) => {
                if self.lesson.is_some() {
                    warn!("Skate 2 tutorial: {name} asked to start while {} runs", self.lesson.as_ref().unwrap().name);
                    return;
                }
                // Retail plays a lesson's TutorialMovie only from the poster's X (0x825242ac, the field's one reader).
                self.queue.push_back(Item::Start(name));
            }
            Request::Lesson(event, message) => self.pending.push_back(Delivery::Lesson(event, message)),
            Request::Teleport(locator) => {
                if teleport(world, &data, &locator) {
                    self.lesson_teleport = Some(self.now);
                }
            }
            Request::Timer(id, seconds) => {
                self.stopped.retain(|(t, _)| *t != id);
                self.timers.push((id, self.now + seconds));
            }
            Request::Central(label, kind) => {
                self.hud.central = Some((data.text(&label), self.now + CENTRAL_SECONDS));
                self.hud.central_kind = kind;
                self.hud.central_title = self.lesson.as_ref().map(|l| data.title(&l.name)).unwrap_or_default();
            }
            Request::Present(group) => {
                let events = self.shared.lock().unwrap().groups.get(&group).cloned().unwrap_or_default();
                self.queue.extend(events.into_iter().map(Item::Event));
                self.queue.push_back(Item::Lesson("presentationeventgroupfinished", json!({"ChallengePresentationGroup": group})));
            }
            Request::HideInstruction => self.hud.instruction = None,
            Request::Outcome(outcome) => {
                if let Some(lesson) = &mut self.lesson {
                    lesson.outcome = Some(outcome);
                }
            }
            Request::Complete => self.finish_lesson(&data),
            Request::Speech(e) => self.event(world, &data, e),
            Request::RemoveTimer(id) => {
                let now = self.now;
                self.stopped.extend(self.timers.iter().filter(|(t, _)| *t == id).map(|(t, at)| (*t, (at - now).max(0.))));
                self.timers.retain(|(t, _)| *t != id);
            }
            Request::Simulation(run) => {
                let mut time = world.resource_mut::<Time<Virtual>>();
                if run {
                    if std::mem::take(&mut self.sim_paused) {
                        time.unpause();
                    }
                } else {
                    time.pause();
                    self.sim_paused = true;
                }
            }
            Request::Ai(id, Some(locator)) => {
                // round2 T5: a full skater where the lesson has its recordings (retail: a cSkater with an AIController).
                let key = self.lesson.as_ref().filter(|l| data.ai_segments.contains_key::<str>(&l.name)).and_then(|l| rider_key(&data, &l.name, id));
                if let (Some(key), Some(at), Some(mut riders)) = (&key, data.spawn_point(&locator, id), world.get_resource_mut::<Riders>()) {
                    riders.spawn(key, at);
                }
                self.ai.insert(id, Ai { locator, key, ..default() });
                self.pending.push_back(Delivery::Lesson("skatercreated", json!({"PlayerID": id})));
            }
            Request::Reposition(id, locator) => {
                if let Some(ai) = self.ai.get_mut(&id) {
                    if let (Some(key), Some(at), Some(mut riders)) = (&ai.key, data.spawn_point(&locator, id), world.get_resource_mut::<Riders>()) {
                        riders.teleport(key, at);
                    }
                    (ai.locator, ai.path, ai.driver) = (locator, None, None);
                }
                self.pending.push_back(Delivery::Lesson("skaterteleportcomplete", json!({"PlayerID": id})));
            }
            Request::ContinueRetry(won) => {
                let title = data.text(&self.shared.lock().unwrap().results_title);
                let body = data.text(if won { "ID_MISSION_CHALLENGE_COMPLETE" } else { "ID_MISSION_CHALLENGE_FAILED" });
                let popup = Popup { title, body, options: vec![data.text("ID_COMMON_CONTINUE"), data.text("ID_COMMON_RETRY")] };
                self.queue.push_back(Item::Dialog { popup, yes: "hudcontinue", no: Some("hudretry"), yes_at: 0, lesson: true });
            }
            Request::Ai(id, None) => drop(self.ai.remove(&id)),
            Request::StartPath(id) => {
                let lap = self.lesson.as_ref().and_then(|l| lap(&data, &l.name, id));
                debug!("Skate 2 tutorial: competitor {id} sets off ({} samples)", lap.map_or(0, |l| l.len()));
                let loops = self.lesson.as_ref().and_then(|l| data.challenges.get(&l.name)).is_none_or(|c| c.fields["RaceHeats"].is_null());
                let lesson = self.lesson.as_ref().map(|l| l.name.to_string()).unwrap_or_default();
                if let Some(ai) = self.ai.get_mut(&id) {
                    (ai.path, ai.loops) = (Some((self.now, -PATH_ABOVE_GROUND)), loops);
                    // round2 T5: FilmAIID i drives recording i - 1 from its start; Follow Slappy's leader waits for the
                    // player (rubber band, INFERRED mode 23).
                    // ponytail: put on the path at rest; retail's FollowPath (105) starts it at half the node's speed.
                    if let Some(paths) = data.ai_segments.get(&lesson)
                        && let Some(i) = usize::try_from(id - 1).ok().filter(|&i| i < paths.len())
                        && let Some(key) = &ai.key
                    {
                        let mut driver = Driver::new(i);
                        driver.band = lesson.contains("follow");
                        // round2 T5 tricks: the rows and its profile (INFERRED: the profile row named as the competitor).
                        driver.tricks = data.ai_tricks.clone();
                        driver.profile = key.split('#').next().and_then(|m| data.ai_profiles.get(m)).cloned().unwrap_or_default();
                        let at = driver.start(paths);
                        if let Some(mut riders) = world.get_resource_mut::<Riders>() {
                            riders.teleport(key, at);
                        }
                        ai.driver = Some(driver);
                    }
                }
            }
            Request::Reel => self.queue.push_back(Item::Reel),
            Request::ResetReplay => world.resource_mut::<Replay>().reset_recording(),
            Request::Photos => self.queue.push_back(Item::Photos),
            Request::KillNis => {
                if matches!(self.playing, Some(Playing::Reel { .. } | Playing::Photos { .. })) {
                    self.playing = None;
                }
                world.resource_mut::<Cutscene>().stop();
                close_replay(world);
            }
        }
    }

    /// Progression.SendMessage: the message's type says what it does.
    fn message(&mut self, data: &Data, name: &str) {
        let Some(m) = data.progression.messages.get(name) else {
            warn!("Skate 2 tutorial: no progression message {name}");
            return;
        };
        let prop = |i: usize| m.properties.get(i).and_then(Value::as_str).unwrap_or("").to_owned();
        match (m.kind.as_str(), m.kind.as_u64()) {
            (Some("nis"), _) => {
                // The pros leave in this one (its actors stand in for them while it plays).
                if name == PROS_LEAVE {
                    self.pros.clear();
                }
                self.queue.push_back(Item::Event(Event { kind: 5, a: prop(0), b: String::new() }));
                self.queue.push_back(Item::Triggers("cMsgNISChallengeNotify"));
            }
            (Some("fmv"), _) => {
                self.queue.push_back(Item::Movie(prop(0)));
                self.queue.push_back(Item::Triggers("cMsgFMVFinishedPlaying"));
            }
            (Some("dialog"), _) => self.queue.push_back(Item::Dialog {
                popup: Popup {
                    title: data.text(&prop(1)),
                    body: data.text(&prop(0)),
                    // Skate 2 lists No first.
                    options: vec![data.text("ID_COMMON_NO"), data.text("ID_COMMON_YES")],
                },
                yes: "cMsgHUDYes",
                no: Some("cMsgHUDNo"),
                yes_at: 1,
                lesson: false,
            }),
            (Some("player_spawn"), _) => self.queue.push_back(Item::Event(Event { kind: 8, a: prop(0), b: String::new() })),
            // Type 1 branches on the message id (0x824d3630): 10, "Show Unlocks Screen", posts cMsgOpenUnlockOverlay
            // (FE state 10). 57 (autosave), 180 ("Open New Path") and the sponsor overlays are no tutorial messages.
            // Character_GetUnlockedList hands the screen each character's vault Text label (Step 1c): ID_COMMON_REDA
            // for reda, the only character the tutorial unlocks (progression.lua).
            (_, Some(1)) if m.id == Some(10) => {
                let unlocked = std::mem::take(&mut self.shared.lock().unwrap().unlocked);
                self.queue.push_back(Item::Unlocks(unlocked.iter().map(|n| format!("ID_COMMON_{}", n.to_uppercase())).collect()));
            }
            // Type 4 (0x824d32c8): CharacterID's line FileID through speech vtable[15], the call
            // ChallengeSpeech.PlayProgressionReminder makes; over gameplay, holding nothing. The export asserts
            // CharacterID 40, CameraMan.
            (_, Some(4)) => {
                let line = m.named.get("FileID").and_then(Value::as_i64).unwrap_or(0);
                self.shared.lock().unwrap().requests.push(Request::Speech(Event { kind: 3, a: "CameraMan".into(), b: line.to_string() }));
            }
            // Type 5 (0x824d3328): cMsgSkate101Show; FE state 79 shows TitleHALID / TextHALID (0x8250fc18).
            (_, Some(5)) => {
                let text = |k: &str| data.text(m.named.get(k).and_then(Value::as_str).unwrap_or(""));
                self.queue.push_back(Item::Skate101 { title: text("TitleHALID"), text: text("TextHALID") });
            }
            // "Atiba"/"Carroll"/"Koston": the pros arriving at Slappy's stand at their locator from now on.
            (_, Some(8)) => match PROS.iter().find(|(message, _)| *message == name) {
                Some((_, key)) => {
                    self.pros.insert((*key).to_owned(), prop(0));
                }
                None => warn!("Skate 2 tutorial: spawn message {name} names no cast member"),
            },
            // ponytail: HUD components and pro/feature locks (6, 12) wait for the HUD movies.
            _ => debug!("Skate 2 tutorial: message {name} has no effect in the port"),
        }
    }

    fn deliver(&mut self, world: &mut World, delivery: Delivery) {
        match delivery {
            Delivery::Lesson(event, message) => {
                let Some(lesson) = &mut self.lesson else { return };
                // G8 I3: DMOs back before a restart's or retry's handler teleports.
                if matches!(event, "restartchallenge" | "hudretry")
                    && let Some(data) = &self.data
                {
                    reset_dmos(world, data, &lesson.name);
                }
                if let Err(error) = lesson.machine.send(event, &message) {
                    self.abort_lesson(error);
                }
            }
            Delivery::Triggers(event) => {
                for trigger in &mut self.triggers {
                    if let Err(error) = trigger.send(event, &Value::Null) {
                        error!("Skate 2 tutorial: {error}");
                        trigger.terminate();
                    }
                }
                self.triggers.retain(|t| !t.terminated());
            }
            Delivery::StartTrigger(name) => {
                let Some(data) = self.data.clone() else { return };
                let Some(t) = data.progression.triggers.get(&name) else {
                    warn!("Skate 2 tutorial: no progression trigger {name}");
                    return;
                };
                let script = Script {
                    name: &name,
                    source: &data.progression_lua,
                    steps: t.states.clone(),
                    initial: &t.initial,
                    enter: "cMsgStateEnter",
                    exit: "cMsgStateExit",
                };
                let started = Machine::new(script, &Value::Null, |lua| progression_natives(lua, &self.shared)).and_then(|mut m| {
                    m.start()?;
                    Ok(m)
                });
                match started {
                    Ok(machine) if !machine.terminated() => self.triggers.push(machine),
                    Ok(_) => {}
                    Err(error) => error!("Skate 2 tutorial: {error}"),
                }
            }
            Delivery::StartLesson(name) => self.start_lesson(world, name),
        }
    }

    fn start_lesson(&mut self, world: &mut World, name: String) {
        let Some(data) = self.data.clone() else { return };
        if self.lesson.is_some() {
            return;
        }
        self.shared.lock().unwrap().clear_lesson();
        *world.resource_mut::<TrickLog>() = TrickLog::default();
        // Marker counts and volumes start over: what happened before the lesson isn't "just" done.
        (self.markers, self.last_evaluation) = (None, self.now);
        self.last_inside.clear();
        match Lesson::new(&data, &name, &self.shared) {
            Ok(lesson) => {
                info!("Skate 2 tutorial: {name} starts");
                self.hud.card = Some((data.title(&name), data.description(&name), self.now + CENTRAL_SECONDS));
                // G8 I3: DMOs back before the lesson's intro teleport (a lesson won and started again in one frame
                // never unstages them).
                reset_dmos(world, &data, &name);
                let lesson = self.lesson.insert(lesson);
                if let Err(error) = lesson.machine.start() {
                    self.abort_lesson(error);
                }
                // Kind 5 (INFERRED: sign-up accepted, sent for every lesson start): Follow Slappy's `_accept` opens the
                // gate before Slappy rides through it.
                if self.lesson.is_some() {
                    for trigger in data.progression.entries.get(&name).map(EntryData::signed_up).unwrap_or_default() {
                        self.pending.push_back(Delivery::StartTrigger(trigger));
                    }
                }
            }
            Err(error) => error!("Skate 2 tutorial: {name} cannot start: {error}"),
        }
    }

    /// Challenge.CompleteChallenge: the lesson's last step ran.
    fn finish_lesson(&mut self, data: &Data) {
        let Some(lesson) = self.lesson.take() else { return };
        self.clear_lesson_hud();
        // UNTRACED: CompleteChallenge with no outcome recorded; taken as an abandon.
        let outcome = lesson.outcome.unwrap_or(Outcome::Abandon);
        info!("Skate 2 tutorial: {} {outcome:?}", lesson.name);
        if outcome == Outcome::Win {
            let mut shared = self.shared.lock().unwrap();
            let p = &mut shared.progress;
            p.active.retain(|a| *a != lesson.name);
            p.completed.push(lesson.name.clone());
            drop(shared);
            self.save();
        }
        for trigger in data.progression.entries.get(&lesson.name).map(|e| e.fired(outcome)).unwrap_or_default() {
            self.pending.push_back(Delivery::StartTrigger(trigger));
        }
    }

    /// A Lua error ends the lesson to free skate (spec: never a crash).
    fn abort_lesson(&mut self, error: String) {
        error!("Skate 2 tutorial: {error}; back to free skate");
        self.lesson = None;
        self.clear_lesson_hud();
    }

    fn clear_lesson_hud(&mut self) {
        self.shared.lock().unwrap().clear_lesson();
        self.ai.clear();
        (self.hud.objective, self.hud.instruction) = (None, None);
        self.timers.clear();
        self.stopped.clear();
        self.lesson_teleport = None;
        self.pinned = false;
        // The lesson's own queued events go with it; progression items stay.
        self.queue.retain(|i| !matches!(i, Item::Event(_) | Item::Lesson(..) | Item::Reel | Item::Photos));
    }

    /// Plays queued items until one is still running.
    fn pump(&mut self, world: &mut World) {
        for _ in 0..1000 {
            if self.playing.is_some() {
                if !self.finished(world) {
                    return;
                }
                self.end_playing(world);
            }
            let Some(item) = self.queue.pop_front() else { return };
            self.begin(world, item);
            self.settle(world);
        }
    }

    fn finished(&mut self, world: &mut World) -> bool {
        let now = self.now;
        match &mut self.playing {
            None => true,
            Some(Playing::Wait(until)) => now >= *until,
            Some(Playing::Scene { start, length, cues, .. }) => {
                let t = start.elapsed().as_secs_f64();
                // Cues are tenths of the scene's own (scaled) time (nis-clock.md 3).
                self.hud.subtitle = subtitle(self.data.as_deref(), cues, world.resource::<Cutscene>().time() as f64);
                // It blooms out to white over its last second (or after an A-skip), then cuts back.
                if t >= *length - FLASH_SECONDS as f64 && !self.flash.as_ref().is_some_and(|f| f.rising) {
                    self.flash = Some(Flash::ramp(self.flash.as_ref(), true));
                }
                t >= *length && self.flash.as_ref().map_or(true, Flash::white)
            }
            Some(Playing::Flash) => self.flash.as_ref().map_or(true, Flash::white),
            Some(Playing::Teleport { from }) => teleport_done(world, now - *from),
            Some(Playing::Movie { movie, cues, start }) => {
                self.hud.subtitle = subtitle(self.data.as_deref(), cues, start.elapsed().as_secs_f64());
                movie.advance(&mut world.resource_mut::<Assets<Image>>())
            }
            Some(Playing::Customiser) => !world.get_resource::<crate::customiser::Customiser>().is_some_and(|c| c.open),
            Some(Playing::Dialog { .. } | Playing::Photos { .. } | Playing::SignUp(_) | Playing::Unlocks { .. } | Playing::Skate101) => false,
            Some(Playing::Reel { at, left, clock }) => {
                // The replay moves with the photo scene's clock (a replay-type scene, nis-clock.md 7): its TIME_SCALE
                // and FREEZE holds, once a frame however often the queue is pumped.
                let now = world.resource::<Time<Real>>().elapsed_secs_f64();
                let real = (now - std::mem::replace(clock, now)).min(0.1);
                *at += real * world.resource::<Cutscene>().rate() as f64;
                *left -= real;
                world.resource_mut::<Replay>().seek(*at);
                let snapped = world.resource_mut::<Cutscene>().take_freeze();
                if snapped {
                    snap(world);
                }
                // A frame hitch can leave FREEZE keys due at the scene's end: one a frame until they're out.
                *left <= 0. && !snapped
            }
            // As a reel, at the scene camera's TIME_SCALE, to its end; then it blooms out to white on that last frame
            // (retail's ends on the roll-away, not under the bloom).
            Some(Playing::Highlight { at, to, clock }) => {
                let now = world.resource::<Time<Real>>().elapsed_secs_f64();
                let real = (now - std::mem::replace(clock, now)).min(0.1);
                *at = (*at + real * world.resource::<Cutscene>().rate() as f64).min(*to);
                world.resource_mut::<Replay>().seek(*at);
                if *at >= *to && !self.flash.as_ref().is_some_and(|f| f.rising) {
                    self.flash = Some(Flash::ramp(self.flash.as_ref(), true));
                }
                *at >= *to && self.flash.as_ref().map_or(true, Flash::white)
            }
        }
    }

    fn end_playing(&mut self, world: &mut World) {
        match self.playing.take() {
            Some(Playing::Movie { movie, .. }) => {
                world.resource_scope(|world, mut images: Mut<Assets<Image>>| movie.stop(&mut world.commands(), &mut images));
                if std::mem::take(&mut self.paused_time) {
                    world.resource_mut::<Time<Virtual>>().unpause();
                }
            }
            Some(Playing::Scene { .. }) => {
                world.resource_mut::<Cutscene>().stop();
                self.flash = Some(Flash::ramp(self.flash.as_ref(), false));
                if std::mem::take(&mut self.paused_time) {
                    world.resource_mut::<Time<Virtual>>().unpause();
                }
            }
            // The replay holds its last frame under the photo pick.
            Some(Playing::Reel { .. }) => {
                world.resource_mut::<Cutscene>().stop();
                self.pending.push_back(Delivery::Lesson("nischallengenotify", Value::Null));
            }
            Some(Playing::Photos { .. }) => close_replay(world),
            Some(Playing::Highlight { .. }) => {
                world.resource_mut::<Cutscene>().stop();
                close_replay(world);
                self.flash = Some(Flash::ramp(self.flash.as_ref(), false));
                if std::mem::take(&mut self.paused_time) {
                    world.resource_mut::<Time<Virtual>>().unpause();
                }
            }
            _ => {}
        }
        self.hud.subtitle = None;
    }

    fn begin(&mut self, world: &mut World, item: Item) {
        let Some(data) = self.data.clone() else { return };
        match item {
            Item::Event(event) => self.event(world, &data, event),
            Item::Movie(name) => self.movie(world, &data, &name),
            Item::Customiser => {
                if world.contains_resource::<crate::customiser::Customiser>() {
                    if let Some(mut models) = world.get_resource_mut::<crate::custom_models::CustomModels>() {
                        models.request_stock();
                    }
                    world.resource_mut::<crate::customiser::Customiser>().begin();
                    self.playing = Some(Playing::Customiser);
                }
            }
            Item::FrontendDone(trigger) => {
                self.shared.lock().unwrap().progress.frontend_done = true;
                self.save();
                self.pending.push_back(Delivery::StartTrigger(trigger));
            }
            Item::Lesson(event, message) => self.pending.push_back(Delivery::Lesson(event, message)),
            Item::Triggers(event) => self.pending.push_back(Delivery::Triggers(event)),
            Item::Dialog { popup, yes, no, yes_at, lesson } => {
                self.hud.dialog = Some(popup);
                self.playing = Some(Playing::Dialog { yes, no, yes_at, lesson });
                // Play holds under a popup: the skater stays put until it's answered.
                world.resource_mut::<Time<Virtual>>().pause();
                self.paused_time = true;
            }
            Item::Start(name) => self.pending.push_back(Delivery::StartLesson(name)),
            // FE state 10's Enter (0x82528060): the central HUD hides, UnlockOverlay opens.
            Item::Unlocks(characters) => {
                self.hud.central = None;
                self.hud.unlocks = Some(characters);
                self.playing = Some(Playing::Unlocks { ready: false });
                // UNTRACED (Step 1h): states 9 and 10 neither pin nor slow the sim; what holds it under Results wasn't
                // traced. Play holds, as under the popup this screen replaces.
                world.resource_mut::<Time<Virtual>>().pause();
                self.paused_time = true;
            }
            // FE state 79's Enter (0x8257c300): cMsgPinPlayer(1), then Skate101.
            Item::Skate101 { title, text } => {
                self.pinned = true;
                self.hud.skate101 = Some((title, text));
                self.playing = Some(Playing::Skate101);
            }
            Item::Reel => self.start_reel(world, &data),
            Item::Photos => self.playing = Some(Playing::Photos { index: 0, forward: true }),
            // Retail blooms to white with gameplay held, then the poster opens (its intro is a film-strip wipe), or,
            // accepted, the lesson starts straight into its HUD: no lesson card.
            Item::Poster(lesson, open) => {
                if !self.white(world, Item::Poster(lesson, open), true) {
                    return;
                }
                self.flash = Some(Flash::ramp(self.flash.as_ref(), false));
                if open {
                    self.hud.signup = Some(lesson);
                    self.playing = Some(Playing::SignUp(lesson));
                } else {
                    self.hud.signup = None;
                    if std::mem::take(&mut self.paused_time) {
                        world.resource_mut::<Time<Virtual>>().unpause();
                    }
                    self.start_lesson(world, lesson.into());
                    self.hud.card = None;
                }
            }
        }
    }

    /// HighlightReel.PlayHighlightReel: the last highlight replayed under its photo scene's camera.
    fn start_reel(&mut self, world: &mut World, data: &Data) {
        let highlight = self.shared.lock().unwrap().highlights.last().cloned();
        world.resource_mut::<Shots>().0.clear();
        let length = highlight.as_ref().and_then(|h| data.scenes.get(&h.scene)).map_or(3., |s| s.duration);
        // ponytail: the scene covers the highlight's end (the landing); how retail lines the two up is unread.
        let from = highlight.as_ref().map_or(0., |h| h.from.max(h.to - length));
        self.hud.central = None;
        let Some(h) = highlight.filter(|_| world.resource_mut::<Replay>().script(from)) else {
            warn!("Skate 2 tutorial: no highlight to replay");
            self.pending.push_back(Delivery::Lesson("nischallengenotify", Value::Null));
            return;
        };
        world.resource_mut::<Cutscene>().start_muted(&data.root, &h.scene);
        for audio in data.scenes.get(&h.scene).map_or(&[][..], |s| &s.audio[..]) {
            self.play(world, &data.root.join(&audio.audio), crate::music::SFX);
        }
        let clock = world.resource::<Time<Real>>().elapsed_secs_f64();
        // The camera covers `length` recording seconds, which take `left` real seconds on its clock. UNTRACED: retail
        // stretches the take over the replay range (nis-clock.md 7); the record's length stands in for it.
        let left = crate::challenge::cutscene::real_length(&data.root, &h.scene).map_or(length, f64::from);
        self.playing = Some(Playing::Reel { at: from, left, clock });
    }

    fn movie(&mut self, world: &mut World, data: &Data, name: &str) {
        let Some(m) = data.movies.get(name) else {
            warn!("Skate 2 tutorial: no movie {name}");
            return;
        };
        let Some(camera) = world.query_filtered::<Entity, With<IsDefaultUiCamera>>().iter(world).next() else { return };
        let (video, wav) = (data.root.join(&m.video), m.audio.as_ref().map(|a| data.root.join(a)));
        let started = world.resource_scope(|world, mut images: Mut<Assets<Image>>| {
            world.resource_scope(|world, mut pcm: Mut<Assets<StreamPcm>>| {
                Movie::start(&mut world.commands(), &mut images, &mut pcm, camera, &video, wav.as_deref())
            })
        });
        match started {
            Ok(movie) => {
                let mut time = world.resource_mut::<Time<Virtual>>();
                // Ours even when already paused: a pause menu's (a restart) ends as it closes.
                time.pause();
                self.paused_time = true;
                self.playing = Some(Playing::Movie { movie, cues: m.subtitles.clone(), start: Instant::now() });
            }
            Err(error) => warn!("Skate 2 tutorial: movie {name} skipped ({error})"),
        }
    }

    /// NISManager blooms the picture out to white before a cut (gameplay held if `hold`): false, with `again` back at
    /// the queue's front, until it's white.
    fn white(&mut self, world: &mut World, again: Item, hold: bool) -> bool {
        if self.flash.as_ref().is_some_and(Flash::white) {
            return true;
        }
        self.flash = Some(Flash::ramp(self.flash.as_ref(), true));
        if hold {
            // Ours even when already paused: a pause menu's (a restart) ends as it closes.
            world.resource_mut::<Time<Virtual>>().pause();
            self.paused_time = true;
        }
        self.queue.push_front(again);
        self.playing = Some(Playing::Flash);
        false
    }

    /// One presentation event. See tutorial_recovery.md for what each type was named from.
    fn event(&mut self, world: &mut World, data: &Data, e: Event) {
        let now = self.now;
        match e.kind {
            // Speech (0x82bc26e8): 2 holds the queue until its line ends (at most 10 s), 3 plays over what follows.
            2 | 3 => {
                let Some(line) = data.speech.get(&e.a).and_then(|s| s.lines.get(&e.b)).and_then(Option::as_ref) else {
                    debug!("Skate 2 tutorial: no speech {} {}", e.a, e.b);
                    return;
                };
                self.play(world, &data.root.join(&line.audio), crate::music::DIALOG);
                if e.kind == 2 {
                    self.playing = Some(Playing::Wait(now + line.seconds.min(10.)));
                }
            }
            // cMsgPinPlayer (0x82bc31f8): 6 holds the skater where they stand, 7 lets go.
            // ponytail: neutral gameplay input, not a physics hold: a skater pinned on a slope would still roll. Hold
            // the body if the spawn or the set-marker locator turns out not to be level.
            6 | 7 => self.pinned = e.kind == 6,
            4 => self.playing = Some(Playing::Wait(now + e.a.trim().parse::<f64>().unwrap_or(0.))),
            // A scene, gameplay held, its camera taking the view: empty second argument a cutscene (full frame, as retail), "True"
            // a flyover (outro_2/3, no actors; retail never shows the player during them).
            5 => {
                let Some(scene) = data.scenes.get(&e.a) else {
                    debug!("Skate 2 tutorial: no scene {}", e.a);
                    return;
                };
                let cutscene = e.b != "True";
                // NISManager blooms the picture out to white first (gameplay held); at white this event comes back
                // round and cuts in, and the flash falls away over the scene's first second.
                if !self.white(world, Item::Event(e.clone()), true) {
                    return;
                }
                self.flash = Some(Flash::ramp(self.flash.as_ref(), false));
                // The first stream is the scene's speech (or full mix), a post bed (the camera's whooshes and hits,
                // ICE's AUDIO_POST_ID) after it; they play out in real time without holding the scene, which ends at
                // its camera's end on its clock (nis-clock.md 2 and 4).
                let length = crate::challenge::cutscene::real_length(&data.root, &e.a).map_or(scene.duration, f64::from);
                for (i, audio) in scene.audio.iter().enumerate() {
                    self.play(world, &data.root.join(&audio.audio), if i == 0 { crate::music::DIALOG } else { crate::music::SFX });
                }
                world.resource_mut::<Cutscene>().start_muted(&data.root, &e.a);
                let mut time = world.resource_mut::<Time<Virtual>>();
                // Ours even when already paused: a pause menu's (a restart) ends as it closes.
                time.pause();
                self.paused_time = true;
                let hides_player = scene.hides_player();
                self.playing = Some(Playing::Scene { start: Instant::now(), length, cues: scene.subtitles.clone(), cutscene, hides_player });
            }
            8 => {
                if teleport(world, data, &e.a) {
                    self.playing = Some(Playing::Teleport { from: now });
                }
            }
            // "mini_teleport" waypoint at locator e.b (Slappy's), not a teleport: the lesson starts at J_Gamestart.
            30 => self.waypoint = Some(e.b),
            31 => self.waypoint = None,
            // e.a names the button animation of the instruction movie.
            20 => {
                debug!("Skate 2 tutorial: instruction {} ({})", e.b, e.a);
                self.hud.instruction = Some(data.text(&e.b));
                self.hud.gesture = e.a;
            }
            21 => self.hud.instruction = None,
            // A highlight (seconds, scene). Retail cuts to the scene's camera over live play, which runs on under the
            // bloom (the ollie's landing and roll-away get into the replay); at white the last seconds replay under
            // that camera, gameplay held.
            19 => {
                if !self.white(world, Item::Event(e.clone()), false) {
                    world.resource_mut::<Cutscene>().start_muted(&data.root, &e.b);
                    return;
                }
                world.resource_mut::<Time<Virtual>>().pause();
                self.paused_time = true;
                self.flash = Some(Flash::ramp(self.flash.as_ref(), false));
                let to = world.resource::<Replay>().now();
                let from = to - e.a.trim().parse::<f64>().unwrap_or(3.);
                // ponytail: with nothing recorded (never mid-lesson) the camera just watches the held skater.
                world.resource_mut::<Replay>().script(from);
                world.resource_mut::<Cutscene>().start_muted(&data.root, &e.b);
                let clock = world.resource::<Time<Real>>().elapsed_secs_f64();
                self.playing = Some(Playing::Highlight { at: from, to, clock });
            }
            // 0x22 (0x82bc2fc4 -> DMOManager slot 1 0x824b10e0 -> 0x82b186b8): clears the challenge-managed flag
            // (locator byte 152 bit 0x80) on the DMOs spawned inside volume e.a and moves none of them. The port has no
            // such flag: lesson start and retry (reset_dmos, DMOResetInfo) are what put DMOs back.
            // UNTRACED: what the 0x80 flag drives (825cd370 at 825cda30 -> 82b125d0).
            34 => {}
            // 0x1C (0x82bc2d88): awards achievement `a` (Meet_Slappy is #11 of Skate 2's 45) through the console's own
            // service; retail shows no game screen for it.
            0x1C => {}
            _ => debug!("Skate 2 tutorial: presentation event {} ({}, {}) has no effect in the port", e.kind, e.a, e.b),
        }
    }

    /// A wav at its Audio Settings `slot`'s volume.
    fn play(&mut self, world: &mut World, path: &Path, slot: usize) {
        match read_wav(path) {
            Ok(pcm) => {
                let settings = crate::music::one_shot(world.get_resource::<crate::graphics_menu::Menu>(), slot);
                let handle = world.resource_mut::<Assets<StreamPcm>>().add(pcm);
                self.audio.retain(|e| world.get_entity(*e).is_ok());
                self.audio.push(world.spawn((AudioPlayer(handle), settings)).id());
            }
            Err(error) => warn!("Skate 2 tutorial: {}: {error}", path.display()),
        }
    }

    fn sense(&mut self, world: &mut World) {
        let Some(data) = self.data.clone() else { return };
        let dt = (self.now - self.last_evaluation).clamp(0., 0.5) as f32;
        self.last_evaluation = self.now;
        let skater = world.resource::<SkaterRuntime>();
        let p = &skater.player_input.physical;
        let velocity = p.skateboard.vector_80.map(f32::from_bits);
        let (on_board, speed) = (p.state.category_12 != 500, Vec3::new(velocity[0], velocity[1], velocity[2]).length());
        let at = skater_position(world);
        // I2 (online audit 10-06): Wipeout.IsWipingOut / the wipeout lesson message are cMsgWipeout, the Wipeout
        // category (S2 0x8234d2e0 / 0x82337f88), not the board manager's flag, which a dismount raises too (B2).
        let wiping = skater.wiping_out();
        let counts = world.get_resource::<crate::session_marker::SessionMarker>().map(|m| (m.sets, m.uses));
        let previous = std::mem::replace(&mut self.markers, counts);
        let changed = |f: fn((u64, u64)) -> u64| matches!((previous, counts), (Some(a), Some(b)) if f(b) > f(a));
        let mut log = std::mem::take(&mut *world.resource_mut::<TrickLog>());
        // Manual time and the sequence keep counting across evaluations; only the tick lists start over.
        {
            let mut keep = world.resource_mut::<TrickLog>();
            (keep.nose, keep.tail, keep.holding) = (log.nose, log.tail, log.holding.clone());
            (keep.sequence, keep.grind, keep.points) = (log.sequence, log.grind, log.points);
            keep.sequence_tricks = log.sequence_tricks.clone();
        }
        let inside: BTreeSet<String> = data.placement.volumes.iter().filter(|(_, v)| v.contains(at)).map(|(n, _)| n.clone()).collect();
        let entered = inside.difference(&self.last_inside).cloned().collect();
        self.last_inside = inside.clone();
        let dmos: Vec<(u64, BTreeSet<String>)> = world
            .resource::<GamePhysics>()
            .tutorial_dmos()
            .map(|(id, c)| (id, data.placement.volumes.iter().filter(|(_, v)| v.contains(c)).map(|(n, _)| n.clone()).collect()))
            .collect();
        let grabbing = world.resource::<SkaterRuntime>().player_state.current() == skate_core::player::state::PhysicalStateId::OffBoardPushing;
        let mut shared = self.shared.lock().unwrap();
        let s = &mut shared.sense;
        if log.begun {
            s.visited.clear();
            self.pending.push_back(Delivery::Lesson("sequencebegin", json!({"PlayerID": 0})));
        }
        if wiping && !s.wiping {
            self.pending.push_back(Delivery::Lesson("wipeout", json!({"PlayerID": 0})));
        }
        s.visited.extend(inside.iter().cloned());
        s.tricks_sequence = log.sequence_tricks.iter().copied().collect();
        (s.grind_metres, s.points) = (log.grind, log.points);
        s.position = at;
        (s.on_board, s.speed, s.wiping) = (on_board, speed, wiping);
        s.wipe_seconds = if wiping { s.wipe_seconds + dt } else { 0. };
        s.tricks_objective.extend(log.tricks.iter().copied());
        s.tricks_frame = std::mem::take(&mut log.tricks);
        s.landed = log.landed;
        (s.holding, s.nose_seconds, s.tail_seconds) = (log.holding, log.nose, log.tail);
        (s.marker_set, s.marker_used) = (changed(|c| c.0), changed(|c| c.1));
        (s.inside, s.entered) = (inside, entered);
        (s.dmos, s.grabbing) = (dmos, grabbing);
    }

    /// The race template's senses: trigger volumes the player and the competitors enter, and FollowTheLeader's
    /// distance callbacks.
    fn race(&mut self, world: &World) {
        let (Some(data), Some(lesson)) = (self.data.clone(), &self.lesson) else { return };
        let mut s = self.shared.lock().unwrap();
        let mut events: Vec<(&'static str, Value)> =
            s.sense.entered.iter().map(|v| ("triggerentercollision", json!({"VolumeID": v, "PlayerID": 0}))).collect();
        for (&id, ai) in &mut self.ai {
            let Some(at) = ai.position(world, &data, &lesson.name, id, self.now) else { continue };
            let inside: BTreeSet<String> = data.placement.volumes.iter().filter(|(_, v)| v.contains(at)).map(|(n, _)| n.clone()).collect();
            events.extend(inside.difference(&ai.inside).map(|v| ("triggerentercollision", json!({"VolumeID": v, "PlayerID": id}))));
            ai.inside = inside;
        }
        let leader = s.follow.leader;
        let at = self.ai.get(&leader).and_then(|ai| ai.position(world, &data, &lesson.name, leader, self.now));
        if let Some(at) = at {
            events.extend(s.follow.crossings(at.distance(skater_position(world)) as f64));
        }
        drop(s);
        self.pending.extend(events.into_iter().map(|(event, message)| Delivery::Lesson(event, message)));
    }

    /// One evaluation of the running lesson's live objectives and hints (spec: the objective evaluator).
    fn evaluate(&mut self, world: &mut World) {
        if self.lesson.is_none() {
            return;
        }
        self.sense(world);
        self.race(world);
        let blocked = matches!(
            self.playing,
            Some(
                Playing::Scene { .. }
                    | Playing::Flash
                    | Playing::Teleport { .. }
                    | Playing::Movie { .. }
                    | Playing::Customiser
                    | Playing::Dialog { .. }
                    | Playing::Reel { .. }
                    | Playing::Highlight { .. }
                    | Playing::Photos { .. }
            )
        ) || self.lesson_teleport.is_some();
        let Self { lesson: Some(lesson), queue, pending, shared, .. } = self else { return };
        let (reset_all, resets, evaluating) = {
            let mut s = shared.lock().unwrap();
            (std::mem::take(&mut s.reset_all), std::mem::take(&mut s.reset), s.evaluating)
        };
        let mut result = Ok(());
        if reset_all {
            lesson.position = None;
            lesson.finished = false;
            result = (0..lesson.objectives.len()).try_for_each(|i| lesson.reset(i));
        }
        for i in resets {
            result = result.and_then(|()| lesson.reset(i));
        }
        if let Err(error) = result {
            return self.abort_lesson(error);
        }
        if !evaluating || lesson.finished || blocked {
            return;
        }
        if lesson.position.is_none() {
            lesson.position = Some(0);
            if !std::mem::replace(&mut lesson.intro_played, true) {
                queue.extend(lesson.intro.iter().cloned().map(Item::Event));
            }
            if let Some(&first) = lesson.order.first() {
                queue.extend(lesson.objectives[first].intro.iter().cloned().map(Item::Event));
            }
        }
        shared.lock().unwrap().sense.completed =
            lesson.objectives.iter().enumerate().filter(|(i, o)| o.done && Some(*i) != lesson.master).count();
        let live: Vec<usize> = lesson.master.into_iter().chain(lesson.current()).collect();
        let landed = shared.lock().unwrap().sense.landed;
        let mut completed = Vec::new();
        for i in live {
            let o = &mut lesson.objectives[i];
            // A points-gated objective judges its trick sequence: over the whole sequence, its hints when it lands.
            let gated = o.points.is_some();
            shared.lock().unwrap().sense.scoped = gated;
            for h in &mut o.hints {
                match h.condition.evaluate(&lesson.machine) {
                    Ok(now) => {
                        if now && if gated { landed } else { !h.was } {
                            queue.extend(h.events.iter().cloned().map(Item::Event));
                        }
                        h.was = now;
                    }
                    Err(error) => return self.abort_lesson(error),
                }
            }
            let met = o.condition.evaluate(&lesson.machine).and_then(|met| match &o.points {
                Some(points) if met => points.evaluate(&lesson.machine),
                _ => Ok(met),
            });
            shared.lock().unwrap().sense.scoped = false;
            match met {
                Ok(true) => completed.push(i),
                Ok(false) => {}
                Err(error) => return self.abort_lesson(error),
            }
        }
        // The disc's messages carry the group MakeObjectives returned (1). They go as the objective completes (the
        // lesson reads the highlight's end frame on delivery); its presentation plays on in the queue.
        let group = json!({"ObjectiveGroup": 1});
        for i in completed {
            let o = &mut lesson.objectives[i];
            o.done = true;
            queue.extend(o.complete.iter().cloned().map(Item::Event));
            if Some(i) == lesson.master {
                lesson.finished = true;
                pending.push_back(Delivery::Lesson("allobjectivecomplete", group));
                break;
            }
            shared.lock().unwrap().sense.completed += 1;
            pending.push_back(Delivery::Lesson(
                "objectivecomplete",
                json!({"ObjectiveGroup": 1, "ObjectiveIndex": i, "IsPointObjective": false, "RequiresHighlight": o.highlight}),
            ));
            let next = lesson.position.map_or(0, |p| p + 1);
            lesson.position = Some(next);
            shared.lock().unwrap().sense.tricks_objective.clear();
            match lesson.order.get(next) {
                Some(&n) => queue.extend(lesson.objectives[n].intro.iter().cloned().map(Item::Event)),
                None if lesson.master.is_none() => {
                    lesson.finished = true;
                    pending.push_back(Delivery::Lesson("allobjectivecomplete", group.clone()));
                }
                None => {}
            }
        }
    }

    /// Mirrors the state into the HUD nodes, guide markers and the pause menu flags.
    fn publish(&mut self, world: &mut World) {
        let hide = matches!(self.playing, Some(Playing::Scene { hides_player: true, .. }));
        self.show_player(world, !hide);
        let (objective, indicators, pause_disabled) = {
            let shared = self.shared.lock().unwrap();
            if let Some(mut marker) = world.get_resource_mut::<crate::session_marker::SessionMarker>() {
                marker.disable_set("skate2-tutorial", shared.marker_set_disabled);
            }
            let objective = self.lesson.as_ref().filter(|_| shared.objective_hud).and_then(|l| {
                // Before evaluation starts, the first objective (the race template never evaluates its own).
                let current = l.position.map_or(l.order.first().copied(), |_| l.current());
                let current = current.map(|i| &l.objectives[i].text).filter(|t| !t.is_empty());
                let text = current.or_else(|| l.master.map(|m| &l.objectives[m].text))?;
                Some(match (l.master, shared.fraction) {
                    (Some(_), Some(n)) => format!("{text}  {n}/{}", l.order.len()),
                    _ => text.clone(),
                })
            });
            let mut indicators = shared.indicators.clone();
            if let Some(l) = &self.lesson {
                indicators.extend(l.current().into_iter().flat_map(|i| l.objectives[i].indicators.iter().cloned()));
            }
            (objective, indicators, shared.pause_disabled)
        };
        // A replay shows no HUD (skate2_hud hides under a scripted replay too).
        self.hud.objective = objective.filter(|_| !matches!(self.playing, Some(Playing::Highlight { .. })));
        // The challenge HUD components: a removed timer holds where it stopped (no Go!, no last-seconds tick); one that
        // ran out, or never started, reads 0.
        let (countdown, timer, distance) = {
            let s = self.shared.lock().unwrap();
            let left = |id: i64| {
                let running = self.timers.iter().find(|(t, _)| *t == id).map(|(_, at)| (at - self.now).max(0.));
                running.or_else(|| self.stopped.iter().find(|(t, _)| *t == id).map(|(_, left)| *left)).unwrap_or(0.)
            };
            let countdown = s.countdown.map(|id| (id, left(id).ceil() as i64));
            (countdown, s.hud_timer.filter(|_| s.timer_shown).map(left), s.distance.filter(|_| s.distance_shown))
        };
        self.place_cast(world);
        let wanted = self.guides(&indicators);
        write_guides(&mut world.resource_mut::<crate::skate2_arrows::Arrows>(), wanted, &mut self.owns_arrows);
        let waypoint = self.waypoint.as_ref().zip(self.data.as_ref()).and_then(|(l, d)| d.locator(l)).map(|m| [m[3][0], m[3][2]]);
        let lessons = self.data.as_ref().map_or_else(Vec::new, |d| minimap_markers(d, &self.shared.lock().unwrap().progress.active));
        let mut marks = world.resource_mut::<crate::skate2_hud::Skate2Waypoint>();
        marks.0 = waypoint;
        if marks.1 != lessons {
            marks.1 = lessons;
        }
        if let Some(mut input) = world.get_resource_mut::<crate::input::ControllerInput>() {
            input.pinned = self.pinned;
        }
        let running = self.lesson.is_some() || !self.triggers.is_empty() || self.playing.is_some() || !self.queue.is_empty();
        // Retail's ambience plays on under a scene's speech (not under a menu opened over it).
        let menu = world.get_resource::<crate::graphics_menu::Menu>().is_some_and(|m| m.open);
        let scene = matches!(self.playing, Some(Playing::Scene { .. } | Playing::Flash | Playing::Highlight { .. } | Playing::Reel { .. }));
        world.resource_mut::<crate::skate_audio::CutsceneHold>().0 = scene && !menu;
        *world.resource_mut::<TutorialMenu>() = TutorialMenu {
            available: self.data.is_some(),
            busy: pause_disabled
                || matches!(
                    self.playing,
                    Some(
                        Playing::Movie { .. }
                            | Playing::Dialog { .. }
                            | Playing::Customiser
                            | Playing::Reel { .. }
                            | Playing::Photos { .. }
                            | Playing::SignUp(_)
                            // A restart under it would drop the cut it comes back round for.
                            | Playing::Flash
                    )
                ),
        };
        // Only on our own edges: the Skate 1 binding shares the resource.
        if running != self.holds_lesson_menu {
            if let Some(mut menu) = world.get_resource_mut::<crate::challenge::LessonMenu>() {
                menu.active = running;
                menu.request = None;
            }
            self.holds_lesson_menu = running;
        }
        let hud = &self.hud;
        let lesson_title = self.lesson.as_ref().zip(self.data.as_ref()).map(|(l, d)| d.title(&l.name)).unwrap_or_default();
        let goofy = world.resource::<SkaterRuntime>().animation.goofy();
        let photo = match self.playing {
            Some(Playing::Photos { index, forward }) => Some((index, forward)),
            _ => None,
        };
        let (count, shown) = {
            let shots = &world.resource::<Shots>().0;
            (shots.len().max(1), photo.and_then(|(i, _)| shots.get(i).or(shots.last()).cloned()))
        };
        let view = View {
            objective: hud.objective.clone().map(|line| (lesson_title, line)),
            instruction: hud.instruction.clone().map(|text| (hud.gesture.clone(), text)),
            subtitle: hud.subtitle.clone(),
            card: hud.card.as_ref().map(|(title, description, _)| (title.clone(), description.clone())),
            signup: hud.signup.zip(self.data.as_ref()).map(|(l, d)| (d.title(l), d.description(l), d.poster_movie(l).map(|(_, label)| label))),
            result: hud.central.as_ref().map(|(banner, _)| (banner.clone(), hud.central_title.clone(), hud.central_kind)),
            popup: hud.dialog.clone(),
            goofy,
            countdown,
            timer,
            distance,
            photos: photo.map(|(i, forward)| (i, count, forward)),
            sign_up: hud.prompt.clone(),
            unlocks: hud.unlocks.clone(),
            skate101: hud.skate101.clone(),
        };
        let on_screen = self.screens.sync(world, &view);
        // Plain text for the challenge HUD components the screens aren't showing.
        let race = {
            let countdown = countdown.filter(|_| !on_screen.countdown).map(|(_, left)| countdown_text(left).trim_start_matches('#').to_owned());
            let timer = timer.filter(|_| !on_screen.timer).map(|left| timer_text(left).trim_start_matches('#').to_owned());
            let distance = distance.filter(|_| !on_screen.distance).map(|(limit, metres)| format!("{metres:.1} m / {limit:.0} m"));
            let lines: Vec<String> = [countdown, timer, distance].into_iter().flatten().collect();
            (!lines.is_empty()).then(|| lines.join("\n"))
        };
        // The photo pick: the chosen photo over the replay; photoselect (or, without it, a caption) around it.
        let caption = photo.filter(|_| !on_screen.photos).map(|(i, _)| {
            let title = self.data.as_ref().map(|d| d.text(&self.shared.lock().unwrap().results_title)).unwrap_or_default();
            format!("{title}\n{} / {}\n[Left / Right] Choose   [A / Enter] Keep   [B / Backspace] Retry", i + 1, count)
        });
        let mut photos = world.query_filtered::<(&mut ImageNode, &mut Node), With<PhotoView>>();
        for (mut image, mut node) in photos.iter_mut(world) {
            // The picked photo stays up through photoselect's outro.
            let display = if shown.is_some() || self.screens.picking() { Display::Flex } else { Display::None };
            if node.display != display {
                node.display = display;
            }
            if let Some(handle) = &shown
                && image.image != *handle
            {
                image.image = handle.clone();
            }
        }
        let hud = &self.hud;
        let mut slots = world.query::<(&Slot, &mut Node, Option<&mut Text>)>();
        for (slot, mut node, text) in slots.iter_mut(world) {
            let shown = match slot {
                // Plain text stands in for whatever the screens aren't showing.
                Slot::Objective => hud.objective.clone().filter(|_| !on_screen.objective),
                Slot::Instruction => hud.instruction.clone().filter(|_| !on_screen.instruction),
                Slot::Central => (hud.central.as_ref().filter(|_| !on_screen.result).map(|c| c.0.clone()))
                    .or_else(|| hud.card.as_ref().filter(|_| !on_screen.card).map(|c| c.0.clone())),
                Slot::Subtitle => hud.subtitle.clone().filter(|_| !on_screen.subtitle),
                Slot::Dialog => hud.dialog.as_ref().filter(|_| !on_screen.popup).map(popup_text)
                    .or_else(|| hud.skate101.as_ref().filter(|_| !on_screen.skate101).map(|(t, b)| format!("{t}\n\n{b}\n\n[A / Enter]")))
                    .or_else(|| hud.unlocks.as_ref().filter(|_| !on_screen.unlocks).map(|c| format!("{}\n\n[A / Enter]", c.join("\n")))),
                Slot::Prompt => hud.prompt.as_ref().filter(|_| !on_screen.sign_up).map(|t| format!("{t}\n[D-pad Up / Up] Sign up")).or_else(|| {
                    let (title, description, movie) = view.signup.as_ref().filter(|_| !on_screen.signup)?;
                    let movie = movie.as_ref().map_or(String::new(), |m| format!("[X] {m}\n"));
                    Some(format!("{title}\n\n{description}\n\n{movie}[A / Enter] Accept   [B / Backspace] Decline"))
                }),
                Slot::Photo => caption.clone(),
                Slot::Race => race.clone(),
            };
            let display = if shown.is_some() { Display::Flex } else { Display::None };
            if node.display != display {
                node.display = display;
            }
            if let (Some(mut text), Some(shown)) = (text, shown) {
                if text.0 != shown {
                    text.0 = shown;
                }
            }
        }
    }
}

impl Machine {
    /// Drops a progression trigger whose handler failed.
    fn terminate(&mut self) {
        let _ = self.lua.load("TerminateWithoutSave()").exec();
    }
}

/// The SignUpHUD's button: D-pad Up (or a keyboard's Up), just pressed. Not the menu navigation's up, which counts the
/// left stick (riding forward into the box signed up, user 10-13) and repeats a held direction.
fn sign_up_pressed(keys: &ButtonInput<KeyCode>, nav: Option<&crate::customiser::Navigation>) -> bool {
    keys.just_pressed(KeyCode::ArrowUp) || nav.is_some_and(|n| n.dpad & 1 != 0)
}

fn skater_position(world: &World) -> Vec3 {
    let m = &world.resource::<SkaterRuntime>().animated_skeleton.roots.animation_to_world;
    Vec3::new(m[3][0], m[3][1], m[3][2])
}

impl Tutorial {
    /// Cast wanted now: the running lesson's GlobalSpawnCharacterInfo plus the pros that have arrived.
    fn place_cast(&self, world: &mut World) {
        let Some(data) = &self.data else { return };
        let mut wanted: BTreeMap<String, [[f32; 4]; 4]> = BTreeMap::new();
        // A photo shows only its scene's actors: Not retail (user 10-13, bug 6), what retail's reel hides is UNTRACED.
        let photo = self.photographing();
        let spawns = self.lesson.as_ref().filter(|_| !photo).and_then(|l| data.challenges.get(&l.name)).map(|c| &c.fields["GlobalSpawnCharacterInfo"]);
        for spawn in spawns.and_then(Value::as_array).into_iter().flatten() {
            let word = |i: usize| spawn.get(i).and_then(Value::as_u64).unwrap_or(0);
            let Some((_, key)) = SPAWN_CHARACTERS.iter().find(|(id, _)| *id == (word(2), word(3))) else { continue };
            // [6] is the spawn locator; Pumping's (challengelocator02) is not on the disc, so [8], its start locator.
            let at = [6, 8].iter().filter_map(|&i| spawn.get(i).and_then(Value::as_str)).find_map(|l| data.locator(l));
            if let Some(at) = at {
                wanted.insert((*key).to_owned(), at);
            }
        }
        // A lesson's own spawn (Atiba at the photo spot) wins over where a pro has stood since arriving.
        for (key, locator) in self.pros.iter().filter(|_| !photo) {
            if let Some(at) = data.locator(locator) {
                wanted.entry(key.clone()).or_insert(at);
            }
        }
        // A cutscene's actors not already standing somewhere appear at the scene's base.
        for (key, at) in world.resource::<Cutscene>().actors() {
            wanted.entry(key).or_insert(at);
        }
        // The competitors are their own copies of the pros (the stand pros leave on their own beat).
        let mut riders = Vec::new();
        for (&id, ai) in self.ai.iter().filter(|_| !photo) {
            let Some(lesson) = &self.lesson else { break };
            let competitor = usize::try_from(id - 1).ok().and_then(|i| data.challenges.get(&lesson.name)?.fields["Competitors"].get(i));
            let (Some(model), Some(at)) = (competitor.and_then(|c| c[0].as_str()), data.spawn_point(&ai.locator, id)) else { continue };
            let key = format!("{model}#ai{id}");
            // The lesson's own spawn of the same character (Slappy at the follow start) gives way to the rider.
            wanted.remove(model);
            wanted.insert(key.clone(), at);
            // A full-skater rider is posed in world space (skate2_cast pose): its root stays at the origin.
            let riding = world.get_resource::<Riders>().is_some_and(|r| r.get(&key).is_some());
            riders.push((key, if riding { Some(Transform::IDENTITY) } else { ai.path.and_then(|_| ai.at(data, &lesson.name, id, self.now)) }));
        }
        let mut cast = world.resource_mut::<Cast>();
        if cast.wanted != wanted {
            cast.wanted = wanted;
        }
        cast.riding = riders.iter().map(|(key, _)| key.clone()).collect();
        let roots: Vec<_> = riders.into_iter().filter_map(|(key, at)| Some((cast.entity(&key)?, at?))).collect();
        for (root, at) in roots {
            if let Some(mut transform) = world.get_mut::<Transform>(root) {
                *transform = at;
            }
        }
    }
}

/// Competitor `id`'s rider and cast key in `lesson`: "{model}#ai{id}".
fn rider_key(data: &Data, lesson: &str, id: i64) -> Option<String> {
    let competitor = data.challenges.get(lesson)?.fields["Competitors"].get(usize::try_from(id - 1).ok()?)?;
    Some(format!("{}#ai{id}", competitor[0].as_str()?))
}

/// round2 T5: each competitor's driver reads its rider after the player's tick and writes the rider's signals for its
/// own; riders whose competitor has gone (a lesson's end, UnspawnAI) leave.
fn drive(mut tutorial: ResMut<Tutorial>, riders: Option<ResMut<Riders>>, skater: Res<SkaterRuntime>) {
    let Some(mut riders) = riders else { return };
    let tutorial = &mut *tutorial;
    riders.retain(|key| keeps_rider(&tutorial.ai, key));
    riders.warm |= tutorial.data.is_some(); // D4: a lesson's AI may spawn: keep its load ready
    let (Some(data), Some(lesson)) = (tutorial.data.clone(), &tutorial.lesson) else { return };
    let Some(paths) = data.ai_segments.get::<str>(&lesson.name) else { return };
    let m = &skater.animated_skeleton.roots.animation_to_world;
    let player = Player { position: Vec3::new(m[3][0], m[3][1], m[3][2]), wiping_out: skater.wiping_out() };
    let driving: Vec<(i64, (usize, usize))> =
        tutorial.ai.iter().filter_map(|(&id, ai)| Some((id, ai.driver.as_ref().map(|d| (d.path(), d.node()))?))).collect();
    for (&id, ai) in &mut tutorial.ai {
        let (Some(driver), Some(key)) = (ai.driver.as_mut(), &ai.key) else { continue };
        let Some(rider) = riders.get_mut(key).filter(|r| !r.failed()) else { continue };
        let others: Vec<_> = driving.iter().filter(|(other, _)| *other != id).map(|(_, at)| *at).collect();
        let step = driver.step(paths, rider.body(), &others, Some(&player));
        rider.intents = step.intents;
        if let Some(at) = step.reposition {
            rider.travel_to(at);
        }
    }
}

/// The riders the S2 tick keeps: its `#ai` competitors while their AI is live, and every other key (Skate 1's
/// `#s1ai` riders, skate1_rider).
fn keeps_rider(ai: &BTreeMap<i64, Ai>, key: &str) -> bool {
    !key.contains("#ai") || ai.values().any(|ai| ai.key.as_deref() == Some(key))
}

/// Competitor `id`'s lap in `lesson` (FilmAIID i -> the i-th path).
fn lap<'a>(data: &'a Data, lesson: &str, id: i64) -> Option<&'a [[f32; 5]]> {
    let lap = data.ai_paths.get(lesson)?.get(usize::try_from(id - 1).ok()?)?;
    (lap.len() >= 2).then_some(&lap[..])
}

/// How far retail's AI path samples sit above the ground they were recorded over: 0.086-0.089 m on every rolling
/// sample of Follow Slappy's lap over this map's collision (round2-research R2, pathground.py).
const PATH_ABOVE_GROUND: f32 = 0.087;

/// Where a competitor is `t` seconds into its lap, facing along it (the skater faces +Z): a closed loop wraps, a
/// route holds at its end. The samples are sparse keys on a 60 Hz grid: lerped.
fn ride(lap: &[[f32; 5]], t: f32, lift: f32, loops: bool) -> Option<Transform> {
    let length = lap.last()?[0];
    if lap.len() < 2 || length <= 0. {
        return None;
    }
    let t = if loops { t.rem_euclid(length) } else { t.clamp(0., length) };
    let i = lap.partition_point(|s| s[0] <= t).clamp(1, lap.len() - 1);
    let point = |s: &[f32; 5]| Vec3::new(s[1], s[2] + lift, s[3]);
    let (a, b) = (&lap[i - 1], &lap[i]);
    let here = point(a).lerp(point(b), ((t - a[0]) / (b[0] - a[0]).max(1e-4)).clamp(0., 1.));
    // Standing still (a key repeated), face where the lap goes next; at a route's end, the way it came.
    let rest: &[[f32; 5]] = if loops { lap } else { &[] };
    let ahead = lap[i..].iter().chain(rest).map(point).find(|p| (*p - here).with_y(0.).length() > 0.05);
    let way = ahead.map_or(point(b) - point(a), |p| p - here);
    Some(Transform::from_translation(here).with_rotation(Quat::from_rotation_y(way.x.atan2(way.z))))
}

/// Hands a scripted replay (the highlight reel) back to gameplay and drops the photos.
fn close_replay(world: &mut World) {
    let mut replay = world.resource_mut::<Replay>();
    if replay.scripted {
        replay.exit();
    }
    world.resource_mut::<Shots>().0.clear();
}

/// Snaps the frame being drawn as one of the reel's photos: the world alone (graphics_menu::SceneTarget), under the HUD,
/// prompts and FPS counter the output camera draws over it.
fn snap(world: &mut World) {
    let shot = world.get_resource::<crate::graphics_menu::SceneTarget>().map_or_else(Screenshot::primary_window, |t| Screenshot::image(t.0.clone()));
    world.spawn(shot).observe(
        |captured: On<ScreenshotCaptured>, mut images: ResMut<Assets<Image>>, mut shots: ResMut<Shots>| {
            shots.0.push(images.add(half(&captured.image)));
            debug!("Skate 2 tutorial: photo {} captured", shots.0.len());
        },
    );
}

/// An image at half size (every other pixel of a 4-byte format), as a photo needs no more.
fn half(image: &Image) -> Image {
    use bevy::{
        asset::RenderAssetUsages,
        render::render_resource::{Extent3d, TextureDimension},
    };
    let (w, h) = (image.width() as usize, image.height() as usize);
    let Some(data) = image.data.as_ref().filter(|d| d.len() == w * h * 4) else { return image.clone() };
    let (hw, hh) = (w / 2, h / 2);
    let mut out = Vec::with_capacity(hw * hh * 4);
    for y in 0..hh {
        for x in 0..hw {
            let i = (y * 2 * w + x * 2) * 4;
            out.extend_from_slice(&data[i..i + 4]);
        }
    }
    let size = Extent3d { width: hw as u32, height: hh as u32, depth_or_array_layers: 1 };
    Image::new(size, TextureDimension::D2, out, image.texture_descriptor.format, RenderAssetUsages::RENDER_WORLD)
}

fn teleport(world: &mut World, data: &Data, locator: &str) -> bool {
    // The player: id 0, so GetSpawnLocation's index 0 (every tutorial call passes 0 or LocalPlayerID).
    let Some(matrix) = data.spawn_point(locator, 0) else {
        warn!("Skate 2 tutorial: no locator {locator}");
        return false;
    };
    match world.resource_mut::<SkaterRuntime>().travel_to(matrix) {
        Ok(()) => true,
        Err(error) => {
            warn!("Skate 2 tutorial: teleport to {locator}: {error}");
            false
        }
    }
}

fn teleport_done(world: &World, elapsed: f64) -> bool {
    elapsed >= TELEPORT_TIMEOUT
        || elapsed >= TELEPORT_MIN && world.resource::<SkaterRuntime>().player_input.pending_teleport().is_none()
}

fn subtitle(data: Option<&Data>, cues: &[Cue], t: f64) -> Option<String> {
    let cue = cues.iter().find(|c| c.start <= t && t < c.end)?;
    Some(data?.text(&cue.label))
}

/// A popup as plain text, with its buttons.
/// An objective's `{0:time}` / `{0:distance}` with its argument ("Manual for {0:time}", 2 -> "Manual for 2 sec").
fn fill(text: &str, arg: &str) -> String {
    text.replace("{0:time}", &format!("{arg} sec")).replace("{0:distance}", &format!("{arg} m"))
}

fn popup_text(p: &Popup) -> String {
    let buttons = match p.options.as_slice() {
        [yes, no, ..] => format!("[A / Enter] {yes}      [B / Backspace] {no}"),
        [only] => format!("[A / Enter] {only}"),
        [] => String::new(),
    };
    [p.title.as_str(), p.body.as_str(), &buttons].into_iter().filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n\n")
}

#[derive(Component, Clone, Copy, PartialEq)]
enum Slot {
    Objective,
    Instruction,
    Central,
    Subtitle,
    Dialog,
    Prompt,
    Photo,
    /// The race's countdown, timer and distance to the leader where their HUD movies (hudcountdown, hudtimer,
    /// huddistance) can't show.
    Race,
}

#[derive(Component)]
struct PhotoView;

fn setup_ui(mut commands: Commands) {
    let text = |slot: Slot, size: f32, node: Node, background: f32| {
        (
            slot,
            Text::new(""),
            TextFont { font_size: size, ..default() },
            TextLayout::new_with_justify(Justify::Center),
            BackgroundColor(Color::srgba(0., 0., 0., background)),
            Node { position_type: PositionType::Absolute, display: Display::None, padding: UiRect::all(px(8)), ..node },
        )
    };
    let wide = |top: Option<f32>, bottom: Option<f32>| Node {
        left: percent(15),
        width: percent(70),
        top: top.map_or(Val::Auto, percent),
        bottom: bottom.map_or(Val::Auto, percent),
        justify_content: JustifyContent::Center,
        ..default()
    };
    commands
        .spawn((
            Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() },
            GlobalZIndex(50),
            Pickable::IGNORE,
        ))
        .with_children(|root| {
            root.spawn(text(Slot::Objective, 22., Node { top: percent(6), left: percent(4), ..default() }, 0.5));
            root.spawn(text(Slot::Central, 40., wide(Some(28.), None), 0.));
            root.spawn(text(Slot::Race, 26., Node { top: percent(6), right: percent(4), ..default() }, 0.5));
            // Retail's "<title> / D-pad Sign up" sits just under mid-screen, bare white text.
            root.spawn(text(Slot::Prompt, 22., wide(Some(45.), None), 0.));
            root.spawn(text(Slot::Instruction, 24., wide(None, Some(22.)), 0.5));
            root.spawn(text(Slot::Subtitle, 22., wide(None, Some(13.)), 0.));
            root.spawn(text(Slot::Dialog, 26., wide(Some(35.), None), 0.85));
            root.spawn((
                PhotoView,
                ImageNode::default(),
                Node {
                    position_type: PositionType::Absolute,
                    display: Display::None,
                    left: percent(15),
                    width: percent(70),
                    top: percent(6),
                    height: percent(70),
                    ..default()
                },
            ));
            root.spawn(text(Slot::Photo, 22., wide(None, Some(4.)), 0.6));
        });
}

/// The tutorial's guides into skate2_arrows, only while it owns the list. publish runs every frame on every map, and
/// with no tutorial data its empty list wiped the Skate 3 modes' arrows each frame (S.K.A.T.E., Spot Battle).
fn write_guides(arrows: &mut crate::skate2_arrows::Arrows, wanted: Vec<(String, [[f32; 4]; 4])>, owned: &mut bool) {
    if wanted.is_empty() && !*owned {
        return;
    }
    *owned = !wanted.is_empty();
    if wanted.iter().any(|(l, _)| l.starts_with("signup:")) && !arrows.more.iter().any(|m| m == SIGNUP_INDICATORS) {
        arrows.more.push(SIGNUP_INDICATORS.into());
    }
    if arrows.wanted != wanted {
        arrows.wanted = wanted;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// User 10-31 (Party Play S.K.A.T.E. on Campus): no arrow at the spot. The tutorial's publish ran every frame
    /// with no lesson data and wrote its empty list over the mode's arrows. Now it leaves a list it does not own, and
    /// gives back (once) one it did.
    #[test]
    fn a_tutorial_with_no_guides_leaves_another_modes_arrows() {
        let at = [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [167., 67., -182., 1.]];
        let skate = vec![("skate_univ_01_vis_1".to_owned(), at)];
        let mut arrows = crate::skate2_arrows::Arrows { wanted: skate.clone(), ..default() };
        let mut owned = false;
        for _ in 0..3 {
            write_guides(&mut arrows, Vec::new(), &mut owned);
        }
        assert_eq!(arrows.wanted, skate, "S.K.A.T.E.'s arrow stays");
        let guide = vec![("guide".to_owned(), at)];
        write_guides(&mut arrows, guide.clone(), &mut owned);
        assert_eq!((arrows.wanted.clone(), owned), (guide, true));
        write_guides(&mut arrows, Vec::new(), &mut owned);
        assert!(arrows.wanted.is_empty() && !owned, "its own guides go back when the lesson ends");
    }

    /// Skate 1 people (L3): the S2 tick still drops an `#ai` rider whose AI left and keeps its live one; Skate 1's
    /// `#s1ai` riders are not its to drop.
    #[test]
    fn the_s2_tick_drops_only_its_own_riders() {
        let ai = BTreeMap::from([(1, Ai { key: Some("slappy#ai1".into()), ..Ai::default() })]);
        assert!(keeps_rider(&ai, "slappy#ai1") && !keeps_rider(&ai, "slappy#ai2"));
        assert!(keeps_rider(&ai, "IP_shingo#s1ai1"));
    }

    /// User 10-09: switching to New San Van online started the Skate 2 tutorial. It never runs in a session.
    #[test]
    fn the_tutorial_never_loads_online_and_comes_back_after_leaving() {
        let mut world = World::new();
        world.insert_resource(CurrentMap {
            path: Some(PathBuf::from(format!("maps/{MAP}.skate"))),
            name: MAP.into(),
            spawn: [0.; 3],
            heading: 0.,
            generation: 1,
        });
        world.insert_resource(crate::multiplayer::Online(true));
        world.init_resource::<Cutscene>();
        world.init_resource::<Time<Virtual>>();
        world.init_resource::<Assets<Image>>();
        world.init_resource::<Replay>();
        world.init_resource::<Shots>();
        let mut tutorial = Tutorial::default();
        tutorial.reload(&mut world);
        assert!(tutorial.data.is_none() && tutorial.pending.is_empty(), "online: no tutorial");
        assert_eq!(tutorial.loaded, Some((1, true)));
        // Leaving re-decides on the same map (generation unchanged); off NSV it settles without loading.
        world.resource_mut::<CurrentMap>().path = Some(PathBuf::from("maps/Other.skate"));
        world.insert_resource(crate::multiplayer::Online(false));
        tutorial.reload(&mut world);
        assert_eq!(tutorial.loaded, Some((1, false)));
    }

    /// User 11-01 (Party Play S.K.A.T.E. "Can You Spell Girl?", bugs 1 + 5): no arrow at the spot. The tutorial stayed
    /// loaded through Party Play on New San Van, and between lessons Meet Slappy's sign-up took the arrows' list back
    /// from the mode's, which writes it once. The session unloads the tutorial, as online does; the mode's arrow stays.
    #[test]
    fn party_play_unloads_the_tutorial_and_leaves_the_modes_arrows() {
        let identity = [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [0., 0., 0., 1.]];
        let data: Arc<Data> = Arc::new(serde_json::from_value(json!({
            "frontend": [], "progression": {"triggers": {}, "messages": {}, "entries": {}},
            "challenges": {"m_1a_01_goto_slappys": {"self": {"Location": "J_Gamestart"}, "steps": {}, "objectives": []}},
            "placement": {"locators": {"J_Gamestart": identity}, "volumes": {}}, "scenes": {}, "movies": {}, "speech": {}, "trick_groups": {}
        })).unwrap());
        let mut world = World::new();
        world.insert_resource(CurrentMap {
            path: Some(PathBuf::from(format!("maps/{MAP}.skate"))),
            name: MAP.into(),
            spawn: [0.; 3],
            heading: 0.,
            generation: 1,
        });
        world.insert_resource(crate::multiplayer::Online(false));
        world.init_resource::<Cutscene>();
        world.init_resource::<Time<Virtual>>();
        world.init_resource::<Assets<Image>>();
        world.init_resource::<Replay>();
        world.init_resource::<Shots>();
        world.init_resource::<crate::party_play::PartyPlay>();
        // Free skate on New San Van: Meet Slappy is open, so the tutorial owns the list with its sign-up.
        let mut tutorial = Tutorial { data: Some(data), loaded: Some((1, false)), ..default() };
        tutorial.shared.lock().unwrap().progress.activate(MEET_SLAPPY);
        let mut arrows = crate::skate2_arrows::Arrows::default();
        write_guides(&mut arrows, tutorial.guides(&[]), &mut tutorial.owns_arrows);
        assert_eq!(arrows.wanted.len(), 1, "Meet Slappy's sign-up");
        // A Party Play session on the same map; skate2_skate_game::begin writes the spot's arrow once.
        world.resource_mut::<crate::party_play::PartyPlay>().active = true;
        let spot = vec![("m_4a_01_skate_carroll_koston_vi01".to_owned(), identity)];
        for frame in 0..3 {
            tutorial.reload(&mut world);
            write_guides(&mut arrows, tutorial.guides(&[]), &mut tutorial.owns_arrows);
            if frame == 0 {
                arrows.wanted = spot.clone();
            }
        }
        assert!(tutorial.data.is_none(), "no tutorial in a Party Play session");
        assert_eq!(arrows.wanted, spot, "the mode's arrow stays");
    }

    /// v12 #10: a map load reset the tutorial, and its stop ended whatever scene was starting: Campus S.K.A.T.E.'s
    /// intro, begun the frame the map arrived. The tutorial stops only scenes of its own.
    #[test]
    fn a_map_load_leaves_another_modes_scene_playing() {
        let mut world = World::new();
        world.insert_resource(CurrentMap {
            path: Some(PathBuf::from("maps/University.skate")),
            name: "University".into(),
            spawn: [0.; 3],
            heading: 0.,
            generation: 1,
        });
        world.insert_resource(crate::multiplayer::Online(false));
        world.init_resource::<Cutscene>();
        world.init_resource::<Time<Virtual>>();
        world.init_resource::<Assets<Image>>();
        world.init_resource::<Replay>();
        world.init_resource::<Shots>();
        world.resource_mut::<Cutscene>().start(Path::new("private/skate3-online/skate"), "skate_univ_02");
        let mut tutorial = Tutorial::default();
        tutorial.reload(&mut world);
        assert!(world.resource::<Cutscene>().active(), "the S.K.A.T.E. intro plays on");
    }

    #[test]
    fn competitors_ride_their_lap_and_wrap() {
        let lap = [[0., 0., 0., 0., 0.], [1., 0., 0., 2., 0.], [1.5, 0., 0., 2., 0.], [2., 2., 0., 2., 0.]];
        let at = |t| ride(&lap, t, 0.5, true).unwrap();
        assert!(at(0.5).translation.abs_diff_eq(Vec3::new(0., 0.5, 1.), 1e-4));
        assert!((at(0.5).rotation * Vec3::Z).abs_diff_eq(Vec3::Z, 1e-4), "faces along +Z");
        // Still between the repeated keys: faces where the lap goes next (+X).
        assert!((at(1.2).rotation * Vec3::Z).abs_diff_eq(Vec3::X, 1e-4));
        assert!(at(2.5).translation.abs_diff_eq(at(0.5).translation, 1e-4), "wraps");
        // A route holds at its end, facing the way it came (+X).
        let end = ride(&lap, 9., 0.5, false).unwrap();
        assert!(end.translation.abs_diff_eq(Vec3::new(2., 0.5, 2.), 1e-4));
        assert!((end.rotation * Vec3::Z).abs_diff_eq(Vec3::X, 1e-4));
    }

    /// Only the photo shots judge a landed sequence; the rest carry a no-op PointsValid and their hints fire on edges
    /// (goto_slappys' Push/Turn never showed while they waited for a landing).
    #[test]
    #[ignore = "requires the converted Skate 2 tutorial"]
    fn only_photo_objectives_are_points_gated() {
        let data = Arc::new(Data::load(PathBuf::from(std::env::var("SKATE2_TUTORIAL_DIR").unwrap())).unwrap());
        let shared = Arc::new(Mutex::new(Shared::default()));
        let gated = |name| Lesson::new(&data, name, &shared).unwrap().objectives[0].points.is_some();
        assert!(!gated("m_1a_01_goto_slappys"));
        assert!(gated("m_1a_05_photo_rail"));
    }

    /// G8 on the disc's own Lua: the gap dumpster at its retail pose (the prop centre physics/props.rs's data test
    /// measures) keeps Meet Slappy's moveable objective open; moved out of both freedom volumes, or the player in the
    /// exit volume, completes it; and the grab hint sees OffBoardPushing.
    #[test]
    #[ignore = "requires the converted Skate 2 tutorial"]
    fn moving_the_gap_dumpster_completes_meet_slappys_moveable() {
        let data = Arc::new(Data::load(PathBuf::from(std::env::var("SKATE2_TUTORIAL_DIR").unwrap())).unwrap());
        let shared = Arc::new(Mutex::new(Shared::default()));
        let name = "m_1a_01_goto_slappys";
        let lesson = Lesson::new(&data, name, &shared).unwrap();
        let i = data.challenges[name].objectives.iter().position(|o| o["Name"] == "m_1a_01_goto_slappys_moveable").unwrap();
        let o = &lesson.objectives[i];
        let volumes = |p: Vec3| data.placement.volumes.iter().filter(|(_, v)| v.contains(p)).map(|(n, _)| n.clone()).collect();
        let sense = |dumpster: Option<Vec3>, player: Vec3, grabbing: bool| {
            let mut s = shared.lock().unwrap();
            s.sense.dmos = dumpster.map(|c| (0xFDA7_2725_7834_E7A3, volumes(c))).into_iter().collect();
            (s.sense.inside, s.sense.grabbing) = (volumes(player), grabbing);
        };
        let met = |c: &Condition| c.evaluate(&lesson.machine).unwrap();
        let (retail, near, exit) =
            (Vec3::new(1697.162, 108.183, -539.224), Vec3::new(1700., 108.5, -545.), Vec3::new(1694.5, 109., -538.));
        sense(Some(retail), near, false);
        assert!(!met(&o.condition), "retail pose: still to move");
        // The grab hint (hints[1], grabbeddmo) holds while pushing next to it.
        assert!(!met(&o.hints[1].condition));
        sense(Some(retail), near, true);
        assert!(met(&o.hints[1].condition));
        // Pushed 2 m along its length (its x axis is (-0.5736, 0, -0.8192)): out of both freedom volumes.
        sense(Some(retail + Vec3::new(1.147, 0., 1.638)), near, false);
        assert!(met(&o.condition), "moved out");
        // No prop at all (unknown tester: never ready, always "in"), or never moved: the exit volume still completes it.
        sense(None, near, false);
        assert!(!met(&o.condition));
        sense(None, exit, false);
        assert!(met(&o.condition), "exit volume");
        sense(Some(retail), exit, false);
        assert!(met(&o.condition), "exit volume, dumpster in place");
    }

    /// I2 (online audit 10-06), B2 in the tutorial: getting off the board (Y), walking and getting back on raises the
    /// board manager's flag but is no wipeout, so m_1a_05 / m_1a_06's `wipeout` handler (FailAllObjectiveStatus,
    /// "Bailed", ChallengeResult Lose) never runs and Wipeout.IsWipingOut stays false (m_1a_06's ramp puzzle is solved
    /// on foot); a manual bail is one wipeout. `sense` runs on the engine's own skater (the mode harness).
    #[test]
    #[ignore = "requires SKATE3_ASSET_ROOT (install assets, read-only)"]
    fn getting_off_the_board_is_no_tutorial_wipeout() {
        use crate::physics::mode_harness::{Harness, Spawn, pads};
        use skate_core::input::xbox::XboxState;
        /// Ticks the harness and runs `sense` after each tick on a World holding its physics and skater: per tick
        /// (the board manager's flag, off the board, Wipeout.IsWipingOut).
        fn run(mut h: Harness, w: &mut World, t: &mut Tutorial, ticks: u64, pad: impl Fn(u64) -> XboxState) -> (Harness, Vec<[bool; 3]>) {
            let mut out = Vec::new();
            for tick in 0..ticks {
                let off = h.tick(pad(tick)).state_category == 500;
                let flag = h.physics.board_wiping_out;
                w.insert_resource(h.physics);
                w.insert_resource(h.skater);
                t.sense(w);
                (h.physics, h.skater) = (w.remove_resource().unwrap(), w.remove_resource().unwrap());
                out.push([flag, off, t.shared.lock().unwrap().sense.wiping]);
            }
            (h, out)
        }
        let wipeouts = |t: &Tutorial| t.pending.iter().filter(|d| matches!(d, Delivery::Lesson("wipeout", _))).count();
        let data: Arc<Data> = Arc::new(serde_json::from_value(json!({
            "frontend": [], "progression": {"triggers": {}, "messages": {}, "entries": {}}, "challenges": {},
            "placement": {"locators": {}, "volumes": {}}, "scenes": {}, "movies": {}, "speech": {}, "trick_groups": {}
        })).unwrap());
        let mut tutorial = Tutorial { data: Some(data), ..default() };
        let mut world = World::new();
        world.init_resource::<TrickLog>();
        let mut h = Harness::load("University", Spawn::Package);
        h.run(60, |_| pads::idle());
        // Y off at 20, walk, Y back on at 300 (the harness survey's dismount + remount).
        let (h, ticks) = run(h, &mut world, &mut tutorial, 500, |t| match t {
            20 | 300 => pads::buttons(pads::Y),
            40..200 => XboxState { left: [0, 24000], ..pads::idle() },
            _ => pads::idle(),
        });
        let up = |i: usize| ticks.iter().filter(|s| s[i]).count();
        eprintln!("dismount: board flag {} ticks, off board {}, IsWipingOut {}", up(0), up(1), up(2));
        assert!(up(0) > 0 && up(1) > 0, "the dismount raised the board manager's flag and left the board");
        assert_eq!((up(2), wipeouts(&tutorial)), (0, 0), "a dismount read as a wipeout");
        let (_, ticks) = run(h, &mut world, &mut tutorial, 600, |t| if t < 30 { pads::bail() } else { pads::idle() });
        assert!(ticks.iter().any(|s| s[2]), "the bail is IsWipingOut");
        assert_eq!(wipeouts(&tutorial), 1, "one wipeout message per bail");
    }

    #[test]
    fn objectives_fill_their_argument() {
        assert_eq!(fill("Stay within {0:distance}", "50"), "Stay within 50 m");
        assert_eq!(fill("Manual for {0:time}", "2"), "Manual for 2 sec");
    }

    #[test]
    fn volumes_contain_their_box_and_names_drop_the_district() {
        let v = Volume { min: [0., 0., 0.], max: [2., 3., 4.] };
        assert!(v.contains(Vec3::new(1., 1., 1.)));
        assert!(!v.contains(Vec3::new(1., -0.1, 1.)));
        assert_eq!(volume_name("DIST_Projects|m_2a_01_tutorial_slappy_vol05"), "m_2a_01_tutorial_slappy_vol05");
    }

    /// Spec "static check": every native the disc's handlers and conditions call is implemented or a listed no-op
    /// (the census of the decompiled scripts and objective sources).
    #[test]
    fn every_referenced_native_is_bound() {
        const REFERENCED: &[&str] = &[
            "Online.GetLocalPlayerID", "Objectives.MakeObjectives", "Objectives.SetEvaluationStatus",
            "Objectives.ResetObjectives", "Objectives.ResetObjective", "Objectives.DeleteObjectiveGroup",
            "HUD2.SetupObjectiveHUDData", "HUD2.SetActiveGroup", "HUD2.CreateGroup", "HUD2.GetGroupHash", "HUD2.Show",
            "HUD2.DeleteGroup", "HUD2.ShowCentralMessage", "Pause.DisablePauseMenuAndBackpack",
            "Pause.EnablePauseMenuAndBackpack", "VisualIndicators.CreateGroup", "VisualIndicators.AddIndicator",
            "VisualIndicators.RemoveIndicator", "VisualIndicators.DeleteGroup", "ChallengePresentation.AddPresentationGroup",
            "ChallengePresentation.AddPresentationEvent", "ChallengePresentation.RemoveAllPresentationGroups",
            "ChallengePresentation.StartPresentation", "HOM.EnableHOMVisuals", "AI.SkaterDistanceFromLocation",
            "AI.RepositionAI", "Effects.StartEffect", "Challenge.CompleteChallenge", "Challenge.WinChallenge",
            "ChallengeAsset.IsChallengeAssetLoadComplete", "ChallengeAsset.ActivateChallengeAssetLoadScreen",
            "ChallengeAsset.DeactivateChallengeAssetLoadScreen", "ChallengeTimer.AddCountdownTimer",
            "DMOs.AddDMOVolumeTester", "DMOs.IsDMOInVolume", "DMOs.IsDMOTesterReady", "DMOs.IsGrabbingDMO",
            "DMOs.ReleaseDMOVolumeTester", "Manual.IsManualing", "Manual.NoseManualTime", "Manual.TailManualTime",
            "ObjectiveGameInfo.SetFractionValue", "ObjectiveHOM.GetWipeoutDuration",
            "ObjectiveInfo.GetTotalNumCompletedObjectives", "ObjectiveLocomotion.IsMoving",
            "ObjectiveLocomotion.IsOffBoard", "ObjectiveLocomotion.IsOnBoard", "ObjectiveLocomotion.IsStoppped",
            "ObjectiveLocomotion.SequenceCompleted", "ObjectiveSessionMarker.JustSetSessionMarker",
            "ObjectiveSessionMarker.JustUsedSessionMarker", "ObjectiveTrigger.EnteredVolume",
            "ObjectiveTrigger.IsInVolume", "Tricks.DidSpecificTrick", "Tricks.DidSpecificTrickGroup",
            "Tricks.DidSpecificTrickGroupThisFrame", "Tricks.DidSpecificTrickThisFrame", "Wipeout.IsWipingOut",
            // The spot-photo template (m_1a_05_photo_rail, m_1a_06_photo_puzzle) and its objectives.
            "AI.EnterAiBasedChallenge", "AI.FilterReset", "AI.LoadPaths", "AI.PinAI", "AI.SpawnAIAtLocation",
            "AI.StartPath", "AI.UnloadPaths", "AI.UnpinAI", "AI.UnspawnAI", "Challenge.AbandonChallenge",
            "Challenge.GetChallengeType", "Challenge.IsReplayingChallenge", "Challenge.LoseChallenge",
            "ChallengeReplay.GetLastFrameNumberWritten", "ChallengeSave.GetSaveData", "ChallengeSave.SetSaveData",
            "ChallengeSpeech.PlayNPCSpeechUsingAttribute", "ChallengeSpeech.PlayNPCSpeechUsingOverrideAttribute",
            "ChallengeTimer.AddHUDCountdownTimer", "ChallengeTimer.GetCountdownTime", "ChallengeTimer.RemoveCountdownTimer",
            "ChallengeTimer.ResetCountdownTimer", "Entity.DoesSkaterExist", "FrontEnd.OpenPhotoSelection",
            "FrontEnd.Results_SetMainTitle", "FrontEnd.Results_SetNumPlayers", "FrontEnd.SetEndChallengeOption",
            "HighlightReel.AddHighlightUsingDefinition", "HighlightReel.CreateHighlightGroup",
            "HighlightReel.DestroyHighlightGroup", "HighlightReel.PlayHighlightReel", "HighlightReel.RemoveAllHighlights",
            "NIS.GetPhotoNISCount", "NIS.KillNIS", "Objectives.AddLastUniqueTrickToList",
            "Objectives.ChangeObjectivePointsRequirement", "Objectives.FailAllObjectiveStatus",
            "Objectives.GetAccumulatedPoints", "Objectives.GetCurrentNumCompletedObjectives",
            "Objectives.GetFractionDenominator", "Objectives.GetNumObjectivesRequiresHighlight",
            "Objectives.GetObjectiveFailureHALText", "Objectives.GetObjectiveNISPlaybackDefinition",
            "Objectives.GetTotalNumObjectives", "Objectives.IsTrackedUniqueTrickGroup", "Objectives.SetFractionNumerator",
            "Pause.PauseSimulation", "Pause.UnpauseSimulation", "Service.ActivateSecurityServiceForChallengeReplay",
            "Service.DeactivateSecurityServiceForChallengeReplay", "SessionMarker.DisableSetSessionMarker",
            "SessionMarker.EnableSetSessionMarker", "SessionMarker.SetSessionMarker",
            "Teleport.TeleportLocalPlayerToSessionMarker", "Time.GetVisualGameTick", "VisualIndicators.ClearAllIndicators",
            "HUD2.ShowModifiedCentralMessage", "HUD2.CreatePreFilmSetupHUD", "HUD2.DeletePreFilmSetupHUD",
            "HUD2.StartCountdown", "HUD2.StartTimer", "HUD2.DPOSetIcon", "HUD2.DPOSetParams", "HUD2.SetABDGestureVisible",
            "HUD2.ShowTrickDisplayContext", "HUD2.ABDHUDClearList", "HUD2.ABDHUDRefreshFilmTrickList",
            "Grind.DidSpecificGrind", "Grind.GetTotalGrindLengthInSequence",
            "ObjectivePoints.IsSequenceNewlyCompletePoints", "ObjectivePoints.AwardSequencePointsImmediately",
            // The race template (m_1a_07_follow_slappy), single player.
            "AI.ClearBehavior", "AI.FilterPaths", "AI.GetPathCount", "AI.LoadRaceHeatPaths", "AI.SelectCharacters",
            "AI.StartLapPaths", "Challenge.GetGameMode", "ChallengeManager.SetManualsAreConnectors",
            "ChallengeManager.SwitchChallengeCamera", "ChallengeReplay.ResetReplayBuffer",
            "ChallengeSpeech.PlayFollowMeWarningSpeech", "ChallengeSpeech.PlayNPCSpeech", "ChallengeSpeech.PlaySkaterSpeech",
            "ChallengeTimer.AddCountupTimer", "ChallengeTimer.GetCountupTime", "ChallengeTimer.PauseTimer",
            "ChallengeTimer.RemoveCountupTimer", "ChallengeTimer.RemoveHUDCountdownTimer", "Debug.DebugTextPermanent",
            "Debug.DebugTextTimed", "Debug.RemoveTextPermanent", "Effects.ResetEffect", "FollowTheLeader.GetDistance",
            "FollowTheLeader.GetMaxDistance", "FollowTheLeader.GetMinDistance", "FollowTheLeader.GetRecoveryCharacter",
            "FollowTheLeader.GetRecoverySpeechID", "FollowTheLeader.GetRepeatTime",
            "FollowTheLeader.GetTimeBeforeWarningPlayed", "FollowTheLeader.GetWarningCharacter",
            "FollowTheLeader.GetWarningFollowee", "FollowTheLeader.GetWarningSpeechID",
            "FollowTheLeader.RegisterDistanceCallback", "FollowTheLeader.SetActorIDs", "FollowTheLeader.SetWarningInfo",
            "FollowTheLeader.StartDistanceUpdates", "FollowTheLeader.StopDistanceUpdates",
            "FollowTheLeader.UnregisterDistanceCallback", "FrontEnd.OpenContinueRetryDialog",
            "FrontEnd.Results_SetActiveResultsMenu", "FrontEnd.Results_SetColTitle", "FrontEnd.Results_SetPlayerData",
            "FrontEnd.Results_SetPlayerStatus", "FrontEnd.Results_SetPlayerTime", "FrontEnd.Results_SetResultsMenuType",
            "HUD2.LeaderboardHUDSetVariantString", "HUD2.SetupDistanceHUDData", "HUD2.ShowChallengeCompleteMessage",
            "HUD2.ShowChallengeFailedMessage", "NIS.ClearPreloadedNIS", "NIS.ResetNISPlayerIndexes",
            "NIS.SubmitNISPlayerIndex", "Objectives.ForceObjectiveComplete", "Race.AddPlayerToRace", "Race.FinishPlayer",
            "Race.GetEstimatedTimeToFinish", "Race.RegisterPlayerTimer", "Race.ResetRaceStats", "Race.SetPlayerProgress",
            "Race.StopRaceStats", "VisualIndicators.AddFollowIndicator", "VisualIndicators.AddTriggerIndicator",
            "VisualIndicators.RemoveByType", "VisualIndicators.RemoveFollowIndicator",
            "VisualIndicators.RemoveTriggerIndicator",
        ];
        const PROGRESSION: &[&str] = &[
            "Progression.IsChallengeCompleteByTitle", "Progression.IsChallengeActiveByTitle",
            "Progression.DeactivateChallengeByTitle", "Progression.ActivateChallenge",
            "Progression.ActivateChallengeByTitle", "Progression.ExecuteChallengeByTitle", "Progression.SendMessage",
            "Progression.GetMessageSettingBool", "Progression.RevertChallengeByTitle",
            "Unlock.UnlockCharacterForFreeskate", "DynamicHull.SetupDynamicHullState",
            "ChallengeAsset.PreloadChallengeAssetsByName",
        ];
        let data: Arc<Data> = Arc::new(serde_json::from_value(json!({
            "frontend": [], "progression": {"triggers": {}, "messages": {}, "entries": {}}, "challenges": {},
            "placement": {"locators": {}, "volumes": {}}, "scenes": {}, "movies": {}, "speech": {}, "trick_groups": {}
        })).unwrap());
        let shared = Arc::new(Mutex::new(Shared::default()));
        // Every enum the template reads exists, and the global it calls.
        let (lua, _) = crate::challenge::sandbox("enums").unwrap();
        lesson_natives(&lua, &shared, &data).unwrap();
        lua.load("assert(Challenge.eChallengeTypes.SpotPhoto == 38 and FrontEnd.eSk8BEEndMissionOptions.Retry_Quit == 1)").exec().unwrap();
        lua.load("cMsgEnterRealTime(Broadcasters.Ports.PresThread) assert(FrontEnd.Results_SetNumPlayers(0) == nil)").exec().unwrap();
        lua.load("assert(Race.eRaceType.FollowMe == 2 and Challenge.GetGameMode() == Challenge.GameMode.SinglePlayer)").exec().unwrap();
        lua.load("assert(FrontEnd.eResultGroupType.FinalResults and VisualIndicators.eChallengeIndicators.FollowMeTarget)").exec().unwrap();
        // The warning row comes back through the getters; a callback's id unregisters it.
        lua.load("FollowTheLeader.SetWarningInfo({10, 50, 39}) assert(FollowTheLeader.GetMaxDistance() == 50)").exec().unwrap();
        lua.load("FollowTheLeader.UnregisterDistanceCallback(FollowTheLeader.RegisterDistanceCallback(1, 10))").exec().unwrap();
        assert!(shared.lock().unwrap().follow.callbacks.is_empty());
        for (names, register) in [
            (REFERENCED, Box::new(|lua: &Lua| lesson_natives(lua, &shared, &data)) as Box<dyn Fn(&Lua) -> mlua::Result<()>>),
            (PROGRESSION, Box::new(|lua: &Lua| progression_natives(lua, &shared))),
        ] {
            let (lua, _) = crate::challenge::sandbox("test").unwrap();
            register(&lua).unwrap();
            for name in names {
                let (ns, f) = name.split_once('.').unwrap();
                let table: Table = lua.globals().raw_get(ns).unwrap();
                assert!(matches!(table.raw_get::<LuaValue>(f).unwrap(), LuaValue::Function(_)), "{name} is not bound");
            }
            // A stub answers nil instead of failing.
            lua.load("assert(HUD2.NotOnTheDisc() == nil and Nowhere.Nothing() == nil)").exec().unwrap();
        }
    }

    /// Trigger_m_1a_07_follow_fail's teleport (progression.lua:463), started by a lose or an abandon (mode 4,
    /// a_win_starts_only_the_win_bindings): the player goes back to the challenge locator.
    #[test]
    fn a_progression_trigger_teleports_the_player() {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (lua, _) = crate::challenge::sandbox("test").unwrap();
        progression_natives(&lua, &shared).unwrap();
        lua.load(r#"Teleport.TeleportLocalPlayerToLocationName("m_1a_07_follow_slappy_challengelocator01")"#).exec().unwrap();
        let s = shared.lock().unwrap();
        assert!(matches!(&s.requests[..], [Request::Teleport(l)] if l == "m_1a_07_follow_slappy_challengelocator01"));
    }

    /// CompleteChallenge starts only the entry's bindings its outcome allows (round2-research2 Q5): Follow Slappy's
    /// win starts `_follow` (mode 0), a lose or an abandon `_fail` (mode 4), and `_accept` (mode 8) never.
    #[test]
    fn a_win_starts_only_the_win_bindings() {
        let entry: EntryData = serde_json::from_value(json!({
            "triggers": ["Trigger_m_1a_07_follow", "Trigger_m_1a_07_follow_fail", "Trigger_m_1a_07_follow_accept"],
            "bindings": [["Trigger_m_1a_07_follow_accept", 8], ["Trigger_m_1a_07_follow", 0], ["Trigger_m_1a_07_follow_fail", 4]],
        }))
        .unwrap();
        assert_eq!(entry.fired(Outcome::Win), ["Trigger_m_1a_07_follow"]);
        assert_eq!(entry.fired(Outcome::Lose), ["Trigger_m_1a_07_follow_fail"]);
        assert_eq!(entry.fired(Outcome::Abandon), ["Trigger_m_1a_07_follow_fail"]);
        // An install from before the bindings export: its trigger list on a win, as before.
        let old: EntryData = serde_json::from_value(json!({"triggers": ["Trigger_m_1a_05_photo_rail"]})).unwrap();
        assert_eq!(old.fired(Outcome::Win), ["Trigger_m_1a_05_photo_rail"]);
        assert!(old.fired(Outcome::Lose).is_empty());
    }

    /// progression.lua's won triggers (flow.md): which lessons each win activates. Learn walks the active ones in
    /// LESSONS order; the walk is the user's retail order and ends at Follow Slappy (user 10-13).
    #[test]
    fn learn_walks_the_seven_retail_lessons_and_ends_at_follow_slappy() {
        let wins: [(&str, &[&str]); 7] = [
            (MEET_SLAPPY, &[FLIPTRICKS]),
            (FLIPTRICKS, &["m_1a_05_photo_rail", "m_1a_04_tutorial_manuals", "m_2a_01_tutorial_slappy"]),
            ("m_1a_05_photo_rail", &["m_1a_06_photo_puzzle"]),
            ("m_1a_04_tutorial_manuals", &[]),
            ("m_1a_06_photo_puzzle", &[FOLLOW_SLAPPY]),
            ("m_2a_01_tutorial_slappy", &[]),
            (FOLLOW_SLAPPY, &["m_1b_01_spotfilm_sponsor_me_01", "hom_tutorial"]),
        ];
        let mut p = Progress::default();
        p.activate(MEET_SLAPPY);
        // A save from an earlier build (Review Focus 1).
        p.active.push("m_1a_07_tutorial_airtricks".into());
        p.active.push("m_1a_08_tutorial_grinds".into());
        let mut walked = Vec::new();
        while let Some(next) = p.next_lesson() {
            assert!(walked.len() < 7, "Learn loops: {walked:?}");
            walked.push(next);
            p.active.retain(|a| a != next);
            p.completed.push(next.to_owned());
            wins.iter().find(|(won, _)| *won == next).unwrap().1.iter().for_each(|l| p.activate(l));
        }
        assert_eq!(walked, [
            MEET_SLAPPY,
            FLIPTRICKS,
            "m_1a_05_photo_rail",
            "m_1a_04_tutorial_manuals",
            "m_1a_06_photo_puzzle",
            "m_2a_01_tutorial_slappy",
            FOLLOW_SLAPPY,
        ]);
    }

    /// Retail's SendMessage type switch (0x824d3020): type 1 id 10 opens UnlockOverlay with what this win unlocked
    /// (0x824d3630), each as the character's label, `ID_COMMON_REDA` for reda (Step 1c); type 4 plays its FileID as a
    /// CameraMan line (0x824d32c8); type 5 is the Skate 101 overlay (0x824d3328 -> FE state 79).
    #[test]
    #[ignore = "requires SKATE2_TUTORIAL_DIR (the Task 2 export)"]
    fn progression_messages_open_their_retail_screens() {
        let data = Arc::new(Data::load(PathBuf::from(std::env::var("SKATE2_TUTORIAL_DIR").unwrap())).unwrap());
        let mut tutorial = Tutorial::default();
        tutorial.shared.lock().unwrap().unlocked.push("reda".into());
        tutorial.message(&data, "Show Unlocks Screen");
        assert!(matches!(tutorial.queue.pop_front(), Some(Item::Unlocks(c)) if c == ["ID_COMMON_REDA"]));
        assert!(tutorial.shared.lock().unwrap().unlocked.is_empty(), "the screen takes the list");
        tutorial.message(&data, "SponsorMeStart");
        assert!(tutorial.queue.is_empty(), "a reminder holds nothing");
        let requests = std::mem::take(&mut tutorial.shared.lock().unwrap().requests);
        assert!(matches!(&requests[..], [Request::Speech(Event { kind: 3, a, b })] if a == "CameraMan" && b == "2727"));
        tutorial.message(&data, "201_Follow_Slappy");
        assert!(matches!(tutorial.queue.pop_front(), Some(Item::Skate101 { title, text })
            if title == "Footage Time!" && text.starts_with("Time to get started on your sponsor me tape")));
    }

    /// Retail's sign-up volume (0x82b790d0 -> 0x82acebb8): an axis-aligned box of half-extent Hash_8022BA64CB8E431D
    /// (3.0 for every lesson) on all three axes around `Location`; 0 or less falls back to 40.
    #[test]
    fn the_sign_up_volume_is_retails_box_around_location() {
        let l = Vec3::new(10., 2., -5.);
        assert!(in_signup_box(l, 3., l + Vec3::new(2.9, -2.9, 2.9)), "a corner inside");
        assert!(!in_signup_box(l, 3., l + Vec3::new(3.1, 0., 0.)));
        assert!(!in_signup_box(l, 3., l + Vec3::new(0., 3.5, 0.)), "a box: height counts");
        assert!(in_signup_box(l, 0., l + Vec3::new(39., 0., 0.)), "unset falls back to 40");
    }

    /// After Flipping Wood 101 the three open lessons carry their sign-up indicator 2 m over `Location` (the vault's
    /// `signup` row offset); a won lesson's goes.
    #[test]
    #[ignore = "requires SKATE2_TUTORIAL_DIR"]
    fn open_lessons_carry_their_sign_up_indicator() {
        let data = Data::load(PathBuf::from(std::env::var("SKATE2_TUTORIAL_DIR").unwrap())).unwrap();
        let mut p = Progress::default();
        p.completed = vec![MEET_SLAPPY.into(), FLIPTRICKS.into()];
        ["m_1a_05_photo_rail", "m_1a_04_tutorial_manuals", "m_2a_01_tutorial_slappy"].iter().for_each(|l| p.activate(l));
        let got = signup_indicators(&data, &p);
        let names: Vec<&str> = got.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, [
            "signup:m_1a_05_photo_rail_challengelocator01",
            "signup:m_1a_04_tutorial_manuals_challengelocator01",
            "signup:m_2a_01_tutorial_slappy_challengelocator01",
        ]);
        let at = data.locator("m_1a_05_photo_rail_challengelocator01").unwrap();
        assert!((got[0].1[3][1] - at[3][1] - 2.).abs() < 1e-4, "2 m up");
        p.completed.push("m_1a_05_photo_rail".into());
        assert_eq!(signup_indicators(&data, &p).len(), 2);
    }

    /// Retail plays a lesson's TutorialMovie only from the poster's X (0x825242ac, the field's one reader): starting a
    /// lesson queues the lesson alone.
    #[test]
    #[ignore = "requires SKATE2_TUTORIAL_DIR"]
    fn executing_a_lesson_plays_no_tutorial_movie() {
        let data = Arc::new(Data::load(PathBuf::from(std::env::var("SKATE2_TUTORIAL_DIR").unwrap())).unwrap());
        assert!(data.challenges[FLIPTRICKS].fields["TutorialMovie"].as_str().is_some(), "fliptricks has a movie");
        let mut tutorial = Tutorial::default();
        tutorial.data = Some(data);
        tutorial.request(&mut World::new(), Request::Execute(FLIPTRICKS.into()));
        assert!(matches!(tutorial.queue.iter().collect::<Vec<_>>()[..], [Item::Start(_)]));
    }

    /// User 10-13 (bug 8): the minimap marks each active lesson (0x824ca998) at its Location with its base icon, but
    /// not ChallengeType 44 (0x824a8ba0) nor a Location at x = y = 0 (0x82472d74); inactive lessons have none.
    #[test]
    fn active_lessons_mark_the_minimap_with_their_icon() {
        let lesson = |kind: i64, location: &str, icon: &str| {
            json!({"self": {"ChallengeType": kind, "Location": location}, "steps": {}, "objectives": [], "icon": icon})
        };
        let at = |x: f32, y: f32, z: f32| json!([[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [x, y, z, 1.]]);
        let data: Data = serde_json::from_value(json!({
            "frontend": [], "progression": {"triggers": {}, "messages": {}, "entries": {}},
            "challenges": {
                "photo": lesson(38, "photo_at", "mini_smag"), "follow": lesson(23, "follow_at", "mini_follow"),
                "type44": lesson(44, "photo_at", "mini_tutorial"), "nowhere": lesson(0, "", "mini_tutorial"),
                "origin": lesson(0, "origin_at", "mini_tutorial"), "inactive": lesson(0, "photo_at", "mini_tutorial"),
            },
            "placement": {"locators": {"photo_at": at(10., 2., 30.), "follow_at": at(-5., 1., 7.), "origin_at": at(0., 0., 9.)}, "volumes": {}},
            "scenes": {}, "movies": {}, "speech": {}, "trick_groups": {}
        }))
        .unwrap();
        let active: Vec<String> = ["follow", "type44", "nowhere", "origin", "photo"].map(String::from).into();
        assert_eq!(minimap_markers(&data, &active), [("mini_follow".to_string(), [-5., 7.]), ("mini_smag".to_string(), [10., 30.])]);
    }

    /// User 10-13 (bugs 9, 10): retail's AI paths sit 0.087 m above the map's ground on every rolling sample
    /// (round2-research R2), so a lap rides PATH_ABOVE_GROUND down wherever it sets off, with no ground probe.
    #[test]
    fn a_lap_rides_on_the_ground_its_path_was_recorded_over() {
        let data: Arc<Data> = Arc::new(serde_json::from_value(json!({
            "frontend": [], "progression": {"triggers": {}, "messages": {}, "entries": {}}, "challenges": {},
            "placement": {"locators": {}, "volumes": {}}, "scenes": {}, "movies": {}, "speech": {}, "trick_groups": {}
        })).unwrap());
        let mut tutorial = Tutorial { data: Some(data), now: 3., ..default() };
        tutorial.ai.insert(1, Ai::default());
        tutorial.request(&mut World::new(), Request::StartPath(1));
        assert_eq!(tutorial.ai[&1].path, Some((3., -PATH_ABOVE_GROUND)));
    }

    /// User 10-13 (bug 6): the photo shows none of the 3D extras: no guide arrows or sign-up indicators, no stand-by
    /// cast (the photographer the lesson spawns, the pros). Only the photo scene's own actors stay.
    #[test]
    fn a_photo_shows_no_guides_or_extras() {
        let identity = [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [0., 0., 0., 1.]];
        let data: Arc<Data> = Arc::new(serde_json::from_value(json!({
            "frontend": [], "progression": {"triggers": {}, "messages": {}, "entries": {}},
            "challenges": {"m_1a_05_photo_rail": {"self": {"Location": "spot"}, "steps": {}, "objectives": []}},
            "placement": {"locators": {"spot": identity}, "volumes": {}}, "scenes": {}, "movies": {}, "speech": {}, "trick_groups": {}
        })).unwrap());
        let mut tutorial = Tutorial { data: Some(data), ..default() };
        tutorial.pros.insert("attiba_jefferson".into(), "spot".into());
        tutorial.shared.lock().unwrap().progress.activate("m_1a_05_photo_rail");
        let mut world = World::new();
        world.init_resource::<Cutscene>();
        world.init_resource::<crate::skate2_cast::Cast>();
        let indicators = ["spot".to_owned()];
        tutorial.place_cast(&mut world);
        assert!(world.resource::<crate::skate2_cast::Cast>().wanted.contains_key("attiba_jefferson"));
        assert_eq!(tutorial.guides(&indicators).len(), 2, "the guide and the open lesson's sign-up indicator");
        for photo in [Playing::Reel { at: 0., left: 1., clock: 0. }, Playing::Photos { index: 0, forward: true }] {
            tutorial.playing = Some(photo);
            tutorial.place_cast(&mut world);
            assert!(world.resource::<crate::skate2_cast::Cast>().wanted.is_empty());
            assert!(tutorial.guides(&indicators).is_empty());
        }
    }

    /// A photo is the world as drawn (graphics_menu::SceneTarget), not the window: the output camera draws the HUD,
    /// prompts and FPS counter over that image (user 10-13: none of them belong in the photo).
    #[test]
    fn photos_capture_the_world_without_the_hud() {
        let mut world = World::new();
        let target = Handle::<Image>::default();
        world.insert_resource(crate::graphics_menu::SceneTarget(target.clone()));
        snap(&mut world);
        let shot = world.query::<&Screenshot>().single(&world).unwrap();
        assert!(matches!(&shot.0, bevy::camera::RenderTarget::Image(i) if i.handle == target), "{:?}", shot.0);
    }

    /// A spoken line plays at Audio Settings' Dialog volume (the user's preview 1 had Dialog at 4 and heard it full).
    #[test]
    fn speech_plays_at_the_dialog_volume() {
        // 16-bit stereo PCM, 4 silent frames.
        let mut wav = Vec::new();
        for chunk in [&b"RIFF"[..], &52u32.to_le_bytes(), b"WAVEfmt ", &16u32.to_le_bytes(), &1u16.to_le_bytes(), &2u16.to_le_bytes(),
            &44100u32.to_le_bytes(), &(44100u32 * 4).to_le_bytes(), &4u16.to_le_bytes(), &16u16.to_le_bytes(), b"data", &16u32.to_le_bytes(), &[0; 16]] {
            wav.extend_from_slice(chunk);
        }
        let path = std::env::temp_dir().join("s2tut-speech-volume-test.wav");
        std::fs::write(&path, wav).unwrap();
        let mut menu = crate::graphics_menu::test_menu();
        menu.set_audio(crate::music::AudioSettings { volumes: [6, 4, 0], ..Default::default() });
        let mut world = World::new();
        world.insert_resource(menu);
        world.init_resource::<Assets<StreamPcm>>();
        Tutorial::default().play(&mut world, &path, crate::music::DIALOG);
        let settings = world.query::<&PlaybackSettings>().single(&world).unwrap();
        assert_eq!(settings.volume, bevy::audio::Volume::Linear(0.4));
    }

    /// Retail signs up on D-pad Up alone (actionoverlay2's glyph): the left stick held forward while riding into the
    /// box, or a held direction's menu repeat, signs nothing up.
    #[test]
    fn only_the_d_pad_signs_up() {
        let keys = ButtonInput::<KeyCode>::default();
        let mut nav = crate::customiser::Navigation::default();
        nav.pressed = 1;
        assert!(!sign_up_pressed(&keys, Some(&nav)), "stick forward");
        nav.dpad = 1;
        assert!(sign_up_pressed(&keys, Some(&nav)));
        let mut keys = ButtonInput::<KeyCode>::default();
        keys.press(KeyCode::ArrowUp);
        assert!(sign_up_pressed(&keys, None), "keyboard Up");
    }

    /// F1 (RETAIL, 0x82b186b8): event 34 clears the challenge-managed flag on DMOs spawned inside the volume and moves
    /// nothing. Meet Slappy's setmarker objective starts with it right as you pass the dumpster you pushed aside.
    #[test]
    fn dmo_reset_volume_event_moves_nothing() {
        let data: Data = serde_json::from_value(json!({
            "frontend": [], "progression": {"triggers": {}, "messages": {}, "entries": {}}, "challenges": {},
            "placement": {"locators": {}, "volumes": {"m_1a_01_goto_slappys_DMO_reset_vol02": {
                "id": "0x7742603E38706", "min": [1688.48, 106.09, -542.96], "max": [1699.89, 110.92, -532.13]}}},
            "scenes": {}, "movies": {}, "speech": {}, "trick_groups": {}
        })).unwrap();
        let mut tutorial = Tutorial::default();
        // No GamePhysics in the world: the handler must not reach for the props.
        tutorial.event(&mut World::new(), &data, Event { kind: 34, a: "2098031459796742".into(), b: String::new() });
    }

    /// Slappy's gate (round2 10-14): DynamicHull.SetupDynamicHullState (0x82b76ff0) keeps the hull's state in the career
    /// progress, which retail saves (cChallengeDynamicHullManager::Data, 0x82b77f60); a save from before the gate has
    /// none, so every hull is in its vault default. INFERRED: the triggers end TerminateWithoutSave, so setting a state
    /// saves nothing by itself. Only a career shows a hull: retail loads one only in career mode (load mask 4,
    /// 0x823a8be8 / 0x824abf58). INFERRED: the port's free skate (tutorial exited) is not career.
    #[test]
    fn dynamic_hull_states_are_career_progress() {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (lua, _) = crate::challenge::sandbox("test").unwrap();
        progression_natives(&lua, &shared).unwrap();
        lua.load(r#"DynamicHull.SetupDynamicHullState("slappys", "Unlock")"#).exec().unwrap();
        let progress = {
            let s = shared.lock().unwrap();
            assert!(!s.dirty, "TerminateWithoutSave: the next save carries it");
            s.progress.clone()
        };
        assert_eq!(progress.hulls.get("slappys").map(String::as_str), Some("Unlock"));
        let saved: Progress = serde_json::from_slice(&serde_json::to_vec(&progress).unwrap()).unwrap();
        assert_eq!(saved.hulls, progress.hulls);
        let old: Progress = serde_json::from_value(json!({"frontend_done": true, "completed": [MEET_SLAPPY]})).unwrap();
        assert!(old.hulls.is_empty());
        assert_eq!(career_hulls(true, &progress), Some(&progress.hulls));
        let free = Progress { skipped: true, ..progress.clone() };
        assert_eq!(career_hulls(true, &free), None, "exited to free skate");
        assert_eq!(career_hulls(false, &progress), None, "no tutorial: online or another map");
    }

    /// Kind 5 (0x824d1668, posted by the challenge flow's state 6, 0x82b78ae8) starts the challenge's bindings of modes
    /// 8, 10 and 11 (0x824d12d8, jump table 0x824cd104): Follow Slappy's `_accept`, which opens the gate. INFERRED:
    /// kind 5 = sign-up accepted (its callers' "signup" / "Entry Fee" strings), sent at every lesson start.
    #[test]
    fn a_lesson_start_starts_only_its_sign_up_bindings() {
        let entry: EntryData = serde_json::from_value(json!({
            "triggers": ["Trigger_m_1a_07_follow", "Trigger_m_1a_07_follow_fail", "Trigger_m_1a_07_follow_accept"],
            "bindings": [["Trigger_m_1a_07_follow_accept", 8], ["Trigger_m_1a_07_follow", 0], ["Trigger_m_1a_07_follow_fail", 4]],
        }))
        .unwrap();
        assert_eq!(entry.signed_up(), ["Trigger_m_1a_07_follow_accept"]);
        let modes: EntryData = serde_json::from_value(json!({
            "triggers": [], "bindings": [["a", 8], ["b", 10], ["c", 11], ["d", 9], ["e", 5], ["f", 0]],
        }))
        .unwrap();
        assert_eq!(modes.signed_up(), ["a", "b", "c"]);
    }

    /// Slappy's gate in the installed handlers (progression.lua:469-476, :456-466): Follow Slappy's sign-up opens the
    /// "slappys" hull; its lose or abandon locks it again unless the follow is won, and always teleports back.
    #[test]
    #[ignore = "requires SKATE2_TUTORIAL_DIR"]
    fn follow_slappys_sign_up_opens_the_gate_and_its_fail_locks_it() {
        let data = Arc::new(Data::load(PathBuf::from(std::env::var("SKATE2_TUTORIAL_DIR").unwrap())).unwrap());
        let accept = data.progression.entries[FOLLOW_SLAPPY].signed_up();
        assert_eq!(accept, ["Trigger_m_1a_07_follow_accept"]);
        let mut tutorial = Tutorial::default();
        tutorial.data = Some(data);
        let mut world = World::new();
        let slappys = |t: &Tutorial| t.shared.lock().unwrap().progress.hulls.get("slappys").cloned();
        assert_eq!(slappys(&tutorial), None, "a new career sets nothing: the vault default (Locked) holds");
        tutorial.deliver(&mut world, Delivery::StartTrigger(accept[0].clone()));
        assert_eq!(slappys(&tutorial).as_deref(), Some("Unlock"));
        tutorial.deliver(&mut world, Delivery::StartTrigger("Trigger_m_1a_07_follow_fail".into()));
        assert_eq!(slappys(&tutorial).as_deref(), Some("Locked"));
        let requests = std::mem::take(&mut tutorial.shared.lock().unwrap().requests);
        assert!(matches!(&requests[..], [Request::Teleport(l)] if l == "m_1a_07_follow_slappy_challengelocator01"));
        // Won: a later fail leaves it open (and still teleports).
        {
            let mut s = tutorial.shared.lock().unwrap();
            s.progress.completed.push(FOLLOW_SLAPPY.into());
            s.progress.hulls.insert("slappys".into(), "Unlock".into());
        }
        tutorial.deliver(&mut world, Delivery::StartTrigger("Trigger_m_1a_07_follow_fail".into()));
        assert_eq!(slappys(&tutorial).as_deref(), Some("Unlock"));
        assert!(tutorial.triggers.is_empty(), "both end TerminateWithoutSave");
    }

    /// 10-29 bugs 2 and 6: GetSpawnLocation (0x82879950) takes a locator's sub-location by index.
    #[test]
    fn a_spawn_takes_its_locators_sub_location() {
        let m = |x: f32, z: f32| [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [x, 0., z, 1.]];
        let data: Data = serde_json::from_value(json!({
            "frontend": [], "progression": {"triggers": {}, "messages": {}, "entries": {}}, "challenges": {},
            "placement": {"locators": {"start": m(0., 0.), "plain": m(7., 7.)}, "volumes": {},
                          "spawn_points": {"start": [m(3., -1.), m(0., 0.)]}},
            "scenes": {}, "movies": {}, "speech": {}, "trick_groups": {}
        }))
        .unwrap();
        assert_eq!(data.spawn_point("start", 0).unwrap()[3], [3., 0., -1., 1.], "the player: spawnpoint01");
        assert_eq!(data.spawn_point("start", 1).unwrap()[3], [0., 0., 0., 1.], "Slappy: spawnpoint02");
        assert_eq!(data.spawn_point("start", 3).unwrap()[3], [3., 0., -16., 1.], "past the last: the first, -5 z per index");
        assert_eq!(data.spawn_point("plain", 2).unwrap()[3], [7., 0., -3., 1.], "no sub-locations: the locator, -5 z per index");
    }

    /// 10-29 bugs 2 and 6 on the installed data: the player never starts on Slappy, and No Mugshots starts at spawnpoint01.
    #[test]
    #[ignore = "requires SKATE2_TUTORIAL_DIR"]
    fn the_follow_and_photo_rail_starts_are_retails_spawn_points() {
        let data = Data::load(PathBuf::from(std::env::var("SKATE2_TUTORIAL_DIR").unwrap())).unwrap();
        let at = |l: &str, i: i64| position(&data.spawn_point(l, i).unwrap());
        assert!(at("m_1a_07_follow_slappy_startlocator01", 0).distance(Vec3::new(1599.753, 100.538, -512.164)) < 0.01);
        assert!(at("m_1a_07_follow_slappy_startlocator01", 1).distance(Vec3::new(1596.832, 100.538, -510.65)) < 0.01);
        assert!(at("m_1a_07_follow_slappy_startlocator01", 0).distance(at("m_1a_07_follow_slappy_startlocator01", 1)) > 3.);
        assert!(at("m_1a_05_photo_rail_startlocator01", 0).distance(Vec3::new(1677.243, 101.63, -503.559)) < 0.01);
    }

    /// 10-29 bug 4: a scene with characters but no current_player (No Mugshots' intro, kind 19) hides the player's
    /// skater, as retail's UpdateVisibleTypes (0x282f) does; one with the player keeps it; a trackless take hides nothing.
    #[test]
    fn a_scene_without_the_player_hides_the_players_skater() {
        let scene = |tracks: Value| serde_json::from_value::<Scene>(json!({"duration": 9.6, "audio": [], "subtitles": [], "tracks": tracks})).unwrap();
        assert!(scene(json!([{"character": "mike_carroll"}, {"character": "eric_koston"}])).hides_player(), "tutorial_m_1a_05_photo_rail");
        assert!(!scene(json!([{"character": "slappy"}, {"character": "current_player"}])).hides_player(), "01_Welcome_Back");
        assert!(!scene(json!([])).hides_player(), "a photo outro or highlight");
    }
}
