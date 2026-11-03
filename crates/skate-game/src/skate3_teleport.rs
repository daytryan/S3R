//! Retail Skate 3's teleporter (challenge graph `teleport` < default, the export's `scripts/graphs/teleport.lua`; spec
//! 12.5, stage1_teleport 2): the Skate School's `tele_sksc_01`-`04` "Back to the top" and `tele_school_to_world` "Enter
//! Port Carverton". A teleport is a Global-2 row: the sign-up manager takes only Global 3 (0x82D529A8), so it has no
//! sign-up box, no sign-up marker, no manager D-pad prompt and no poster. Its graph runs from activation (0x82BD18C0),
//! draws its own location indicator and HUD, and asks "Are you sure?" when the HUD's button is pressed.
//!
//! [`Teleporter`] is the graph handler by handler (line numbers are teleport.lua's). The disc file's state and message
//! names are hashed; each is hash64("challenges/teleport/<state>[/<message>]") of the name used here (tests::NAMES).
//! What it asks of the natives is kept on it for the glue: the indicator, the HUD, the trigger reset, the popup, the
//! exit to move to. The messages [`Msg`] has no variant for (teleportersignup, hudyes, hudno, hudpopupoption) come in
//! through [`Teleporter::input`]. [`Teleports`] is the activity manager's part: activation, a challenge's start and end.
//! Not tested in play.
use std::collections::BTreeMap;

use crate::skate3_career::data::{CareerData, Place, Row};
use crate::skate3_modes::{self, Graph, Handled, Msg, Rt};

/// eGlobalType (registered at 0x8283DEC0) Full = 2: teleports and gotos. The sign-up manager's is SignUp = 3.
pub const FULL: u32 = 2;
/// The teleport HUD's button: the `signup` component's input handler 0x82666430 tests mask 1 (FE key 16, mask table
/// 0x825EC350). INFERRED (spec 18.24): mask 1 is D-pad Up, as the Xbox 360 pad bit order and "<B:DPad_Up> to Sign-Up".
pub const TELEPORT_BUTTON: u32 = 1;
/// L56: one exit's yes / no popup text, "Are you sure?".
pub const CONFIRM: &str = "ID_PAUSEMENU_CONFIRM";
/// L58: several exits' list popup text.
pub const LIST_TEXT: &str = "ID_TELEPORT_STADIUM_RAMPS_DESC";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum St {
    Initial,
    SetupTeleporter,
    WaitForTeleportVolume,
    WaitForConfirmation,
    Teleporting,
}

/// What L54-61 opens: Popup.ShowYesNoPopup(Title, text), or FrontEnd.ShowGenericAttributePopup(Title, text, titles)
/// (`retail_ui`'s list popup).
#[derive(Clone, Debug, PartialEq)]
pub enum Popup {
    YesNo { title: String, text: &'static str },
    List { title: String, text: &'static str, options: Vec<String> },
}

/// The graph's messages that [`Msg`] has no variant for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Input {
    /// teleportersignup { ActivityKey } (cMsgTeleporterSignUp 0x23B59DD1), the row key as an f32.
    SignUp(f32),
    /// hudyes / hudno: the yes / no popup's answer.
    Yes,
    No,
    /// hudpopupoption { OptionIndex }: the list popup's choice, 0-based.
    Pick(i64),
}

/// vars.kActivityID: the vars builder 0x82D057C0 stores the row's u64 key through 0x82F57EE0 and frsp (0x82D058A4).
/// INFERRED: 0x82F57EE0 converts the u64 to a double, so the key is rounded twice.
#[must_use]
pub fn activity_id(key: u64) -> f32 {
    key as f64 as f32
}

/// Whether activating the row starts its graph at once (0x82BD18C0: the Global-2 set AM+0x10278). AddActivity returns
/// early for a row with FD40E4A31027C984 set (0x82BCF57C..0x82BCF588), so `tele_world_to_school` never runs.
#[must_use]
pub fn starts_on_activation(row: &Row) -> bool {
    row.global == FULL && !row.disabled
}

/// One teleport row's graph and what it asks of the natives.
#[derive(Clone, Debug)]
pub struct Teleporter {
    pub row: String,
    /// Online.GetLocalPlayerID.
    pub me: usize,
    /// vars.kActivityID ([`activity_id`]).
    pub activity: f32,
    /// self.TeleporterTrigger: the volume's id as [`Msg::Trigger`]'s `volume` carries it (the u64 bit for bit).
    pub trigger: Option<i64>,
    pub title: String,
    /// self.Location and self.SignUpIndicator.
    pub location: String,
    pub indicator: String,
    /// self.TeleportExitLocation and self.TeleportExitTitles.
    pub exits: Vec<Place>,
    pub titles: Vec<String>,
    /// vars.TeleportIndex.
    index: i64,
    /// VisualIndicators.AddLocationIndicator at `location` with `indicator` (L18).
    pub marker: bool,
    /// HUD2.SetupTeleportHUD made the HUD (Some), and HUD2.ShowTeleportHUD's last word on it.
    pub hud: Option<bool>,
    /// Trigger.ResetTriggerVolumeState was called: INFERRED, the trigger forgets who is inside, so a skater still in
    /// the volume gets a fresh triggerentercollision. The glue clears it once it has.
    pub rearm: bool,
    /// The popup up, until it is answered.
    pub popup: Option<Popup>,
    /// Teleport.ToWorldFileLocation's exit (L10), for the glue to take.
    pub moved: Option<Place>,
}

impl Teleporter {
    /// The graph of an activated teleport row, None for any other row or a disabled one.
    #[must_use]
    pub fn for_row(data: &CareerData, name: &str) -> Option<Self> {
        let row = data.row(name).filter(|r| starts_on_activation(r) && r.graph == "teleport")?;
        Some(Teleporter {
            row: name.to_owned(),
            me: 0,
            activity: activity_id(row.key),
            // INFERRED: the export names a teleport's one tTriggerVolumeInstanceID `<row>_volume_01` (the five stage-1 rows).
            trigger: data.volumes.get(&format!("{name}_volume_01")).map(|v| v.id as i64),
            title: row.title.clone(),
            location: row.location.clone(),
            indicator: row.indicator.clone(),
            exits: row.exits.clone(),
            // shortcut: TeleportExitTitles is not exported (no row has two exits), so the list shows the exits' locator names; export them when a row does.
            titles: row.exits.iter().map(|e| e.location.clone()).collect(),
            index: 0,
            marker: false,
            hud: None,
            rearm: false,
            popup: None,
            moved: None,
        })
    }

    /// HUD2.ShowTeleportHUD (0x82852298): shows or hides it and turns its input on or off.
    fn show(&mut self, rt: &mut Rt<St>, on: bool) {
        rt.note(format!("teleporthud {on}"));
        self.hud = Some(on);
    }

    /// The HUD's `signup` component in mode 2 (0x8285222C): with its input on, a press of mask 1 (0x82666430) posts
    /// teleportersignup with the row key (0x826660F0).
    pub(crate) fn press(&mut self, rt: &mut Rt<St>, mask: u32) {
        if mask & TELEPORT_BUTTON != 0 && self.hud == Some(true) {
            self.input(rt, Input::SignUp(self.activity));
        }
    }

    /// One of the [`Input`] messages, then its state changes (as [`skate3_modes::dispatch`]).
    pub(crate) fn input(&mut self, rt: &mut Rt<St>, input: Input) {
        match (rt.state, input) {
            // L43-48: compares two f32s, so two keys that round alike both match (spec 18.27).
            (St::WaitForTeleportVolume, Input::SignUp(key)) if key == self.activity => {
                self.show(rt, false);
                rt.change(St::WaitForConfirmation);
            }
            // L63-67.
            (St::WaitForConfirmation, Input::Yes) => {
                self.popup = None;
                self.index = 0;
                rt.change(St::Teleporting);
            }
            // L69-72.
            (St::WaitForConfirmation, Input::No) => {
                self.popup = None;
                rt.change(St::WaitForTeleportVolume);
            }
            // L74-78.
            (St::WaitForConfirmation, Input::Pick(i)) => {
                self.popup = None;
                self.index = i;
                rt.change(St::Teleporting);
            }
            _ => {}
        }
        skate3_modes::settle(self, rt);
    }
}

impl Graph for Teleporter {
    type State = St;
    fn name(state: St) -> &'static str {
        match state {
            St::Initial => "initial",
            St::SetupTeleporter => "setup_teleporter",
            St::WaitForTeleportVolume => "wait_for_teleport_volume",
            St::WaitForConfirmation => "wait_for_confirmation",
            St::Teleporting => "teleporting",
        }
    }

    fn on(&mut self, rt: &mut Rt<St>, msg: &Msg) -> Handled {
        match (rt.state, msg) {
            // L5-7.
            (St::Initial, Msg::Enter) => rt.change(St::SetupTeleporter),
            // L15-21: the indicator group, the location indicator, the graph's own HUD (SetupTeleportHUD 0x82852180).
            (St::SetupTeleporter, Msg::Enter) => {
                rt.note(format!("indicator {} {}", self.location, self.indicator));
                self.marker = true;
                rt.note("teleporthud");
                self.hud = Some(false);
                rt.change(St::WaitForTeleportVolume);
            }
            // L24-26.
            (St::WaitForTeleportVolume, Msg::Enter) => {
                rt.note(format!("resettrigger {}", self.trigger.map_or_else(|| "nil".to_owned(), |v| v.to_string())));
                self.rearm = true;
            }
            // L29-40: the local player in the TeleporterTrigger volume; the 3.5 sign-up box is not used.
            (St::WaitForTeleportVolume, &Msg::Trigger { player, volume, inside }) if player == self.me && self.trigger == Some(volume) => {
                self.show(rt, inside);
            }
            // L54-61.
            (St::WaitForConfirmation, Msg::Enter) => {
                let title = self.title.clone();
                self.popup = Some(if self.exits.len() == 1 {
                    rt.note(format!("yesno {title} {CONFIRM}"));
                    Popup::YesNo { title, text: CONFIRM }
                } else {
                    rt.note(format!("list {title} {LIST_TEXT} {}", self.titles.len()));
                    Popup::List { title, text: LIST_TEXT, options: self.titles.clone() }
                });
            }
            // L9-13: TeleportExitLocation[TeleportIndex + 1]; past the end the Lua passes nil.
            (St::Teleporting, Msg::Enter) => {
                let exit = usize::try_from(self.index).ok().and_then(|i| self.exits.get(i)).cloned();
                rt.note(format!("teleport {} {}", self.me, exit.as_ref().map_or("nil", |p| p.location.as_str())));
                self.moved = exit;
                rt.change(St::WaitForTeleportVolume);
            }
            _ => {}
        }
        Ok(())
    }
}

/// The activity manager's teleports: the running Global-2 graphs (AM+0x10278's) and the stopped ones (AM+0x12154).
#[derive(Default)]
pub struct Teleports {
    pub(crate) running: BTreeMap<String, (Teleporter, Rt<St>)>,
    /// Stopped by a challenge's start, or activated while one runs; restarted at its end.
    pub stopped: Vec<String>,
    /// A challenge runs (0x82BD2200's busy test).
    pub busy: bool,
}

impl Teleports {
    /// ChallengeStatus.ActivateChallenge: the set's rebuild (0x82BD18C0) starts a newly added row's graph at once
    /// (0x82BD0538), or defers it while a challenge runs. False for a row that never runs.
    pub fn activate(&mut self, data: &CareerData, row: &str, now: f64) -> bool {
        if self.running.contains_key(row) || self.stopped.iter().any(|r| r == row) {
            return true;
        }
        let Some(t) = Teleporter::for_row(data, row) else { return false };
        if self.busy {
            self.stopped.push(row.to_owned());
        } else {
            self.run(t, now);
        }
        true
    }

    fn run(&mut self, mut t: Teleporter, now: f64) {
        let mut rt = Rt::new(St::Initial, 1, now);
        skate3_modes::start(&mut t, &mut rt);
        self.running.insert(t.row.clone(), (t, rt));
    }

    /// A removed row: its graph is reset (0x82D05CB0) and stopped.
    pub fn deactivate(&mut self, row: &str) {
        self.running.remove(row);
        self.stopped.retain(|r| r != row);
    }

    /// A challenge starts (0x82BD0E98 -> 0x82BD1E70): every running graph stops and goes on the deferred list.
    pub fn challenge_started(&mut self) {
        self.busy = true;
        self.stopped.extend(std::mem::take(&mut self.running).into_keys());
    }

    /// The challenge ends (0x82BD2090): each graph on the deferred list restarts (0x82D051A8 at 0x82BD21A4). INFERRED
    /// (spec 18.24): from its start state, so the marker and the HUD are made again.
    pub fn challenge_ended(&mut self, data: &CareerData, now: f64) {
        self.busy = false;
        for row in std::mem::take(&mut self.stopped) {
            if let Some(t) = Teleporter::for_row(data, &row) {
                self.run(t, now);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skate3_career::data::Volume;
    use crate::skate3_modes::tests::{Ev, compare, lua_log, model_log};
    use crate::skate3_modes::{dispatch, start};

    const IN: Msg = Msg::Trigger { player: 0, volume: 7, inside: true };
    const OUT: Msg = Msg::Trigger { player: 0, volume: 7, inside: false };

    fn data(rows: &[(&str, usize, bool)]) -> CareerData {
        let mut d = CareerData::default();
        for &(name, exits, disabled) in rows {
            let row = Row {
                key: 42,
                title: format!("ID_{name}"),
                kind: 45,
                graph: "teleport".into(),
                world: "dist_skateschool".into(),
                location: format!("{name}_loc"),
                indicator: "teleport".into(),
                disabled,
                global: FULL,
                signup_half: 3.5,
                exits: (0..exits).map(|i| Place { world: "dist_university".into(), location: format!("{name}_dest_{i}") }).collect(),
                ..Row::default()
            };
            d.rows.insert(name.to_owned(), row);
            let volume = Volume { map: "maps/SkateSchool.skate".into(), min: [0.; 3], max: [1.; 3], id: 7 };
            d.volumes.insert(format!("{name}_volume_01"), volume);
        }
        d
    }

    fn one(exits: usize) -> (Teleporter, Rt<St>) {
        let d = data(&[("tele_sksc_01", exits, false)]);
        let mut g = Teleporter::for_row(&d, "tele_sksc_01").unwrap();
        let mut rt = Rt::new(St::Initial, 2, 0.);
        start(&mut g, &mut rt);
        (g, rt)
    }

    #[test]
    fn activating_a_teleport_row_starts_its_graph_with_no_box_or_poster() {
        let d = data(&[("tele_sksc_01", 1, false)]);
        let mut ts = Teleports::default();
        assert!(ts.activate(&d, "tele_sksc_01", 0.));
        let (g, rt) = &ts.running["tele_sksc_01"];
        assert_eq!(rt.state, St::WaitForTeleportVolume);
        assert!(g.marker && g.rearm);
        assert_eq!((g.hud, g.trigger, g.popup.clone()), (Some(false), Some(7), None));
        // Global 2, not the sign-up manager's 3 (0x82D529A8): no box, no sign-up marker, no poster.
        assert_eq!(d.rows["tele_sksc_01"].global, FULL);
    }

    #[test]
    fn its_hud_shows_only_inside_the_trigger() {
        let (mut g, mut rt) = one(1);
        dispatch(&mut g, &mut rt, Msg::Trigger { player: 1, volume: 7, inside: true });
        dispatch(&mut g, &mut rt, Msg::Trigger { player: 0, volume: 8, inside: true });
        assert_eq!(g.hud, Some(false));
        dispatch(&mut g, &mut rt, IN);
        assert_eq!(g.hud, Some(true));
        g.press(&mut rt, 2);
        assert_eq!(rt.state, St::WaitForTeleportVolume);
        dispatch(&mut g, &mut rt, OUT);
        assert_eq!(g.hud, Some(false));
        // A hidden HUD takes no input.
        g.press(&mut rt, TELEPORT_BUTTON);
        assert_eq!(rt.state, St::WaitForTeleportVolume);
    }

    #[test]
    fn dpad_up_then_yes_moves_to_the_exit() {
        let (mut g, mut rt) = one(1);
        let other = g.activity + 1.;
        g.input(&mut rt, Input::SignUp(other));
        assert_eq!(rt.state, St::WaitForTeleportVolume);
        dispatch(&mut g, &mut rt, IN);
        g.rearm = false;
        g.press(&mut rt, TELEPORT_BUTTON);
        assert_eq!(rt.state, St::WaitForConfirmation);
        assert_eq!(g.hud, Some(false));
        assert_eq!(g.popup, Some(Popup::YesNo { title: "ID_tele_sksc_01".into(), text: CONFIRM }));
        g.input(&mut rt, Input::Yes);
        assert_eq!(g.moved, Some(Place { world: "dist_university".into(), location: "tele_sksc_01_dest_0".into() }));
        assert_eq!((rt.state, g.popup.clone()), (St::WaitForTeleportVolume, None));
        assert!(g.rearm);
    }

    #[test]
    fn several_exits_open_the_list_popup() {
        let (mut g, mut rt) = one(3);
        dispatch(&mut g, &mut rt, IN);
        g.press(&mut rt, TELEPORT_BUTTON);
        assert_eq!(g.popup, Some(Popup::List { title: "ID_tele_sksc_01".into(), text: LIST_TEXT, options: g.titles.clone() }));
        g.input(&mut rt, Input::Pick(2));
        assert_eq!(g.moved, Some(g.exits[2].clone()));
        assert_eq!(rt.state, St::WaitForTeleportVolume);
    }

    #[test]
    fn no_returns_to_waiting() {
        let (mut g, mut rt) = one(1);
        dispatch(&mut g, &mut rt, IN);
        g.press(&mut rt, TELEPORT_BUTTON);
        g.rearm = false;
        g.input(&mut rt, Input::No);
        assert_eq!((rt.state, g.moved.clone(), g.popup.clone(), g.hud), (St::WaitForTeleportVolume, None, None, Some(false)));
        assert!(g.rearm);
        // The reset trigger hands the skater still inside a fresh enter: the prompt is back.
        dispatch(&mut g, &mut rt, IN);
        assert_eq!(g.hud, Some(true));
    }

    #[test]
    fn a_challenge_start_stops_every_teleport_and_its_end_restarts_them() {
        let d = data(&[("tele_sksc_01", 1, false), ("tele_sksc_02", 1, false), ("tele_sksc_03", 1, false)]);
        let mut ts = Teleports::default();
        assert!(ts.activate(&d, "tele_sksc_01", 0.) && ts.activate(&d, "tele_sksc_02", 0.));
        {
            let (g, rt) = ts.running.get_mut("tele_sksc_02").unwrap();
            dispatch(g, rt, IN);
            g.press(rt, TELEPORT_BUTTON);
            assert_eq!(rt.state, St::WaitForConfirmation);
        }
        ts.challenge_started();
        assert!(ts.running.is_empty());
        assert_eq!(ts.stopped, ["tele_sksc_01", "tele_sksc_02"]);
        // One activated while the challenge runs waits for its end.
        assert!(ts.activate(&d, "tele_sksc_03", 1.));
        assert!(ts.running.is_empty());
        ts.challenge_ended(&d, 2.);
        assert!(ts.stopped.is_empty());
        assert_eq!(ts.running.len(), 3);
        for (g, rt) in ts.running.values() {
            assert_eq!((rt.state, g.hud, g.popup.clone()), (St::WaitForTeleportVolume, Some(false), None));
            assert!(g.marker);
        }
    }

    #[test]
    fn tele_world_to_school_never_runs() {
        let d = data(&[("tele_world_to_school", 1, true)]);
        assert!(Teleporter::for_row(&d, "tele_world_to_school").is_none());
        let mut ts = Teleports::default();
        assert!(!ts.activate(&d, "tele_world_to_school", 0.));
        ts.challenge_started();
        ts.challenge_ended(&d, 1.);
        assert!(ts.running.is_empty() && ts.stopped.is_empty());
    }

    /// teleport.lua's hashed ids: hash64("challenges/teleport/<state>[/<message>]") of these names.
    const NAMES: [(&str, &str); 18] = [
        ("Hash_0859FA23CF19CC79", "initial"),
        ("Hash_2035975BC3990877", "stateenter"),
        ("Hash_EA319E47BF5F271E", "teleporting"),
        ("Hash_CEFE6647959C5CD1", "stateenter"),
        ("Hash_B411F0AAD83DAF91", "setup_teleporter"),
        ("Hash_9F3301BEA532FB70", "stateenter"),
        ("Hash_4E4592CB48E943A3", "wait_for_teleport_volume"),
        ("Hash_C85C8DE81CE27576", "stateenter"),
        ("Hash_051BBC6B16B063BE", "triggerentercollision"),
        ("Hash_50782142D9ADC947", "triggerexitcollision"),
        ("Hash_E771F4519B1944E6", "teleportersignup"),
        ("Hash_37CB8DE322C23720", "global"),
        ("Hash_F1C6CE2B47F9E656", "challengeassetloadcomplete"),
        ("Hash_8A68F8BE1ECEB1B4", "wait_for_confirmation"),
        ("Hash_7380A9CE624B6B5E", "stateenter"),
        ("Hash_3CF962E25CD3F395", "hudyes"),
        ("Hash_5B93DFECD9CAC028", "hudno"),
        ("Hash_EDA6AC1017B69A8C", "hudpopupoption"),
    ];

    /// The natives teleport.lua calls, logging what [`Teleporter`] notes, and its messages as plan events.
    const SETUP: &str = r#"
VARS.kActivityID = 5
VisualIndicators = {CreateGroup = function() return 3 end,
  AddLocationIndicator = function(_, loc, ind) log_line("indicator " .. tostring(loc) .. " " .. tostring(ind)) end}
HUD2.SetupTeleportHUD = function() log_line("teleporthud"); return 9 end
HUD2.ShowTeleportHUD = function(_, on) log_line("teleporthud " .. tostring(on)) end
Trigger = {ResetTriggerVolumeState = function(v) log_line("resettrigger " .. tostring(v)) end}
Teleport = {ToWorldFileLocation = function(id, loc) log_line("teleport " .. id .. " " .. tostring(loc)) end}
Popup = {ShowYesNoPopup = function(t, d) log_line("yesno " .. t .. " " .. d) end}
FrontEnd.ShowGenericAttributePopup = function(t, d, titles) log_line("list " .. t .. " " .. d .. " " .. #titles) end
function DELIVER(kind, v, w)
  if kind == "enter" then SEND("triggerentercollision", {PlayerID = v, VolumeID = w})
  elseif kind == "exit" then SEND("triggerexitcollision", {PlayerID = v, VolumeID = w})
  elseif kind == "signup" then SEND("teleportersignup", {ActivityKey = v})
  elseif kind == "yes" then SEND("hudyes")
  elseif kind == "no" then SEND("hudno")
  elseif kind == "option" then SEND("hudpopupoption", {OptionIndex = v})
  else return false end
  return true
end
"#;

    /// The row's self table: TeleportExitLocation is a reflected array (`.Count`, 1-based).
    fn self_table(g: &Teleporter) -> String {
        fn quoted<'a>(v: impl Iterator<Item = &'a String>) -> Vec<String> {
            v.map(|s| format!("{s:?}")).collect()
        }
        let mut exits = quoted(g.exits.iter().map(|e| &e.location));
        exits.push(format!("Count = {}", g.exits.len()));
        let trigger = g.trigger.map_or_else(|| "nil".to_owned(), |v| v.to_string());
        format!(
            "SELF = {{Title = {:?}, Location = {:?}, SignUpIndicator = {:?}, TeleporterTrigger = {trigger}, TeleportExitLocation = {{{}}}, TeleportExitTitles = {{{}}}}}",
            g.title,
            g.location,
            g.indicator,
            exits.join(", "),
            quoted(g.titles.iter()).join(", ")
        )
    }

    fn plans(seed: u64, runs: usize) -> Vec<Vec<(f64, Ev)>> {
        let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut next = |n: u64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x % n
        };
        let mut out = Vec::new();
        for _ in 0..runs {
            let (mut run, mut t) = (Vec::new(), 0.);
            for _ in 0..1 + next(6) {
                t += 0.25 + next(20) as f64 / 10.;
                let ev = match next(10) {
                    0..=2 => Ev::Other("enter", next(2) as i64, if next(4) == 0 { 8. } else { 7. }),
                    3 => Ev::Other("exit", next(2) as i64, 7.),
                    4 | 5 => Ev::Other("signup", if next(5) == 0 { 6 } else { 5 }, 0.),
                    6 => Ev::Other("yes", 0, 0.),
                    7 => Ev::Other("no", 0, 0.),
                    _ => Ev::Other("option", next(4) as i64, 0.),
                };
                run.push((t, ev));
            }
            out.push(run);
        }
        out
    }

    /// teleport.lua itself (SKATE3_CAREER_DIR = the skate3-career export, with scripts/graphs/) decides every press,
    /// answer and trigger the model takes the same way, for every teleport row the export has and 0 to 3 exits.
    #[test]
    #[ignore = "requires the skate3-career export (SKATE3_CAREER_DIR)"]
    fn skate3_teleport_matches_teleport_lua() {
        let dir = std::path::PathBuf::from(std::env::var_os("SKATE3_CAREER_DIR").expect("set SKATE3_CAREER_DIR"));
        let lua_src = std::fs::read_to_string(dir.join("scripts/graphs/teleport.lua")).unwrap();
        let src = NAMES.iter().fold(lua_src, |s, &(hash, name)| s.replace(hash, name));
        let d = crate::skate3_career::load(&dir).unwrap();
        for row in ["tele_sksc_01", "tele_sksc_02", "tele_sksc_03", "tele_sksc_04", "tele_school_to_world"] {
            assert!(Teleporter::for_row(&d, row).is_some_and(|t| t.trigger.is_some() && t.exits.len() == 1), "{row}");
        }
        assert!(Teleporter::for_row(&d, "tele_world_to_school").is_none());
        let rows: Vec<Teleporter> = d.rows.keys().filter_map(|k| Teleporter::for_row(&d, k)).collect();
        assert!(rows.len() >= 5);
        let (mut games, mut lines) = (0, 0);
        let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
        for (i, base) in rows.iter().enumerate() {
            for seed in 1..=12u64 {
                let mut g = base.clone();
                g.me = (seed % 2) as usize;
                g.activity = 5.;
                g.trigger = (seed % 5 != 0).then_some(7);
                let n = [1, 0, 2, 3][(seed as usize + i) % 4];
                if n != 1 {
                    g.exits = (0..n).map(|k| Place { world: "w".into(), location: format!("{}_exit_{k}", g.row) }).collect();
                    g.titles = g.exits.iter().map(|e| e.location.clone()).collect();
                }
                let setup = format!("N, ME, FIRST = 2, {}, 0\n{}\n{SETUP}", g.me, self_table(&g));
                let plans = plans(seed * 977 + i as u64, 8);
                let (lua, errors) = lua_log(&src, &setup, "initial", "wait_for_teleport_volume", &plans);
                let mut rt = Rt::new(St::Initial, 2, 0.);
                let other = |g: &mut Teleporter, rt: &mut Rt<St>, kind: &str, v: i64, w: f64| {
                    let input = match kind {
                        "enter" | "exit" => return Some(Msg::Trigger { player: v as usize, volume: w as i64, inside: kind == "enter" }),
                        "signup" => Input::SignUp(v as f32),
                        "yes" => Input::Yes,
                        "no" => Input::No,
                        "option" => Input::Pick(v),
                        _ => return None,
                    };
                    g.input(rt, input);
                    None
                };
                let model = model_log(&mut g, &mut rt, St::WaitForTeleportVolume, &plans, |_| None, |_| false, other);
                compare(&format!("{} seed {seed} exits {n}", g.row), &lua, &model, &errors);
                assert!(errors.is_empty(), "{errors:?}");
                games += 1;
                lines += lua.len();
                for key in ["yesno", "list ", "teleport 0 ", "teleport 1 ", "teleporthud true", "resettrigger nil"] {
                    *seen.entry(key).or_default() += lua.iter().filter(|l| l.contains(key)).count();
                }
                // An exit past the end (a pick, or yes with no exit): ToWorldFileLocation gets nil.
                *seen.entry("teleport nil").or_default() += lua.iter().filter(|l| l.contains(" teleport ") && l.ends_with(" nil")).count();
            }
        }
        eprintln!("covered: {seen:?}");
        assert!(seen.values().all(|&n| n > 0), "every branch exercised: {seen:?}");
        eprintln!("teleport.lua oracle: {games} runs, {lines} log lines identical");
    }
}
