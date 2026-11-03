//! Learn > On Screen Help: Skate 3's Trick Analyzer (hud2/trickanalyser) and Manual Meter (hud2/manualmeter), the
//! retail movies on the shared APT runtime (apt_screen layer [`HUD_METERS`], prepared by tools/prepare_hud_meters.py)
//! driven as TU3 drives them. Skate 1 and Skate 2 have neither, so they show only under the Skate 3 HUD.
//! Evidence, addresses and open items: docs/superpowers/TRICK_ANALYSIS_MANUAL_METER_2026-10-03.md.
//!
//! - Manual Meter: driver 8255E080 (each frame), HUD 8266A220/8266A278/8266A310.
//! - Trick Analyzer: recorder 826962D8/82696030 (right-stick history and recognised gestures), trick message
//!   82693EA0 -> trace 82694260 -> mesh 8266B560, HUD 82669878/826698D8/82669950/82669A08, native 82663350.
use crate::{
    apt_scene::{self, Draw, Vertex},
    apt_screen::{HUD_METERS, Layer, Screen, Screens},
    input::gesture_input::GestureInput,
    physics::{GamePhysics, PlayerControls, SkaterRuntime},
};
use bevy::prelude::*;
use serde_json::{Value as Json, json};
use skate_core::scoring::catalog::IDENTIFIERS;
use std::{collections::VecDeque, path::Path};

/// Learn > On Screen Help, retail settings [0x8306707C] bytes 159 (Trick Analyzer) and 161 (Manual Meter), both
/// cleared by 826102A8 each boot. Kept in the game settings instead (user's choice; [`restore`]).
#[derive(Resource, Default)]
pub(crate) struct OnScreenHelp {
    pub trick_analyzer: bool,
    pub manual_meter: bool,
    /// The prepared movies are installed.
    pub available: bool,
}
impl OnScreenHelp {
    /// 8261DE88: each choice names what picking it does.
    pub fn choices(&self) -> Vec<String> {
        let ta = if self.trick_analyzer { "ID_CROSSBAR_TRICK_ANALYZER_OFF" } else { "ID_CROSSBAR_TRICK_ANALYZER_ON" };
        let mm = if self.manual_meter { "ID_CROSSBAR_MANUAL_METER_OFF" } else { "ID_CROSSBAR_MANUAL_METER_ON" };
        vec![ta.into(), mm.into()]
    }
    /// 8261EC50: a choice toggles its HUD and turns the other off.
    pub fn choose(&mut self, choice: usize) {
        match choice {
            0 => (self.trick_analyzer, self.manual_meter) = (!self.trick_analyzer, false),
            1 => (self.manual_meter, self.trick_analyzer) = (!self.manual_meter, false),
            _ => {}
        }
    }
}

// ------------------------------------------------------------------ Manual Meter
/// 8255E080's sounds, handed to the front-end player as GlobalFEPlaySound does ([[obj+12]] vtable +0x2C, (id, 0)):
/// the sweet spot found, `manual_good` (lis 0x955A / ori 0xAC0B, 0x4FF0 / 0x9C4D at 8255E264..8255E284; sk8_menu
/// sample 0x192), and a found spot lost by pushing the stick all the way, `manual_bad` (8255E2BC..; sample 0x191).
const SWEET_SPOT: u64 = 0x955A_AC0B_4FF0_9C4D;
const LOST: u64 = 0xA38A_E3D6_62E5_0818;

/// 8255E080's state: 0 off, 1 in the manual zone looking, 2 holding the sweet spot, 3 airborne with the stick
/// part-way (pre-manual).
#[derive(Default)]
struct Meter {
    state: u8,
    timer: f32,
}
impl Meter {
    /// One frame of 8255E080: `v` is the right stick's Y, up negative; `manualing` the InManualZone flag; `air`
    /// the filtered category Air. The sound it plays, if any.
    fn step(&mut self, v: f32, manualing: bool, air: bool) -> Option<u64> {
        let partway = (0. < v && v < 0.9) || (-0.9 < v && v < 0.);
        if !manualing {
            if partway && air {
                self.state = 3;
            } else {
                (self.timer, self.state) = (0., 0);
            }
            return None;
        }
        if -0.9 < v && v < 0.9 {
            match self.state {
                0 | 3 => self.state = 1,
                1 => {
                    self.timer += 1. / 60.;
                    if self.timer > 0.2 {
                        self.state = 2;
                        return Some(SWEET_SPOT);
                    }
                }
                _ => {}
            }
            return None;
        }
        let lost = (self.state == 2 && v.abs() > 0.9).then_some(LOST);
        (self.timer, self.state) = (0., 0);
        lost
    }
}

/// ManualMeterHUD (8266A310): the movie's SetState for a published state, by the stick's side.
fn meter_movie_state(state: u8, v: f32) -> u8 {
    match state {
        0 => 1,
        1 => if v < 0. { 2 } else { 4 },
        _ => if v < 0. { 3 } else { 5 },
    }
}

// ------------------------------------------------------------------ Trick Analyzer
/// 82694260's output capacity: the deepest the trace reads.
const HISTORY: usize = 200;
/// 82696030: a shown trace clears after this many frames.
const SHOWN_FRAMES: u32 = 180;
/// The trace colour (248, 148, 28), from the record 82669700 reads at construction.
const RIBBON: [f32; 4] = [248. / 255., 148. / 255., 28. / 255., 1.];

/// 82694260: the right stick's path behind a recognised gesture. `history` is newest first; `start` is how many
/// frames back to begin (`max(now - 2, frame) - frame`). Walks back until the stick sat still for 5 samples,
/// smoothing each over the 5 around it. Newest point first.
fn trace(history: &VecDeque<[f32; 2]>, start: usize) -> Vec<[f32; 2]> {
    let end = history.len().min(HISTORY);
    let (mut previous, mut still, mut points) = ([0f32; 2], 0, Vec::new());
    for i in start..end {
        let s = history[i];
        if (s[0] - previous[0]).abs() < 0.01 && (s[1] - previous[1]).abs() < 0.01 {
            still += 1;
            if still >= 5 {
                break;
            }
        }
        previous = s;
        let mut sum = [0f32; 2];
        for k in i as isize - 2..=i as isize + 2 {
            let k = if k < start as isize { start } else if k >= end as isize { end - 1 } else { k as usize };
            sum = [sum[0] + history[k][0], sum[1] + history[k][1]];
        }
        points.push([sum[0] * 0.2, sum[1] * 0.2]);
    }
    points
}

/// 8266B560: the ribbon's vertices in the analyzer's space, drawn oldest point first, 2 px either side; none for
/// fewer than 2 points. 4 per segment: a QUADLIST.
fn mesh(points: &[[f32; 2]]) -> Vec<[f32; 2]> {
    if points.len() <= 1 {
        return Vec::new();
    }
    let side = |p: [f32; 2], d: f32| [p[0] * 50. + d, p[1] * 50.];
    let last = points[points.len() - 1];
    let mut out = vec![side(last, -2.), side(last, 2.)];
    for (i, &p) in points[..points.len() - 1].iter().enumerate().rev() {
        out.extend([side(p, -2.), side(p, 2.)]);
        if i != 0 {
            out.extend([side(p, 2.), side(p, -2.)]);
        }
    }
    out
}

/// 8266B560's gesture art: tTrickGuideInputSlot item 0's index into 0x830273C8 (58 and 59 name none).
pub(crate) const GESTURES: [&str; 58] = [
    "", "grind_5_0_Grind", "grind_fs_5_0", "grind_fs_bluntslide", "grind_fs_boardslide", "grind_fs_crooks",
    "grind_fs_lipslide", "grind_fs_noseblunt", "grind_fs_overcrooks", "grind_fs_salad", "grind_fs_noseslide",
    "grind_fs_tailslide", "grind_nose_grind", "grind_fs_feeble", "grind_fs_smith", "grind_fs_willie",
    "grind_fs_overwillie", "nollie", "nollie_360_flip", "nollie_360_hardflip", "nollie_360_heelflip",
    "nollie_360_inward_heel", "nollie_360popshuvit", "nollie_fs_360popshuvit", "nollie_fs_popshuvit",
    "nollie_hardflip", "nollie_heelflip", "nollie_inward_heelflip", "nollie_kickflip", "nollie_popshuvit",
    "nollie_varial_flip", "nollie_varial_heelflip", "nollie_fs_popshuvit_late", "nollie_popshuvit_late",
    "nollie_underflip_shuv", "nollie_underflip_heelflip", "nollie_underflip_kickflip", "ollie", "ollie_360_flip",
    "ollie_360_hardflip", "ollie_360_heelflip", "ollie_360_inward_heelflip", "ollie_360popshuvit",
    "ollie_fs_360popshuvit", "ollie_fs_popshuvit", "ollie_hardflip", "ollie_heelflip", "ollie_inward_heelflip",
    "ollie_kickflip", "ollie_popshuvit", "ollie_varial_heelflip", "ollie_varial_kickflip",
    "ollie_underflip_kickflip", "ollie_underflip_shuv", "ollie_fs_popshuvit_late", "ollie_popshuvit_late",
    "ollie_underflip_heelflip", "revert",
];
/// The scorable vault class and its tTrickGuideInputSlot array.
const SCORABLE: &str = "Hash_6918469984A8C596";
const GUIDE_SLOT: &str = "Hash_03ACC9F200807B87";

/// Each scorable's gesture art name (None without a record, which leaves the shown art unchanged).
pub(crate) fn guides(root: &Path) -> Result<Vec<Option<&'static str>>, String> {
    let data = skate_data::collections::Collections::shared(root)?; // perf 10-18: one parse per process
    Ok(IDENTIFIERS
        .iter()
        .map(|(identifier, _, _)| {
            let field = data.field(SCORABLE, identifier, GUIDE_SLOT).ok()?;
            let item = field.array.as_ref()?.items.first()?;
            let index = u32::from_str_radix(item.get(..8)?, 16).ok()? as usize;
            Some(GESTURES.get(index).copied().unwrap_or(""))
        })
        .collect())
}

/// TrickAnalyzer recorder (826962D8's +4592.. slots) and its HUD component (8266B4D8: +12..+24), and the HUD's
/// own animation (+52 segments, +56 half-step, +60 finished).
#[derive(Default)]
struct Analyzer {
    history: VecDeque<[f32; 2]>,
    /// Recognised right-stick gestures since the last clear: (pattern, frame).
    slots: [Option<(String, u64)>; 16],
    displayed: bool,
    counter: u32,
    gesture: String,
    stance: bool,
    vertices: Vec<[f32; 2]>,
    dirty: bool,
    segments: usize,
    half: u8,
    finished: bool,
}
impl Analyzer {
    /// 826962D8 for the right stick: its deadzoned sample, and what its recognisers matched.
    fn record(&mut self, sample: [f32; 2], matched: &[String], frame: u64) {
        self.history.push_front(sample);
        self.history.truncate(HISTORY);
        for name in matched {
            if self.displayed {
                self.clear();
            }
            if let Some(slot) = self.slots.iter_mut().find(|s| s.is_none()) {
                *slot = Some((name.clone(), frame));
            }
        }
    }
    /// 82696030 after the input: a shown trace stays 180 frames.
    fn tick(&mut self) {
        if self.displayed {
            if self.counter >= SHOWN_FRAMES {
                self.clear();
            } else {
                self.counter += 1;
            }
        }
    }
    /// 82694A30, with the component's Clear.
    fn clear(&mut self) {
        (self.displayed, self.counter) = (false, 0);
        self.slots = Default::default();
        self.vertices.clear();
        self.gesture.clear();
        self.stance = false;
        self.dirty = true;
    }
    /// 82693EA0: a begun (announced) trick shows the first recorded gesture that maps to it.
    fn trick(&mut self, identifier: &str, now: u64, guide: Option<&str>, stance: bool) {
        let maps = |pattern: &str| {
            crate::input::gesture_mapping::rows()
                .any(|r| r.0 == pattern && (r.1.eq_ignore_ascii_case(identifier) || r.2.eq_ignore_ascii_case(identifier)))
        };
        let Some(frame) = self.slots.iter().flatten().find(|(pattern, _)| maps(pattern)).map(|s| s.1) else { return };
        let start = (now.saturating_sub(2).max(frame) - frame) as usize;
        let points = trace(&self.history, start);
        // 8266B560: the art only from a record; the old ribbon stays when there is no new one.
        self.stance = stance;
        if let Some(guide) = guide {
            self.gesture = guide.into();
        }
        if points.len() > 1 {
            self.vertices = mesh(&points);
        }
        self.dirty = true;
        self.displayed = true;
    }
    /// 82669950 then 82669A08, one frame while shown: SetState calls for the movie, and the vertices to draw.
    fn frame(&mut self) -> (bool, usize) {
        let mut set_state = false;
        if std::mem::take(&mut self.dirty) {
            set_state = self.finished;
            (self.half, self.segments, self.finished) = (0, 1, false);
        }
        if self.vertices.is_empty() {
            return (set_state, 0);
        }
        if self.half < 1 {
            self.half += 1;
        } else {
            (self.half, self.segments) = (0, self.segments + 1);
        }
        let shown = self.segments * 4;
        if shown == self.vertices.len() {
            self.finished = true;
            set_state = true;
        }
        (set_state, shown.min(self.vertices.len()))
    }
    /// 82669878 / 826698D8.
    fn reset_hud(&mut self) {
        (self.half, self.segments, self.finished) = (0, 1, false);
    }
}

/// The ribbon's first `count` vertices as quads (0,1,2)(0,2,3) on the stage, by the analyzer clip's `matrix`.
fn ribbon(vertices: &[[f32; 2]], count: usize, matrix: [f32; 6]) -> Draw {
    let at = |p: [f32; 2]| Vertex { position: apt_scene::apply(matrix, p), uv: [0.5; 2] };
    let vertices = vertices[..count - count % 4]
        .chunks(4)
        .flat_map(|q| [q[0], q[1], q[2], q[0], q[2], q[3]])
        .map(at)
        .collect();
    Draw { texture: "runtime/white.rgba".into(), size: [1, 1], vertices, multiply: RIBBON, add: [0.; 4] }
}

// ------------------------------------------------------------------ Bevy side
#[derive(Resource, Default)]
struct HudMeters {
    meter: Meter,
    /// ManualMeterHUD: the published v / state, its changed flag (+20), and the movie state last set (+52).
    meter_v: f32,
    meter_state: u8,
    meter_changed: bool,
    meter_shown: u8,
    analyzer: Analyzer,
    /// The right stick's recognisers, fed what the physics' own see (theirs are private to PlayerControls).
    gestures: Option<GestureInput>,
    guides: Vec<Option<&'static str>>,
    frame: u64,
    /// The movies are showing (Show sent).
    showing: [bool; 2],
    failed: bool,
}

pub(crate) struct HudMetersPlugin;
impl Plugin for HudMetersPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<OnScreenHelp>()
            .init_resource::<HudMeters>()
            .add_systems(Startup, |mut help: ResMut<OnScreenHelp>, config: Res<crate::config::Config>| {
                let root = config.asset_root.join("private/hud-meters/runtime");
                help.available = ["trickanalyser", "manualmeter"].iter().all(|m| root.join(format!("{m}.json")).is_file());
            })
            .add_systems(Update, restore.run_if(resource_added::<crate::graphics_menu::Menu>))
            .add_systems(
                FixedUpdate,
                update.after(crate::app::SimulationSet::Physics).run_if(crate::graphics_menu::gameplay_active),
            );
    }
}

/// The saved On Screen Help choice, once the settings are loaded.
fn restore(menu: Res<crate::graphics_menu::Menu>, mut help: ResMut<OnScreenHelp>) {
    [help.trick_analyzer, help.manual_meter] = menu.on_screen_help();
}

const MOVIES: [&str; 2] = ["trickanalyser", "manualmeter"];

#[allow(clippy::too_many_arguments)]
fn update(
    mut meters: ResMut<HudMeters>,
    help: Res<OnScreenHelp>,
    skater: Option<Res<SkaterRuntime>>,
    controls: Option<Res<PlayerControls>>,
    physics: Option<Res<GamePhysics>>,
    screens: Option<ResMut<Screens>>,
    menu: Option<Res<crate::graphics_menu::Menu>>,
    map: Option<Res<crate::map_transition::CurrentMap>>,
    config: Res<crate::config::Config>,
    mut sounds: MessageWriter<crate::skate_audio::FrontEndSoundRequest>,
    replay: Option<Res<crate::replay::hud::ReplayHud>>,
) {
    let (Some(skater), Some(controls), Some(physics), Some(mut screens)) = (skater, controls, physics, screens) else { return };
    let m = &mut *meters;
    if m.failed || !help.available {
        return;
    }
    let words = controls.controller.words();
    let stick = [f32::from_bits(words[9]), f32::from_bits(words[10])];
    let input = &skater.player_input;
    // ponytail: the block byte [[obj+28]+75] both drivers check is not modelled (always clear).
    if help.manual_meter {
        let v = -stick[1];
        let manualing = input.processed.flags_2484 & 0x2000 != 0;
        let air = input.physical.filtered_state_0 == 2;
        if let Some(sound) = m.meter.step(v, manualing, air) {
            sounds.write(crate::skate_audio::FrontEndSoundRequest(sound));
        }
        m.meter_v = v;
        if m.meter.state != m.meter_state {
            (m.meter_state, m.meter_changed) = (m.meter.state, true);
        }
    }
    m.frame += 1;
    let sample = [stick[0], -stick[1]].map(|v| if v.abs() < 0.1 { 0. } else { v });
    let mut matched = Vec::new();
    if help.trick_analyzer {
        if m.gestures.is_none() {
            match GestureInput::load(&config.asset_root).and_then(|g| Ok((g, guides(&config.asset_root)?))) {
                Ok((g, guides)) => (m.gestures, m.guides) = (Some(g), guides),
                Err(error) => {
                    warn!("Trick Analyzer unavailable: {error}");
                    m.failed = true;
                    return;
                }
            }
        }
        let gestures = m.gestures.as_mut().unwrap();
        let mut scratch = skate_core::graph::intents::IntentMap::new();
        let axes = [[words[7], words[8]], [words[9], words[10]]].map(|p| p.map(f32::from_bits));
        gestures.publish(axes, physics.difficulty_index(), controls.actor_flags, input.physical.state.state_16, &mut scratch);
        matched = std::mem::take(&mut gestures.recognized);
    }
    let frame = m.frame;
    m.analyzer.record(sample, &matched, frame);
    m.analyzer.tick();
    // cMsgTrickBegun (0x714CAED3; its name, fields and registration are at 822F1EF0 / 823A1CC8 / 8268EB04): 82DF0878
    // sends one for each carrier 82DA5DE0 announced this tick, i.e. the scoring runtime's `announced`. Not the
    // repetition count, which only rises when the carrier ends (an air trick's at landing): that showed the trace late.
    if help.trick_analyzer {
        let stance = !skater.animation.goofy() != skater.animation.packet.riding_switch;
        for &identifier in &skater.scoring.announced {
            let guide = IDENTIFIERS.iter().position(|i| i.0 == identifier).and_then(|id| m.guides.get(id).copied().flatten());
            m.analyzer.trick(identifier, frame, guide, stance);
        }
    }

    let skate3 = map.as_deref().is_none_or(|map| crate::skate2_hud::hud_game(menu.as_deref(), map) == 3);
    // Retail shows neither over the Replay Editor (user 10-27).
    let visible = skate3 && menu.as_deref().is_none_or(|m| m.hud_visible()) && !replay.is_some_and(|r| r.open());
    let layer = screens.layer(HUD_METERS);
    let result = drive(layer, m, [help.trick_analyzer && visible, help.manual_meter && visible]);
    if let Err(error) = result {
        error!("On Screen Help stopped: {error}");
        layer.open.clear();
        layer.extra.remove("ta-ribbon");
        m.failed = true;
    }
}

/// Shows / hides the two movies and runs their HUD components for one frame.
fn drive(layer: &mut Layer, m: &mut HudMeters, show: [bool; 2]) -> Result<(), String> {
    for (i, movie) in MOVIES.iter().enumerate() {
        if show[i] && layer.get(movie).is_none() {
            layer.open(movie, movie, &[])?;
        }
        let Some(screen) = layer.get(movie) else { continue };
        if show[i] != m.showing[i] {
            m.showing[i] = show[i];
            match (i, show[i]) {
                // 82669878 / 826698D8.
                (0, on) => {
                    screen.call("SetState", &[json!(if on { 1 } else { 2 })])?;
                    m.analyzer.reset_hud();
                }
                // 8266A220 / 8266A278.
                (_, true) => {
                    screen.call("SetState", &[json!(7)])?;
                    m.meter_shown = 7;
                }
                (_, false) => {
                    if m.meter_shown != 6 {
                        screen.call("SetState", &[json!(6)])?;
                    }
                    m.meter_shown = 0;
                }
            }
        }
    }
    if m.showing[1] {
        if let Some(screen) = layer.get(MOVIES[1]) {
            meter_frame(screen, m)?;
        }
    }
    layer.extra.remove("ta-ribbon");
    if let Some(screen) = layer.open.get_mut(MOVIES[0]).filter(|_| m.showing[0]) {
        if let Some(draw) = analyzer_frame(screen, &mut m.analyzer)? {
            layer.extra.insert("ta-ribbon", vec![draw]);
        }
    }
    Ok(())
}

/// 82669950 + 82669A08, one frame of the shown analyzer: the movie told what changed, and the ribbon drawn so far
/// (in mTrickAnalyzer's place, while it shows).
fn analyzer_frame(screen: &mut Screen, a: &mut Analyzer) -> Result<Option<Draw>, String> {
    screen.answer("HUDComponents.TrickAnalyzer_GetTrickData", json!([a.gesture, a.stance]));
    let (set_state, count) = a.frame();
    if set_state {
        screen.call("SetState", &[json!(3)])?;
    }
    let clip = screen.clip("mTrickAnalyzer").ok_or("trickanalyser: no mTrickAnalyzer")?;
    // Its authored fill is a solid blue 100x100 placeholder for the native ribbon (the stick's ±1 at x50), which
    // would cover mBacker's stick gate entirely; like apt_scene's image_renderer placeholders, it isn't drawn.
    screen.vm.set(clip, "_alpha", crate::apt_vm::Value::Number(0.))?;
    Ok((count >= 4 && screen.vm.get(clip, "_visible").truth()).then(|| ribbon(&a.vertices, count, screen.stage_matrix(clip))))
}

/// 8266A310, one frame of a shown meter: a changed state picks the movie's look, then the arrow follows the stick.
fn meter_frame(screen: &mut Screen, m: &mut HudMeters) -> Result<(), String> {
    if std::mem::take(&mut m.meter_changed) {
        let want = meter_movie_state(m.meter_state, m.meter_v);
        if m.meter_shown != want {
            screen.call("SetState", &[json!(want)])?;
            m.meter_shown = want;
        }
    }
    let percent = ((m.meter_v * 100.) as i32).to_string();
    screen.call_at("_global.MeterForManualing", "Update", &[Json::from(percent)]).map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn on_screen_help_is_one_or_the_other() {
        let mut help = OnScreenHelp::default();
        assert_eq!(help.choices(), ["ID_CROSSBAR_TRICK_ANALYZER_ON", "ID_CROSSBAR_MANUAL_METER_ON"]);
        help.choose(0);
        assert!(help.trick_analyzer && !help.manual_meter);
        assert_eq!(help.choices(), ["ID_CROSSBAR_TRICK_ANALYZER_OFF", "ID_CROSSBAR_MANUAL_METER_ON"]);
        help.choose(1);
        assert!(!help.trick_analyzer && help.manual_meter);
        help.choose(1);
        assert!(!help.trick_analyzer && !help.manual_meter);
    }

    #[test]
    fn manual_meter_sounds_are_the_vaults_manual_records() {
        use crate::core_menu_runtime::fe_sound_id;
        assert_eq!((SWEET_SPOT, LOST), (fe_sound_id("manual_good"), fe_sound_id("manual_bad")));
    }

    #[test]
    fn manual_meter_finds_and_loses_the_sweet_spot() {
        let mut m = Meter::default();
        // Airborne, stick part-way: pre-manual.
        assert_eq!(m.step(-0.5, false, true), None);
        assert_eq!(m.state, 3);
        // Landed in the manual zone: looking, then 0.2 s on the spot finds it.
        m.step(-0.5, true, false);
        assert_eq!(m.state, 1);
        let found = (0..20).find_map(|_| m.step(-0.5, true, false));
        assert_eq!((found, m.state), (Some(SWEET_SPOT), 2));
        // Pushed all the way: lost, with its sound.
        assert_eq!(m.step(-1.0, true, false), Some(LOST));
        assert_eq!(m.state, 0);
        // Exactly 0.9 is not "past" 0.9: reset, silently.
        m.state = 2;
        assert_eq!(m.step(0.9, true, false), None);
        // Grounded off the zone, or a centred stick in the air: off.
        assert_eq!((m.step(0., false, true), m.state), (None, 0));
        assert_eq!([0, 1, 2, 3].map(|s| meter_movie_state(s, -0.3)), [1, 2, 3, 3]);
        assert_eq!([0, 1, 2, 3].map(|s| meter_movie_state(s, 0.3)), [1, 4, 5, 5]);
    }

    #[test]
    fn trace_walks_back_to_where_the_stick_rested() {
        // Newest first: a flick, then the stick at rest for longer than 5 samples.
        let mut history: VecDeque<[f32; 2]> = [[0., -1.], [0., -0.5], [0., 0.5], [0., 1.]].into();
        history.extend(std::iter::repeat_n([0., 0.], 10));
        let points = trace(&history, 0);
        // 4 moving samples, then rest samples until 5 in a row read still (the first rest one moved).
        assert_eq!(points.len(), 4 + 1 + 4);
        // 5-tap mean, clamped at the start: (s0*3 + s1 + s2) / 5.
        assert!((points[0][1] - (-1. * 3. - 0.5 + 0.5) * 0.2).abs() < 1e-6, "{points:?}");
        // The ribbon: oldest point first, 4 vertices a segment.
        let v = mesh(&points[..3]);
        assert_eq!(v.len(), 4 * 3 - 4);
        let p = |i: usize, d: f32| [points[i][0] * 50. + d, points[i][1] * 50.];
        assert_eq!(v, [p(2, -2.), p(2, 2.), p(1, -2.), p(1, 2.), p(1, 2.), p(1, -2.), p(0, -2.), p(0, 2.)]);
        assert!(mesh(&points[..1]).is_empty());
    }

    #[test]
    fn analyzer_shows_the_gesture_behind_a_finished_trick() {
        let mut a = Analyzer::default();
        for i in 0..10 {
            a.record([0., 0.], &[], i);
        }
        // The flick: down then up; the recogniser reports Kickflip on the last sample.
        for (i, y) in [0.5f32, 1., 0.5, -0.5, -1.].into_iter().enumerate() {
            let matched = if i == 4 { vec!["Kickflip".to_string()] } else { vec![] };
            a.record([0.3, y], &matched, 10 + i as u64);
        }
        // A trick the gesture doesn't map to shows nothing.
        a.trick("ollie", 20, Some("ollie"), true);
        assert!(!a.displayed);
        // Kickflip's mirrored intent is Heelflip: either finishes it.
        a.trick("heelflip", 16, Some("ollie_heelflip"), true);
        assert!(a.displayed && a.gesture == "ollie_heelflip" && a.stance);
        assert!(a.vertices.len() >= 8 && a.vertices.len() % 4 == 0);
        // The HUD adds a segment every 2 frames; the frame it completes asks the movie for the art (SetState 3).
        let quads = a.vertices.len() / 4;
        let mut frames = Vec::new();
        for _ in 0..quads * 2 + 4 {
            frames.push(a.frame());
        }
        assert_eq!(frames[0], (false, 4));
        assert_eq!(frames[1], (false, 8.min(a.vertices.len())));
        let completes: Vec<usize> = frames.iter().enumerate().filter(|f| f.1.0).map(|f| f.0).collect();
        assert_eq!(completes, [2 * quads - 3, 2 * quads - 2], "SetState 3 on both frames the count matches");
        // 180 frames later it clears, and the next frame tells the movie (gestureOff).
        for _ in 0..=SHOWN_FRAMES {
            a.tick();
        }
        assert!(!a.displayed && a.vertices.is_empty() && a.gesture.is_empty());
        assert_eq!(a.frame(), (true, 0));
        // The clear dropped the recorded gesture.
        a.trick("kickflip", 220, None, false);
        assert!(!a.displayed);
        // A new gesture while one shows clears it first.
        a.record([0.3, -1.], &["Kickflip".to_string()], 230);
        a.trick("kickflip", 231, None, false);
        assert!(a.displayed);
        a.record([0., 0.], &["Heelflip".to_string()], 240);
        assert!(!a.displayed && a.slots[0].as_ref().is_some_and(|s| s.0 == "Heelflip") && a.slots[1].is_none());
    }

    /// The prepared movies (SKATE3_ASSET_ROOT/private/hud-meters).
    fn root() -> Option<PathBuf> {
        let root = PathBuf::from(std::env::var_os("SKATE3_ASSET_ROOT")?).join("private/hud-meters");
        root.join("runtime/trickanalyser.json").is_file().then_some(root)
    }
    fn shoot(draws: &[Draw], root: &Path, name: &str) {
        if let Some(out) = std::env::var_os("SKATE_CORE_MENU_OUT").map(PathBuf::from) {
            crate::core_menu_runtime::tests::raster_draws(draws, root, &out.join(format!("{name}.png")));
        }
    }
    fn run(screen: &mut Screen, frames: usize) {
        (0..frames).for_each(|_| screen.advance().unwrap());
    }
    fn number(screen: &Screen, clip: usize, key: &str) -> f64 {
        screen.vm.get(clip, key).number()
    }

    #[test]
    fn manual_meter_movie_follows_the_stick() {
        let Some(root) = root() else { return };
        let mut screen = Screen::load(&root.join("runtime/manualmeter.json"), &[]).unwrap();
        run(&mut screen, 2);
        let arrow = screen.clip("mArrow").expect("mArrow");
        let rest = number(&screen, arrow, "_y");
        screen.call("SetState", &[json!(7)]).unwrap();
        run(&mut screen, 30);
        for (state, name) in [(1, "all-off"), (2, "nose-rollover"), (3, "nose-on"), (4, "tail-rollover"), (5, "tail-on")] {
            screen.call("SetState", &[json!(state)]).unwrap();
            let v = if state <= 3 { -50 } else { 50 };
            screen.call_at("_global.MeterForManualing", "Update", &[json!(v.to_string())]).unwrap();
            run(&mut screen, 20);
            assert!((number(&screen, arrow, "_y") - (rest + f64::from(v) * 0.8)).abs() < 1e-3, "{name}");
            assert!(!screen.draws().unwrap().is_empty(), "{name}");
            shoot(&screen.draws().unwrap(), &root, &format!("manual-meter-{name}"));
        }
        screen.call("SetState", &[json!(6)]).unwrap();
        run(&mut screen, 60);
        assert!(screen.bindings.unanswered.is_empty(), "{:?}", screen.bindings.unanswered);
    }

    #[test]
    fn trick_analyzer_movie_traces_and_shows_the_gesture() {
        let Some(root) = root() else { return };
        let mut screen = Screen::load(&root.join("runtime/trickanalyser.json"), &[]).unwrap();
        let mut a = Analyzer::default();
        for i in 0..10 {
            a.record([0., 0.], &[], i);
        }
        // A kickflip flick: down, then up and across.
        let flick = [[0., 0.4], [0., 0.9], [0.2, 0.6], [0.4, 0.], [0.5, -0.6], [0.4, -0.9]];
        for (i, p) in flick.into_iter().enumerate() {
            let matched = if i == flick.len() - 1 { vec!["Kickflip".to_string()] } else { vec![] };
            a.record(p, &matched, 10 + i as u64);
        }
        a.trick("kickflip", 16, Some("ollie_kickflip"), true);
        let mut layer_draws = Vec::new();
        screen.call("SetState", &[json!(1)]).unwrap();
        a.reset_hud();
        for _ in 0..60 {
            let draw = analyzer_frame(&mut screen, &mut a).unwrap();
            screen.advance().unwrap();
            layer_draws = screen.draws().unwrap();
            layer_draws.extend(draw);
        }
        assert!(a.finished);
        let gesture = screen.clip("mGestureAnim").expect("mGestureAnim");
        assert_eq!(screen.vm.get(gesture, "_visible"), crate::apt_vm::Value::Bool(true));
        shoot(&layer_draws, &root, "trick-analyzer-kickflip");
        assert!(screen.bindings.unanswered.is_empty(), "{:?}", screen.bindings.unanswered);
    }

    #[test]
    #[ignore = "requires the installed stock vault"]
    fn guides_name_the_gesture_art() {
        let root = PathBuf::from(std::env::var_os("SKATE3_ASSET_ROOT").unwrap());
        let guides = guides(&root).unwrap();
        let of = |id: &str| guides[IDENTIFIERS.iter().position(|i| i.0 == id).unwrap()];
        assert_eq!(
            ["kickflip", "heelflip", "360flip", "n_kickflip", "ollie", "nosemanual"].map(of),
            [Some("ollie_kickflip"), Some("ollie_heelflip"), Some("ollie_360_flip"), Some("nollie_kickflip"), Some("ollie"), Some("")]
        );
    }
}
