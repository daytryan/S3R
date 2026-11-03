//! Skate 2's Death Race on New_San_Vanelona, on the match core (docs/superpowers/specs/2026-10-02-skate2-online-modes-plan.md).
//! Data: skate2port/old_death_race.py -> private/skate2-online/death-race/ (death_race.json; the intro scenes'
//! tutorial.json + scene/; indicator/ textures).
//!
//! Retail runs every race from one Lua template (the r_*.lua run_heat handlers). Its rules are ported here ([`Run`])
//! for the local racer rather than run:
//! - Split triggers count in order. A leg's last split ends the leg once all of them were hit in order; the last
//!   leg's ends the race (score = race time in ms).
//! - Downhill gates (race type 0) add their Time_Bonus to the racer's countdown the first time through. Trick-for-time
//!   gates (type 1) add floor(sequence points / RaceTrickForTimeMultiplier) for a trick sequence done in the gate.
//! - A RaceGateSkipable race shows every gate of the leg and the finish at once. The others light the next gate,
//!   grey the one after it, and must be taken in order.
//! - The countdown running out puts the racer out (DNF).
//! - A racer who finished or is out is pinned (AI.PinAI) until the race ends for everyone; then end_heat (3 s) and
//!   cleanup_heat (2 s: "Challenge Complete!" when placed within PlaceHigherToWin, else "Challenge Failed", the pause
//!   menu off) come before the results ([`Match::outro`]).
//!
//! Checked against the disc's template Lua (r_1_1_fabio_mem1_ol, decompiled to scratch: setup_heat 344-547,
//! cleanup_heat 549-607, countdown 609-707, run_heat 709-1524, end_heat 1717-1853, highlight_reel 2031-2044). Online
//! rows have no KnockoutPerLeg, NoPushing, WipeOutLimit or RaceTerminateVolumes (old_death_race.py asserts the first
//! three; none has the last), so those rules are not ported for them.
//!
//! Gates are the racegate outline (an arch) over the trigger volume, and the course chevrons (VisualIndicators) sit at
//! their locators; both are drawn as skate2_arrows draws indicators. Gates make no sound: their fe sounds are null
//! samples on the disc.
//!
//! setup_heat fades to black (0.5 s), puts each racer on its start slot at black, then plays its online NIS and fades
//! back in (0.5 s) at the NIS's end. The NIS is Online_RaceStarts_0N (N = players, 2..8; 07/08 are not on the disc,
//! so 06 stands in), placed by the heat's NIS definition (5 s). Then hudcountdown runs 3-2-1-Go, silent because
//! challenge type 0x13 has no countdown audio, and the TimerHUD starts.
//!
//! Test solo: `SKATE_MODE=death_race:r_2_1_dr_ur_01_ol` (any race id in death_race.json).
use crate::{
    apt_screen::{SKATE2_ONLINE, Screens},
    challenge::cutscene::{Cutscene, ground},
    config::Config,
    map_transition::CurrentMap,
    match_core::{Match, Mode, Phase, Playable},
    physics::{GamePhysics, SkaterRuntime},
    skate2_contests::leaderboard_answers,
    skate2_screens::{Central, TutorialScreens, View, countdown_text, timer_text},
    skate3_modes::ChallengeFade,
};
use bevy::prelude::*;
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    f32::consts::{FRAC_PI_2, PI},
    path::{Path, PathBuf},
};

const DIR: &str = "private/skate2-online/death-race";
const MAP: &str = "New_San_Vanelona";
/// A racer out of the race: it shows as 99:59.99, after every finisher. ponytail: the core has no DNF flag yet.
const DNF: i64 = 5_999_990;
/// eRaceType.TrickForTime.
const TRICK_FOR_TIME: u8 = 1;
/// Trick time has no ceiling, so the core's end of the run allows this much more.
const TRICK_TIME_CAP: f32 = 300.;
/// "Go!" stays up this long into the run.
const GO_FOR: f32 = 1.;
/// setup_heat's FadeOut and FadeIn (Effects.StartEffect 0.5) and its fadeout timer (0.5): the teleport is at black.
const FADE: f32 = 0.5;
/// setup_heat's TeleportDoneTimerID: every racer teleported, this long, then the NIS.
const TELEPORT_DONE: f32 = 0.01;
/// end_heat's EndRoundTimerID and cleanup_heat's CompletionMessageID.
const END_HEAT: f32 = 3.;
const CLEANUP_HEAT: f32 = 2.;
/// HUD2.ShowCentralMessage's time on screen (the tutorial's CENTRAL_SECONDS).
const CENTRAL_FOR: f32 = 3.;
/// The racegate outline (asset 0001AE2703E38707) spans x -5.07..4.97 m.
const ARCH_WIDTH: f32 = 10.04;
/// LeaderboardHUD on Skate 2's online HUD layer.
const LEADERBOARD: &str = "1-leaderboard";
/// The minimap icons of the race natives (S2 0x822B8404..): AddRaceGate (0x826DB5D8) without an icon of its own,
/// AddRaceCheckPoint (0x826DB6E0) and AddRaceFinish (0x826DB7B0).
const GATE_ICON: &str = "mini_gate";
const CHECKPOINT_ICON: &str = "mini_spotracegate";
const FINISH_ICON: &str = "mini_gate_final";

#[derive(Deserialize)]
struct Data {
    shapes: HashMap<String, Vec<[f32; 2]>>,
    styles: HashMap<String, Style>,
    strings: HashMap<String, String>,
    races: HashMap<String, Race>,
}
#[derive(Deserialize)]
struct Style {
    texture: String,
    background: Option<String>,
    tint: [f32; 4],
}
#[derive(Deserialize)]
struct Race {
    /// A string id of `Data::strings`, or '#' text.
    #[serde(default)]
    title: String,
    online: bool,
    race_type: u8,
    skipable: bool,
    trick_multiplier: f32,
    end_camera: Option<String>,
    /// PlaceHigherToWin, CentralMessageHALIDChallengeComplete / Failed. ponytail: an older death_race.json lacks them;
    /// every race's are 1, ID_CHALLENGE_CENTRAL_MESSAGE_CHALLENGECOMPLETE and ID_MISSION_CHALLENGE_FAILED.
    #[serde(default)]
    place_to_win: Option<usize>,
    #[serde(default)]
    complete: Option<String>,
    #[serde(default)]
    failed: Option<String>,
    /// RaceGateGreyIcon: the grey gate's minimap icon (mini_gate_next on every online race).
    #[serde(default)]
    grey_icon: Option<String>,
    ribbons: Ribbons,
    chevrons: Vec<Chevron>,
    heat: Heat,
    locators: HashMap<String, [[f32; 4]; 4]>,
    volumes: BTreeMap<String, Aabb>,
    intro: Vec<Intro>,
}
#[derive(Deserialize)]
struct Ribbons {
    grey: Look,
    #[serde(rename = "final")]
    last: Look,
    leg_finish: Look,
}
/// A ribbon instance: the outline (indicator type) and the style.
#[derive(Deserialize)]
struct Look {
    ribbon: String,
    style: String,
}
#[derive(Deserialize)]
struct Chevron {
    locator: String,
    #[serde(rename = "type")]
    shape: String,
    style: String,
}
#[derive(Deserialize)]
struct Heat {
    time_limit: f32,
    start: String,
    /// The start locator's per-racer children (world matrices).
    #[serde(default)]
    slots: Vec<[[f32; 4]; 4]>,
    legs: Vec<Leg>,
    terminate: Vec<Option<String>>,
}
#[derive(Deserialize)]
struct Leg {
    gates: Vec<Gate>,
    splits: Vec<Option<String>>,
    end_bonus: f32,
}
#[derive(Deserialize)]
struct Gate {
    volume: Option<String>,
    bonus: f32,
    ribbon: String,
    style: String,
}
#[derive(Deserialize, Clone, Copy)]
struct Aabb {
    min: [f32; 3],
    max: [f32; 3],
}
impl Aabb {
    fn contains(&self, p: Vec3) -> bool {
        (0..3).all(|k| self.min[k] <= p[k] && p[k] <= self.max[k])
    }
    fn centre(&self) -> Vec3 {
        (Vec3::from(self.min) + Vec3::from(self.max)) / 2.
    }
}
#[derive(Deserialize)]
struct Intro {
    scene: String,
    seconds: f32,
    tracks: usize,
}

impl Race {
    /// A leg's finish: its last split trigger.
    fn finish(&self, leg: usize) -> Option<&str> {
        self.heat.legs[leg].splits.last()?.as_deref()
    }
    fn start(&self) -> Option<[[f32; 4]; 4]> {
        self.locators.get(&self.heat.start).copied()
    }
    /// The heat teleport's spot for a racer: AI.RepositionAI(id, StartLocation, id), resolved as
    /// online_map::locator_slot.
    fn spawn(&self, slot: usize) -> Option<[[f32; 4]; 4]> {
        Some(crate::online_map::locator_slot(&self.start()?, &self.heat.slots, slot))
    }
    /// The core's end of the run: the time limit plus every bonus the race can give.
    fn cap(&self) -> Option<f32> {
        let bonuses: f32 = self.heat.legs.iter().map(|l| l.end_bonus + l.gates.iter().map(|g| g.bonus).sum::<f32>()).sum();
        let tricks = if self.race_type == TRICK_FOR_TIME { TRICK_TIME_CAP } else { 0. };
        (self.heat.time_limit > 0.).then_some(self.heat.time_limit + bonuses + tricks)
    }
    /// Its start and every split trigger are placed. (A few gates and chevrons are missing on the disc itself; a race
    /// runs without them, as retail does.)
    fn playable(&self) -> bool {
        let placed = |s: &Option<String>| s.as_ref().is_some_and(|s| self.volumes.contains_key(s));
        self.start().is_some() && !self.heat.legs.is_empty() && self.heat.legs.iter().all(|l| !l.splits.is_empty() && l.splits.iter().all(placed))
    }
    /// Where the course comes into `volume` from: the gate or finish before it, else the start.
    fn approach(&self, volume: &str) -> Vec3 {
        let mut from = self.start().map_or(Vec3::ZERO, |m| Vec3::from_slice(&m[3][..3]));
        for (k, leg) in self.heat.legs.iter().enumerate() {
            for v in leg.gates.iter().map(|g| g.volume.as_deref()).chain([self.finish(k)]).flatten() {
                if v == volume {
                    return from;
                }
                if let Some(b) = self.volumes.get(v) {
                    from = b.centre();
                }
            }
        }
        from
    }
}

/// What a rule step asks of the racer's clock and HUD.
#[derive(Debug, PartialEq)]
enum Note {
    /// Time added, with ID_RACE_SECONDS_ADDED.
    Seconds(f32),
    /// A leg's EndTimeBonus, added quietly.
    LegBonus(f32),
    Finished,
    /// A RaceTerminateVolume: out of the race.
    Out,
}

/// One racer's run: the race template's run_heat handlers (triggerentercollision, triggerexitcollision,
/// sequencecomplete) and the indicators the local racer sees.
#[derive(Default)]
struct Run {
    leg: usize,
    /// LastLapGateIndex: split triggers hit in order this leg.
    split: usize,
    /// HeatGateFlags: a gate is used up (or, in a gate-order race, not reachable yet).
    used: Vec<bool>,
    /// InTrickSpot / TrickSpotTick (real seconds): trick-for-time gates the racer is in, and when it last entered an
    /// unused one or left one.
    in_spot: Vec<bool>,
    spot_at: Vec<Option<f32>>,
    /// The trigger indicators up: volume -> (outline, style).
    shown: BTreeMap<String, (String, String)>,
    finish_shown: bool,
    /// The minimap's race objects the Lua adds beside the indicators: each gate's (MapIconGatesID: volume -> icon) and
    /// the leg finish's (MapIconFinishID: volume, icon), apart, as a finish volume can be a gate's too.
    gate_icons: BTreeMap<String, String>,
    finish_icon: Option<(String, &'static str)>,
    finished: bool,
    /// `shown` changed since the drawing last looked.
    changed: bool,
}
impl Run {
    fn new(race: &Race) -> Self {
        let mut run = Self::default();
        run.begin_leg(race, 0);
        run
    }

    fn show(&mut self, volume: Option<&str>, ribbon: &str, style: &str) {
        if let Some(v) = volume {
            self.shown.insert(v.to_owned(), (ribbon.to_owned(), style.to_owned()));
            self.changed = true;
        }
    }
    /// A gate's indicator down, and its minimap object with it (Map.RemoveRaceObject(MapIconGatesID[i])).
    fn hide(&mut self, volume: Option<&str>) {
        if let Some(v) = volume {
            self.changed |= self.shown.remove(v).is_some();
            self.gate_icons.remove(v);
        }
    }
    fn gate_icon(&mut self, gate: &Gate, icon: &str) {
        if let Some(v) = &gate.volume {
            self.gate_icons.insert(v.clone(), icon.to_owned());
        }
    }
    /// A gate lit: its own ribbon, and Map.AddRaceGate(volume) on the minimap.
    fn show_lit(&mut self, gate: &Gate) {
        self.show(gate.volume.as_deref(), &gate.ribbon, &gate.style);
        self.gate_icon(gate, GATE_ICON);
    }
    /// The gate after the next: RaceGateGreyRibbon, and AddRaceGate(volume, RaceGateGreyIcon) (nil: AddRaceGate's own).
    fn show_grey(&mut self, race: &Race, gate: &Gate) {
        self.show(gate.volume.as_deref(), &race.ribbons.grey.ribbon, &race.ribbons.grey.style);
        self.gate_icon(gate, race.grey_icon.as_deref().unwrap_or(GATE_ICON));
    }
    /// The leg's finish: RaceFinalGateRibbon on the last leg, else RaceLegFinishGateRibbon; on the minimap AddRaceFinish
    /// on the last leg, else AddRaceCheckPoint, and always AddRaceCheckPoint from the countdown (r_1_1 677).
    fn show_finish(&mut self, race: &Race, countdown: bool) {
        let last = self.leg + 1 == race.heat.legs.len();
        let look = if last { &race.ribbons.last } else { &race.ribbons.leg_finish };
        self.show(race.finish(self.leg), &look.ribbon, &look.style);
        self.finish_shown = true;
        let icon = if last && !countdown { FINISH_ICON } else { CHECKPOINT_ICON };
        self.finish_icon = race.finish(self.leg).map(|v| (v.to_owned(), icon));
    }

    /// Countdown stateenter and each new leg: the gates armed and the leg's indicators up.
    fn begin_leg(&mut self, race: &Race, leg: usize) {
        let gates = &race.heat.legs[leg].gates;
        (self.leg, self.split, self.finish_shown, self.changed) = (leg, 0, false, true);
        self.used = (0..gates.len()).map(|j| !race.skipable && j > 0).collect();
        self.in_spot = vec![false; gates.len()];
        self.spot_at = vec![None; gates.len()];
        self.shown.clear();
        (self.gate_icons, self.finish_icon) = (BTreeMap::new(), None);
        for (i, gate) in gates.iter().enumerate() {
            match (race.skipable, i) {
                (true, _) | (false, 0) => self.show_lit(gate),
                (false, 1) => self.show_grey(race, gate),
                _ => {}
            }
        }
        if race.skipable || gates.len() < 2 {
            // Leg 0 is begun only by the countdown (Run::new).
            self.show_finish(race, leg == 0);
        }
    }

    /// A trick-for-time gate used in a gate-order race: the next one lit, the one after it grey, and the finish
    /// once the last gates are reached.
    fn next_trick_gate(&mut self, race: &Race, i: usize) {
        let gates = &race.heat.legs[self.leg].gates;
        if let Some(g) = gates.get(i + 1) {
            self.show_lit(g);
        }
        if let Some(g) = gates.get(i + 2) {
            self.show_grey(race, g);
        }
        // Lua (1-based): i > Count - 2.
        if i + 3 > gates.len() && !self.finish_shown {
            self.show_finish(race, false);
        }
    }
    fn use_trick_gate(&mut self, race: &Race, i: usize) {
        self.used[i] = true;
        if !race.skipable && i + 1 < self.used.len() {
            self.used[i + 1] = false;
        }
    }

    /// triggerentercollision.
    fn enter(&mut self, race: &Race, volume: &str, now: f32) -> Vec<Note> {
        let mut notes = Vec::new();
        if self.finished {
            return notes;
        }
        let splits = &race.heat.legs[self.leg].splits;
        for (i, split) in splits.iter().enumerate() {
            if split.as_deref() == Some(volume) && i == self.split {
                self.split = i + 1;
            }
        }
        if race.finish(self.leg) == Some(volume) && self.split == splits.len() {
            self.split = 0;
            if self.leg + 1 == race.heat.legs.len() {
                self.finished = true;
                return vec![Note::Finished];
            }
            let bonus = race.heat.legs[self.leg].end_bonus;
            if race.heat.time_limit > 0. && bonus > 0. {
                notes.push(Note::LegBonus(bonus));
            }
            self.begin_leg(race, self.leg + 1);
        }
        let gates = &race.heat.legs[self.leg].gates;
        for (i, gate) in gates.iter().enumerate() {
            if gate.volume.as_deref() != Some(volume) || self.used[i] {
                continue;
            }
            if race.race_type == TRICK_FOR_TIME {
                // ponytail: SetManualsAreConnectors(true) (manuals link tricks inside the gate) isn't ported.
                (self.spot_at[i], self.in_spot[i]) = (Some(now), true);
                continue;
            }
            if race.heat.time_limit > 0. && gate.bonus > 0. {
                notes.push(Note::Seconds(gate.bonus));
                self.hide(gate.volume.as_deref());
                // A gate-order downhill race lights the next gates here but, as retail, never re-arms them (no online
                // race is one).
                if !race.skipable {
                    if i + 2 < gates.len() {
                        // ponytail: retail's icon here is RaceGateNormalIcon (not exported), taken as AddRaceGate's own.
                        self.show_lit(&gates[i + 1]);
                    }
                    if i + 3 < gates.len() {
                        self.show_grey(race, &gates[i + 2]);
                    }
                }
            }
            self.used[i] = true;
        }
        if race.heat.terminate.iter().any(|t| t.as_deref() == Some(volume)) {
            self.finished = true;
            notes.push(Note::Out);
        }
        notes
    }

    /// triggerexitcollision: leaving a trick-for-time gate without a trick going uses it up.
    fn exit(&mut self, race: &Race, volume: &str, now: f32, tricking: bool) {
        if self.finished || race.race_type != TRICK_FOR_TIME {
            return;
        }
        let gates = &race.heat.legs[self.leg].gates;
        for (i, gate) in gates.iter().enumerate() {
            if gate.volume.as_deref() != Some(volume) {
                continue;
            }
            self.spot_at[i] = Some(now);
            if tricking {
                continue;
            }
            self.in_spot[i] = false;
            if self.used[i] {
                continue;
            }
            self.hide(gate.volume.as_deref());
            if !race.skipable {
                self.next_trick_gate(race, i);
            }
            self.use_trick_gate(race, i);
        }
    }

    /// sequencecomplete: a landed trick sequence that `started` (real seconds) pays time in the trick-for-time gates
    /// it was done in.
    fn sequence(&mut self, race: &Race, points: f32, started: f32) -> Vec<Note> {
        let mut notes = Vec::new();
        if self.finished || race.race_type != TRICK_FOR_TIME {
            return notes;
        }
        let gates = &race.heat.legs[self.leg].gates;
        for (i, gate) in gates.iter().enumerate() {
            if !(self.in_spot[i] || self.spot_at[i].is_some_and(|t| t >= started)) {
                continue;
            }
            if !race.skipable && self.used[i] {
                break;
            }
            let add = if points > 0. && race.trick_multiplier > 0. { (points / race.trick_multiplier).floor() } else { 0. };
            notes.push(Note::Seconds(add));
            self.hide(gate.volume.as_deref());
            if !race.skipable {
                self.next_trick_gate(race, i);
            }
            self.use_trick_gate(race, i);
            (self.spot_at[i], self.in_spot[i]) = (None, false);
        }
        notes
    }
}

/// The minimap's race objects for `run` (icon, world x, z), each at its volume's centre: the gates', then the leg's
/// finish. ponytail: the map's own draw order isn't traced.
fn race_objects(race: &Race, run: &Run) -> Vec<(String, [f32; 2])> {
    let at = |v: &str| race.volumes.get(v).map(|b| [b.centre().x, b.centre().z]);
    let gates = run.gate_icons.iter().filter_map(|(v, icon)| Some((icon.clone(), at(v.as_str())?)));
    let finish = run.finish_icon.as_ref().and_then(|(v, icon)| Some(((*icon).to_owned(), at(v.as_str())?)));
    gates.chain(finish).collect()
}

/// setup_heat's OnlineNISs pick (1-based clamp(players - 1, 1, count)) and the track for this slot: the scene key in
/// tutorial.json (a slot past the scene's tracks gets the camera alone) and its length.
fn intro_key(row: &str, intro: &[Intro], players: usize, slot: usize) -> Option<(String, f32)> {
    if intro.is_empty() {
        return None;
    }
    let pick = &intro[players.saturating_sub(1).clamp(1, intro.len()) - 1];
    let key = if slot < pick.tracks { format!("{row}|{}|{slot}", pick.scene) } else { format!("{row}|{}", pick.scene) };
    Some((key, pick.seconds))
}

/// The racegate outline over a gate's trigger volume: on the ground under its centre, across the way the course comes
/// in, as wide as the volume. ponytail: retail's volumes are axis-aligned boxes and the exe's fit of the outline to one
/// (the spline indicator manager, 0x8267b480) isn't traced; this spans the side of the box that faces the travel.
fn arch(b: &Aabb, from: Vec3, floor: f32) -> Transform {
    let (width, yaw) = across(Vec3::from(b.min), Vec3::from(b.max), from);
    Transform::from_translation(b.centre().with_y(floor))
        .with_rotation(Quat::from_rotation_y(yaw))
        .with_scale(Vec3::new((width / ARCH_WIDTH).clamp(0.3, 4.), 1., 1.))
}

/// A box's side that faces the way the course comes in (`from` to its centre): its width and the yaw that turns +X
/// along it (Skate 2's arch; Skate 1's race gate model).
pub(crate) fn across(min: Vec3, max: Vec3, from: Vec3) -> (f32, f32) {
    let (size, travel) = (max - min, (min + max) / 2. - from);
    if travel.x.abs() > travel.z.abs() { (size.z, FRAC_PI_2.copysign(travel.x)) } else { (size.x, if travel.z < 0. { PI } else { 0. }) }
}

fn load(root: &Path) -> Option<Data> {
    let path = root.join("death_race.json");
    let text = std::fs::read_to_string(&path).map_err(|e| warn!("Death Race: {}: {e}", path.display())).ok()?;
    serde_json::from_str(&text).map_err(|e| warn!("Death Race: {}: {e}", path.display())).ok()
}

fn skater_position(skater: &SkaterRuntime) -> Vec3 {
    let m = &skater.animated_skeleton.roots.animation_to_world;
    Vec3::new(m[3][0], m[3][1], m[3][2])
}

/// The local racer's race, from setup_heat (the match's Intro) to its end.
struct Active {
    row: String,
    since: f32,
    /// When the start teleport was made (or given up).
    placed: Option<f32>,
    intro_started: bool,
    /// While the intro scene plays: when it ends.
    intro: Option<f32>,
    /// setup_heat's FadeIn: the NIS's notify (its end), or the countdown without one.
    faded_in: Option<f32>,
    run: Option<Run>,
    running: bool,
    /// The racer's countdown (HeatTimerID) runs out at this time.
    deadline: Option<f32>,
    inside: BTreeSet<String>,
    /// A trick sequence is open, and when it opened.
    open: bool,
    opened: f32,
    /// The central message, its presentation and until when it's up.
    central: Option<(String, Central, f32)>,
    out: bool,
    /// Results: the gates are down.
    over: bool,
}

#[derive(Resource, Default)]
struct DeathRace {
    data: Option<Data>,
    root: PathBuf,
    active: Option<Active>,
    /// Bumped whenever what's drawn in the world changes.
    visual: u64,
    screens: TutorialScreens,
    clear_hud: bool,
    /// LeaderboardHUD is open with this many rows.
    leaderboard: Option<usize>,
    /// It failed this race; the board shows the rows.
    leaderboard_failed: bool,
}

/// ponytail: the stand-in for any HUD movie that fails (and for single-player results).
#[derive(Component)]
struct Board;

pub(crate) struct Skate2DeathRacePlugin;
impl Plugin for Skate2DeathRacePlugin {
    fn build(&self, app: &mut App) {
        let root = app.world().get_resource::<Config>().map(|c| c.asset_root.join(DIR)).unwrap_or_default();
        let data = load(&root);
        if let (Some(data), Some(mut playable)) = (&data, app.world_mut().get_resource_mut::<Playable>()) {
            playable.0.extend(data.races.iter().filter(|(_, r)| r.online && r.playable()).map(|(id, _)| id.clone()));
        }
        let mut screens = TutorialScreens::default();
        screens.silent_countdown = true;
        app.insert_resource(DeathRace { data, root, screens, ..default() })
            .add_systems(Startup, spawn_board)
            .add_systems(FixedUpdate, tick.after(crate::app::SimulationSet::Physics))
            .add_systems(Update, (drive, visuals, hud).chain())
            .add_systems(Update, end_camera.after(crate::camera::present).before(crate::app::FrameSet::Verification));
    }
}

fn spawn_board(mut commands: Commands) {
    commands.spawn((
        Board,
        Text::new(""),
        TextFont { font_size: 20., ..default() },
        TextLayout::new_with_justify(Justify::Right),
        BackgroundColor(Color::srgba(0., 0., 0., 0.45)),
        Node { position_type: PositionType::Absolute, display: Display::None, top: percent(16), right: percent(3), padding: UiRect::all(px(8)), ..default() },
        GlobalZIndex(50),
        Pickable::IGNORE,
    ));
}

/// The match's phases for a Death Race: setup (time limit, intro, solo start), the gates at the countdown, the clock
/// at the start, and the end.
#[allow(clippy::too_many_arguments)]
fn drive(
    mut dr: ResMut<DeathRace>,
    mut m: ResMut<Match>,
    mut cutscene: ResMut<Cutscene>,
    mut skater: ResMut<SkaterRuntime>,
    map: Res<CurrentMap>,
    time: Res<Time<Real>>,
    mut fade: Option<ResMut<ChallengeFade>>,
    mut race_map: Option<ResMut<crate::skate2_hud::Skate2RaceMap>>,
    mut warned: Local<String>,
) {
    let now = time.elapsed_secs();
    let dr = &mut *dr;
    // A Skate 1 row (skate1_online owns its match) is not ours.
    let ours = m.mode == Some(Mode::DeathRace) && !m.owned;
    let on_map = map.path.as_deref().and_then(Path::file_stem).is_some_and(|s| s == MAP);
    let Some(race) = dr.data.as_ref().and_then(|d| d.races.get(&m.row)).filter(|r| ours && on_map && r.playable()) else {
        if let Some(active) = dr.active.take() {
            if active.intro.is_some() {
                cutscene.stop();
            }
            if let Some(fade) = fade.as_mut() {
                fade.0 = 0.;
            }
            dr.visual += 1;
            dr.clear_hud = true;
        }
        if let Some(map) = race_map.as_mut().filter(|r| !r.0.is_empty()) {
            map.0.clear();
        }
        if ours && *warned != m.row {
            warn!("Death Race: no playable race {:?} on this map ({})", m.row, dr.root.display());
            warned.clone_from(&m.row);
        }
        return;
    };
    if dr.active.as_ref().is_none_or(|a| a.row != m.row) {
        // setup_heat
        m.time_limit = race.cap();
        m.outro = END_HEAT + CLEANUP_HEAT;
        m.intro = matches!(m.phase, Phase::Intro { .. });
        dr.active = Some(Active {
            row: m.row.clone(),
            since: now,
            placed: None,
            intro_started: false,
            intro: None,
            faded_in: None,
            run: None,
            running: false,
            deadline: None,
            inside: BTreeSet::new(),
            open: false,
            opened: 0.,
            central: None,
            out: false,
            over: false,
        });
        dr.visual += 1;
    }
    let a = dr.active.as_mut().expect("set above");
    let intro = matches!(m.phase, Phase::Intro { .. });
    // setup_heat (344-547): FadeOut 0.5 and a 0.5 s timer; at black RepositionAI(id, StartLocation, id, false, 0) (no
    // fade: at once) and PinAI (the core holds racers through its intro and countdown); every racer teleported, 0.01 s,
    // the online NIS; at its nischallengenotify (its end) FadeIn 0.5 and the countdown. The match's intro covers all
    // of it. ponytail: the others' skaterteleportcomplete isn't relayed; this racer's own teleport starts its NIS (the
    // core starts the consoles' countdowns together).
    if a.placed.is_none() && now - a.since >= FADE && !m.teleporting() {
        // A teleport still pending (the core's spawn) refuses this one: tried again each frame through the intro.
        let refused = matches!(race.spawn(m.slot).map(|at| skater.travel_to(at)), Some(Err(_)));
        if !refused || !intro {
            a.placed = Some(now);
        }
        if refused && !intro {
            warn!("Death Race: the start teleport was refused");
        }
    }
    if !a.intro_started && a.placed.is_some_and(|t| now - t >= TELEPORT_DONE) {
        a.intro_started = true;
        // The scene is picked by the player count.
        match intro_key(&m.row, &race.intro, m.standings.len().max(1), m.slot).filter(|_| intro) {
            Some((key, seconds)) => {
                cutscene.start(&dr.root, &key);
                a.intro = Some(now + seconds);
            }
            // ponytail: retail goes to the countdown with its FadeOut still up (no notify); every race has its NIS.
            None => (m.intro, a.faded_in) = (false, Some(now)),
        }
    }
    if a.intro.is_some_and(|end| now >= end || !intro) {
        a.intro = None;
        cutscene.stop();
        (m.intro, a.faded_in) = (false, Some(now));
    }
    let black = heat_black(now, a.since, a.intro.is_some(), a.faded_in);
    if let Some(fade) = fade.as_mut()
        && fade.0 != black
    {
        fade.0 = black;
    }
    if a.run.is_none() && matches!(m.phase, Phase::Countdown { .. } | Phase::Running { .. }) {
        a.run = Some(Run::new(race));
        dr.visual += 1;
    }
    if let Phase::Running { started, .. } = m.phase
        && !a.running
    {
        // run_heat stateenter: the racer's countdown starts; volumes it's already in don't count as entered.
        a.running = true;
        a.deadline = (race.heat.time_limit > 0.).then_some(started + race.heat.time_limit);
        let p = skater_position(&skater);
        a.inside = race.volumes.iter().filter(|(_, v)| v.contains(p)).map(|(n, _)| n.clone()).collect();
        (m.score, m.finished) = (DNF, false);
    }
    // cleanup_heat: the gates and the leaderboard go, "Challenge Complete!" / "Challenge Failed" for 2 s with the pause
    // menu off.
    let cleanup = cleanup(&m, now);
    if (cleanup || matches!(m.phase, Phase::Results { .. })) && !a.over {
        a.over = true;
        dr.visual += 1;
        if cleanup {
            let won = placed(&m) <= race.place_to_win.unwrap_or(1) && !a.out;
            let (id, central) = if won {
                (race.complete.as_deref().unwrap_or("ID_CHALLENGE_CENTRAL_MESSAGE_CHALLENGECOMPLETE"), Central::Complete)
            } else {
                (race.failed.as_deref().unwrap_or("ID_MISSION_CHALLENGE_FAILED"), Central::Failed)
            };
            let text = dr.data.as_ref().and_then(|d| d.strings.get(id)).cloned().unwrap_or_else(|| id.to_owned());
            a.central = Some((text, central, now + CLEANUP_HEAT));
        }
    }
    if m.pause_disabled != cleanup {
        m.pause_disabled = cleanup;
    }
    // A racer who finished or ran out of time is pinned (run_heat: AI.PinAI) until the race is over; one who finished
    // online watches the others from the end camera (`end_camera`).
    if matches!(m.phase, Phase::Running { .. }) {
        let pinned = m.token != 0 && m.finished;
        if m.hold != pinned {
            m.hold = pinned;
        }
        // Skate 3's rule (user 10-28): a finished racer waiting for the rest can emote (skate3_modes::gestures).
        let gestures = crate::skate3_modes::gestures(&m, false);
        if m.gestures != gestures {
            m.gestures = gestures;
        }
    }
    // The minimap's gates and finish (Map.AddRace*) while the gates are up: the countdown to cleanup_heat (549-574).
    if let Some(map) = race_map.as_mut() {
        let objects = a.run.as_ref().filter(|_| !a.over).map_or_else(Vec::new, |r| race_objects(race, r));
        if map.0 != objects {
            map.0 = objects;
        }
    }
}

/// setup_heat's screen: black over FADE from the setup, held through the teleport; FadeIn over FADE from the NIS's
/// notify.
// ponytail: whether the challenge FadeOut (Effects slot 0x82b7faf0) draws over the NIS isn't traced; the NIS shows
// (else its 5 s would play under black).
fn heat_black(now: f32, since: f32, nis: bool, faded_in: Option<f32>) -> f32 {
    match faded_in {
        Some(t) => 1. - ((now - t) / FADE).clamp(0., 1.),
        None if nis => 0.,
        None => ((now - since) / FADE).clamp(0., 1.),
    }
}

/// cleanup_heat: the last CLEANUP_HEAT seconds of the core's outro.
fn cleanup(m: &Match, now: f32) -> bool {
    m.ending.is_some_and(|end| now >= end - CLEANUP_HEAT && matches!(m.phase, Phase::Running { .. }))
}

/// This racer's place at end_heat (1-based): online its rank in the standings (finishers by time, then the rest, as
/// EndHeatPointSorter has them), solo first (no AI racers here).
fn placed(m: &Match) -> usize {
    m.standings.iter().position(|s| s.local).map_or(1, |i| i + 1)
}

/// The run, each physics tick: volumes entered and left, trick sequences, the racer's countdown.
fn tick(mut dr: ResMut<DeathRace>, mut m: ResMut<Match>, skater: Res<SkaterRuntime>, time: Res<Time<Real>>) {
    let now = time.elapsed_secs();
    let DeathRace { data, active, visual, .. } = &mut *dr;
    let (Some(data), Some(a)) = (data.as_ref(), active.as_mut()) else { return };
    let Phase::Running { started, .. } = m.phase else { return };
    let (Some(race), Some(run)) = (data.races.get(&a.row), a.run.as_mut()) else { return };
    if !a.running || m.mode != Some(Mode::DeathRace) {
        return;
    }
    // Retail's "doing a trick or in the air" at a gate's exit: a sequence is open.
    let s = &skater.scoring;
    let open = s.sequence_open();
    if open && !a.open {
        a.opened = now;
    }
    a.open = open;
    if run.finished {
        return;
    }
    let p = skater_position(&skater);
    let mut notes = Vec::new();
    // ponytail: volumes changed in one tick are handled in name order (retail: the engine's trigger order).
    for (name, volume) in &race.volumes {
        let inside = volume.contains(p);
        if inside == a.inside.contains(name) {
            continue;
        }
        if inside {
            a.inside.insert(name.clone());
            notes.extend(run.enter(race, name, now));
        } else {
            a.inside.remove(name);
            run.exit(race, name, now, open);
        }
    }
    if s.sequence_landed {
        notes.extend(run.sequence(race, s.sequence_score, a.opened));
    }
    let text = |key: &str| data.strings.get(key).cloned().unwrap_or_else(|| key.to_owned());
    for note in notes {
        match note {
            Note::Seconds(add) => {
                if let Some(d) = a.deadline.as_mut() {
                    *d += add;
                }
                a.central = Some((text("ID_RACE_SECONDS_ADDED").replace("{0:s}", &add.to_string()), Central::Plain, now + CENTRAL_FOR));
            }
            Note::LegBonus(add) => {
                if let Some(d) = a.deadline.as_mut() {
                    *d += add;
                }
            }
            Note::Finished => {
                (m.finished, m.score) = (true, ((now - started) * 1000.).round() as i64);
                // Online only while another racer is still active (run_heat 934-948); ponytail: solo too, so a test
                // sees its finish.
                if m.token == 0 || m.standings.iter().any(|s| !s.local && !s.finished) {
                    a.central = Some((text("ID_RACE_FINISHED"), Central::Plain, now + CENTRAL_FOR));
                }
            }
            Note::Out => (m.finished, m.score, a.out) = (true, DNF, true),
        }
    }
    if !run.finished && a.deadline.is_some_and(|d| now >= d) {
        // timerexpired: out. Single player shows ID_MISSION_CHALLENGE_FAILED; online shows nothing.
        run.finished = true;
        (m.finished, m.score, a.out) = (true, DNF, true);
        if m.token == 0 {
            a.central = Some((text("ID_MISSION_CHALLENGE_FAILED"), Central::Plain, now + CENTRAL_FOR));
        }
    }
    if std::mem::take(&mut run.changed) {
        *visual += 1;
    }
}

#[derive(Default)]
struct Drawn {
    seen: u64,
    roots: Vec<Entity>,
    meshes: HashMap<String, Handle<Mesh>>,
    materials: HashMap<String, (Handle<StandardMaterial>, Option<Handle<StandardMaterial>>)>,
}

/// The course chevrons (from setup) and the gate indicators up (from the countdown), until the results.
#[allow(clippy::too_many_arguments)]
fn visuals(
    mut commands: Commands,
    dr: Res<DeathRace>,
    physics: Option<Res<GamePhysics>>,
    server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut drawn: Local<Drawn>,
) {
    let drawn = &mut *drawn;
    if drawn.seen == dr.visual {
        return;
    }
    drawn.seen = dr.visual;
    for root in drawn.roots.drain(..) {
        commands.entity(root).despawn();
    }
    let (Some(data), Some(a)) = (&dr.data, dr.active.as_ref().filter(|a| !a.over)) else { return };
    let Some(race) = data.races.get(&a.row) else { return };
    let mut wanted: Vec<(&str, Transform, &str, &str)> = (race.chevrons.iter())
        .filter_map(|c| {
            let m = race.locators.get(&c.locator)?;
            Some((c.locator.as_str(), Transform::from_matrix(Mat4::from_cols_array_2d(m)), c.shape.as_str(), c.style.as_str()))
        })
        .collect();
    for (volume, (shape, style)) in a.run.iter().flat_map(|r| &r.shown) {
        let Some(b) = race.volumes.get(volume) else { continue };
        let floor = physics.as_deref().and_then(|p| ground(p, b.centre().with_y(b.max[1]))).map_or(b.min[1], |g| g.y);
        wanted.push((volume, arch(b, race.approach(volume), floor), shape, style));
    }
    for (name, transform, shape, style) in wanted {
        let (Some(outline), Some(look)) = (data.shapes.get(shape), data.styles.get(style)) else { continue };
        let mesh = drawn.meshes.entry(shape.to_owned()).or_insert_with(|| meshes.add(crate::skate2_arrows::ribbon(outline))).clone();
        let (glow, darken) = (drawn.materials.entry(style.to_owned()))
            .or_insert_with(|| {
                let [r, g, b, alpha] = look.tint;
                let pass = |texture: &str, alpha_mode, color| StandardMaterial {
                    base_color: color,
                    base_color_texture: Some(server.load(format!("{DIR}/indicator/{texture}"))),
                    unlit: true,
                    alpha_mode,
                    double_sided: true,
                    cull_mode: None,
                    ..default()
                };
                // As skate2_arrows: the darken pass (alpha blend), then the colour pass added (ONE, ONE).
                let mut glow = pass(&look.texture, AlphaMode::Add, Color::linear_rgba(r, g, b, alpha));
                glow.depth_bias = 1.;
                let darken = look.background.as_deref().map(|t| materials.add(pass(t, AlphaMode::Blend, Color::WHITE)));
                (materials.add(glow), darken)
            })
            .clone();
        let root = commands
            .spawn((Name::new(format!("Death Race indicator {name}")), transform, Visibility::Inherited))
            .with_children(|root| {
                if let Some(darken) = darken {
                    root.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(darken)));
                }
                root.spawn((Mesh3d(mesh), MeshMaterial3d(glow)));
            })
            .id();
        drawn.roots.push(root);
    }
}

/// The race HUD: CountdownHUD, TimerHUD and CentralHUD on Skate 2's own movies (LeaderboardHUD: [`leaderboard`]).
fn view(a: &Active, m: &Match, now: f32) -> View {
    let countdown = match m.phase {
        Phase::Countdown { ends } => Some((1, (ends - now).ceil().max(1.) as i64)),
        Phase::Running { started, .. } if now - started < GO_FOR => Some((1, 0)),
        _ => None,
    };
    // HUD2.StartTimer at run_heat; it keeps running after the finish, as retail's does.
    let timer = a.deadline.filter(|_| matches!(m.phase, Phase::Running { .. })).map(|d| f64::from((d - now).max(0.)));
    let result = a.central.as_ref().filter(|(.., until)| now < *until).map(|(text, central, _)| (text.clone(), String::new(), *central));
    View { countdown, timer, result, ..default() }
}

/// A LeaderboardHUD row: name, score (-1: out, which the movie shows as '--'), time text, this racer.
type Row = (String, i64, String, bool);

/// RankNameTime rows in standings order (finishers by time, then the rest), this racer alone solo. A racer's time
/// counts up until they finish, as retail's per-racer HeatTotalTime count-up timers do.
fn rows(m: &Match, me: &str, now: f32) -> Vec<Row> {
    let elapsed = match m.phase {
        Phase::Running { started, .. } => ((now - started) * 1000.) as i64,
        _ => 0,
    };
    let row = |name: &str, score: i64, finished: bool, local: bool| {
        let time = Mode::DeathRace.show(if finished { score } else { elapsed });
        (name.to_owned(), if finished && score >= DNF { -1 } else { 0 }, format!("#{time}"), local)
    };
    if m.standings.is_empty() {
        return vec![row(me, m.score, m.finished, true)];
    }
    m.standings.iter().map(|s| row(&s.name, s.score, s.finished, s.local)).collect()
}

/// LeaderboardHUD (setup_heat's LeaderboardHUDInit: RankNameTime, the race's title and its variant) on Skate 2's
/// online HUD layer from the countdown to end_heat, refreshed as the times change. false: not showing (none wanted,
/// or the movie failed; the board shows the rows then).
fn leaderboard(world: &mut World, wanted: Option<(&[Row], &str, &str)>) -> bool {
    let online = world.resource::<Match>().token != 0;
    let (open, failed) = {
        let dr = world.resource::<DeathRace>();
        (dr.leaderboard, dr.leaderboard_failed)
    };
    if wanted.is_some() && failed {
        return false;
    }
    let Some(mut screens) = world.get_resource_mut::<Screens>() else { return false };
    let layer = screens.layer(SKATE2_ONLINE);
    let state = match wanted {
        None => {
            if open.is_some() {
                layer.close(LEADERBOARD);
            }
            (None, false)
        }
        Some((rows, title, variant)) => {
            let answers = leaderboard_answers(title, online, 4, variant, rows, None);
            let result = (|| -> Result<(), String> {
                match layer.get(LEADERBOARD).filter(|_| open == Some(rows.len())) {
                    Some(screen) => crate::skate2_contests::refresh_leaderboard(screen, answers),
                    None => {
                        let screen = layer.open(LEADERBOARD, "leaderboardhud", &answers)?;
                        screen.call("SetState", &[json!(3)])?;
                        screen.call("SetState", &[json!(1)]).map(drop)
                    }
                }
            })();
            match result {
                Ok(()) => (Some(rows.len()), false),
                Err(error) => {
                    warn!("Death Race: leaderboardhud: {error}");
                    layer.close(LEADERBOARD);
                    (None, true)
                }
            }
        }
    };
    drop(screens);
    let mut dr = world.resource_mut::<DeathRace>();
    (dr.leaderboard, dr.leaderboard_failed) = state;
    dr.leaderboard.is_some()
}

/// The race's title and variant as LeaderboardHUD shows them ('#' text from death_race.json's strings).
fn heading(data: &Data, race: &Race) -> (String, String) {
    let text = |key: &str| if key.starts_with('#') { key.to_owned() } else { format!("#{}", data.strings.get(key).map_or(key, String::as_str)) };
    let variant = if race.race_type == TRICK_FOR_TIME { "ID_RACE_VARIANT_TRICKFORTIME" } else { "ID_RACE_VARIANT_DOWNHILL" };
    (text(&race.title), text(variant))
}

fn hud(world: &mut World) {
    let now = world.resource::<Time<Real>>().elapsed_secs();
    let me = crate::skate2_contests::player_name(world);
    let wanted = {
        let (dr, m) = (world.resource::<DeathRace>(), world.resource::<Match>());
        let racing = matches!(m.phase, Phase::Countdown { .. } | Phase::Running { .. }) && !cleanup(m, now);
        // Online the results screen (online_hud) has the times.
        let solo_results = m.token == 0 && matches!(m.phase, Phase::Results { .. });
        let heading = dr.data.as_ref().zip(dr.active.as_ref()).and_then(|(d, a)| Some(heading(d, d.races.get(&a.row)?)));
        dr.active.as_ref().map(|a| (view(a, m, now), racing, solo_results, rows(m, &me, now), heading.unwrap_or_default()))
    };
    let movie = leaderboard(world, wanted.as_ref().filter(|w| w.1).map(|(_, _, _, rows, (title, variant))| (&rows[..], &title[..], &variant[..])));
    let wanted = wanted.map(|(view, racing, solo_results, rows, _)| {
        // ponytail: the board stands in for LeaderboardHUD when it fails, and for the single-player results screen.
        let board = if racing && !movie || solo_results {
            (rows.iter().enumerate())
                .map(|(i, (name, out, time, _))| format!("{}. {name}  {}", i + 1, if *out < 0 { "DNF" } else { time.trim_start_matches('#') }))
                .collect()
        } else {
            Vec::new()
        };
        (view, board)
    });
    let lines = world.resource_scope(|world, mut dr: Mut<DeathRace>| {
        if std::mem::take(&mut dr.clear_hud) {
            dr.screens.clear(world);
        }
        let (view, mut lines) = wanted?;
        let shown = dr.screens.sync(world, &view);
        // Plain text for what the movies aren't showing.
        lines.extend(view.countdown.filter(|_| !shown.countdown).map(|(_, left)| countdown_text(left).trim_start_matches('#').to_owned()));
        lines.extend(view.timer.filter(|_| !shown.timer).map(|left| timer_text(left).trim_start_matches('#').to_owned()));
        lines.extend(view.result.filter(|_| !shown.result).map(|(text, ..)| text));
        Some(lines)
    });
    let mut boards = world.query_filtered::<(&mut Text, &mut Node), With<Board>>();
    for (mut text, mut node) in boards.iter_mut(world) {
        let wanted = lines.as_ref().filter(|l| !l.is_empty()).map(|l| l.join("\n"));
        let display = if wanted.is_some() { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
        if let Some(t) = wanted
            && text.0 != t
        {
            text.0 = t;
        }
    }
}

/// Online, a racer who finished watches the finish from OnlineEndCameraLocation while others still race
/// (SwitchChallengeCamera; it looks along the locator's Z row, toward the finish in every race but kotm_2).
fn end_camera(dr: Res<DeathRace>, m: Res<Match>, mut cameras: Query<&mut Transform, With<crate::camera::GameplayCamera>>) {
    let Some(a) = dr.active.as_ref() else { return };
    let watching = m.token != 0 && m.finished && !a.out && matches!(m.phase, Phase::Running { .. }) && m.ending.is_none();
    if !watching || !m.standings.iter().any(|s| !s.local && !s.finished) {
        return;
    }
    let race = dr.data.as_ref().and_then(|d| d.races.get(&a.row));
    let Some(c) = race.and_then(|r| r.locators.get(r.end_camera.as_ref()?)) else { return };
    for mut transform in &mut cameras {
        *transform = Transform::from_translation(Vec3::from_slice(&c[3][..3])).looking_to(Vec3::from_slice(&c[2][..3]), Vec3::Y);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Gates g0.. (5 s each) on axis-aligned boxes along +z; splits s0..; the finish is the leg's last split.
    fn race(race_type: u8, skipable: bool, legs: &[(usize, usize)]) -> Race {
        let mut volumes = serde_json::Map::new();
        let leg = |k: usize, gates: usize, splits: usize| {
            json!({
                "gates": (0..gates).map(|i| json!({"volume": format!("l{k}g{i}"), "bonus": 5, "ribbon": "arch", "style": "orange"})).collect::<Vec<_>>(),
                "splits": (0..splits).map(|i| json!(format!("l{k}s{i}"))).collect::<Vec<_>>(),
                "end_bonus": 7,
            })
        };
        for (k, &(gates, splits)) in legs.iter().enumerate() {
            for (n, kind) in [(gates, 'g'), (splits, 's')] {
                for i in 0..n {
                    let z = (k * 100 + i * 10) as f32;
                    volumes.insert(format!("l{k}{kind}{i}"), json!({"min": [-5., 0., z], "max": [5., 4., z + 2.]}));
                }
            }
        }
        let look = |style: &str| json!({"ribbon": "arch", "style": style});
        serde_json::from_value(json!({
            "online": true, "race_type": race_type, "skipable": skipable, "trick_multiplier": 10, "end_camera": null,
            "ribbons": {"grey": look("grey"), "final": look("red"), "leg_finish": look("green")},
            "chevrons": [],
            "heat": {
                "time_limit": 60, "start": "start", "terminate": [],
                "legs": legs.iter().enumerate().map(|(k, &(g, s))| leg(k, g, s)).collect::<Vec<_>>(),
            },
            "locators": {"start": [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [0., 0., -10., 1.]]},
            "volumes": volumes,
            "intro": [],
        }))
        .unwrap()
    }

    fn styles(run: &Run) -> Vec<(&str, &str)> {
        run.shown.iter().map(|(v, (_, s))| (v.as_str(), s.as_str())).collect()
    }

    #[test]
    fn splits_count_in_order_and_the_last_one_finishes() {
        let race = race(0, true, &[(2, 3)]);
        let mut run = Run::new(&race);
        assert_eq!(styles(&run), [("l0g0", "orange"), ("l0g1", "orange"), ("l0s2", "red")], "skipable: all gates and the finish");
        assert_eq!(run.enter(&race, "l0s2", 0.), [], "the finish before the splits doesn't count");
        assert_eq!(run.enter(&race, "l0s1", 0.), [], "out of order");
        assert_eq!(run.enter(&race, "l0g1", 0.), [Note::Seconds(5.)], "a gate pays the first time");
        assert_eq!(run.enter(&race, "l0g1", 0.), [], "...only");
        assert!(!run.shown.contains_key("l0g1"), "and its indicator goes");
        for split in ["l0s0", "l0s1"] {
            assert_eq!(run.enter(&race, split, 0.), []);
        }
        assert_eq!(run.enter(&race, "l0s2", 0.), [Note::Finished]);
        assert_eq!(run.enter(&race, "l0g0", 0.), [], "nothing counts after the finish");
    }

    #[test]
    fn legs_restart_their_gates_and_pay_the_end_bonus() {
        let race = race(0, true, &[(1, 1), (1, 1)]);
        let mut run = Run::new(&race);
        assert_eq!(styles(&run), [("l0g0", "orange"), ("l0s0", "green")], "a leg's finish is the leg-finish ribbon");
        assert_eq!(run.enter(&race, "l0s0", 0.), [Note::LegBonus(7.)]);
        assert_eq!((run.leg, styles(&run)), (1, vec![("l1g0", "orange"), ("l1s0", "red")]));
        assert_eq!(run.enter(&race, "l1s0", 0.), [Note::Finished]);
    }

    /// GAP-7 (online audit 10-28): the minimap gets each race object the Lua adds beside an indicator (r_1_1 636-690,
    /// 1080-1115, 1268-1414): AddRaceGate's own mini_gate for a lit gate, the race's RaceGateGreyIcon for a grey one
    /// (nil: AddRaceGate's own), the leg's finish as AddRaceCheckPoint's mini_spotracegate from the countdown, even on
    /// the only leg, and later AddRaceFinish's mini_gate_final on the last leg; a gate's object goes with its indicator.
    #[test]
    fn the_minimap_gets_the_race_gates_and_finish() {
        let icons = |race: &Race, run: &Run| race_objects(race, run).into_iter().map(|(icon, _)| icon).collect::<Vec<_>>();
        let downhill = race(0, true, &[(2, 3)]);
        let mut run = Run::new(&downhill);
        assert_eq!(
            race_objects(&downhill, &run),
            [("mini_gate".to_owned(), [0., 1.]), ("mini_gate".to_owned(), [0., 11.]), ("mini_spotracegate".to_owned(), [0., 21.])]
        );
        run.enter(&downhill, "l0g1", 0.);
        assert_eq!(icons(&downhill, &run), ["mini_gate", "mini_spotracegate"], "a bonus gate's object goes with its indicator");
        // A gate-order trick race: the next gate lit, the one after it grey, the only leg's finish once near the end.
        let mut trick = race(TRICK_FOR_TIME, false, &[(4, 1)]);
        assert_eq!(icons(&trick, &Run::new(&trick)), ["mini_gate", "mini_gate"], "no RaceGateGreyIcon: AddRaceGate's own");
        trick.grey_icon = Some("mini_gate_next".into());
        let mut run = Run::new(&trick);
        assert_eq!(icons(&trick, &run), ["mini_gate", "mini_gate_next"]);
        for (i, t) in [(0, 1.), (1, 3.), (2, 5.)] {
            let gate = format!("l0g{i}");
            run.enter(&trick, &gate, t);
            run.exit(&trick, &gate, t + 1., false);
        }
        assert_eq!(icons(&trick, &run), ["mini_gate", "mini_gate_final"]);
        // Two legs: the countdown's finish is a checkpoint, the next (last) leg's the finish.
        let legs = race(0, true, &[(1, 1), (1, 1)]);
        let mut run = Run::new(&legs);
        assert_eq!(icons(&legs, &run), ["mini_gate", "mini_spotracegate"]);
        run.enter(&legs, "l0s0", 0.);
        assert_eq!(icons(&legs, &run), ["mini_gate", "mini_gate_final"]);
    }

    #[test]
    fn trick_for_time_gates_go_in_order_and_pay_for_tricks() {
        let race = race(TRICK_FOR_TIME, false, &[(4, 1)]);
        let mut run = Run::new(&race);
        assert_eq!(styles(&run), [("l0g0", "orange"), ("l0g1", "grey")], "the next gate lit, the one after grey");
        // In gate 1 before gate 0: not armed.
        run.enter(&race, "l0g1", 1.);
        run.exit(&race, "l0g1", 2., false);
        assert_eq!(run.sequence(&race, 500., 1.5), [], "gate 1 isn't armed yet");
        // Gate 0 ridden through without a trick: used up, no time, gate 1 armed.
        run.enter(&race, "l0g0", 3.);
        run.exit(&race, "l0g0", 4., false);
        assert_eq!(styles(&run), [("l0g1", "orange"), ("l0g2", "grey")]);
        // A trick in gate 1, landed after leaving it mid-sequence: floor(points / 10) seconds.
        run.enter(&race, "l0g1", 5.);
        run.exit(&race, "l0g1", 6., true);
        assert_eq!(run.sequence(&race, 257., 5.5), [Note::Seconds(25.)]);
        assert_eq!(styles(&run), [("l0g2", "orange"), ("l0g3", "grey")]);
        assert_eq!(run.sequence(&race, 257., 5.5), [], "a gate pays once");
        // Past gate Count - 2 (Lua's i > Count - 2), the finish shows too.
        run.enter(&race, "l0g2", 7.);
        run.exit(&race, "l0g2", 8., false);
        assert_eq!(styles(&run), [("l0g3", "orange"), ("l0s0", "red")]);
        assert_eq!(run.enter(&race, "l0s0", 9.), [Note::Finished]);
    }

    /// end_heat then cleanup_heat (r_1_1_fabio_mem1_ol.lua 549-607, 1717-1853): the core's outro ends with cleanup_heat's
    /// 2 s; the place that wins is the standings' (finishers by time, then the rest: EndHeatPointSorter).
    #[test]
    fn the_race_ends_with_end_heat_then_cleanup_heat() {
        use crate::match_core::Standing;
        let mut m = Match { mode: Some(Mode::DeathRace), phase: Phase::Running { started: 0., ends: None }, token: 1, ..default() };
        assert!(!cleanup(&m, 50.));
        m.ending = Some(50. + END_HEAT + CLEANUP_HEAT);
        assert!(!cleanup(&m, 52.9) && cleanup(&m, 53.), "end_heat's 3 s, then cleanup_heat");
        let standing = |name: &str, score, local| Standing { name: name.into(), score, finished: true, local };
        m.standings = vec![standing("Ann", 38_500, false), standing("Danny", 45_120, true)];
        assert_eq!(placed(&m), 2, "second: not within PlaceHigherToWin 1");
        m.standings.clear();
        assert_eq!(placed(&m), 1, "solo");
    }

    /// setup_heat (344-466): FadeOut 0.5 from the setup, the teleport at black (0.5 s in), held black until the NIS,
    /// FadeIn 0.5 from its notify.
    #[test]
    fn the_start_teleport_is_hidden_by_setup_heats_fade() {
        let black = |now, nis, faded_in| heat_black(now, 10., nis, faded_in);
        assert_eq!([black(10., false, None), black(10.25, false, None), black(10. + FADE, false, None)], [0., 0.5, 1.]);
        assert_eq!(black(10.51, false, None), 1., "the teleport and TELEPORT_DONE at black");
        assert_eq!(black(12., true, None), 0., "the NIS shows");
        assert_eq!([black(15.5, false, Some(15.5)), black(15.75, false, Some(15.5)), black(16., false, Some(15.5))], [1., 0.5, 0.]);
    }

    #[test]
    fn intro_picks_the_scene_for_the_players_and_the_track_for_the_slot() {
        let intro: Vec<Intro> = (2..=6).map(|n| Intro { scene: format!("S{n}"), seconds: 5., tracks: n }).collect();
        assert_eq!(intro_key("r", &intro, 1, 0), Some(("r|S2|0".into(), 5.)), "solo plays the 2-player scene");
        assert_eq!(intro_key("r", &intro, 4, 3), Some(("r|S4|3".into(), 5.)));
        assert_eq!(intro_key("r", &intro, 9, 7).unwrap().0, "r|S6", "past the last scene: its camera alone");
        assert_eq!(intro_key("r", &[], 2, 0), None);
    }

    #[test]
    fn arch_spans_the_volume_across_the_travel() {
        let b = Aabb { min: [0., 0., 0.], max: [20.08, 4., 2.] };
        let t = arch(&b, Vec3::new(10., 0., -50.), 1.);
        assert_eq!(t.translation, Vec3::new(10.04, 1., 1.));
        assert!((t.scale.x - 2.).abs() < 1e-4 && t.rotation.angle_between(Quat::IDENTITY) < 1e-4);
        // Coming along -x: turned a quarter, as wide as the box is deep (never thinner than 0.3 of the outline).
        let t = arch(&b, Vec3::new(80., 0., 1.), 0.);
        assert!((t.scale.x - 0.3).abs() < 1e-4 && (t.rotation * Vec3::Z).x < -0.99);
        let b = Aabb { min: [0., 0., 0.], max: [2., 4., 5.02] };
        let t = arch(&b, Vec3::new(80., 0., 1.), 0.);
        assert!((t.scale.x - 0.5).abs() < 1e-4 && (t.rotation * Vec3::Z).x < -0.99);
    }

    /// The intro's tracks are local: its racers stand on identity TRAJECTORYs within a few metres of the origin, facing
    /// +z. So the export turns a scene by yaw(definition dir) alone (the way the start slots face), not against the
    /// scenes' own dir (yaw -0.726, copied from RaceStarts_1's world placement at kotm_2).
    #[test]
    #[ignore = "needs SKATE3_ASSET_ROOT with private/skate2-online/death-race"]
    fn intro_racers_stand_at_the_scene_origin_facing_z() {
        let assets = PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap());
        let bank = skate_data::abin::Bank::load(&assets.join("private/stock/data/anim/OnBoard.abin")).unwrap();
        let bones = bank.hierarchy().unwrap().bone_names.clone();
        let trajectory = bones.iter().position(|b| b == "TRAJECTORY").unwrap();
        for anim in ["Online_Race_Skater31", "Online_Race_Skater41", "Online_Race_Skater51"] {
            let (poser, clip) = crate::cas_pose::Poser::scene(&installed().join(format!("scene/anim/{anim}.abin")), &bones).unwrap();
            for t in [0., poser.length(&clip).unwrap()] {
                let tr = poser.pose(&clip, t, false, &[]).unwrap()[trajectory];
                let z = tr.z_axis.truncate();
                assert!(z.x.atan2(z.z).abs() < 0.01 && tr.w_axis.truncate().length() < 3., "{anim} t={t}: {tr:?}");
            }
        }
    }

    fn installed() -> PathBuf {
        PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").expect("SKATE3_ASSET_ROOT")).join(DIR)
    }

    /// Every online race loads, has its intro and the outlines and styles it shows, and a racer through every split in
    /// order finishes it.
    #[test]
    #[ignore = "needs SKATE3_ASSET_ROOT with private/skate2-online/death-race"]
    fn installed_races_run_to_the_finish() {
        let root = installed();
        let data = load(&root).expect("death_race.json");
        let online: Vec<_> = data.races.iter().filter(|(_, r)| r.online).collect();
        assert_eq!(online.len(), 21);
        for (id, race) in online {
            assert!(race.playable(), "{id}");
            // Every racer gets its own spot on the start (4..8 slots).
            assert!(race.heat.slots.len() >= 4 && race.spawn(1) == Some(race.heat.slots[1]), "{id}");
            let (key, _) = intro_key(id, &race.intro, 2, 0).unwrap_or_else(|| panic!("{id}: no intro"));
            let scenes: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("tutorial.json")).unwrap()).unwrap();
            let scene = &scenes["scenes"][&key];
            assert!(root.join(scene["camera"].as_str().unwrap()).exists() && root.join(scene["tracks"][0]["anim"].as_str().unwrap()).exists(), "{key}");
            let mut run = Run::new(race);
            for (k, leg) in race.heat.legs.iter().enumerate() {
                for g in &leg.gates {
                    assert!(data.shapes.contains_key(&g.ribbon) && data.styles.contains_key(&g.style), "{id}: gate {g:?}", g = g.volume);
                }
                assert_eq!(run.leg, k, "{id}");
                for split in leg.splits.iter().flatten() {
                    run.enter(race, split, 0.);
                }
            }
            assert!(run.finished, "{id}");
            for c in &race.chevrons {
                assert!(data.shapes.contains_key(&c.shape) && data.styles.contains_key(&c.style), "{id}: {}", c.locator);
            }
        }
    }

    /// Six racers at retail's CHALLENGE_MAX (online audit 10-12, step 2): finishers by time, then the racers out (DNF),
    /// then the ones still racing; a racer who left is dropped. Hand-computed: 79.9 s (slot 2), 83.45 s (0), 91.02 s (5),
    /// out (1), still racing (4); slot 3 left. The HUD rows hold all five and show the out racer as '--'.
    #[test]
    fn six_racers_place_by_time_then_the_rest() {
        let results = [Some((83_450, true)), Some((DNF, true)), Some((79_900, true)), None, Some((0, false)), Some((91_020, true))];
        assert_eq!(crate::skate2_contests::tests::placed(&results, true), [2, 0, 5, 1, 4]);
        let standing = |name: &str, score, finished, local| crate::match_core::Standing { name: name.into(), score, finished, local };
        let mut m = Match { mode: Some(Mode::DeathRace), phase: Phase::Running { started: 0., ends: None }, ..default() };
        m.standings = vec![
            standing("Cy", 79_900, true, false),
            standing("Ann", 83_450, true, false),
            standing("Fay", 91_020, true, true),
            standing("Bo", DNF, true, false),
            standing("Eve", 0, false, false),
        ];
        let shown = rows(&m, "Fay", 100.);
        assert_eq!(shown.iter().map(|r| r.1).collect::<Vec<_>>(), [0, 0, 0, -1, 0]);
        assert_eq!(shown[4].2, "#1:40.00", "still racing: the count-up");
        assert_eq!(placed(&m), 3);
    }

    /// Every online race's gates through a ride in order (online audit 10-12, step 3): each leg opens lit / grey /
    /// finish as run_heat's rules say (a skipable race: every gate lit and the finish; a gate-order one: the first lit,
    /// the next grey, the finish once two gates or fewer are left), the last leg's finish wears RaceFinalGateRibbon and
    /// the others RaceLegFinishGateRibbon, every outline shown has its shape and style and (but for the disc's own
    /// missing volumes, listed) a placed volume, an armed gate goes once ridden, and the splits finish the race.
    #[test]
    #[ignore = "needs SKATE3_ASSET_ROOT with private/skate2-online/death-race"]
    fn installed_race_gates_show_in_order() {
        let data = load(&installed()).expect("death_race.json");
        let mut unplaced = std::collections::BTreeSet::new();
        let style = |run: &Run, v: Option<&str>| v.and_then(|v| run.shown.get(v)).map(|(_, s)| s.clone());
        for (id, race) in data.races.iter().filter(|(_, r)| r.online) {
            let mut run = Run::new(race);
            for (k, leg) in race.heat.legs.iter().enumerate() {
                assert_eq!(run.leg, k, "{id}");
                let finish = if k + 1 == race.heat.legs.len() { &race.ribbons.last } else { &race.ribbons.leg_finish };
                for (i, g) in leg.gates.iter().enumerate().filter(|(_, g)| g.volume.is_some() && g.volume.as_deref() != race.finish(k)) {
                    let want = match (race.skipable, i) {
                        (true, _) | (false, 0) => Some(g.style.clone()),
                        (false, 1) => Some(race.ribbons.grey.style.clone()),
                        _ => None,
                    };
                    assert_eq!(style(&run, g.volume.as_deref()), want, "{id} leg {k} gate {i}");
                }
                let finish_up = race.skipable || leg.gates.len() < 2;
                assert_eq!(style(&run, race.finish(k)), finish_up.then(|| finish.style.clone()), "{id} leg {k}: the finish");
                for (v, (shape, s)) in &run.shown {
                    assert!(data.shapes.contains_key(shape) && data.styles.contains_key(s), "{id}: {v}");
                    if !race.volumes.contains_key(v) {
                        unplaced.insert(format!("{id}:{v}"));
                    }
                }
                for (i, g) in leg.gates.iter().enumerate() {
                    let Some(v) = g.volume.as_deref() else { continue };
                    let armed = !run.used[i];
                    run.enter(race, v, i as f32);
                    if race.race_type == TRICK_FOR_TIME {
                        run.exit(race, v, i as f32 + 0.5, false);
                    }
                    if armed && (race.race_type == TRICK_FOR_TIME || race.heat.time_limit > 0. && g.bonus > 0.) {
                        assert!(!run.shown.contains_key(v) || race.finish(k) == Some(v), "{id} leg {k}: gate {i} stays up once ridden");
                    }
                }
                for split in leg.splits.iter().flatten() {
                    run.enter(race, split, 0.);
                }
            }
            assert!(run.finished, "{id}");
        }
        println!("outlines on volumes the disc doesn't place: {unplaced:?}");
    }

    /// An online race's Skate 2 HUD through a match, as `hud` and online_hud drive it: nothing in the intro,
    /// LeaderboardHUD (RankNameTime) through the countdown and the run (a finisher, a racer out, one still racing), the
    /// results screen after. Renders to SKATE_ONLINE_HUD_SHOTS when set.
    #[test]
    #[ignore = "needs SKATE3_ASSET_ROOT with private/skate2-online/death-race and private/hud-skate2/online"]
    fn online_race_hud_through_a_match() {
        use crate::{apt_screen::Screen, match_core::Standing};
        let assets = PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap());
        let root = assets.join("private/hud-skate2/online");
        let shots = std::env::var("SKATE_ONLINE_HUD_SHOTS").ok().map(PathBuf::from);
        let shoot = |screen: &Screen, name: &str| {
            assert!(screen.bindings.unanswered.iter().all(|k| crate::online_hud::tests::results_noop(k)), "{name}: asks {:?}", screen.bindings.unanswered);
            let draws = screen.draws().unwrap();
            assert!(!draws.is_empty(), "{name}");
            if let Some(out) = &shots {
                std::fs::create_dir_all(out).unwrap();
                crate::core_menu_runtime::tests::raster_draws(&draws, &root, &out.join(format!("deathrace_{name}.png")));
            }
        };
        let data = load(&installed()).expect("death_race.json");
        let (title, variant) = heading(&data, &data.races["r_2_1_dr_ur_01_ol"]);
        assert_eq!((title.as_str(), variant.as_str()), ("#Traffic King", "#Downhill"));
        let standing = |name: &str, score, finished, local| Standing { name: name.into(), score, finished, local };
        let mut m = Match { mode: Some(Mode::DeathRace), phase: Phase::Intro { since: 0. }, token: 1, ..default() };
        m.standings = vec![standing("Ann", 0, false, false), standing("Danny", DNF, false, true), standing("Bo", 0, false, false)];
        let racing = |m: &Match| matches!(m.phase, Phase::Countdown { .. } | Phase::Running { .. });
        let mut now = 0.;
        while !racing(&m) {
            assert!(matches!(m.phase, Phase::Intro { .. }), "the intro shows no leaderboard");
            now += 1. / 60.;
            crate::match_core::advance(&mut m, now);
        }
        let answers = leaderboard_answers(&title, true, 4, &variant, &rows(&m, "Danny", now), None);
        let mut screen = Screen::load(&root.join("runtime/leaderboardhud.json"), &answers).unwrap();
        for state in [3, 1] {
            screen.call("SetState", &[json!(state)]).unwrap();
        }
        while !matches!(m.phase, Phase::Running { .. }) {
            now += 1. / 60.;
            crate::match_core::advance(&mut m, now);
            screen.advance().unwrap();
        }
        shoot(&screen, "countdown");
        // 40 s in: Ann finished in 38.5 s, Bo is out, we race on.
        now += 40.;
        m.standings = vec![standing("Ann", 38_500, true, false), standing("Bo", DNF, true, false), standing("Danny", DNF, false, true)];
        let shown = rows(&m, "Danny", now);
        assert_eq!(shown[1].1, -1, "out: '--'");
        assert_eq!(shown[2].2, "#0:40.00", "still racing: the count-up");
        // Set up as Ann, Danny, Bo; now Bo ranks above Danny.
        crate::skate2_contests::refresh_leaderboard(&mut screen, leaderboard_answers(&title, true, 4, &variant, &shown, None)).unwrap();
        for _ in 0..30 {
            screen.advance().unwrap();
        }
        let named = |rank: usize| {
            let field = screen.clip(&format!("mcLeaderboardHUD.playerName{rank}.mLeaderboardData.name_txt")).unwrap();
            screen.vm.get(field, "_displayText").text()
        };
        assert_eq!([named(0), named(1), named(2)], ["Ann", "Bo", "Danny"], "the refresh keeps each name on its rank");
        shoot(&screen, "running");
        // The host ends the run: results.
        m.standings = vec![standing("Ann", 38_500, true, false), standing("Danny", 45_120, true, true), standing("Bo", DNF, true, false)];
        m.phase = Phase::Results { until: now + 8. };
        let answers = crate::online_hud::results_answers(Mode::DeathRace, &m.standings, &title, "ID_MAP_FILTER_CHALLENGETYPES_DEATHRACE");
        let mut screen = Screen::load(&root.join("runtime/roundresults_lb.json"), &answers).unwrap();
        for _ in 0..90 {
            screen.advance().unwrap();
        }
        shoot(&screen, "results");
    }
}
