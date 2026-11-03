//! Skate 1's five tutorial missions (he_u1_film01, he_u3_film01, he_u2_follow01, he_u2_photo01, he_u1_skate01) and
//! the pro tutorials' extra natives: the C++ side of each Lua native as plain state (the objective holder, the film,
//! photo, follow and S.K.A.T.E. managers). skate1_challenge.rs owns the Machine, the engine and the screens: it calls
//! `register` inside the Machine's natives (after its own), `tick` once a frame and applies `drain`'s requests.
//! Evidence per native: docs/superpowers/plans/2026-10-14-skate1-p5-missions.md and the s1-missions ledger.
// ponytail: unused outside the tests until the lead wires start_mission (plan L1); drop this then.
#![cfg_attr(not(test), allow(dead_code))]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use mlua::{FromLuaMulti, IntoLuaMulti, Lua, Table, Value as LuaValue};
use serde_json::{Value, json};

pub(crate) type Shared = Arc<Mutex<Missions>>;
/// A world transform, as skate1_challenge's locators and `travel_to` take it.
pub(crate) type M4 = [[f32; 4]; 4];

/// [0x830a3f64] when no challenge holds the objectives.
const NO_CHALLENGE: i64 = 32;
// eChallengeTypes (0x827ac090).
const PHOTO: i64 = 6;
const FILM: i64 = 7;
const OWN_THE_SPOT: i64 = 9;
// eObjective (0x827c5050).
const POINTS: i64 = 1;
const TRICK_TYPE: i64 = 2;
const JUMP_DISTANCE: i64 = 4;
const SPECIFIC_ANY: i64 = 18;
const FILM_TIME: i64 = 19;
const FOLLOW_DISTANCE: i64 = 20;
const FOLLOW_DESTINATION: i64 = 21;
const BREAK_BONES: i64 = 23;
const GET_LINE: i64 = 24;
const NO_SKATE_ZONE: i64 = 25;
const PHOTO_CUSTOM: i64 = 27;
const LINE_POINTS: i64 = 28;

/// RETAIL enum tables (scratch/enums_all.txt, by address); a value is its name's position. `register` adds the
/// names a table lacks and keeps the lead's (eSk8HUDState must be retail there too: lead edit L4).
const ENUMS: [(&str, &str, &str); 21] = [
    ("Challenge", "eChallengeFlow", "Exit Restart Win Lose Kill"), // 827abe80
    ("Challenge", "eChallengeIndicators", "None Signup_Thrasher Signup_SBM Signup_Tutorial Obstacle RaceNextGate \
        RaceGates RaceFinish SpotNextGates SpotGates SpotFinish ChallengeBoundary FollowMeTarget Teleporter PinchPoint"), // 827ad928
    ("Challenge", "eChallengePath", "Hero AntiHero ProgressionFree"), // 827acc40
    ("Film", "eFilmChallengeType", "NoLine Regular CompleteLine"), // 827af668
    ("HUD", "eSk8HUDState", "Idle FreeSkate NIS MapScreenMain LogBookFront SpotBibleFront SkateShopClerk \
        SkateShopLounge SkateShopStore SkateShopCoffeeTable SkateShopSponsers SkateShopMagazines Sidekick SidekickVideo \
        ReadMessage MissionFilmChallenge MissionSpotChallenge MissionPhotoChallenge MissionSkateChallenge \
        MissionTutorialChallenge Backpack TermsOfService Options EndChallengeOptions MedicalBill UnlockOverlay Replay \
        PhotoSave ReplaySave OnlinePhotoSave OnlineFootageSave RatingMenu Cheats TrickTutorial \
        MissionDeathRaceChallenge MissionSpotRaceChallenge MissionBestTrickChallenge MissionProChallenge \
        MissionJamChallenge MissionDemoChallenge MissionXGamesMegarampChallenge MissionXGamesStreetSkatingChallenge \
        CaC CaCDetails ExtrasMenu BonusVideo Credits MainMenu BootFlow LeaderBoard XGamesLeaderBoard MoviePlayer \
        MoviePlayerROP MoviePlayerNoTransition LoadingTips LoadingController LoadingQuick LoadingBlank LoadingWelcome \
        Intro HowToPlaySubscreen HowToPlay AdidasPromotion TransitMap Profile SignUpPoster Popup \
        MissionFollowMeChallenge VirtualKeyboard ComingSoon PartyPlay Challenge101 ChallengeInfo TutorialComplete \
        PressA FreeSkateOption Teleporter JackedOverlay UnlockCoverageOverlay MainMenuOptions ESRB"), // 827afa70
    ("HUD", "eSk8HUDFilmState", "Load SelectFilm Record Success Failed ShowReplay Options Hide Countdown \
        UpdateObjectives UpdateObjectivesForReal"), // 827b26b0
    ("HUD", "eSk8HUDPhotoState", "Load SignUpPoster Active Success LeftArea Failed TakePicture ViewPhotos Options \
        Hide MedicalBill"), // 827b2af8
    ("HUD", "eSk8HUDFollowMeState", "Load ActionOverlay Intro InProgress Success Failed Outro Countdown"), // 827b2f40
    ("HUD", "eSk8HUDSkateState", "Load Setup SignUpPoster StartRound StartLeader LeaderSuccess LeaderFail \
        StartFollower FollowerSuccess FollowerFail Options Leaderboard PressAToContinue RefreshTrickDisplay \
        MessageSetTrick MessageTrickSet MessageCopyTrick MessageTrickCopied MessageTrickWrong MessageTrickABD \
        MessageLeftArea MessageRanOutOfTime MessageBailed MessageTrickNotSet MessageTrickNotCopied"), // 827b3270
    ("HUD", "eSk8HUDBestTrickState", "Introduce Active RoundLeaderboard Hide OverallLeaderboard FinalLeaderboard \
        SignUpPoster TrickCallout Setup LeftArea Countdown DisplayTimeLeft Forfeit Load MessageChallengeForfeit"), // 827b4948
    ("ChallengeSFX", "eSk8MissionSFX", "CameraShutter OwnSpot ContestLoad ContestStart ContestEnd ContestUnload"), // 827bd620
    ("ChallengeMusic", "eSk8MissionMusicTrigger", "Countdown Start Abort AlmostOver Over Win Lose"), // 827bd990
    ("HUD", "eSk8HUDFilmFailedState", "Unknown Bailed NoLine LeftNoSkateZone NotEnoughPoints"), // 827b8a48
    ("HUD", "eSk8HUDPhotoFailedState", "Unknown Bailed DidntGrindRail DidntClearWishbone NotEnoughPoints"), // 827b8c58
    ("HUD", "eSk8HUDTutorialState", "Load SignUpPoster Active LeftArea EnterArea ShowController HideController \
        UpdateController UpdateObjectives ShowObjectives HideObjectives Congratulations Failed TutorialComplete \
        FailedObjectives ShowTimer HideTimer CompleteOverlay MarkObjectivesComplete ShowDistanceMeter \
        HideDistanceMeter ShowIndicator HideIndicator ShowRetryDialog MessageGatePassed"), // 827b66d8
    ("NIS", "eNISChallengeNotification", "NIS_CHALLENGE_NOTIFY_NONE NIS_CHALLENGE_NOTIFY_COMPLETED \
        NIS_FE_NOTIFY_ENTER_PHOTOVIEW NIS_FE_NOTIFY_PLAYING_KILLABLE"), // 827bfa40
    ("NIS", "eSceneType", "SCENE_TYPE_NONE SCENE_TYPE_FLYTHROUGH SCENE_TYPE_CHALLENGE_WIN \
        SCENE_TYPE_SKATE_INTRO_OFFER SCENE_TYPE_SKATE_INTRO_ACCEPT SCENE_TYPE_SKATE_INTRO_DECLINE \
        SCENE_TYPE_SKATE_OUTRO_WIN SCENE_TYPE_SKATE_OUTRO_LOSE SCENE_TYPE_SPOT_INTRO SCENE_TYPE_SPOT_OUTRO \
        SCENE_TYPE_PRO_INTRO SCENE_TYPE_PRO_CHALLENGE_INTRO SCENE_TYPE_PRO_CHALLENGE_OUTRO \
        SCENE_TYPE_RIGHT_OF_PASSAGE SCENE_TYPE_TUTORIAL_INTRO SCENE_TYPE_TUTORIAL_OUTRO_SUCCESS \
        SCENE_TYPE_TUTORIAL_OUTRO_FAILURE SCENE_TYPE_VIEW_MODE"), // 827bf360
    ("Objectives", "eObjective", "Unknown Points TrickType Tutorial JumpDistance MeasurementSpecificTrickType \
        MeasurementSpecificTrick MeasurementCumulativeTrickType MeasurementCumulativeTrick GeneralTrickObjective \
        SecurityChase EnterObstacle EnterGap JumpGap GrindRail HitEachSpot LineTimerPoints LineTimerXofYPoints \
        SpecificNumberOfAnyTricks FilmTime FollowDistance FollowDestination GeneralTrickObjectiveSingle BreakBones \
        GetIntoALine InANoSkateZone InAChase PhotoCustom LinePoints"), // 827c5050
    ("Skate", "eSkateChallengeType", "Trick TrickSequence TrickType TrickTypeSequence"), // 827be5e0
    ("Transition", "eChallengeTransitionType", "Normal ToNIS ToGameplay ToGameplayLoadScreen ToChallengeAssetLoad \
        ToFESignup"), // 827c6de8
    ("Trick", "eTrickType", "None Air Flip Gap Grab Grind Invert Lip Manual Slide Spin Revert"), // 827aa308
];

/// The tables this file creates; an unknown name in them reports through the lead's `__missing`.
const TABLES: [&str; 17] = ["Objectives", "ChallengeManager", "ChallengeReplay", "Film", "Camera", "FollowTheLeader",
    "Skate", "Random", "Entity", "LivingWorld", "Photo", "Transition", "Trigger", "Contest", "ChallengeSFX",
    "ChallengeMusic", "Time"];

/// Lua the natives need around them. Player ids stay with the lead's AI bindings. The objective rows are rebuilt by
/// the HUD screens' state handlers (holder vt+20): film Countdown (0x82368d30 case 8), photo Active (0x8236ab60
/// case 2), FollowMe Countdown (0x8236c190 case 7). ChangeHUDState only queues the state (0x8234e720), so the
/// rebuild lands on the next tick.
const GLUE: &str = r#"
Broadcasters = Broadcasters or {}
Broadcasters.Ports = {PresThread = 0}
function cMsgEnterRealTime() __s1m.realtime() end
do
  local change = HUD.ChangeHUDState
  HUD.ChangeHUDState = function(state, sub)
    local s = HUD.eSk8HUDState
    if (state == s.MissionFilmChallenge and sub == HUD.eSk8HUDFilmState.Countdown)
        or (state == s.MissionPhotoChallenge and sub == HUD.eSk8HUDPhotoState.Active)
        or (state == s.MissionFollowMeChallenge and sub == HUD.eSk8HUDFollowMeState.Countdown) then
      __s1m.rebuild()
    end
    -- Not retail (user 10-15, Rolling Rehab "you're able to move before the timer starts"): the skater is held from
    -- the film Countdown to Record. Retail's scripts never pin here, the Countdown handler (0x82368d30 case 8) only
    -- resets its timer display and AddHUDCountdownTimer (0x827a9dd8) only queues the timer. UNTRACED: the player
    -- reposition's messages (0x823f4a08, VMX). A challenge's end unpins (Skate1Challenge::end).
    if state == s.MissionFilmChallenge and sub == HUD.eSk8HUDFilmState.Countdown then
      AI.PinAI(0)
    elseif state == s.MissionFilmChallenge and sub == HUD.eSk8HUDFilmState.Record then
      AI.UnpinAI(0)
    end
    return change(state, sub)
  end
end
do
  local reposition, pin, unpin = AI.RepositionAI, AI.PinAI, AI.UnpinAI
  AI.RepositionAI = function(id, at, ...) if id == 0 then return reposition(id, at, ...) end return __s1m.reposition(id, at, ...) end
  AI.PinAI = function(id) if id == 0 then return pin(id) end return __s1m.pin(id, true) end
  AI.UnpinAI = function(id) if id == 0 then return unpin(id) end return __s1m.pin(id, false) end
  -- INFERRED (name; Community Spirit's teardown passes self.EndLocation, self.Location): the first locator when the
  -- challenge has one, else the second. UNTRACED: the C++ body (its pick may test the locator's existence instead).
  AI.RepositionAIAtOneOrOther = function(id, a, b)
    if a ~= nil and a ~= "" then return AI.RepositionAI(id, a) end
    return AI.RepositionAI(id, b)
  end
end
-- RETAIL 0x827b9af0 -> 0x82340048: a tutorial objective row; units flag 1 = the _FEET label with the integer
-- distance x3, 0 = ID_PRO_TUTORIAL_NOLLIE_OBJ1 with the distance (research6 3e). U14: Settings > Units decides.
HUD.SetNollieTutorialObjective = function(slot, distance)
  if __s1m.imperial() then
    return HUD.SetTutorialObjective(slot, "ID_PRO_TUTORIAL_NOLLIE_OBJ1_FEET", distance * 3)
  end
  return HUD.SetTutorialObjective(slot, "ID_PRO_TUTORIAL_NOLLIE_OBJ1", distance)
end
for _, name in ipairs(__s1m_tables) do
  if getmetatable(_G[name]) == nil then
    setmetatable(_G[name], {__index = function(_, key)
      return function() if __missing then __missing(name .. "." .. key) end end
    end})
  end
end
"#;

/// The mission's `self` as the scripts read it: RequiredTrickType's hex strings ("00000005") become the eTrickType
/// integers they compare with Trick.eTrickType; the arrays the scripts loop over but the vault leaves out are empty
/// (C++ struct-backed fields: an absent array is Count 0; ledger ruling).
pub(crate) fn fields(raw: &Value) -> Value {
    let mut f = raw.clone();
    if let Some(types) = f.get_mut("RequiredTrickType").and_then(Value::as_array_mut) {
        for t in types {
            if let Some(hex) = t.as_str() {
                *t = json!(i64::from_str_radix(hex, 16).unwrap_or(0));
            }
        }
    }
    for key in ["GeneralTrickObjective", "SpecificMeasurementObjective", "TotalMeasurementObjective",
                "BestTrickSpectatorWaypoint"] {
        if f.get(key).is_none_or(Value::is_null) {
            f[key] = json!([]);
        }
    }
    f
}

/// tutorial.json's `steps` for one mission. A null handler is an empty chunk on the disc: no handler.
pub(crate) fn steps(mission: &Value) -> BTreeMap<String, BTreeMap<String, String>> {
    let Some(steps) = mission.get("steps").and_then(Value::as_object) else { return BTreeMap::new() };
    steps
        .iter()
        .map(|(step, events)| {
            let events = events.as_object().into_iter().flatten();
            (step.clone(), events.filter_map(|(e, f)| Some((e.clone(), f.as_str()?.to_owned()))).collect())
        })
        .collect()
}

/// The game's side, every frame (lead edit L13).
#[derive(Clone, Default)]
pub(crate) struct Facts {
    pub dt: f32,
    /// World positions by Lua id: the player (0) and each spawned AI rider.
    pub positions: BTreeMap<i64, [f32; 3]>,
    /// The player's transform (Film.SetSessionMarker).
    pub player: M4,
    /// ChallengeReplay.GetLastFrameNumberWritten.
    pub frame: i64,
    /// The cameraman's speech channel is playing (AskIfCameramanIsSpeaking's reply).
    pub cameraman_playing: bool,
    pub no_skate_zone: bool,
    /// The last completed air per Lua id.
    pub airs: BTreeMap<i64, Air>,
    /// Trick.IsInAirWithValidLaunchTrick per id.
    pub valid_launch: BTreeMap<i64, bool>,
    pub in_air_or_grinding: bool,
    pub in_sequence: bool,
    pub line_timer_on: bool,
    /// eTrickType of each entry in the player's active trick list (IsDoingTrickType).
    pub doing: Vec<i64>,
    /// The current sequence's count per eTrickType (GetNumTrickType).
    pub sequence_types: [i64; 12],
    /// Bones broken (GetFEBonesBrokenCount).
    pub bones: i64,
    /// GetCharacterPercentageWillDoABDTrick per AI id (Skate1Ai::abd).
    pub abd: BTreeMap<i64, i64>,
    pub player_regular: bool,
    /// The player's SessionMarker counters (session_marker `sets` / `uses`).
    pub marker_sets: u32,
    pub marker_uses: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Air {
    /// The landed trick's IDENTIFIERS index (FirstScorableId's rule).
    pub scorable: i64,
    /// Signed 180s turned in the air.
    pub spins: i64,
    pub switched: bool,
    /// Horizontal distance, metres.
    pub horizontal: f32,
}

/// What the lead carries out (lead edit L8).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Request {
    /// Film.ReturnPlayerToSessionMarker.
    PlayerTo(M4),
    /// An AI rider native for that Lua id (Skate1Ai).
    Ai(i64, AiCall),
    LoadPaths,
    UnloadPaths,
    SelectCharacters,
    FilterReset,
    /// AI.FilterPaths: the round, 0-based.
    FilterPaths(usize),
    /// A speech table line, "speaker:line:context".
    Speech { speaker: i64, line: i64, context: i64 },
    StopCameraman,
    Nis(Nis),
    /// ChallengeReplay.EnterFilmReplay(frame).
    FilmReplay(i64),
    /// cMsgEnterRealTime: the film replay ends.
    RealTime,
    /// HUD.SetWinState.
    WinState(bool),
    /// Challenge.LoseChallenge.
    Lose,
    ResetScoring(bool),
    ManualsAreConnectors(bool),
    /// LivingWorld.StartTakingPhotos / StopTakingPhotos.
    Photographing(bool),
    /// Skate.TriggerSkate101 (15), Photo.TriggerSessionMarker101 (14).
    Skate101(u8),
    SwitchCamera(i64),
    /// Camera.SetGameplayCamera(n) / ResetGameplayCamera.
    Camera(Option<i64>),
    EndTransition,
    ResetVolume(String),
    DisableWipeoutReset,
    AmbientSkaters(bool),
    AiChallenge(bool),
    /// Challenge.SetTriggerVisual(volume, eChallengeIndicators).
    Indicator { volume: String, kind: i64 },
    ClearIndicators(i64),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum AiCall {
    Spawn(String),
    /// RepositionAI(id, locator, slot): arg 3 when a number, else the id (RETAIL 0x827c07a0).
    Reposition(String, i64),
    Unspawn,
    Pin(bool),
    Regular(bool),
    StartPath { branching: bool, avoid: Vec<i64> },
    FollowTrickPattern { trick: i64, spins: i64 },
    ClearBehavior,
    BandOff,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Nis {
    Offer(String),
    /// The scene and whether its end raises the transition's load screen. INFERRED from its twin EnterIntroOfferNIS
    /// (RETAIL 0x827bfe10): 0x827bfe90 is read as the same code. It stores
    /// load type 4 for ToGameplayLoadScreen / ToChallengeAssetLoad (3 / 4), else 0, and the NIS's end (0x827cf080)
    /// starts a non-zero one (0x827cf430 -> LoadingQuick, research.md "Loading screens").
    Accept(String, bool),
    Decline(String),
    /// EnterFlythroughNIS(scene, transition): Community Spirit's per-round scene; its end posts nischallengenotify.
    Flythrough(String),
    /// EnterReplayNIS: replay frames.
    Replay { scene: String, from: i64, to: i64 },
    Kill,
}

/// The objective holder `[0x830fcb18 + 4*active]` (ledger: holder layout).
#[derive(Default)]
pub(crate) struct Holder {
    /// The running mission's eChallengeTypes ([0x830a3f64]).
    pub(crate) active: i64,
    pub(crate) rows: Vec<Row>,
    /// +12 UpdateBackEndTotalMissionTime.
    pub(crate) time: f64,
    /// +16 TellBackEndChallengeWon / Lost.
    pub(crate) backend_won: bool,
    /// +20 / +24 PrepareCashState.
    pub(crate) cash: (i64, i64),
    /// Film +604 TellBackEndInReplay.
    pub(crate) replay: bool,
    /// Photo +584 PreparePhotoFailState.
    pub(crate) fail: i64,
    /// Photo +600 / +604 (UpdateBoundaryState): the session marker state while the holder carries the mission's id.
    pub(crate) boundary: Option<i64>,
}

/// The player's session marker as this mission sees it: the lead's counters last tick, and the snapshot
/// (challenge type, boundary state) taken by the last set (RETAIL 0x823d58b0).
#[derive(Default)]
struct Marker {
    sets: u32,
    uses: u32,
    snapshot: Option<(i64, i64)>,
}

/// The skate manager [0x830fcb28] (ledger: Skate manager layout).
#[derive(Default)]
struct SkateMgr {
    /// The used-trick ring (+584, 3 x 36 B): trick, spins (+692 + 8k), switched (+696 + 8k).
    ring: [(i64, i64, bool); 3],
    /// +716 the newest slot (-1 after a reset), +717 the count.
    newest: i64,
    count: usize,
    /// PrepareForExactTrick 1 / PrepareForTrickType 2 (the ring patterns' first word).
    kind: i64,
    /// The scratchpad items (+732.. ; the Lua index is 1-based), the last set spins (+768) / switch (+772),
    /// the index (+779).
    items: [i64; 3],
    spins: i64,
    switched: bool,
    index: i64,
    /// The ring slot SetPatternLastTrick found repeated (the HUD's "already been done").
    hit: Option<usize>,
    /// The last trick a leader set (SKATEMission.GetTrickLanded).
    landed: Option<i64>,
    /// Per player id: rank (+28), status (+100), order (+108), round time (+96).
    players: BTreeMap<i64, (i64, i64, i64, f64)>,
    /// [mgr+28] UpdateCurrentPlayerIndex.
    current: i64,
}

impl SkateMgr {
    /// ResetSkateTrackers (0x82343218).
    fn reset(&mut self) {
        let kind = self.kind;
        *self = Self { newest: -1, spins: -1, kind, ..Self::default() };
    }

    fn used(&self) -> Vec<i64> {
        self.ring.iter().take(self.count).map(|r| r.0).collect()
    }
}

/// One 88-byte objective row: +24 kind, +4 trick, +28 done, +32 label, +72 target, +76 current, +84 checkbox.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Row {
    pub(crate) kind: i64,
    pub(crate) trick: i64,
    pub(crate) done: bool,
    pub(crate) label: String,
    pub(crate) target: i64,
    pub(crate) current: i64,
    pub(crate) checkbox: bool,
}

#[derive(Default)]
pub(crate) struct Missions {
    fields: Value,
    pub(crate) holder: Holder,
    requests: Vec<Request>,
    next_id: i64,
    now: f64,
    countups: BTreeMap<i64, f64>,
    facts: Facts,
    /// Film.SetSessionMarker.
    film_marker: Option<M4>,
    ask_cameraman: bool,
    marker: Marker,
    skate: SkateMgr,
    follow: crate::challenge::follow::Follow,
    /// AI ids between SpawnAIAtLocation and UnspawnAI.
    spawned: BTreeSet<i64>,
    /// AI.SpawnSkaterRegular per id.
    regular: BTreeMap<i64, bool>,
    rng: u64,
    win_state: bool,
    /// A queued HUD state that rebuilds the rows (GLUE).
    rebuild: bool,
    pub(crate) contest: Contest,
}

/// Community Spirit's contest as Contest.* writes it (what its HUD and leaderboard read); competitors 1-based, the
/// player 1. UNTRACED: the C++ contest object (its natives' bodies); fallback: the values as the script passes them.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Contest {
    pub(crate) round: i64,
    pub(crate) rounds: i64,
    /// SetCompetitorScore: this round's best trick per competitor.
    pub(crate) scores: BTreeMap<i64, i64>,
    /// SetCompetitorPoints: this round's points per competitor.
    pub(crate) points: BTreeMap<i64, i64>,
    /// SetOverallStanding: competitor -> (round score, total points, rank).
    pub(crate) standings: BTreeMap<i64, (i64, i64, i64)>,
    /// SetForfeitTimeLeft, as skate1_online_hud keeps it (-1 -> 255).
    pub(crate) forfeit: i64,
    /// HUD.UpdateCurrentScore.
    pub(crate) current: i64,
}

impl Missions {
    /// The holder starts without rows: retail builds them only in the HUD states (GLUE). Ruling: a fresh holder per
    /// mission, where retail keeps one per challenge type (ledger U1).
    pub(crate) fn new(fields: &Value) -> Self {
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64);
        Self {
            holder: Holder {
                active: fields.get("ChallengeType").and_then(Value::as_i64).unwrap_or(NO_CHALLENGE),
                ..Holder::default()
            },
            fields: fields.clone(),
            rng: seed | 1,
            ..Self::default()
        }
    }

    /// Random.GetRandomNumber: 1 + floor(r * n / 2^32) (RETAIL formula). UNTRACED source (U10): xorshift64.
    fn random(&mut self, n: i64) -> i64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        1 + (((self.rng >> 32) as i64 * n) >> 32)
    }

    fn ai(&mut self, id: i64, call: AiCall) {
        self.requests.push(Request::Ai(id, call));
    }

    /// A queued row rebuild, now: the screens call it before their frame, as retail's HUD state handler builds the
    /// rows before the overlay's SetState reads them (skate1_challenge `screens_frame`; agent B, item B6).
    pub(crate) fn rebuild_rows(&mut self) {
        if std::mem::take(&mut self.rebuild) {
            self.holder.rows = rows(&self.fields);
        }
    }

    /// One frame: the events this file posts itself, in order.
    pub(crate) fn tick(&mut self, facts: &Facts) -> Vec<(String, Value)> {
        self.now += f64::from(facts.dt);
        self.facts = facts.clone();
        if std::mem::take(&mut self.rebuild) {
            self.holder.rows = rows(&self.fields);
        }
        let mut out = Vec::new();
        if std::mem::take(&mut self.ask_cameraman) {
            out.push(("cameramanspeaking".to_owned(), json!({"IsPlaying": facts.cameraman_playing})));
        }
        if let (Some(a), Some(b)) = (facts.positions.get(&self.follow.leader), facts.positions.get(&0)) {
            let d = (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f32>().sqrt();
            out.extend(self.follow.crossings(f64::from(d)).into_iter().map(|(e, v)| (e.to_owned(), v)));
        }
        // RETAIL: the player's set posts cMsgSetMarker (handler 0x823d58b0 snapshots the live boundary); the use posts
        // cMsgUseMarker, whose handler 0x823d59f0 posts markerreset while the snapshot's challenge still runs
        // (0x823d2468 -> instance +1841, set by start 0x82bfd380). This mission is running while it ticks.
        if facts.marker_sets > self.marker.sets {
            self.set_marker();
        }
        if let (true, Some((kind, state))) = (facts.marker_uses > self.marker.uses, self.marker.snapshot) {
            out.push(("markerreset".to_owned(), json!({"PlayerID": 0, "ChallengeType": kind, "MarkerState": state})));
        }
        (self.marker.sets, self.marker.uses) = (facts.marker_sets, facts.marker_uses);
        out
    }

    fn set_marker(&mut self) {
        self.marker.snapshot = self.holder.boundary.map(|s| (self.holder.active, s));
    }

    pub(crate) fn drain(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }

    /// The last HUD.SetWinState.
    pub(crate) fn won(&self) -> bool {
        self.win_state
    }

    fn id(&mut self) -> i64 {
        self.next_id += 1;
        self.next_id
    }

    fn say(&mut self, speaker: i64, line: i64, context: i64) {
        self.requests.push(Request::Speech { speaker, line, context });
    }

    fn air(&self, id: i64) -> Air {
        self.facts.airs.get(&id).copied().unwrap_or_default()
    }

    fn first(&mut self, kind: i64) -> Option<&mut Row> {
        self.holder.rows.iter_mut().find(|r| r.kind == kind)
    }

    /// HUD.IncrementRequiredTrickTypeNumDone (0x827b99a8).
    fn increment(&mut self, trick: i64) {
        if [NO_CHALLENGE, 24, 25, 29].contains(&self.holder.active) {
            return;
        }
        if let Some(r) = self.holder.rows.iter_mut().find(|r| r.kind == TRICK_TYPE && r.trick == trick) {
            r.current += 1;
        }
    }

    /// HUD.SetWinState (0x827b9dd0): the manager's vt+116, a `blr` for films (0x82328070).
    fn set_win_state(&mut self, won: bool) {
        if self.holder.active != FILM {
            self.win_state = won;
            self.requests.push(Request::WinState(won));
        }
    }
}

/// The rows the objective builder 0x8233d738 adds from the mission's fields, in its order; the holder keeps 3.
pub(crate) fn rows(f: &Value) -> Vec<Row> {
    let int = |k: &str| f.get(k).and_then(Value::as_f64).unwrap_or(0.) as i64;
    let text = |k: &str| f.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
    let row = |kind: i64| Row { kind, ..Row::default() };
    let challenge = int("ChallengeType");
    let mut rows = Vec::new();
    if challenge == PHOTO {
        if int("BreakBonesCount") > 0 {
            // INFERRED target = BreakBonesCount (unreachable: My First Photo has 0).
            rows.push(Row { target: int("BreakBonesCount"), ..row(BREAK_BONES) });
            return rows;
        }
        let custom = text("PhotoCustomObjective");
        if !custom.is_empty() {
            rows.push(Row { label: custom, ..row(PHOTO_CUSTOM) });
        }
    }
    if challenge == FILM {
        if f.get("InANoSkateZoneObjective") == Some(&json!(true)) {
            rows.push(Row { label: "ID_OBJECTIVE_NOSKATEZONE".into(), ..row(NO_SKATE_ZONE) });
        }
        if int("FilmChallengeType") != 0 {
            rows.push(Row { label: "ID_OBJECTIVE_GETLINE".into(), ..row(GET_LINE) });
        }
    }
    if int("PointRequirement") > 0 {
        rows.push(Row { target: int("PointRequirement"), ..row(POINTS) });
    }
    let targets = f.get("Hash_769F1AAE200A496C").and_then(Value::as_array).cloned().unwrap_or_default();
    let tricks = f.get("RequiredTrickType").and_then(Value::as_array).cloned().unwrap_or_default();
    for (i, trick) in tricks.iter().filter_map(Value::as_i64).enumerate() {
        if trick != 0 {
            let target = targets.get(i).and_then(Value::as_i64).unwrap_or(0);
            rows.push(Row {
                trick,
                target: if target == 0 { 1 } else { target },
                checkbox: challenge == PHOTO,
                ..row(TRICK_TYPE)
            });
        }
    }
    if int("JumpObjective") > 0 {
        // INFERRED target = JumpObjective (unreachable: 0 in both films).
        rows.push(Row { target: int("JumpObjective"), ..row(JUMP_DISTANCE) });
    }
    if int("Hash_CDD06431584F5AD9") > 0 {
        rows.push(Row { target: int("Hash_CDD06431584F5AD9"), ..row(SPECIFIC_ANY) });
    }
    if f.get("Hash_3774A91CE72FE9BB") == Some(&json!(true)) {
        rows.push(Row { label: "ID_CHALLENGE_FILM_HE_U1_FILM01_OBJ".into(), ..row(FILM_TIME) });
    }
    let distance = int("FollowTheLeaderFailureDistance");
    if distance > 0 {
        rows.push(Row { target: distance, label: text("Hash_09BC21052E21CB32"), ..row(FOLLOW_DISTANCE) });
        // INFERRED: the label field by its hash B1175B0D6C12C680 ("ID_OBJECTIVE_GET_TO_SCHOOL" in the data).
        rows.push(Row { label: text("Hash_B1175B0D6C12C680"), ..row(FOLLOW_DESTINATION) });
    }
    rows.truncate(3);
    rows
}

/// 0x8233b7e0: whether a row is satisfied.
fn done(row: &Row, active: i64) -> bool {
    match row.kind {
        POINTS | LINE_POINTS if active == OWN_THE_SPOT => row.current > row.target,
        POINTS | LINE_POINTS | TRICK_TYPE | BREAK_BONES => row.current >= row.target,
        3..=22 | 24..=27 => row.done,
        _ => false,
    }
}

/// eTrickType -> the TrickType row's title, singular (target <= 1) and plural (table 0x830a3f68, 12 bytes per type;
/// the other types are NULL).
const TRICK_TITLES: [(i64, &str, &str); 7] = [
    (2, "ID_OBJECTIVE_TITLE_TRICK_FLIP", "ID_OBJECTIVE_TITLE_TRICK_FLIPS"),
    (3, "ID_OBJECTIVE_TITLE_TRICK_GAP", "ID_OBJECTIVE_TITLE_TRICK_GAPS"),
    (4, "ID_OBJECTIVE_TITLE_TRICK_GRAB", "ID_OBJECTIVE_TITLE_TRICK_GRABS"),
    (5, "ID_OBJECTIVE_TITLE_TRICK_GRIND", "ID_OBJECTIVE_TITLE_TRICK_GRINDS"),
    (8, "ID_OBJECTIVE_TITLE_TRICK_MANUAL", "ID_OBJECTIVE_TITLE_TRICK_MANUALS"),
    (10, "ID_OBJECTIVE_TITLE_TRICK_SPIN", "ID_OBJECTIVE_TITLE_TRICK_SPINS"),
    (11, "ID_OBJECTIVE_TITLE_TRICK_REVERT", "ID_OBJECTIVE_TITLE_TRICK_REVERTS"),
];
/// 0x824838f0: the units flag [[0x830fce9c]+18904] picks "{0:d} m" (0x92099686, x1) at 0, "{0:d} ft" (0x413c56da, x3)
/// at 1. RETAIL: init 0x8231d360 sets 1 and clears it for XGetGameRegion 0x02FE (Europe, 0x824834f0); the profile
/// option (loaded at 0x8231a078) overrides it. Ruling U14 (coordinator 10-14): S3R's Settings > Units decides
/// (`hud_runtime::imperial()`), not a region constant.
fn units(imperial: bool) -> (&'static str, i64) {
    if imperial { ("413c56da", 3) } else { ("92099686", 1) }
}

/// "{0:d}" / "{0:s}" -> args[0], "{1:d}" -> args[1].
fn fill(template: &str, args: &[String]) -> String {
    let mut s = template.to_owned();
    for (i, a) in args.iter().enumerate() {
        s = s.replace(&format!("{{{i}:d}}"), a).replace(&format!("{{{i}:s}}"), a);
    }
    s
}

/// The objective HUD's rows: (title 0x8233ab48, result 0x8233b9d8, done 0x8233b7e0, checkbox +84). `text` localizes
/// an ID name or a hex label key and answers "" for a missing one.
// ponytail: the titles/results of kinds 4, 17, 18, 23, 28 aren't ported: no Skate 1 mission builds those rows. They
// fall back to the row label / "".
pub(crate) fn objectives(m: &Missions, text: &dyn Fn(&str) -> String) -> Vec<(String, String, bool, bool)> {
    let active = m.holder.active;
    m.holder
        .rows
        .iter()
        .map(|r| {
            let target = r.target.to_string();
            let title = match r.kind {
                POINTS if active == OWN_THE_SPOT => fill(&text("ID_OBJECTIVE_BEATPOINTS"), &[target]),
                POINTS => fill(&text("ID_OBJECTIVE_TITLE_GET_POINTS"), &[target]),
                TRICK_TYPE => TRICK_TITLES
                    .iter()
                    .find(|t| t.0 == r.trick)
                    .map_or_else(String::new, |t| fill(&text(if r.target <= 1 { t.1 } else { t.2 }), &[target])),
                // UNTRACED: 0x824838f0's buffer is passed at +1 (its first character); fallback: the whole string.
                FOLLOW_DISTANCE => {
                    let (label, scale) = units(crate::hud_runtime::imperial());
                    fill(&text("ID_OBJECTIVE_STAY_WITHIN"), &[fill(&text(label), &[(r.target * scale).to_string()])])
                }
                // INFERRED: the label is localized when the row is built (0x82392050).
                _ => text(&r.label),
            };
            let result = match r.kind {
                POINTS => title.clone(),
                // "#UNKNOWN" in retail; the screens draw the checkbox instead (agreed with s1-screens).
                TRICK_TYPE if r.checkbox => String::new(),
                TRICK_TYPE => fill(&text("ID_COMMON_NUMBER_OVER_NUMBER"), &[r.current.to_string(), r.target.to_string()]),
                _ => String::new(),
            };
            (title, result, done(r, active), r.checkbox)
        })
        .collect()
}

/// The global table `name`, created when absent.
fn table(lua: &Lua, name: &str) -> mlua::Result<Table> {
    match lua.globals().raw_get::<LuaValue>(name)? {
        LuaValue::Table(t) => Ok(t),
        _ => {
            let t = lua.create_table()?;
            lua.globals().raw_set(name, t.clone())?;
            Ok(t)
        }
    }
}

// ponytail: Skate 2's `bind` is typed to its own Shared; this is the same five lines for Missions.
fn bind<A, R, F>(lua: &Lua, table: &Table, shared: &Shared, name: &str, f: F) -> mlua::Result<()>
where
    A: FromLuaMulti,
    R: IntoLuaMulti,
    F: Fn(&mut Missions, A) -> R + Send + 'static,
{
    let shared = shared.clone();
    table.raw_set(name, lua.create_function(move |_, args: A| Ok(f(&mut shared.lock().unwrap(), args)))?)
}

/// Registers every mission native. Call after skate1_challenge's `natives` (it wraps three of the lead's AI natives).
pub(crate) fn register(lua: &Lua, shared: &Shared) -> mlua::Result<()> {
    for (table_name, name, names) in ENUMS {
        let t = table(lua, table_name)?;
        let e = match t.raw_get::<LuaValue>(name)? {
            LuaValue::Table(e) => e,
            _ => {
                let e = lua.create_table()?;
                t.raw_set(name, e.clone())?;
                e
            }
        };
        for (i, n) in names.split_whitespace().enumerate() {
            if e.raw_get::<LuaValue>(n)?.is_nil() {
                e.raw_set(n, i as i64)?;
            }
        }
    }
    for name in TABLES {
        table(lua, name)?;
    }
    objective_natives(lua, shared)?;
    film_natives(lua, shared)?;
    follow_natives(lua, shared)?;
    ai_natives(lua, shared)?;
    photo_natives(lua, shared)?;
    skate_natives(lua, shared)?;
    pro_natives(lua, shared)?;
    contest_natives(lua, shared)?;
    let s1m = lua.create_table()?;
    bind(lua, &s1m, shared, "realtime", |m, ()| m.requests.push(Request::RealTime))?;
    bind(lua, &s1m, shared, "rebuild", |m, ()| m.rebuild = true)?;
    s1m.set("imperial", lua.create_function(|_, ()| Ok(crate::hud_runtime::imperial()))?)?;
    bind(lua, &s1m, shared, "reposition", |m, (id, at, slot): (i64, Option<String>, Option<LuaValue>)| {
        let slot = match slot {
            Some(LuaValue::Integer(n)) => n,
            Some(LuaValue::Number(n)) => n as i64,
            _ => id,
        };
        if let Some(at) = at {
            m.requests.push(Request::Ai(id, AiCall::Reposition(at, slot)));
        }
    })?;
    bind(lua, &s1m, shared, "pin", |m, (id, on): (i64, bool)| m.requests.push(Request::Ai(id, AiCall::Pin(on))))?;
    lua.globals().raw_set("__s1m", s1m)?;
    lua.globals().raw_set("__s1m_tables", TABLES.to_vec())?;
    lua.load(GLUE).set_name("@skate1-missions").exec()
}

fn objective_natives(lua: &Lua, shared: &Shared) -> mlua::Result<()> {
    let o = table(lua, "Objectives")?;
    bind(lua, &o, shared, "GetStatusAllObjectives", |m, ()| {
        let active = m.holder.active;
        active != NO_CHALLENGE && m.holder.rows.iter().all(|r| done(r, active))
    })?;
    bind(lua, &o, shared, "SetObjectiveComplete", |m, kind: i64| {
        if let Some(r) = m.first(kind) {
            r.done = true;
        }
    })?;
    bind(lua, &o, shared, "SetObjectiveStatus", |m, (kind, on): (i64, bool)| {
        if let Some(r) = m.first(kind) {
            r.done = on;
        }
    })?;
    bind(lua, &o, shared, "GetTotalNumObjectives", |m, ()| m.holder.rows.len() as i64)?;
    bind(lua, &o, shared, "GetCurrentNumCompletedObjectives", |m, ()| {
        let active = m.holder.active;
        m.holder.rows.iter().filter(|r| done(r, active)).count() as i64
    })?;
    // RETAIL 0x827c5e68 / 0x827c5f98 / 0x827c60d8 / 0x827c6208 only touch measurement rows (kinds 5-8), which no
    // Skate 1 mission builds (ledger U2): no-ops here.
    for name in ["GetGrindMeasurementObjectivesSatisfied", "GetAirMeasurementObjectivesSatisfied",
                 "GetManualMeasurementObjectivesSatisfied", "GetPowerslideMeasurementObjectivesSatisfied"] {
        bind(lua, &o, shared, name, |_, _: LuaValue| ())?;
    }
    bind(lua, &o, shared, "GetTrickMeasurementObjectivesSatisfied", |m, _: LuaValue| {
        for r in m.holder.rows.iter_mut().filter(|r| r.kind == SPECIFIC_ANY) {
            r.current += 1;
            r.done = r.current >= r.target;
        }
    })?;
    let hud = table(lua, "HUD")?;
    bind(lua, &hud, shared, "IncrementRequiredTrickTypeNumDone", |m, (_type, trick): (LuaValue, i64)| {
        m.increment(trick)
    })?;
    bind(lua, &hud, shared, "PrepareCashState", |m, (a, b): (i64, i64)| m.holder.cash = (a, b))?;
    bind(lua, &hud, shared, "PrepareFilmFailState", |_, _: LuaValue| ())?; // RETAIL 0x827b91f0: no-op
    bind(lua, &hud, shared, "PreparePhotoFailState", |m, reason: i64| m.holder.fail = reason)?;
    bind(lua, &hud, shared, "SetWinState", |m, won: bool| m.set_win_state(won))?;
    let cm = table(lua, "ChallengeManager")?;
    bind(lua, &cm, shared, "UpdateBackEndScore", |m, score: f64| {
        if let Some(r) = m.holder.rows.iter_mut().find(|r| [POINTS, BREAK_BONES, LINE_POINTS].contains(&r.kind)) {
            r.current = score as i64;
        }
    })?;
    bind(lua, &cm, shared, "TellBackEndChallengeWon", |m, ()| m.holder.backend_won = true)?;
    bind(lua, &cm, shared, "TellBackEndChallengeLost", |m, ()| m.holder.backend_won = false)?;
    bind(lua, &cm, shared, "TellBackEndInReplay", |m, ()| m.holder.replay = true)?;
    bind(lua, &cm, shared, "UpdateBackEndTotalMissionTime", |m, t: f64| m.holder.time = t)?;
    Ok(())
}

/// A tSpeechInfo / tContextSpeechInfo attribute row: {speaker, line << 16, context}.
fn attribute(t: &Table) -> (i64, i64, i64) {
    let at = |i: i64| t.raw_get::<Option<f64>>(i).ok().flatten().unwrap_or(0.) as i64;
    (at(1), at(2) >> 16, at(3))
}

fn film_natives(lua: &Lua, shared: &Shared) -> mlua::Result<()> {
    let film = table(lua, "Film")?;
    // INFERRED (names; the retry flow): the marker is the player's transform when set.
    bind(lua, &film, shared, "SetSessionMarker", |m, _: LuaValue| m.film_marker = Some(m.facts.player))?;
    bind(lua, &film, shared, "ReturnPlayerToSessionMarker", |m, _: LuaValue| {
        if let Some(at) = m.film_marker {
            m.requests.push(Request::PlayerTo(at));
        }
    })?;
    let replay = table(lua, "ChallengeReplay")?;
    bind(lua, &replay, shared, "GetLastFrameNumberWritten", |m, ()| m.facts.frame)?;
    // RETAIL 0x827bc128: message 0x3E580FCA(frame) to the replay system.
    bind(lua, &replay, shared, "EnterFilmReplay", |m, frame: i64| m.requests.push(Request::FilmReplay(frame)))?;
    bind(lua, &replay, shared, "CameraOutroMovieSetupWithCameraInfo", |_, _: LuaValue| ())?; // RETAIL no-op
    let camera = table(lua, "Camera")?;
    // RETAIL 0x827c6d70: [0x831ab140]+4992 = n (reset: -1). UNTRACED consumer (U4): the lead's no-op.
    bind(lua, &camera, shared, "SetGameplayCamera", |m, n: i64| m.requests.push(Request::Camera(Some(n))))?;
    bind(lua, &camera, shared, "ResetGameplayCamera", |m, ()| m.requests.push(Request::Camera(None)))?;
    let speech = table(lua, "ChallengeSpeech")?;
    bind(lua, &speech, shared, "AskIfCameramanIsSpeaking", |m, ()| m.ask_cameraman = true)?;
    bind(lua, &speech, shared, "StopCameramanSpeech", |m, ()| m.requests.push(Request::StopCameraman))?; // 0x827bbe20
    // RETAIL 0x827bbbc8 (flags 0) / 0x827bbc58 (flags = context) / 0x827bbca0 / 0x827bba78 (flags = followee).
    bind(lua, &speech, shared, "PlayNPCSpeechUsingAttribute", |m, t: Table| {
        let (speaker, line, _) = attribute(&t);
        m.say(speaker, line, 0)
    })?;
    bind(lua, &speech, shared, "PlayContextSpeechUsingAttribute", |m, t: Table| {
        let (speaker, line, context) = attribute(&t);
        m.say(speaker, line, context)
    })?;
    bind(lua, &speech, shared, "PlayNPCSpeech", |m, (speaker, line): (i64, i64)| m.say(speaker, line, 0))?;
    bind(lua, &speech, shared, "PlayFollowMeWarningSpeech", |m, (speaker, line, followee): (i64, i64, i64)| {
        m.say(speaker, line, followee)
    })?;
    let cm = table(lua, "ChallengeManager")?;
    // RETAIL rec+122 / rec+120 (0x827bc790); UNTRACED consumers (U7): the lead's no-op.
    bind(lua, &cm, shared, "SetResetScoring", |m, (on, _id): (bool, LuaValue)| m.requests.push(Request::ResetScoring(on)))?;
    // The engine's connector flag is the player's (Observer::split). Community Spirit sets it per competitor (user
    // 10-15): a rider's call no longer flips the player's.
    bind(lua, &cm, shared, "SetManualsAreConnectors", |m, (on, id): (bool, Option<i64>)| {
        if id.unwrap_or(0) == 0 {
            m.requests.push(Request::ManualsAreConnectors(on))
        }
    })?;
    // RETAIL 0x827bd5c8 -> 0x82419688(world, id); INFERRED (U5): the camera follows that skater.
    bind(lua, &cm, shared, "SwitchCamera", |m, id: i64| m.requests.push(Request::SwitchCamera(id)))?;
    let boundary = table(lua, "ChallengeBoundary")?;
    bind(lua, &boundary, shared, "GetIfLocalPlayerIsInNoSkateZone", |m, ()| m.facts.no_skate_zone)?;
    let challenge = table(lua, "Challenge")?;
    bind(lua, &challenge, shared, "CheckIfCheatEnabled", |_, _: LuaValue| 0)?; // INFERRED: no cheats in the port
    // RETAIL 0x827adfe8; its sub-calls 0x823f9780 / 0x823d2b28 are UNTRACED (U9). Fallback: the career result is a loss.
    bind(lua, &challenge, shared, "LoseChallenge", |m, _: LuaValue| m.requests.push(Request::Lose))?;
    let trick = table(lua, "Trick")?;
    bind(lua, &trick, shared, "GetCompletedAirHorizontalDistance", |m, id: i64| m.air(id).horizontal)?;
    // Only called inside loops over GeneralTrickObjective, which no mission has (fields(): []).
    bind(lua, &trick, shared, "SetPattern", |_, _: LuaValue| ())?;
    bind(lua, &trick, shared, "MatchedPattern", |_, _: LuaValue| false)?;
    let timer = table(lua, "ChallengeTimer")?;
    bind(lua, &timer, shared, "AddCountupTimer", |m, ()| {
        let id = m.id();
        m.countups.insert(id, m.now);
        id
    })?;
    bind(lua, &timer, shared, "GetCountupTime", |m, id: Option<i64>| {
        id.and_then(|id| m.countups.get(&id)).map_or(0., |start| m.now - start)
    })?;
    bind(lua, &timer, shared, "RemoveCountupTimer", |m, id: Option<i64>| {
        if let Some(id) = id {
            m.countups.remove(&id);
        }
    })?;
    Ok(())
}

fn follow_natives(lua: &Lua, shared: &Shared) -> mlua::Result<()> {
    use crate::challenge::follow::GETTERS;
    let follow = table(lua, "FollowTheLeader")?;
    bind(lua, &follow, shared, "SetActorIDs", |m, leader: i64| m.follow.leader = leader)?;
    // RETAIL 0x827c22b8 copies the row; the two speech ids keep only the high half (line << 16 in the data).
    bind(lua, &follow, shared, "SetWarningInfo", |m, row: Table| {
        m.follow.info = row.sequence_values::<f64>().map(|v| v.unwrap_or(0.)).collect();
    })?;
    for (i, name) in GETTERS.into_iter().enumerate() {
        bind(lua, &follow, shared, name, move |m, ()| {
            let v = m.follow.info.get(i).copied().unwrap_or(0.);
            if i == 3 || i == 6 { ((v as i64) >> 16) as f64 } else { v }
        })?;
    }
    // RETAIL 0x827c2630 -> 0x82244ef0(mgr, id, float d). The crossing rule is Skate 2's (INFERRED: same manager).
    bind(lua, &follow, shared, "RegisterDistanceCallback", |m, (_id, metres): (LuaValue, f64)| {
        let id = m.id();
        m.follow.callbacks.push((id, metres, true));
        id
    })?;
    bind(lua, &follow, shared, "UnregisterDistanceCallback", |m, id: Option<i64>| {
        m.follow.callbacks.retain(|c| Some(c.0) != id)
    })?;
    bind(lua, &follow, shared, "StartDistanceUpdates", |m, ()| m.follow.updating = true)?;
    bind(lua, &follow, shared, "StopDistanceUpdates", |m, ()| m.follow.updating = false)?;
    bind(lua, &follow, shared, "ResetDistance", |m, ()| m.follow.distance = 0.)?; // RETAIL 0x827c2720
    bind(lua, &follow, shared, "GetDistance", |m, ()| m.follow.distance)?; // RETAIL 0x827c2280: float (Skate 2 rounds)
    Ok(())
}

fn ai_natives(lua: &Lua, shared: &Shared) -> mlua::Result<()> {
    let ai = table(lua, "AI")?;
    bind(lua, &ai, shared, "SpawnAIAtLocation", |m, (id, at): (i64, Option<String>)| {
        m.spawned.insert(id);
        m.ai(id, AiCall::Spawn(at.unwrap_or_default()))
    })?;
    bind(lua, &ai, shared, "UnspawnAI", |m, id: i64| {
        m.spawned.remove(&id);
        m.ai(id, AiCall::Unspawn)
    })?;
    bind(lua, &ai, shared, "SpawnSkaterRegular", |m, (id, regular): (i64, bool)| {
        m.regular.insert(id, regular);
        m.ai(id, AiCall::Regular(regular))
    })?;
    bind(lua, &ai, shared, "GetIfSkaterIsRegularStance", |m, id: i64| {
        if id == 0 { m.facts.player_regular } else { m.regular.get(&id).copied().unwrap_or(true) }
    })?;
    bind(lua, &ai, shared, "GetCharacterPercentageWillDoABDTrick", |m, id: i64| m.facts.abd.get(&id).copied().unwrap_or(0))?;
    // UNTRACED return (U16; P4 load_paths -> bool, asynchronous here): nil; fp02 stores it and never reads it.
    bind(lua, &ai, shared, "LoadPaths", |m, _: LuaValue| m.requests.push(Request::LoadPaths))?;
    bind(lua, &ai, shared, "LoadSKATEProPaths", |_, _: LuaValue| 0)?; // RETAIL 0x827c0490 stub
    bind(lua, &ai, shared, "UnloadPaths", |m, _: LuaValue| m.requests.push(Request::UnloadPaths))?;
    bind(lua, &ai, shared, "SelectCharacters", |m, _: LuaValue| m.requests.push(Request::SelectCharacters))?;
    bind(lua, &ai, shared, "FilterReset", |m, ()| m.requests.push(Request::FilterReset))?;
    // The scripts pass self.RoundPaths[round]; Skate1Ai::filter_paths takes the round.
    bind(lua, &ai, shared, "FilterPaths", |m, (_this, row): (LuaValue, String)| {
        let rounds = m.fields.get("RoundPaths").and_then(Value::as_array).cloned().unwrap_or_default();
        if let Some(round) = rounds.iter().position(|r| r.as_str() == Some(row.as_str())) {
            m.requests.push(Request::FilterPaths(round));
        }
    })?;
    // P4: AI.StartPath = (true, []). INFERRED: an explicit arg 2 (fp02's false) is honoured.
    bind(lua, &ai, shared, "StartPath", |m, (id, branching): (i64, Option<bool>)| {
        m.ai(id, AiCall::StartPath { branching: branching.unwrap_or(true), avoid: Vec::new() })
    })?;
    bind(lua, &ai, shared, "ClearBehavior", |m, id: i64| m.ai(id, AiCall::ClearBehavior))?;
    bind(lua, &ai, shared, "SwitchOffFollowMeRubberBanding", |m, id: i64| m.ai(id, AiCall::BandOff))?;
    bind(lua, &ai, shared, "CAIBackToAmbient", |_, _: LuaValue| ())?; // RETAIL 0x827c1648: li r3,0; blr
    bind(lua, &ai, shared, "EnterAiBasedChallenge", |m, ()| m.requests.push(Request::AiChallenge(true)))?;
    bind(lua, &ai, shared, "ExitAiBasedChallenge", |_, ()| ())?; // RETAIL no-op
    bind(lua, &ai, shared, "EnterAmbientSkaterChallenge", |m, ()| m.requests.push(Request::AmbientSkaters(true)))?;
    bind(lua, &ai, shared, "ExitAmbientSkaterChallenge", |m, ()| m.requests.push(Request::AmbientSkaters(false)))?;
    // P4 R23: the gesture consumer is UNTRACED; nothing.
    bind(lua, &ai, shared, "TriggerPositiveGesture", |_, _: LuaValue| ())?;
    bind(lua, &ai, shared, "TriggerNegativeGesture", |_, _: LuaValue| ())?;
    let entity = table(lua, "Entity")?;
    // Ruling: synchronous, so S.K.A.T.E.'s check_ai `while true` loop ends.
    bind(lua, &entity, shared, "DoesSkaterExist", |m, id: i64| id == 0 || m.spawned.contains(&id))?;
    let nis = table(lua, "NIS")?;
    bind(lua, &nis, shared, "EnterIntroOfferNIS", |m, (scene, _t): (String, LuaValue)| m.requests.push(Request::Nis(Nis::Offer(scene))))?;
    bind(lua, &nis, shared, "EnterIntroAcceptNIS", |m, (scene, t): (String, LuaValue)| {
        let loads = match t {
            LuaValue::Integer(n) => matches!(n, 3 | 4),
            LuaValue::Number(n) => n == 3. || n == 4.,
            _ => false,
        };
        m.requests.push(Request::Nis(Nis::Accept(scene, loads)))
    })?;
    bind(lua, &nis, shared, "EnterIntroDeclineNIS", |m, scene: String| m.requests.push(Request::Nis(Nis::Decline(scene))))?;
    bind(lua, &nis, shared, "EnterReplayNIS", |m, (scene, from, to): (String, f64, f64)| {
        m.requests.push(Request::Nis(Nis::Replay { scene, from: from as i64, to: to as i64 }))
    })?;
    bind(lua, &nis, shared, "KillNIS", |m, ()| m.requests.push(Request::Nis(Nis::Kill)))?;
    let transition = table(lua, "Transition")?;
    bind(lua, &transition, shared, "EndTransition", |m, ()| m.requests.push(Request::EndTransition))?; // 0x827c74f0
    let random = table(lua, "Random")?;
    bind(lua, &random, shared, "GetRandomNumber", |m, n: i64| m.random(n))?;
    Ok(())
}

fn photo_natives(lua: &Lua, shared: &Shared) -> mlua::Result<()> {
    let challenge = table(lua, "Challenge")?;
    // RETAIL 0x827aedd8: zeroes the spot/pro/photo holders, then writes (self id, state) for self's type.
    // ponytail: photo only; no S1 mission calls it as OwnTheSpot (9) or Pro (28).
    bind(lua, &challenge, shared, "UpdateBoundaryState", |m, (_this, state): (LuaValue, i64)| {
        m.holder.boundary = (m.holder.active == PHOTO).then_some(state)
    })?;
    bind(lua, &challenge, shared, "ResetBoundaryState", |m, _: LuaValue| m.holder.boundary = None)?; // 0x827aef38
    bind(lua, &challenge, shared, "ResetSessionOverride", |m, _: LuaValue| {
        m.holder.boundary = m.holder.boundary.map(|_| 0) // 0x827af118: +604 = +600 = 0, the id stays
    })?;
    // RETAIL 0x827af218 posts cMsgSetMarker; its handler 0x823d58b0 snapshots the live boundary.
    bind(lua, &challenge, shared, "ResetChallengeSessionMarker", |m, _: LuaValue| m.set_marker())?;
    let living = table(lua, "LivingWorld")?;
    bind(lua, &living, shared, "StartTakingPhotos", |m, _: LuaValue| m.requests.push(Request::Photographing(true)))?;
    bind(lua, &living, shared, "StopTakingPhotos", |m, _: LuaValue| m.requests.push(Request::Photographing(false)))?;
    // Only looped over BestTrickSpectatorWaypoint, absent in every mission.
    bind(lua, &living, shared, "ActivateSpectatorWaypoint", |_, _: LuaValue| ())?;
    bind(lua, &living, shared, "DeactivateSpectatorWaypoint", |_, _: LuaValue| ())?;
    let photo = table(lua, "Photo")?;
    bind(lua, &photo, shared, "TriggerSessionMarker101", |m, ()| m.requests.push(Request::Skate101(14)))?; // 0x827c4948
    let trigger = table(lua, "Trigger")?;
    bind(lua, &trigger, shared, "ResetTriggerVolumeState", |m, name: String| m.requests.push(Request::ResetVolume(name)))?;
    let trick = table(lua, "Trick")?;
    bind(lua, &trick, shared, "IsInSequence", |m, _: LuaValue| m.facts.in_sequence)?;
    bind(lua, &trick, shared, "IsSkaterInAirOrGrinding", |m, _: LuaValue| m.facts.in_air_or_grinding)?;
    // RETAIL tracker +3610 + type; INFERRED: counted per sequence.
    bind(lua, &trick, shared, "GetNumTrickType", |m, (_id, t): (LuaValue, usize)| {
        m.facts.sequence_types.get(t).copied().unwrap_or(0)
    })?;
    bind(lua, &trick, shared, "GetFEBonesBrokenCount", |m, ()| m.facts.bones as f64)?; // 0x827abda0
    // RETAIL 0x827abe10: skater+516 |= 0x10000000, a one-frame pulse (0x824153a8 copies it to +4993 and clears it)
    // latched by the bail state (vt 0x8217b1c8: 0x8268e368 sets +92; enter 0x8268e1f0 clears it). While latched the
    // bail never raises +81, the auto-reset after the bail delay. The lead's runtime skips that bail's reset (L15).
    bind(lua, &trick, shared, "DisableWipeoutReset", |m, _: LuaValue| m.requests.push(Request::DisableWipeoutReset))?;
    Ok(())
}

/// Trick.GetTrickTypeFromScorableID / GetTrickIndexFromScorableID over Skate 1's EScorableID range (0..=138).
/// RETAIL check (peer-trick-defs.md): every S1 def's TrickType (def+184) equals IDENTIFIERS' type for its name.
fn scorable_type(id: i64) -> i64 {
    usize::try_from(id).ok().filter(|&i| i <= 138).map_or(0, |i| skate_core::scoring::catalog::IDENTIFIERS[i].2 as i64)
}

fn skate_natives(lua: &Lua, shared: &Shared) -> mlua::Result<()> {
    let skate = table(lua, "Skate")?;
    bind(lua, &skate, shared, "ResetSkateTrackers", |m, ()| m.skate.reset())?;
    bind(lua, &skate, shared, "PrepareForExactTrick", |m, ()| (m.skate.kind, m.skate.ring) = (1, [(-1, -1, false); 3]))?;
    bind(lua, &skate, shared, "PrepareForTrickType", |m, ()| (m.skate.kind, m.skate.ring) = (2, [(-1, -1, false); 3]))?;
    bind(lua, &skate, shared, "SetScratchPadPatternItem", |m, (i, trick, spins, sw): (i64, i64, i64, bool)| {
        if let Some(item) = usize::try_from(i - 1).ok().and_then(|i| m.skate.items.get_mut(i)) {
            *item = trick;
        }
        (m.skate.spins, m.skate.switched, m.skate.landed) = (spins, sw, Some(trick));
    })?;
    bind(lua, &skate, shared, "SetScratchPadPatternIndex", |m, i: i64| m.skate.index = i - 1)?;
    bind(lua, &skate, shared, "CompareScratchPadPatternItem", |m, (i, trick, spins, _sw): (i64, i64, i64, LuaValue)| {
        let item = usize::try_from(i - 1).ok().and_then(|i| m.skate.items.get(i)).copied();
        item == Some(trick) && m.skate.spins == spins
    })?;
    // RETAIL type split; the flag setter (+0xe26 / +0xe29) is UNTRACED. Fallback: the id's last air against the newest
    // ring entry.
    bind(lua, &skate, shared, "MatchedPattern", |m, (id, kind): (i64, i64)| {
        let Some(&(trick, spins, _)) = usize::try_from(m.skate.newest).ok().and_then(|i| m.skate.ring.get(i)) else {
            return false;
        };
        let air = m.air(id);
        let same = match kind {
            0 | 1 => air.scorable == trick,
            2 | 3 => scorable_type(air.scorable) == scorable_type(trick),
            _ => return false,
        };
        // Playtest 10-15 diagnostic (an exact copy judged wrong): the ring's trick against the player's air.
        // ponytail: remove once a preview log has settled it.
        let name = |i: i64| usize::try_from(i).ok().and_then(|i| skate_core::scoring::catalog::IDENTIFIERS.get(i)).map_or("-", |e| e.0);
        bevy::log::info!(
            "Skate 1 S.K.A.T.E.: MatchedPattern id={id} kind={kind} ring={trick} {} spins={spins} air={} {} spins={} same={same}",
            name(trick),
            air.scorable,
            name(air.scorable),
            air.spins
        );
        same && (kind % 2 == 1 || air.spins.abs() == spins)
    })?;
    bind(lua, &skate, shared, "StartSkatePath", |m, (id, branching, avoid): (i64, bool, bool)| {
        let avoid = if avoid { m.skate.used() } else { Vec::new() };
        m.ai(id, AiCall::StartPath { branching, avoid })
    })?;
    bind(lua, &skate, shared, "TriggerSkate101", |m, ()| m.requests.push(Request::Skate101(15)))?; // 0x827bf2f8
    bind(lua, &skate, shared, "SetPlayerRank", |m, (id, v): (i64, i64)| m.skate.players.entry(id).or_default().0 = v)?;
    bind(lua, &skate, shared, "SetPlayerStatus", |m, (id, v): (i64, i64)| m.skate.players.entry(id).or_default().1 = v)?;
    bind(lua, &skate, shared, "SetPlayerOrder", |m, (id, v): (i64, i64)| m.skate.players.entry(id).or_default().2 = v)?;
    bind(lua, &skate, shared, "SetRoundTime", |m, (id, t): (i64, f64)| m.skate.players.entry(id).or_default().3 = t)?;
    bind(lua, &skate, shared, "UpdateCurrentPlayerIndex", |m, i: i64| m.skate.current = i)?;
    let trick = table(lua, "Trick")?;
    // RETAIL 0x82343270: the repeat check is the exact TrickIndex + |spins| (loose = 0; peer-trick-defs.md run 3).
    bind(lua, &trick, shared, "SetPatternLastTrick", |m, (trick, id): (i64, i64)| {
        let air = m.air(id);
        let spins = air.spins.abs();
        if let Some(slot) = m.skate.ring.iter().take(m.skate.count).position(|r| r.0 == trick && r.1 == spins) {
            m.skate.hit = Some(slot);
            return false;
        }
        let next = ((m.skate.newest + 1) % 3) as usize;
        m.skate.ring[next] = (trick, spins, air.switched);
        (m.skate.newest, m.skate.count, m.skate.hit, m.skate.landed) =
            (next as i64, (m.skate.count + 1).min(3), None, Some(trick));
        true
    })?;
    // SkateChallengeType is 0 (Trick) in the data: these are unreachable. UNTRACED bodies; fallback false.
    for name in ["SetPatternLastTrickType", "SetPatternLastSequence", "SetPatternLastSequenceType"] {
        bind(lua, &trick, shared, name, |_, _: LuaValue| false)?;
    }
    bind(lua, &trick, shared, "GetTrickTypeFromScorableID", |_, id: i64| scorable_type(id))?;
    // INFERRED: identity over Skate 1's EScorableID range.
    bind(lua, &trick, shared, "GetTrickIndexFromScorableID", |_, id: i64| if (0..=138).contains(&id) { id } else { -1 })?;
    bind(lua, &trick, shared, "GetCompletedAir180Count", |m, id: i64| m.air(id).spins.abs())?;
    bind(lua, &trick, shared, "DidCompletedAirStartSwitched", |m, id: i64| m.air(id).switched)?;
    bind(lua, &trick, shared, "IsInAirWithValidLaunchTrick", |m, id: i64| {
        m.facts.valid_launch.get(&id).copied().unwrap_or(false)
    })?;
    let ai = table(lua, "AI")?;
    // research 3.4: the follower copies the newest pattern with the leader's spins.
    bind(lua, &ai, shared, "FollowTrickPattern", |m, (id, spins): (i64, i64)| {
        let trick = usize::try_from(m.skate.newest).ok().map_or(-1, |i| m.skate.ring[i].0);
        m.ai(id, AiCall::FollowTrickPattern { trick, spins: spins.abs() })
    })?;
    Ok(())
}

/// Tricks.GetGestureString(SKATEMission.GetTrickDisplayIndex()), 0x82440d58: the set trick's record's ControllerGesture
/// (+0x9c) named by trick_gestures' export table 0x830a4060 (gesture_name). Grinds, grabs and manuals are 0, mcnone.
/// ponytail: the record is found by catalog name (retail goes index -> record through 0x8279ae28).
fn trick_gesture(trick: i64) -> &'static str {
    gesture_of(usize::try_from(trick).ok().and_then(|i| skate_core::scoring::catalog::IDENTIFIERS.get(i)).map_or("", |e| e.0))
}
/// The same by the trick's identifier (the engine's and Skate 1's spell them alike).
pub(crate) fn gesture_of(identifier: &str) -> &'static str {
    crate::skate1_screens::gesture_name(CONTROLLER_GESTURES.iter().find(|e| e.0 == identifier).map_or(0, |e| e.1))
}
/// skater.vlt (Skate 1 data/db/skater, class 6918469984A8C596) ControllerGesture, eSk8ControllerGesture, the nonzero ones.
const CONTROLLER_GESTURES: [(&str, i64); 47] = [
    ("360flip", 1), ("360hardflip", 33), ("360inwardheelflip", 35), ("360popshuvit", 2), ("christ_air", 30),
    ("fs360popshuvit", 36), ("fspopshuvit", 20), ("hardflip", 37), ("heelflip", 6), ("heelflip2", 6), ("heelflip3", 6),
    ("heelflip4", 6), ("inwardheelflip", 38), ("kickflip", 7), ("kickflip2", 7), ("kickflip3", 7), ("kickflip4", 7),
    ("laserflip", 34), ("latebackfootheelflip", 6), ("latebackfootkickflip", 7), ("lateheelflip", 6),
    ("latekickflip", 7), ("n_360flip", 42), ("n_360hardflip", 14), ("n_360inwardheelflip", 52), ("n_360popshuvit", 44),
    ("n_fs360popshuvit", 45), ("n_fspopshuvit", 41), ("n_hardflip", 46), ("n_heelflip", 47), ("n_heelflip2", 47),
    ("n_heelflip3", 47), ("n_heelflip4", 47), ("n_inwardheelflip", 48), ("n_kickflip", 16), ("n_kickflip2", 16),
    ("n_kickflip3", 16), ("n_kickflip4", 16), ("n_laserflip", 43), ("n_popshuvit", 49), ("n_varialheelflip", 50),
    ("n_varialkickflip", 51), ("nollie", 13), ("ollie", 18), ("popshuvit", 21), ("varialheelflip", 39),
    ("varialkickflip", 40),
];

impl Missions {
    /// SkateMission's answers for the S.K.A.T.E. HUD (skate1_online_hud::SkateView).
    pub(crate) fn skate_view(&self, player: &dyn Fn(i64) -> String, trick: &dyn Fn(i64) -> String)
        -> crate::skate1_online_hud::SkateView {
        let mut order: Vec<(i64, i64)> = self.skate.players.iter().map(|(&id, p)| (p.2, id)).collect();
        order.sort();
        let ids: Vec<i64> = order.into_iter().map(|(_, id)| id).collect();
        let active = ids.iter().position(|&id| id == self.skate.current).unwrap_or(0);
        crate::skate1_online_hud::SkateView {
            names: ids.iter().map(|&id| player(id)).collect(),
            active,
            // INFERRED: SetPlayerStatus carries the player's letters.
            letters: self.skate.players.get(&self.skate.current).map_or(0, |p| p.1),
            abds: self.skate.used().into_iter().map(trick).collect(),
            hit: self.skate.hit,
            landed: self.skate.landed.map(trick).unwrap_or_default(),
            // SKATEMission.IsTrickMirrored 0x824624a0 reads hud+0x315, which SetScratchPadPatternItem's 0x82343ac0
            // writes as the set trick's switched XOR the local player's profile +0x7b. +0x7b is regular: its pro
            // writer 0x8271fa38 takes 0x8271f8b0's table (1 for danny_way, jason_dill; 0 for chris_haslam,
            // mike_carroll, paul_rodriguez, as GetProInfo's REGULAR / GOOFY). FlipGesture(true) is trick_gestures'
            // _xscale -100, so the art is drawn for a goofy skater riding goofy, as Skate 2's gesture_item (user 10-15
            // run12: the circle showed the flick the wrong way).
            gesture: self.skate.landed.map(|t| (trick_gesture(t), self.skate.switched != self.facts.player_regular)),
        }
    }

    /// Mission.GetDistanceHUDText's inputs (s1-screens S1): (leader-player distance, max).
    /// RETAIL: the update 0x823f4f98 keeps the last distance at [[0x830fcb18]+92]+616 (stored only while updating);
    /// SetWarningInfo 0x827c22b8 stores info[1] (max) at +624 as an integer. 0x8243b818 answers whether or not it
    /// ran: the nollie lesson (fp_u3_tutorial02) never calls it, and gating on it showed "undefined" (playtest 10-15).
    /// UNTRACED: +624 before any SetWarningInfo. Fallback: -1, which 0x8243b818 shows as ratio 0.
    pub(crate) fn follow_distance(&self) -> Option<(f32, i64)> {
        let max = self.follow.info.get(1).map_or(-1, |&max| max as i64);
        Some((self.follow.distance as f32, max))
    }

    /// The local player sets the trick (SetPlayerOrder(0, 1)).
    pub(crate) fn leader(&self) -> bool {
        self.skate.players.get(&0).is_some_and(|p| p.2 == 1)
    }
}

fn pro_natives(lua: &Lua, shared: &Shared) -> mlua::Result<()> {
    let trick = table(lua, "Trick")?;
    bind(lua, &trick, shared, "ClearTrickTypeMultiplier", |_, ()| ())?; // RETAIL 0x827ab7a8 (research6 3a)
    bind(lua, &trick, shared, "IsDoingTrickType", |m, (_id, t): (LuaValue, i64)| m.facts.doing.contains(&t))?; // 3b
    bind(lua, &trick, shared, "IsLineTimerOn", |m, _: LuaValue| m.facts.line_timer_on)?; // 0x827abcb0
    // RETAIL 0x827abc00 (research6 3c; peer-trick-defs.md): def+12 TrickName of class 0x6918469984A8C596, "" out of
    // range. 134 of the 136 S1 names are IDENTIFIERS' lowercased (S1 grinds are upper case, fs/bs_feeble_willie are
    // absent; no script compares grind names; P-Rod's "n" test holds). The index only round-trips into this native
    // (RETAIL, peer run 2), so the port's IDENTIFIERS id (L6) is a consistent stand-in for the retail slot order
    // (INFERRED). UNTRACED: fs/bs_feeble_willie (S1-only grinds, no IDENTIFIERS id) never reach here; no mission
    // reads a grind's name and the "n" test is Flip/Air only.
    bind(lua, &trick, shared, "GetTrickName", |_, i: i64| {
        usize::try_from(i).ok().and_then(|i| skate_core::scoring::catalog::IDENTIFIERS.get(i)).map_or("", |t| t.0)
    })?;
    let challenge = table(lua, "Challenge")?;
    bind(lua, &challenge, shared, "ClearVisualIndicatorsByType", |m, kind: i64| {
        m.requests.push(Request::ClearIndicators(kind)) // research6 3d
    })?;
    // INFERRED (name; research6 3d: the pros set the next gate with it).
    bind(lua, &challenge, shared, "SetTriggerVisual", |m, (volume, kind): (String, i64)| {
        m.requests.push(Request::Indicator { volume, kind })
    })?;
    Ok(())
}

/// Community Spirit (he_u4_besttrick01) and Danny Way's tutorial (fp_u3_tutorial03), user 10-15.
fn contest_natives(lua: &Lua, shared: &Shared) -> mlua::Result<()> {
    let contest = table(lua, "Contest")?;
    // INFERRED: InitScores clears the contest; the round count follows from SetMaxRounds.
    bind(lua, &contest, shared, "InitScores", |m, _: LuaValue| m.contest = Contest::default())?;
    bind(lua, &contest, shared, "SetMaxRounds", |m, (_this, n): (LuaValue, i64)| m.contest.rounds = n)?;
    bind(lua, &contest, shared, "SetCurrentRound", |m, (_this, n): (LuaValue, i64)| m.contest.round = n)?;
    bind(lua, &contest, shared, "ResetRoundScores", |m, _: LuaValue| {
        (m.contest.scores, m.contest.points) = (BTreeMap::new(), BTreeMap::new())
    })?;
    bind(lua, &contest, shared, "SetCompetitorScore", |m, (_this, i, score): (LuaValue, i64, i64)| {
        m.contest.scores.insert(i, score);
    })?;
    // INFERRED: winning = this round's best score above every other competitor's (a tie is not winning).
    bind(lua, &contest, shared, "CheckIfWinning", |m, (_this, i): (LuaValue, i64)| {
        let mine = m.contest.scores.get(&i).copied().unwrap_or(0);
        m.contest.scores.iter().all(|(&j, &s)| j == i || s < mine)
    })?;
    bind(lua, &contest, shared, "SetCompetitorPoints", |m, (i, points): (i64, i64)| {
        m.contest.points.insert(i, points);
    })?;
    bind(lua, &contest, shared, "SetOverallStanding", |m, (_this, i, round, total, rank): (LuaValue, i64, i64, i64, i64)| {
        m.contest.standings.insert(i, (round, total, rank));
    })?;
    bind(lua, &contest, shared, "SetForfeitTimeLeft", |m, (_this, t): (LuaValue, i64)| {
        m.contest.forfeit = if t >= 0 { t } else { 255 }
    })?;
    // Called tricks only: CalledTricksEnabled is false in the data, so nothing is called. UNTRACED bodies; nothing.
    bind(lua, &contest, shared, "SetCalledTrickData", |_, _: LuaValue| ())?;
    bind(lua, &contest, shared, "CalledTrickDataDone", |_, _: LuaValue| ())?;
    let hud = table(lua, "HUD")?;
    bind(lua, &hud, shared, "UpdateCurrentScore", |m, n: i64| m.contest.current = n)?;
    let ai = table(lua, "AI")?;
    // ponytail: printed only (the script's debug lines); retail names the slot's character. Fallback: "".
    bind(lua, &ai, shared, "GetCharacterName", |_, _: LuaValue| "")?;
    bind(lua, &ai, shared, "GetCharacterModel", |_, _: LuaValue| "")?;
    // UNTRACED: the character's own winning line. Fallback 0, so the script plays AudioAnnouncerNewCAIWinning.
    bind(lua, &ai, shared, "GetCharacterAnnouncerWinningIndex", |_, _: LuaValue| 0)?;
    bind(lua, &ai, shared, "GetCharacterAnnouncerWinningPlayerContext", |_, _: LuaValue| 0)?;
    // UNTRACED: where a wiped-out rider restarts. Fallback: nothing; the rider gets up where he fell.
    bind(lua, &ai, shared, "SetAIBreadCrumbLocation", |_, _: LuaValue| ())?;
    // RETAIL 0x827bbb18: PlayAnnouncerSpeech(n) -> 0x822ab198(speech, 32, n, 0) when n != 0; 0x827bbb60:
    // PlayContextedAnnouncerSpeech(n, context) -> (32, n, context).
    let speech = table(lua, "ChallengeSpeech")?;
    bind(lua, &speech, shared, "PlayAnnouncerSpeech", |m, n: i64| {
        if n != 0 {
            m.say(32, n, 0)
        }
    })?;
    bind(lua, &speech, shared, "PlayContextedAnnouncerSpeech", |m, (n, context): (i64, Option<i64>)| {
        m.say(32, n, context.unwrap_or(0))
    })?;
    // UNTRACED: the contest's music and stingers (no Skate 1 mission audio is exported). Fallback: silence.
    bind(lua, &table(lua, "ChallengeMusic")?, shared, "PlayMusic", |_, _: LuaValue| ())?;
    bind(lua, &table(lua, "ChallengeSFX")?, shared, "PlayChallengeSFX", |_, _: LuaValue| ())?;
    // Streaming focus: the port keeps the whole map loaded.
    bind(lua, &table(lua, "ChallengeAsset")?, shared, "LockStreamFocusOnTrigger", |_, _: LuaValue| ())?;
    let nis = table(lua, "NIS")?;
    bind(lua, &nis, shared, "EnterFlythroughNIS", |m, (scene, _t): (String, LuaValue)| {
        m.requests.push(Request::Nis(Nis::Flythrough(scene)))
    })?;
    // UNTRACED formula; INFERRED from skate1_online.lua's a + rand % (b - a + 1), on GetRandomNumber's draw.
    bind(lua, &table(lua, "Random")?, shared, "GetRandomNumberRange", |m, (a, b): (i64, i64)| a - 1 + m.random((b - a + 1).max(1)))?;
    // The frame base of sequencecomplete's StartTick (skate1_challenge: the replay frame), which the script compares.
    bind(lua, &table(lua, "Time")?, shared, "GetVisualGameTick", |m, ()| m.facts.frame)?;
    // ponytail: fp_u3_tutorial03 stores it in vars.speed and never reads it. Fallback: 0.
    bind(lua, &table(lua, "Skate")?, shared, "GetPlayerSpeed", |_, _: LuaValue| 0.)?;
    Ok(())
}

/// The lead's tables as Lua stand-ins that record calls (in game the lead's real natives run).
#[cfg(test)]
pub(crate) mod harness {
    use super::*;
    use crate::challenge::{Machine, Script};

    pub(crate) const LEAD: &str = r#"
calls, missing = {}, {}
function __missing(name) missing[#missing + 1] = name end
local function rec(name) return function(...) calls[#calls + 1] = name end end
local timer = 0
local function add() timer = timer + 1 return timer end
Challenge = {WinChallenge = rec("WinChallenge"), CompleteChallenge = rec("CompleteChallenge"),
  CheckIfTutorialMode = function() return 1 end, GetLogbookReplayFlag = function() return 0 end,
  SetAllChallengeVisualIndicators = rec("Indicators"), RemoveAllChallengeVisualIndicators = rec("NoIndicators")}
ChallengeTimer = {AddCountdownTimer = add, AddHUDCountdownTimer = add, RemoveCountdownTimer = rec("RemoveTimer"),
  ResetCountdownTimer = rec("ResetTimer"), RemoveHUDCountdownTimer = rec("RemoveHUDTimer")}
HUD = {ChangeHUDState = function(s, sub) calls[#calls + 1] = "HUD " .. tostring(s) .. " " .. tostring(sub) end,
  SetMissionKey = rec("SetMissionKey"),
  SetTutorialObjective = function(i, label, v) calls[#calls + 1] = "Objective " .. i .. " " .. label .. " " .. tostring(v) end,
  eSk8HUDState = {FreeSkate = 1, MissionTutorialChallenge = 19}}
ChallengeSpeech = {PlayCameramanSpeech = function(id) calls[#calls + 1] = "Cameraman " .. id end,
  PlayNPCSpeechUsingAttribute = rec("lead no-op")}
NIS = {eSceneType = {SCENE_TYPE_TUTORIAL_INTRO = 0}, EnterExtendedNIS = rec("ExtendedNIS")}
Trick = {}
ChallengeBoundary = {ResetOOBEffect = rec("ResetOOB"), StartOOBEffect = rec("StartOOB")}
Pause = {PauseSimulation = rec("Pause"), UnpauseSimulation = rec("Unpause"), EnablePauseMenuAndBackpack = rec("EnablePause"),
  DisablePauseMenuAndBackpack = rec("DisablePause")}
AI = {RepositionAI = function(id, at) calls[#calls + 1] = "Player to " .. tostring(at) end,
  PinAI = rec("Pin player"), UnpinAI = rec("Unpin player"), StreamTeleportLocalPlayer = rec("Stream")}
Debug = {DebugTextPermanent = function() return 0 end, DebugTextTimed = rec("DebugTextTimed"), RemoveTextPermanent = rec("RemoveText")}
Online = {GetLocalPlayerID = function() return 0 end}
ChallengeAsset = {ChallengeAccepted = rec("ChallengeAccepted"), IsChallengeAssetLoadComplete = function() return true end,
  ActivateChallengeAssetLoadScreen = rec("ActivateLoad"), DeactivateChallengeAssetLoadScreen = rec("DeactivateLoad")}
for _, name in ipairs({"Challenge", "ChallengeTimer", "HUD", "ChallengeSpeech", "NIS", "Trick", "ChallengeBoundary",
    "Pause", "AI", "Debug", "Online", "ChallengeAsset"}) do
  setmetatable(_G[name], {__index = function(_, key) return function() __missing(name .. "." .. key) end end})
end
"#;

    /// A mission's Machine over P1's output, or None without SKATE3_ASSET_ROOT (the asset tests are #[ignore]).
    pub(crate) fn mission(name: &str) -> Option<(Machine, Shared)> {
        let root = std::path::PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").ok()?).join("private/challenges-skate1");
        let data: Value = serde_json::from_str(&std::fs::read_to_string(root.join("tutorial.json")).ok()?).ok()?;
        let mission = data["missions"].get(name).or_else(|| data["pro_tutorials"].get(name))?.clone();
        let source = std::fs::read_to_string(root.join(format!("scripts/{name}.lua"))).ok()?;
        let fields = fields(&mission["self"]);
        let shared: Shared = Arc::new(Mutex::new(Missions::new(&fields)));
        let s = shared.clone();
        let script = Script { name, source: &source, steps: steps(&mission), initial: "initial", enter: "stateenter",
                              exit: "stateexit" };
        let machine = Machine::new(script, &fields, |lua| {
            lua.load(LEAD).exec()?;
            register(lua, &s)
        })
        .unwrap();
        machine.vars().set("kActivityID", 0).unwrap(); // lead edit L1
        Some((machine, shared))
    }

    fn list(machine: &Machine, name: &str) -> Vec<String> {
        let t: Table = machine.lua.globals().get(name).unwrap();
        t.sequence_values::<String>().map(Result::unwrap).collect()
    }

    /// The recorded lead calls.
    pub(crate) fn calls(machine: &Machine) -> Vec<String> {
        list(machine, "calls")
    }

    /// Natives the script reached that nobody registered.
    pub(crate) fn missing(machine: &Machine) -> Vec<String> {
        list(machine, "missing")
    }

    /// Hands this frame's posted events to the machine.
    pub(crate) fn tick(machine: &mut Machine, shared: &Shared, facts: &Facts) {
        let out = shared.lock().unwrap().tick(facts);
        for (event, message) in out {
            machine.send(&event, &message).unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lua_with(shared: &Shared) -> Lua {
        let lua = Lua::new();
        lua.load(harness::LEAD).exec().unwrap();
        lua.globals().set("ChangeState", lua.create_function(|_, _: String| Ok(())).unwrap()).unwrap();
        register(&lua, shared).unwrap();
        lua
    }
    fn shared(fields: Value) -> Shared {
        Arc::new(Mutex::new(Missions::new(&super::fields(&fields))))
    }
    /// A mission whose HUD state already built the rows.
    fn built(fields: Value) -> Shared {
        let s = shared(fields);
        s.lock().unwrap().rebuild = true;
        s.lock().unwrap().tick(&Facts::default());
        s
    }
    fn filming_101() -> Value {
        json!({"ChallengeType": 7, "PointRequirement": 500, "RequiredTrickType": ["00000005"],
               "Hash_769F1AAE200A496C": [2], "FilmChallengeType": 0, "InANoSkateZoneObjective": false})
    }

    #[test]
    fn fields_turn_hex_trick_types_into_integers_and_fill_absent_arrays() {
        let f = fields(&json!({"RequiredTrickType": ["00000002", "00000004"], "Name": "x"}));
        assert_eq!(f["RequiredTrickType"], json!([2, 4]));
        for key in ["GeneralTrickObjective", "SpecificMeasurementObjective", "TotalMeasurementObjective",
                    "BestTrickSpectatorWaypoint"] {
            assert_eq!(f[key], json!([]), "{key}");
        }
    }

    #[test]
    fn steps_drop_null_handlers() {
        let s = steps(&json!({"steps": {"a": {"stateenter": null, "hudyes": "h_1"}}}));
        assert_eq!(s["a"].len(), 1);
        assert_eq!(s["a"]["hudyes"], "h_1");
    }

    #[test]
    fn register_fills_retail_enums_and_keeps_existing_keys() {
        let lua = lua_with(&shared(json!({})));
        let get = |src: &str| lua.load(src).eval::<Option<i64>>().unwrap();
        assert_eq!(get("return HUD.eSk8HUDFilmState.UpdateObjectivesForReal"), Some(10));
        assert_eq!(get("return HUD.eSk8HUDState.MissionFollowMeChallenge"), Some(67));
        assert_eq!(get("return HUD.eSk8HUDFollowMeState.Accept"), None); // RETAIL: absent
        assert_eq!(get("return Objectives.eObjective.FollowDestination"), Some(21));
        assert_eq!(get("return NIS.eNISChallengeNotification.NIS_FE_NOTIFY_ENTER_PHOTOVIEW"), Some(2));
        assert_eq!(get("return NIS.eSceneType.SCENE_TYPE_TUTORIAL_INTRO"), Some(0)); // the lead's stays
        assert_eq!(get("return Broadcasters.Ports.PresThread"), Some(0));
    }

    #[test]
    fn an_unknown_native_reports_missing_and_does_nothing() {
        let lua = lua_with(&shared(json!({})));
        lua.load("Skate.NoSuchNative(1)").exec().unwrap();
        assert_eq!(lua.load("return missing[1]").eval::<String>().unwrap(), "Skate.NoSuchNative");
    }

    #[test]
    fn rows_follow_the_builder_order_per_mission() {
        let kinds = |f: Value| {
            rows(&fields(&f)).iter().map(|r| (r.kind, r.trick, r.target, r.checkbox, r.label.clone())).collect::<Vec<_>>()
        };
        // Filming 101: Points 500, Grind x2.
        assert_eq!(kinds(filming_101()), [(1, 0, 500, false, String::new()), (2, 5, 2, false, String::new())]);
        // Rolling Rehab: Flip, Grab, FilmTime.
        let rehab = json!({"ChallengeType": 7, "PointRequirement": 0, "RequiredTrickType": ["00000002", "00000004"],
                           "Hash_769F1AAE200A496C": [1, 1], "Hash_3774A91CE72FE9BB": true});
        assert_eq!(kinds(rehab), [(2, 2, 1, false, String::new()), (2, 4, 1, false, String::new()),
                                  (19, 0, 0, false, "ID_CHALLENGE_FILM_HE_U1_FILM01_OBJ".into())]);
        // My First Photo: PhotoCustom, Points 100, Grind (target 0 -> 1, checkbox).
        let photo = json!({"ChallengeType": 6, "BreakBonesCount": 0, "PhotoCustomObjective": "ID_OBJECTIVE_EITHERRAIL",
                           "PointRequirement": 100, "RequiredTrickType": ["00000005"], "Hash_769F1AAE200A496C": [0]});
        assert_eq!(kinds(photo), [(27, 0, 0, false, "ID_OBJECTIVE_EITHERRAIL".into()), (1, 0, 100, false, String::new()),
                                  (2, 5, 1, true, String::new())]);
        // Follow Shingo: FollowDistance 60, FollowDestination.
        let follow = json!({"ChallengeType": 23, "FollowTheLeaderFailureDistance": 60,
                            "Hash_09BC21052E21CB32": "ID_OBJECTIVE_STAY_WITHIN",
                            "Hash_B1175B0D6C12C680": "ID_OBJECTIVE_GET_TO_SCHOOL"});
        assert_eq!(kinds(follow), [(20, 0, 60, false, "ID_OBJECTIVE_STAY_WITHIN".into()),
                                   (21, 0, 0, false, "ID_OBJECTIVE_GET_TO_SCHOOL".into())]);
        // S.K.A.T.E.: none.
        assert!(kinds(json!({"ChallengeType": 4})).is_empty());
    }

    #[test]
    fn done_rule_per_kind() {
        let row = |kind, current, target, d| Row { kind, current, target, done: d, ..Row::default() };
        assert!(done(&row(1, 500, 500, false), 7));
        assert!(!done(&row(1, 500, 500, false), 9)); // OwnTheSpot: strictly more
        assert!(done(&row(2, 2, 2, false), 7));
        assert!(!done(&row(19, 0, 0, false), 7));
        assert!(done(&row(19, 0, 0, true), 7));
        assert!(!done(&row(29, 9, 0, true), 7));
    }

    #[test]
    fn objective_natives_drive_filming_101() {
        let s = built(filming_101());
        let lua = lua_with(&s);
        let eval = |src: &str| lua.load(src).eval::<LuaValue>().unwrap();
        assert_eq!(eval("return Objectives.GetTotalNumObjectives()").as_i64(), Some(2));
        lua.load("HUD.IncrementRequiredTrickTypeNumDone(7, Trick.eTrickType.Grind)").exec().unwrap();
        lua.load("HUD.IncrementRequiredTrickTypeNumDone(7, Trick.eTrickType.Grind)").exec().unwrap();
        lua.load("HUD.IncrementRequiredTrickTypeNumDone(7, Trick.eTrickType.Flip)").exec().unwrap(); // no Flip row
        assert_eq!(eval("return Objectives.GetCurrentNumCompletedObjectives()").as_i64(), Some(1));
        assert_eq!(eval("return Objectives.GetStatusAllObjectives()").as_boolean(), Some(false));
        lua.load("ChallengeManager.UpdateBackEndScore(650)").exec().unwrap();
        assert_eq!(eval("return Objectives.GetStatusAllObjectives()").as_boolean(), Some(true));
        lua.load("ChallengeManager.TellBackEndChallengeWon() ChallengeManager.UpdateBackEndTotalMissionTime(30)").exec().unwrap();
        let m = s.lock().unwrap();
        assert!(m.holder.backend_won);
        assert_eq!(m.holder.time, 30.);
    }

    #[test]
    fn set_objective_status_marks_the_first_row_of_its_kind() {
        let s = built(json!({"ChallengeType": 23, "FollowTheLeaderFailureDistance": 60}));
        let lua = lua_with(&s);
        lua.load("Objectives.SetObjectiveComplete(Objectives.eObjective.FollowDistance)").exec().unwrap();
        lua.load("Objectives.SetObjectiveStatus(Objectives.eObjective.FollowDestination, false)").exec().unwrap();
        let m = s.lock().unwrap();
        assert_eq!(m.holder.rows.iter().map(|r| r.done).collect::<Vec<_>>(), [true, false]);
    }

    #[test]
    fn the_hud_states_rebuild_the_rows_on_the_next_frame() {
        let rows_after = |fields: Value, src: &str| {
            let s = shared(fields);
            lua_with(&s).load(src).exec().unwrap();
            let mut m = s.lock().unwrap();
            let before = m.holder.rows.len();
            m.tick(&Facts::default());
            (before, m.holder.rows.len())
        };
        let film = "HUD.ChangeHUDState(HUD.eSk8HUDState.MissionFilmChallenge, HUD.eSk8HUDFilmState.Countdown)";
        assert_eq!(rows_after(filming_101(), film), (0, 2));
        let record = "HUD.ChangeHUDState(HUD.eSk8HUDState.MissionFilmChallenge, HUD.eSk8HUDFilmState.Record)";
        assert_eq!(rows_after(filming_101(), record), (0, 0));
        let photo = json!({"ChallengeType": 6, "PointRequirement": 100});
        let active = "HUD.ChangeHUDState(HUD.eSk8HUDState.MissionPhotoChallenge, HUD.eSk8HUDPhotoState.Active)";
        assert_eq!(rows_after(photo, active), (0, 1));
        let follow = json!({"ChallengeType": 23, "FollowTheLeaderFailureDistance": 60});
        let countdown = "HUD.ChangeHUDState(HUD.eSk8HUDState.MissionFollowMeChallenge, HUD.eSk8HUDFollowMeState.Countdown)";
        assert_eq!(rows_after(follow, countdown), (0, 2));
        // A retry's Countdown starts the counts again, and the lead's ChangeHUDState still runs.
        let s = built(filming_101());
        let lua = lua_with(&s);
        lua.load(format!("ChallengeManager.UpdateBackEndScore(650) {film}")).exec().unwrap();
        s.lock().unwrap().tick(&Facts::default());
        assert_eq!(s.lock().unwrap().holder.rows[0].current, 0);
        assert_eq!(lua.load("return calls[#calls]").eval::<String>().unwrap(), "HUD 15 8");
    }

    #[test]
    fn film_ignores_set_win_state_and_follow_reports_it() {
        let film = shared(filming_101());
        lua_with(&film).load("HUD.SetWinState(true)").exec().unwrap();
        let mut m = film.lock().unwrap();
        assert!(!m.won() && m.drain().is_empty());
        let follow = shared(json!({"ChallengeType": 23}));
        lua_with(&follow).load("HUD.SetWinState(true)").exec().unwrap();
        let mut m = follow.lock().unwrap();
        assert!(m.won());
        assert_eq!(m.drain(), [Request::WinState(true)]);
    }

    /// U14: imperial = "{0:d} ft" x3, metric = "{0:d} m" x1.
    #[test]
    fn units_follow_the_units_setting() {
        assert_eq!(units(true), ("413c56da", 3));
        assert_eq!(units(false), ("92099686", 1));
    }

    #[test]
    fn objective_text_matches_the_hud_builders() {
        let strings: BTreeMap<&str, &str> = [
            ("ID_OBJECTIVE_TITLE_GET_POINTS", "Get {0:d} Points"), ("ID_OBJECTIVE_TITLE_TRICK_GRIND", "Do {0:d} Grind"),
            ("ID_OBJECTIVE_TITLE_TRICK_GRINDS", "Do {0:d} Grinds"), ("ID_COMMON_NUMBER_OVER_NUMBER", "{0:d}/{1:d}"),
            ("ID_OBJECTIVE_STAY_WITHIN", "Stay within {0:s}"), ("413c56da", "{0:d} ft"), ("92099686", "{0:d} m"),
            ("ID_OBJECTIVE_GET_TO_SCHOOL", "Get to the school"), ("ID_OBJECTIVE_EITHERRAIL", "Hit up the stairs"),
        ]
        .into();
        let text = |k: &str| strings.get(k).copied().unwrap_or("").to_owned();
        let s = built(filming_101());
        s.lock().unwrap().holder.rows[1].current = 1;
        assert_eq!(objectives(&s.lock().unwrap(), &text), [
            ("Get 500 Points".to_owned(), "Get 500 Points".to_owned(), false, false),
            ("Do 2 Grinds".to_owned(), "1/2".to_owned(), false, false),
        ]);
        let follow = built(json!({"ChallengeType": 23, "FollowTheLeaderFailureDistance": 60,
            "Hash_09BC21052E21CB32": "ID_OBJECTIVE_STAY_WITHIN", "Hash_B1175B0D6C12C680": "ID_OBJECTIVE_GET_TO_SCHOOL"}));
        assert_eq!(objectives(&follow.lock().unwrap(), &text), [
            // Whatever Settings > Units holds (a global; this test doesn't write it): 60 m = 180 ft.
            (if crate::hud_runtime::imperial() { "Stay within 180 ft" } else { "Stay within 60 m" }.to_owned(),
             String::new(), false, false),
            ("Get to the school".to_owned(), String::new(), false, false),
        ]);
        let photo = built(json!({"ChallengeType": 6, "PhotoCustomObjective": "ID_OBJECTIVE_EITHERRAIL",
            "PointRequirement": 100, "RequiredTrickType": ["00000005"], "Hash_769F1AAE200A496C": [0]}));
        let photo = objectives(&photo.lock().unwrap(), &text);
        assert_eq!(photo[2], ("Do 1 Grind".to_owned(), String::new(), false, true));
        assert_eq!(photo[0].0, "Hit up the stairs");
    }

    #[test]
    fn speech_attributes_split_speaker_line_and_context() {
        let s = shared(json!({"ChallengeType": 6}));
        let lua = lua_with(&s);
        lua.load("ChallengeSpeech.PlayNPCSpeechUsingAttribute({29, 6619136, Count = 2})
                  ChallengeSpeech.PlayContextSpeechUsingAttribute({31, 65601536, 268435456, Count = 3})
                  ChallengeSpeech.PlayNPCSpeech(24, 308)
                  ChallengeSpeech.PlayFollowMeWarningSpeech(31, 1401, 1)").exec().unwrap();
        assert_eq!(s.lock().unwrap().drain(), [
            Request::Speech { speaker: 29, line: 101, context: 0 },
            Request::Speech { speaker: 31, line: 1001, context: 268435456 },
            Request::Speech { speaker: 24, line: 308, context: 0 },
            Request::Speech { speaker: 31, line: 1401, context: 1 },
        ]);
    }

    #[test]
    fn the_cameraman_answer_arrives_on_the_next_tick() {
        let s = shared(json!({"ChallengeType": 7}));
        lua_with(&s).load("ChallengeSpeech.AskIfCameramanIsSpeaking()").exec().unwrap();
        let mut m = s.lock().unwrap();
        let facts = Facts { cameraman_playing: true, ..Facts::default() };
        assert_eq!(m.tick(&facts), [("cameramanspeaking".to_owned(), json!({"IsPlaying": true}))]);
        assert!(m.tick(&facts).is_empty());
    }

    #[test]
    fn the_session_marker_returns_the_player_where_it_was_set() {
        let s = shared(json!({"ChallengeType": 7}));
        let lua = lua_with(&s);
        let mut at = [[0.; 4]; 4];
        at[3] = [1., 2., 3., 1.];
        s.lock().unwrap().tick(&Facts { player: at, ..Facts::default() });
        lua.load("Film.SetSessionMarker(0)").exec().unwrap();
        s.lock().unwrap().tick(&Facts::default());
        lua.load("Film.ReturnPlayerToSessionMarker(0)").exec().unwrap();
        assert_eq!(s.lock().unwrap().drain(), [Request::PlayerTo(at)]);
    }

    #[test]
    fn countup_timers_count_seconds() {
        let s = shared(json!({}));
        let lua = lua_with(&s);
        let id: i64 = lua.load("return ChallengeTimer.AddCountupTimer()").eval().unwrap();
        s.lock().unwrap().tick(&Facts { dt: 1.5, ..Facts::default() });
        let t: f64 = lua.load(format!("return ChallengeTimer.GetCountupTime({id})")).eval().unwrap();
        assert_eq!(t, 1.5);
        lua.load(format!("ChallengeTimer.RemoveCountupTimer({id})")).exec().unwrap();
        assert_eq!(lua.load(format!("return ChallengeTimer.GetCountupTime({id})")).eval::<f64>().unwrap(), 0.);
    }

    #[test]
    #[ignore = "needs SKATE3_ASSET_ROOT (P1 output)"]
    fn filming_101_runs_to_a_win() {
        let (mut m, s) = harness::mission("he_u3_film01").expect("SKATE3_ASSET_ROOT");
        m.start().unwrap();
        m.send("hudloaded", &Value::Null).unwrap();
        assert_eq!(m.state(), "challenge_setup");
        harness::tick(&mut m, &s, &Facts { frame: 1234, ..Facts::default() });
        let prefilm: i64 = m.vars().get("PrefilmTimerId").unwrap();
        m.send("timerexpired", &json!({"TimerID": prefilm})).unwrap();
        assert_eq!(m.state(), "filming");
        for _ in 0..2 {
            m.send("grindcompleted", &json!({"PlayerID": 0, "Wipedout": 0})).unwrap();
        }
        assert_eq!(m.state(), "filming", "the 500 points are still missing");
        m.send("pointsawarded", &json!({"PlayerID": 0, "Score": 650})).unwrap();
        assert_eq!(m.state(), "postfilm_display");
        assert!(s.lock().unwrap().holder.backend_won);
        let wait: i64 = m.vars().get("ReplayWaitID").unwrap();
        m.send("timerexpired", &json!({"TimerID": wait})).unwrap();
        assert!(s.lock().unwrap().drain().contains(&Request::FilmReplay(1234)));
        m.send("hudcontinue", &Value::Null).unwrap();
        assert_eq!(m.state(), "complete");
        assert!(s.lock().unwrap().drain().contains(&Request::RealTime));
        let calls = harness::calls(&m);
        assert!(calls.contains(&"WinChallenge".to_owned()) && calls.contains(&"CompleteChallenge".to_owned()));
        assert_eq!(harness::missing(&m), Vec::<String>::new());
    }

    #[test]
    #[ignore = "needs SKATE3_ASSET_ROOT (P1 output)"]
    fn rolling_rehab_wins_when_the_film_time_runs_out_after_a_flip_and_a_grab() {
        let (mut m, s) = harness::mission("he_u1_film01").expect("SKATE3_ASSET_ROOT");
        m.start().unwrap();
        m.send("hudloaded", &Value::Null).unwrap();
        assert_eq!(m.state(), "play_intro");
        harness::tick(&mut m, &s, &Facts::default()); // cameramanspeaking{IsPlaying = false}
        assert_eq!(m.state(), "challenge_setup");
        harness::tick(&mut m, &s, &Facts::default()); // the prefilm countdown's frames: prefilm_display's rebuild lands
        let prefilm: i64 = m.vars().get("PrefilmTimerId").unwrap();
        m.send("timerexpired", &json!({"TimerID": prefilm})).unwrap();
        m.send("trickcomplete", &json!({"PlayerID": 0, "TrickType": 2})).unwrap();
        m.send("trickcomplete", &json!({"PlayerID": 0, "TrickType": 4})).unwrap();
        assert_eq!(m.state(), "filming", "FilmTime is only done when the clock runs out");
        let film: i64 = m.vars().get("FilmTimerID").unwrap();
        m.send("timerexpired", &json!({"TimerID": film})).unwrap();
        assert_eq!(m.state(), "postfilm_display");
        assert!(s.lock().unwrap().holder.backend_won);
        assert_eq!(harness::missing(&m), Vec::<String>::new());
    }

    #[test]
    #[ignore = "needs SKATE3_ASSET_ROOT (P1 output)"]
    fn filming_101_retry_starts_the_counts_again() {
        let (mut m, s) = harness::mission("he_u3_film01").expect("SKATE3_ASSET_ROOT");
        m.start().unwrap();
        m.send("hudloaded", &Value::Null).unwrap();
        harness::tick(&mut m, &s, &Facts::default());
        let prefilm: i64 = m.vars().get("PrefilmTimerId").unwrap();
        m.send("timerexpired", &json!({"TimerID": prefilm})).unwrap();
        m.send("grindcompleted", &json!({"PlayerID": 0, "Wipedout": 0})).unwrap();
        assert_eq!(s.lock().unwrap().holder.rows[1].current, 1);
        let film: i64 = m.vars().get("FilmTimerID").unwrap();
        m.send("timerexpired", &json!({"TimerID": film})).unwrap(); // Regular film: timer out = lose
        assert_eq!(m.state(), "postfilm_display");
        m.send("hudretry", &Value::Null).unwrap();
        assert!(s.lock().unwrap().drain().iter().any(|r| matches!(r, Request::PlayerTo(_))));
        let restart: i64 = m.vars().get("SmallDelayTimerID").unwrap(); // he_u3_film01 [restart]
        m.send("timerexpired", &json!({"TimerID": restart})).unwrap();
        harness::tick(&mut m, &s, &Facts::default());
        assert_eq!(s.lock().unwrap().holder.rows[1].current, 0);
    }

    #[test]
    fn warning_rows_unpack_speech_ids_and_crossings_post_squared_distances() {
        let s = shared(json!({"ChallengeType": 23}));
        let lua = lua_with(&s);
        lua.load("FollowTheLeader.SetWarningInfo({25, 6, 31, 91815936, 1, 24, 20185088, 1, 12, Count = 9})
                  FollowTheLeader.SetActorIDs(1)
                  far = FollowTheLeader.RegisterDistanceCallback(1, FollowTheLeader.GetMinDistance())
                  FollowTheLeader.StartDistanceUpdates()").exec().unwrap();
        let get = |n: &str| lua.load(format!("return FollowTheLeader.{n}()")).eval::<f64>().unwrap();
        assert_eq!((get("GetWarningSpeechID"), get("GetRecoverySpeechID"), get("GetMaxDistance")), (1401., 308., 6.));
        let facts = |x: f32| Facts { positions: [(0, [0., 0., 0.]), (1, [x, 0., 0.])].into(), ..Facts::default() };
        let mut m = s.lock().unwrap();
        assert!(m.tick(&facts(10.)).is_empty());
        assert_eq!(m.tick(&facts(30.)), [("playerradiusexit".to_owned(), json!({"PlayerID": 1, "Distance": 625.0}))]);
        assert_eq!(m.tick(&facts(20.)), [("playerradiusenter".to_owned(), json!({"PlayerID": 1, "Distance": 625.0}))]);
    }

    /// s1-pros M1: the distance meter's (distance, max); before SetWarningInfo (the nollie lesson never calls it), max -1.
    #[test]
    fn follow_distance_answers_without_the_warning_info() {
        let s = shared(json!({"ChallengeType": 23}));
        let lua = lua_with(&s);
        lua.load("FollowTheLeader.SetActorIDs(1) FollowTheLeader.StartDistanceUpdates()").exec().unwrap();
        let facts = Facts { positions: [(0, [0.; 3]), (1, [3., 4., 0.])].into(), ..Facts::default() };
        s.lock().unwrap().tick(&facts);
        assert_eq!(s.lock().unwrap().follow_distance(), Some((5., -1)));
        lua.load("FollowTheLeader.SetWarningInfo({25, 6, 31, 91815936, 1, 24, 20185088, 1, 12, Count = 9})").exec().unwrap();
        assert_eq!(s.lock().unwrap().follow_distance(), Some((5., 6)));
    }

    #[test]
    fn reset_distance_zeroes_it() {
        let s = shared(json!({"ChallengeType": 23}));
        let lua = lua_with(&s);
        lua.load("FollowTheLeader.SetActorIDs(1) FollowTheLeader.StartDistanceUpdates()").exec().unwrap();
        s.lock().unwrap().tick(&Facts { positions: [(0, [0.; 3]), (1, [3., 4., 0.])].into(), ..Facts::default() });
        assert_eq!(lua.load("return FollowTheLeader.GetDistance()").eval::<f64>().unwrap(), 5.);
        lua.load("FollowTheLeader.ResetDistance()").exec().unwrap();
        assert_eq!(lua.load("return FollowTheLeader.GetDistance()").eval::<f64>().unwrap(), 0.);
    }

    /// User 10-15 (Rolling Rehab): the film countdown holds the skater until Record.
    #[test]
    fn a_film_countdown_holds_the_skater_until_record() {
        let s = shared(json!({}));
        let lua = lua_with(&s);
        lua.load("local f = HUD.eSk8HUDState.MissionFilmChallenge \
            HUD.ChangeHUDState(f, HUD.eSk8HUDFilmState.Countdown) calls[#calls + 1] = 'countdown' \
            HUD.ChangeHUDState(f, HUD.eSk8HUDFilmState.Record)").exec().unwrap();
        let calls: Vec<String> = lua.load("return calls").eval::<Table>().unwrap().sequence_values().map(Result::unwrap).collect();
        let pins: Vec<_> = calls.iter().filter(|c| c.contains("pin player") || c.contains("Pin player") || *c == "countdown").collect();
        assert_eq!(pins, ["Pin player", "countdown", "Unpin player"]);
    }

    #[test]
    fn player_ids_stay_with_the_lead() {
        let s = shared(json!({}));
        let lua = lua_with(&s);
        lua.load("AI.RepositionAI(0, 'start') AI.PinAI(0) AI.RepositionAI(1, 'waiting') AI.PinAI(1) AI.UnpinAI(1)").exec().unwrap();
        assert_eq!(s.lock().unwrap().drain(), [
            Request::Ai(1, AiCall::Reposition("waiting".into(), 1)),
            Request::Ai(1, AiCall::Pin(true)),
            Request::Ai(1, AiCall::Pin(false)),
        ]);
        let calls: Vec<String> = lua.load("return calls").eval::<Table>().unwrap().sequence_values().map(Result::unwrap).collect();
        assert_eq!(calls, ["Player to start", "Pin player"]);
    }

    #[test]
    fn a_spawned_skater_exists_at_once_until_unspawned() {
        let s = shared(json!({}));
        let lua = lua_with(&s);
        let exists = |id: i64| lua.load(format!("return Entity.DoesSkaterExist({id})")).eval::<bool>().unwrap();
        assert!(exists(0) && !exists(1));
        lua.load("AI.SpawnAIAtLocation(1, 'he_u1_skate01_waiting', 1)").exec().unwrap();
        assert!(exists(1));
        lua.load("AI.UnspawnAI(1)").exec().unwrap();
        assert!(!exists(1));
    }

    #[test]
    fn stance_abd_paths_and_nis_requests() {
        let s = shared(json!({"RoundPaths": ["aa", "bb"]}));
        let lua = lua_with(&s);
        s.lock().unwrap().tick(&Facts { abd: [(1, 35)].into(), player_regular: false, ..Facts::default() });
        lua.load("AI.SpawnSkaterRegular(1, false)").exec().unwrap();
        let get = |src: &str| lua.load(src).eval::<LuaValue>().unwrap();
        assert_eq!(get("return AI.GetIfSkaterIsRegularStance(1)").as_boolean(), Some(false));
        assert_eq!(get("return AI.GetIfSkaterIsRegularStance(0)").as_boolean(), Some(false));
        assert_eq!(get("return AI.GetIfSkaterIsRegularStance(2)").as_boolean(), Some(true));
        assert_eq!(get("return AI.GetCharacterPercentageWillDoABDTrick(1)").as_i64(), Some(35));
        lua.load("AI.FilterReset() AI.FilterPaths({}, 'bb') AI.StartPath(1) AI.StartPath(1, false) AI.CAIBackToAmbient({})
                  NIS.EnterIntroOfferNIS('NIS_ROP_2001_shingo_S01_0', 5) NIS.EnterReplayNIS('he_u2_photo01', 10, 90)
                  NIS.KillNIS() Transition.EndTransition()").exec().unwrap();
        let r = s.lock().unwrap().drain();
        assert_eq!(r[0], Request::Ai(1, AiCall::Regular(false)));
        assert_eq!(&r[1..], [
            Request::FilterReset, Request::FilterPaths(1),
            Request::Ai(1, AiCall::StartPath { branching: true, avoid: vec![] }),
            Request::Ai(1, AiCall::StartPath { branching: false, avoid: vec![] }),
            Request::Nis(Nis::Offer("NIS_ROP_2001_shingo_S01_0".into())),
            Request::Nis(Nis::Replay { scene: "he_u2_photo01".into(), from: 10, to: 90 }),
            Request::Nis(Nis::Kill), Request::EndTransition,
        ]);
    }

    /// Review 10-15 minor 5: the accept NIS's transition loads only for 3 / 4; anything else is ignored, not an error.
    #[test]
    fn accept_nis_ignores_an_unexpected_transition() {
        let s = shared(json!({}));
        let lua = lua_with(&s);
        lua.load("NIS.EnterIntroAcceptNIS('a', 4) NIS.EnterIntroAcceptNIS('b', 3.0) NIS.EnterIntroAcceptNIS('c', {})
                  NIS.EnterIntroAcceptNIS('d', 'x') NIS.EnterIntroAcceptNIS('e')").exec().unwrap();
        assert_eq!(s.lock().unwrap().drain(), [
            Request::Nis(Nis::Accept("a".into(), true)), Request::Nis(Nis::Accept("b".into(), true)),
            Request::Nis(Nis::Accept("c".into(), false)), Request::Nis(Nis::Accept("d".into(), false)),
            Request::Nis(Nis::Accept("e".into(), false)),
        ]);
    }

    #[test]
    fn random_numbers_stay_in_one_to_n() {
        let mut m = Missions::new(&json!({}));
        m.rng = 0x9E3779B97F4A7C15;
        let draws: Vec<i64> = (0..1000).map(|_| m.random(4)).collect();
        assert!(draws.iter().all(|d| (1..=4).contains(d)));
        assert!((1..=4).all(|n| draws.contains(&n)));
    }

    // Task 5's run-through; it also needs Task 6's AI natives (spawn, DoesSkaterExist, EndTransition).
    #[test]
    #[ignore = "needs SKATE3_ASSET_ROOT (P1 output)"]
    fn follow_shingo_runs_to_the_school() {
        let (mut m, s) = harness::mission("he_u2_follow01").expect("SKATE3_ASSET_ROOT");
        m.start().unwrap();
        m.send("hudyes", &Value::Null).unwrap();
        assert_eq!(m.state(), "leader_ready");
        let requests = s.lock().unwrap().drain();
        assert!(requests.contains(&Request::Ai(1, AiCall::Spawn("he_u2_follow01_shingo_locator".into()))));
        assert!(requests.contains(&Request::EndTransition));
        let small: i64 = m.vars().get("SmallDelayTimerID").unwrap();
        m.send("timerexpired", &json!({"TimerID": small})).unwrap();
        let countdown: i64 = m.vars().get("CountdownTimerID").unwrap();
        m.send("timerexpired", &json!({"TimerID": countdown})).unwrap();
        assert_eq!(m.state(), "racing");
        let facts = Facts { positions: [(0, [0.; 3]), (1, [30., 0., 0.])].into(), ..Facts::default() };
        harness::tick(&mut m, &s, &facts);
        assert_eq!(m.state(), "racing", "30 m is a warning, not the 60 m failure");
        assert_eq!(s.lock().unwrap().holder.rows.len(), 2, "built by the FollowMe Countdown");
        m.send("triggerentercollision", &json!({"VolumeID": "he_u2_follow01_destination", "PlayerID": 0})).unwrap();
        assert_eq!(m.state(), "success");
        assert!(s.lock().unwrap().won());
        assert!(s.lock().unwrap().holder.rows.iter().all(|r| r.done));
    }

    #[test]
    fn boundary_state_and_marker_reset_follow_the_photo_rules() {
        let s = shared(json!({"ChallengeType": 6}));
        let lua = lua_with(&s);
        lua.load("Challenge.UpdateBoundaryState({}, 1)").exec().unwrap();
        let mut m = s.lock().unwrap();
        // RETAIL 0x823d58b0 / 0x823d59f0: setting the marker snapshots the live boundary; using it posts the snapshot.
        m.tick(&Facts { marker_sets: 1, ..Facts::default() });
        m.holder.boundary = Some(2);
        assert_eq!(m.tick(&Facts { marker_sets: 1, marker_uses: 1, ..Facts::default() }),
                   [("markerreset".to_owned(), json!({"PlayerID": 0, "ChallengeType": 6, "MarkerState": 1}))]);
        drop(m);
        // The script's order (he_u2_photo01 l.96-97): the override zeroes the state, then the marker re-snapshots it.
        lua.load("Challenge.ResetSessionOverride({}) Challenge.ResetChallengeSessionMarker(0)").exec().unwrap();
        let mut m = s.lock().unwrap();
        assert_eq!(m.holder.boundary, Some(0));
        assert_eq!(m.tick(&Facts { marker_sets: 1, marker_uses: 2, ..Facts::default() })[0].1["MarkerState"], 0);
        drop(m);
        // ResetBoundaryState clears the holder id: a snapshot taken now has nothing to post (0x823d59f0 needs id > 0).
        lua.load("Challenge.ResetBoundaryState({}) Challenge.ResetChallengeSessionMarker(0)").exec().unwrap();
        assert!(s.lock().unwrap().tick(&Facts { marker_sets: 1, marker_uses: 3, ..Facts::default() }).is_empty());
    }

    #[test]
    fn photo_reads_come_from_facts() {
        let s = shared(json!({"ChallengeType": 6}));
        let lua = lua_with(&s);
        let mut types = [0; 12];
        types[5] = 2;
        s.lock().unwrap().tick(&Facts { in_sequence: true, in_air_or_grinding: true, bones: 3, sequence_types: types,
                                         ..Facts::default() });
        let get = |src: &str| lua.load(src).eval::<LuaValue>().unwrap();
        assert_eq!(get("return Trick.IsInSequence(0)").as_boolean(), Some(true));
        assert_eq!(get("return Trick.IsSkaterInAirOrGrinding(0)").as_boolean(), Some(true));
        assert_eq!(get("return Trick.GetNumTrickType(0, Trick.eTrickType.Grind)").as_i64(), Some(2));
        assert_eq!(get("return Trick.GetFEBonesBrokenCount()").as_f64(), Some(3.));
        lua.load("LivingWorld.StartTakingPhotos({}, 0) Photo.TriggerSessionMarker101() Trigger.ResetTriggerVolumeState('v')
                  Trick.DisableWipeoutReset() HUD.PreparePhotoFailState(2)").exec().unwrap();
        let mut m = s.lock().unwrap();
        assert_eq!(m.drain(), [Request::Photographing(true), Request::Skate101(14), Request::ResetVolume("v".into()),
                               Request::DisableWipeoutReset]);
        assert_eq!(m.holder.fail, 2);
    }

    #[test]
    #[ignore = "needs SKATE3_ASSET_ROOT (P1 output)"]
    fn my_first_photo_runs_to_the_photo_view() {
        use crate::challenge::Machine;
        let (mut m, s) = harness::mission("he_u2_photo01").expect("SKATE3_ASSET_ROOT");
        m.start().unwrap();
        m.send("hudloaded", &Value::Null).unwrap();
        m.send("hudyes", &Value::Null).unwrap();
        assert_eq!(m.state(), "wait_to_enter_challenge_area");
        assert!(s.lock().unwrap().drain().contains(&Request::Skate101(14)));
        let enter = |m: &mut Machine, v: &str| m.send("triggerentercollision", &json!({"VolumeID": v, "PlayerID": 0})).unwrap();
        enter(&mut m, "he_u2_photo01_scoreboundary_01");
        assert_eq!(m.state(), "wait_to_enter_trick_area");
        enter(&mut m, "he_u2_photo01_spotvolume_01");
        assert_eq!(m.state(), "photographing");
        s.lock().unwrap().tick(&Facts { frame: 200, ..Facts::default() });
        m.send("grindcompleted", &json!({"PlayerID": 0, "Wipedout": 0})).unwrap();
        m.send("sequencecomplete", &json!({"PlayerID": 0, "SequenceScore": 150, "StartTick": 100, "Wipedout": false})).unwrap();
        assert!(s.lock().unwrap().won());
        let success: i64 = m.vars().get("SuccessTimerID").unwrap();
        m.send("timerexpired", &json!({"TimerID": success})).unwrap();
        assert_eq!(m.state(), "postphoto_display");
        let delay: i64 = m.vars().get("ReplayDelayTimerID").unwrap();
        m.send("timerexpired", &json!({"TimerID": delay})).unwrap();
        assert!(s.lock().unwrap().drain().contains(&Request::Nis(Nis::Replay { scene: "he_u2_photo01".into(), from: 100, to: 200 })));
        m.send("nischallengenotify", &json!({"NotificationID": 2})).unwrap();
        m.send("hudcontinue", &Value::Null).unwrap();
        assert_eq!(m.state(), "complete");
    }

    fn kickflip() -> i64 {
        skate_core::scoring::catalog::IDENTIFIERS.iter().position(|i| i.0 == "kickflip").unwrap() as i64
    }

    #[test]
    fn a_repeated_trick_with_the_same_spins_is_refused() {
        let s = shared(json!({"ChallengeType": 4}));
        let lua = lua_with(&s);
        lua.load("Skate.ResetSkateTrackers() Skate.PrepareForExactTrick()").exec().unwrap();
        let k = kickflip();
        s.lock().unwrap().tick(&Facts { airs: [(1, Air { scorable: k, spins: 0, ..Air::default() })].into(), ..Facts::default() });
        let set = |lua: &Lua| lua.load(format!("return Trick.SetPatternLastTrick({k}, 1)")).eval::<bool>().unwrap();
        assert!(set(&lua));
        assert!(!set(&lua), "already been done");
        s.lock().unwrap().tick(&Facts { airs: [(1, Air { scorable: k, spins: 2, ..Air::default() })].into(), ..Facts::default() });
        assert!(set(&lua), "a 360 kickflip is another trick");
    }

    #[test]
    fn the_scratchpad_compares_the_trick_and_the_last_spins() {
        let s = shared(json!({"ChallengeType": 4}));
        let lua = lua_with(&s);
        let k = kickflip();
        lua.load(format!("Skate.SetScratchPadPatternItem(1, {k}, 0, false) Skate.SetScratchPadPatternIndex(2)")).exec().unwrap();
        let cmp = |t: i64, spins: i64| {
            lua.load(format!("return Skate.CompareScratchPadPatternItem(1, {t}, {spins}, false)")).eval::<bool>().unwrap()
        };
        assert!(cmp(k, 0));
        assert!(!cmp(k, 1));
        assert!(!cmp(k + 1, 0));
    }

    #[test]
    fn matched_pattern_checks_the_followers_landing() {
        let s = shared(json!({"ChallengeType": 4}));
        let lua = lua_with(&s);
        let k = kickflip();
        s.lock().unwrap().tick(&Facts { airs: [(1, Air { scorable: k, ..Air::default() })].into(), ..Facts::default() });
        lua.load(format!("Skate.ResetSkateTrackers() Trick.SetPatternLastTrick({k}, 1)")).exec().unwrap();
        s.lock().unwrap().tick(&Facts { airs: [(0, Air { scorable: k, ..Air::default() })].into(), ..Facts::default() });
        assert!(lua.load("return Skate.MatchedPattern(0, 0)").eval::<bool>().unwrap());
        s.lock().unwrap().tick(&Facts { airs: [(0, Air { scorable: k, spins: 1, ..Air::default() })].into(), ..Facts::default() });
        assert!(!lua.load("return Skate.MatchedPattern(0, 0)").eval::<bool>().unwrap());
        assert!(!lua.load("return Skate.MatchedPattern(0, 4)").eval::<bool>().unwrap());
    }

    #[test]
    fn start_skate_path_avoids_the_used_tricks_when_asked() {
        let s = shared(json!({"ChallengeType": 4}));
        let lua = lua_with(&s);
        let k = kickflip();
        s.lock().unwrap().tick(&Facts { airs: [(1, Air { scorable: k, ..Air::default() })].into(), ..Facts::default() });
        lua.load(format!("Skate.ResetSkateTrackers() Trick.SetPatternLastTrick({k}, 1)
                  Skate.StartSkatePath(1, false, true) Skate.StartSkatePath(1, false, false)
                  AI.FollowTrickPattern(1, 0) Skate.TriggerSkate101()")).exec().unwrap();
        assert_eq!(s.lock().unwrap().drain(), [
            Request::Ai(1, AiCall::StartPath { branching: false, avoid: vec![k] }),
            Request::Ai(1, AiCall::StartPath { branching: false, avoid: vec![] }),
            Request::Ai(1, AiCall::FollowTrickPattern { trick: k, spins: 0 }),
            Request::Skate101(15),
        ]);
    }

    #[test]
    fn scorable_lookups_use_the_catalog() {
        let s = shared(json!({}));
        let lua = lua_with(&s);
        let k = kickflip();
        let get = |src: String| lua.load(src).eval::<i64>().unwrap();
        assert_eq!(get(format!("return Trick.GetTrickTypeFromScorableID({k})")), 2); // Flip
        assert_eq!(get(format!("return Trick.GetTrickIndexFromScorableID({k})")), k);
        assert_eq!(get("return Trick.GetTrickIndexFromScorableID(139)".into()), -1);
        s.lock().unwrap().tick(&Facts { airs: [(1, Air { spins: -3, switched: true, ..Air::default() })].into(), ..Facts::default() });
        assert_eq!(get("return Trick.GetCompletedAir180Count(1)".into()), 3);
        assert!(lua.load("return Trick.DidCompletedAirStartSwitched(1)").eval::<bool>().unwrap());
    }

    #[test]
    #[ignore = "needs SKATE3_ASSET_ROOT (P1 output)"]
    fn skate_reaches_the_first_leader_turn_and_shingo_sets_a_trick() {
        let (mut m, s) = harness::mission("he_u1_skate01").expect("SKATE3_ASSET_ROOT");
        m.start().unwrap();
        assert!(s.lock().unwrap().drain().contains(&Request::Nis(Nis::Offer("NIS_ROP_2001_shingo_S01_0".into()))));
        m.send("nischallengenotify", &json!({"NotificationID": 1})).unwrap();
        m.send("hudloaded", &Value::Null).unwrap();
        m.send("hudyes", &Value::Null).unwrap();
        assert_eq!(m.state(), "new_accepted");
        m.send("nischallengenotify", &json!({"NotificationID": 1})).unwrap();
        assert_eq!(m.state(), "npc_leader_attempt", "check_ai must not spin");
        let r = s.lock().unwrap().drain();
        assert!(r.contains(&Request::Ai(1, AiCall::Spawn("he_u1_skate01_waiting".into()))));
        assert!(r.contains(&Request::FilterPaths(0)));
        assert!(r.contains(&Request::Ai(1, AiCall::Reposition("he_u1_skate01_spawn".into(), 0))), "arg 3 is the slot");
        let start: i64 = m.vars().get("TimerID").unwrap();
        m.send("timerexpired", &json!({"TimerID": start})).unwrap();
        assert!(s.lock().unwrap().drain().iter().any(|r| matches!(r, Request::Ai(1, AiCall::StartPath { .. }))));
        let k = kickflip();
        s.lock().unwrap().tick(&Facts { airs: [(1, Air { scorable: k, ..Air::default() })].into(), ..Facts::default() });
        m.send("aircompleted", &json!({"PlayerID": 1, "FirstScorableId": k, "Wipedout": 0})).unwrap();
        assert_eq!(m.vars().get::<bool>("AttemptSuccessful").unwrap(), true);
        let view = s.lock().unwrap().skate_view(&|id| format!("p{id}"), &|t| format!("t{t}"));
        assert_eq!(view.landed, format!("t{k}"));
        assert_eq!(view.gesture, Some(("mckickflip2", false)), "a goofy player's kickflip is drawn as is");
        s.lock().unwrap().tick(&Facts { player_regular: true, ..Facts::default() });
        let view = s.lock().unwrap().skate_view(&|id| format!("p{id}"), &|t| format!("t{t}"));
        assert_eq!(view.gesture, Some(("mckickflip2", true)), "a regular player's is mirrored");
    }

    /// Playtest run10: a bail Shingo's rider posts as wipeout, then the landing's aircompleted (skate1_rider), loses
    /// him the lead: the script's `Wipedout ~= true` passes the number, so only the wipeout's lockout catches it.
    #[test]
    #[ignore = "needs SKATE3_ASSET_ROOT (P1 output)"]
    fn a_shingo_bail_hands_the_lead_on() {
        let (mut m, s) = harness::mission("he_u1_skate01").expect("SKATE3_ASSET_ROOT");
        m.start().unwrap();
        for (event, message) in [("nischallengenotify", json!({"NotificationID": 1})), ("hudloaded", Value::Null), ("hudyes", Value::Null), ("nischallengenotify", json!({"NotificationID": 1}))] {
            m.send(event, &message).unwrap();
        }
        let start: i64 = m.vars().get("TimerID").unwrap();
        m.send("timerexpired", &json!({"TimerID": start})).unwrap();
        let k = kickflip();
        s.lock().unwrap().tick(&Facts { airs: [(1, Air { scorable: k, ..Air::default() })].into(), ..Facts::default() });
        m.send("wipeout", &json!({"PlayerID": 1})).unwrap();
        m.send("aircompleted", &json!({"PlayerID": 1, "FirstScorableId": k, "Wipedout": 1})).unwrap();
        assert_eq!(m.vars().get::<bool>("AttemptSuccessful").unwrap(), false);
        let delay: i64 = m.vars().get("DelayTimerID").unwrap();
        m.send("timerexpired", &json!({"TimerID": delay})).unwrap();
        assert_eq!(m.state(), "npc_leader_failure");
    }

    /// Every name in the vault table is a catalog trick (a typo would silently draw mcnone); others draw mcnone.
    #[test]
    fn the_set_tricks_gestures_name_catalog_tricks() {
        let index = |n: &str| skate_core::scoring::catalog::IDENTIFIERS.iter().position(|i| i.0 == n).unwrap() as i64;
        for (name, gesture) in CONTROLLER_GESTURES {
            assert_eq!(trick_gesture(index(name)), crate::skate1_screens::gesture_name(gesture), "{name}");
        }
        let names = ["kickflip2", "lateheelflip", "n_360hardflip", "christ_air", "fspowerslide"];
        assert_eq!(names.map(|n| trick_gesture(index(n))), ["mckickflip2", "mcheelflip", "mcnollie360hardflip", "mc360christair", "mcnone"]);
        assert_eq!(trick_gesture(-1), "mcnone");
    }

    #[test]
    fn pro_natives_read_the_skater_and_route_indicators() {
        let s = shared(json!({"ChallengeType": 28}));
        let lua = lua_with(&s);
        s.lock().unwrap().tick(&Facts { doing: vec![8], line_timer_on: true, ..Facts::default() });
        let get = |src: &str| lua.load(src).eval::<LuaValue>().unwrap();
        assert_eq!(get("return Trick.IsDoingTrickType(0, Trick.eTrickType.Manual)").as_boolean(), Some(true));
        assert_eq!(get("return Trick.IsDoingTrickType(0, Trick.eTrickType.Grind)").as_boolean(), Some(false));
        assert_eq!(get("return Trick.IsLineTimerOn(0)").as_boolean(), Some(true));
        assert_eq!(get("return Trick.GetTrickName(0)").to_string().unwrap(), "fspowerslide");
        assert_eq!(get("return Trick.GetTrickName(100000)").to_string().unwrap(), "");
        lua.load("Trick.ClearTrickTypeMultiplier()
                  Challenge.ClearVisualIndicatorsByType(Challenge.eChallengeIndicators.SpotNextGates)
                  Challenge.SetTriggerVisual('gate_1', Challenge.eChallengeIndicators.SpotFinish)
                  __s1m.imperial = function() return true end
                  HUD.SetNollieTutorialObjective(1, 30)
                  __s1m.imperial = function() return false end
                  HUD.SetNollieTutorialObjective(1, 30)").exec().unwrap();
        assert_eq!(s.lock().unwrap().drain(), [Request::ClearIndicators(8), Request::Indicator { volume: "gate_1".into(), kind: 10 }]);
        let calls: Vec<String> = lua.load("return calls").eval::<Table>().unwrap().sequence_values().map(Result::unwrap).collect();
        // U14: imperial = _FEET x3, metric = the plain label in metres.
        assert_eq!(calls, ["Objective 1 ID_PRO_TUTORIAL_NOLLIE_OBJ1_FEET 90", "Objective 1 ID_PRO_TUTORIAL_NOLLIE_OBJ1 30"]);
    }

    #[test]
    #[ignore = "needs SKATE3_ASSET_ROOT (P1 output)"]
    fn every_pro_tutorial_and_mission_builds_and_starts() {
        for name in ["fp_u3_tutorial01", "fp_u3_tutorial02", "fp_u3_tutorial04", "fp_u3_tutorial05", "he_u1_film01",
                     "he_u3_film01", "he_u2_follow01", "he_u2_photo01", "he_u1_skate01"] {
            let (mut m, _) = harness::mission(name).expect("SKATE3_ASSET_ROOT");
            m.start().unwrap_or_else(|e| panic!("{name}: {e}"));
            let missing = harness::missing(&m);
            assert!(missing.is_empty(), "{name}: {missing:?}");
        }
    }
}
