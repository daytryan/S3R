//! Retail controller rumble (research: `.superpowers/sdd/2026-10-04-controllers/research.md` §2).
//!
//! Each physics observation runs the stock `rumble` vault entries through the evaluator
//! `0x8259DFB0` (predicate, input range, decay/low/high graphs, surface multipliers, intensity) and
//! stores the results per channel as the handler `0x82694D38` does; the 60 Hz mixer `0x82695010`
//! sums the channels into the two motors and decays them. The PhysOut conditioner (slot B+40) the
//! skeleton and landing entries read is the audio port's own (`audio_state::Conditioner`).
//!
//! Stock-enabled entries left off because the port does not publish their input: channel 13
//! (`Hash_17D814B0…`, off-board weight blend `0x8259D128`), 15 (`offboard_landing`, off-board
//! landed flag `[sk+72]+320` and velocity `+64`), and the wheel entry's periodic crack pass
//! (`Hash_3AEF41AC…`, class `E9CC3355…`). Contact type 60 (context byte 14) is not published
//! either and reads as false.
use bevy::prelude::*;
use skate_data::collections::Collections;
use std::path::Path;
use std::sync::mpsc::{Receiver, channel};

use super::audio_state::{Conditioner, ConditionerOutput};
use super::components::contacts::SurfaceMap;
use super::tuning::AudioTuning;
use crate::skate_audio::{PlayerAudioObservation, RetailAudioInputs};

const CLASS: &str = "rumble";
const INFO: &str = "Hash_411498F11DA46A57";
/// tRumbleInfo fields of the wheel-surface table, by surface class (`0x8259E660` jump table).
const SURFACE_CLASSES: [&str; 13] = [
    "Hash_5DEC62F9E5E532B6", "Hash_448B4190EB67753C", "Hash_11B301792AA327D2", "Hash_66A8B24B61FD7894",
    "Hash_EE74DBD90352604C", "Hash_EBA3C72594D3EF76", "Hash_949B9058C735828C", "Hash_E2CA9346FBF5538D",
    "Hash_33B07FFAF42E4D48", "Hash_67BF6E8BBAE77B7E", "Hash_383B43610B3E7C36", "Hash_F57ACDC7FF105337",
    "Hash_9D81A32199351A39",
];
/// tRumbleInfo fields of the `grind` table, by grind class (`0x8259EAC8` jump table; class 14 is
/// also material 0's).
const GRIND_CLASSES: [&str; 15] = [
    "Hash_98E215BA1887238B", "Hash_C9074F3D0B68C324", "Hash_6535508B9BBD9567", "Hash_DDF83883AEA80D48",
    "Hash_177238192805D0A6", "Hash_55F2D3456F0E9A73", "Hash_FBF8AF1E08EE6371", "Hash_B94061697CE751A8",
    "Hash_64B4E771B55FB8B1", "Hash_46577EB4C4763897", "Hash_416D21D9CDFF5761", "Hash_B4F3137132B4F6EE",
    "Hash_6EF69C4E6C0418EF", "Hash_3F18199E444B8785", "Hash_63DCE27AF4C0C3C1",
];
/// tRumbleInfo fields of the `physics` table, by Collision+16 class 0..7 (`0x8259E8F8` jump table).
const PHYSICS_CLASSES: [&str; 8] = [
    "Hash_E981DBBDF67ECAF3", "Hash_BFE8C265A2A6D17D", "Hash_22B4F2FC54FEB29B", "Hash_A5EC4468351C7BC5",
    "Hash_FAE0C51871300C04", "Hash_4F3ADA04CC30A3C6", "Hash_602710258D5C8E9C", "Hash_A7A0F3662068AD7D",
];

/// What an entry's predicate, input and surface lookup read (the vtables at `0x82300334 + 0x18·i`).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Source {
    /// Always; Σ of the eight body-contact impacts at B40+92.
    Skeleton,
    /// Ground or Air collector live and wheels newly landed this frame; Σ of their impacts (B40+52 / +68).
    Landing,
    /// Feet on the board and wheels down; |deck X · velocity| × 0.1 (≤ 1); the physics surface.
    Powerslide,
    /// Feet on the board and wheels down; ground speed (Motion+164); wheel 0's surface.
    Wheel,
    /// Feet on the board and a truck touching (Collision 3473/3474); ground speed; Grinds+216 surface.
    Truck,
    /// Feet on the board and the deck touching (Collision 3475); ground speed; Grinds+216 surface.
    Deck,
    /// State flag 52 or 53; ground speed; the physics surface.
    FootBrake,
    /// State flag 54; ground speed; the physics surface.
    DeckBrake,
}

/// (vault key, channel, source) of every stock-enabled entry the port can drive.
// UNTRACED: the wheel crack pass (8259CD78 -> 8259EDA8/8258FDA8), ch13 (8259D118/8259D128) and
// ch15 (8259D350/8259D3A0) are not ported.
const EVENTS: [(&str, u8, Source); 8] = [
    ("skeleton", 0, Source::Skeleton),
    ("landing", 2, Source::Landing),
    ("powerslide", 3, Source::Powerslide),
    ("wheel", 6, Source::Wheel),
    ("truck", 7, Source::Truck),
    ("deck", 8, Source::Deck),
    ("foot_brake", 9, Source::FootBrake),
    ("deck_brake", 10, Source::DeckBrake),
];

/// An 8-point graph (`0x82481E10`): piecewise linear, clamped at both ends.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Graph {
    x: [f32; 8],
    y: [f32; 8],
}

impl Graph {
    fn at(&self, t: f32) -> f32 {
        if t <= self.x[0] {
            return self.y[0];
        }
        for i in 1..8 {
            if t <= self.x[i] {
                let span = self.x[i] - self.x[i - 1];
                let f = if span > 0.0 { (t - self.x[i - 1]) / span } else { 1.0 };
                return self.y[i - 1] + (self.y[i] - self.y[i - 1]) * f;
            }
        }
        self.y[7]
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Entry {
    channel: u8,
    source: Source,
    enabled: bool,
    use_surface: bool,
    use_low_graph: bool,
    use_high_graph: bool,
    input_min: f32,
    input_max: f32,
    intensity: f32,
    decay: Graph,
    high: Graph,
    low: Graph,
}

impl Entry {
    /// `0x8259DFB0` after the predicate: (low, high, decay) for input `x`, or nothing.
    fn evaluate(&self, x: f32, surface: [f32; 2]) -> Option<(f32, f32, f32)> {
        if !self.enabled || !(x > self.input_min) {
            return None;
        }
        let t = ((x - self.input_min) / (self.input_max - self.input_min)).min(1.0);
        let decay = self.decay.at(t);
        let mut high = if self.use_high_graph { self.high.at(t) } else { t };
        let mut low = if self.use_low_graph { self.low.at(t) } else { t };
        if self.use_surface {
            high *= surface[0];
            low *= surface[1];
        }
        let (low, high) = (low * self.intensity, high * self.intensity);
        (low != 0.0 || high != 0.0).then_some((low, high, decay))
    }
}

struct Table {
    entries: Vec<Entry>,
    /// [high, low] multipliers per wheel-surface class and per grind class.
    wheel: [[f32; 2]; 13],
    grind: [[f32; 2]; 15],
    physics: [[f32; 2]; 8],
    surfaces: SurfaceMap,
}

impl Table {
    fn load(data: &Collections) -> Result<Self, String> {
        let graph = |key: &str, name: &str| -> Result<Graph, String> {
            let w = data.words::<20>(CLASS, key, name)?;
            Ok(Graph { x: std::array::from_fn(|i| f32::from_bits(w[4 + i])), y: std::array::from_fn(|i| f32::from_bits(w[12 + i])) })
        };
        let entries = EVENTS
            .iter()
            .map(|&(key, channel, source)| {
                Ok(Entry {
                    channel,
                    source,
                    enabled: data.boolean(CLASS, key, "enabled")?,
                    use_surface: data.boolean(CLASS, key, "use_surface_rumble")?,
                    use_low_graph: data.boolean(CLASS, key, "Hash_BFC317E55D999647")?,
                    use_high_graph: data.boolean(CLASS, key, "Hash_7EB32E0A819DBF81")?,
                    input_min: data.float(CLASS, key, "input_min")?,
                    input_max: data.float(CLASS, key, "input_max")?,
                    intensity: data.float(CLASS, key, "intensity_scale")?,
                    decay: graph(key, "decay")?,
                    high: graph(key, "high_freq_graph")?,
                    low: graph(key, "low_freq_graph")?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let info = |key: &str, field: &str| -> Result<[f32; 2], String> {
            let w = data.words::<3>(INFO, key, field)?;
            Ok([f32::from_bits(w[0]), f32::from_bits(w[1])])
        };
        let mut wheel = [[0.0; 2]; 13];
        for (slot, field) in wheel.iter_mut().zip(SURFACE_CLASSES) {
            *slot = info("Hash_9631416952242947", field)?;
        }
        let mut grind = [[0.0; 2]; 15];
        for (slot, field) in grind.iter_mut().zip(GRIND_CLASSES) {
            *slot = info("grind", field)?;
        }
        let mut physics = [[0.0; 2]; 8];
        for (slot, field) in physics.iter_mut().zip(PHYSICS_CLASSES) {
            *slot = info("physics", field)?;
        }
        Ok(Self { entries, wheel, grind, physics, surfaces: SurfaceMap::load(data)? })
    }

    /// `0x8259E660`: a wheel material's [high, low] multipliers (material 0: none).
    fn wheel_surface(&self, material: u32) -> [f32; 2] {
        if material == 0 {
            return [0.0; 2];
        }
        let class = self.surfaces.lookup(material as i32 - 1, 4) as usize;
        self.wheel.get(class).copied().unwrap_or([0.0; 2])
    }

    /// `0x8259EAC8`: a grind material's [high, low] multipliers (material 0 reads class 14).
    fn grind_surface(&self, material: u32) -> [f32; 2] {
        let class = if material == 0 { 14 } else { self.surfaces.lookup(material as i32 - 1, 16) as usize };
        self.grind.get(class).copied().unwrap_or([0.0; 2])
    }

    /// `0x8259E8F8`: Collision+16's [high, low] from the `physics` table (13 reads class 2; 8..12 and >13 none).
    fn physics_surface(&self, class: u32) -> [f32; 2] {
        let class = if class == 13 { 2 } else { class } as usize;
        self.physics.get(class).copied().unwrap_or([0.0; 2])
    }

    /// One skater tick (`0x8259E3D0`): (channel, low, high, decay) for every entry that fires.
    /// `landed` is the previous tick's per-wheel landed latch.
    fn tick(&self, i: &RetailAudioInputs, b40: &ConditionerOutput, landed: [bool; 4]) -> Vec<(u8, f32, f32, f32)> {
        // UNTRACED: ctx+14 (a live B60+4264 contact of type 60) also counts; its producer is unported.
        let on_board = i.feet_in_deck_box[0] || i.feet_in_deck_box[1];
        let wheels_down = i.wheel_count > 0;
        let newly: [bool; 4] = std::array::from_fn(|w| b40.wheel_landed_68[w] && !landed[w]);
        let wheel0 = self.wheel_surface(i.wheel_audio_surfaces[0]);
        // 8259CF30: Grinds(B+16)+216, not the B40 latch.
        let grind = self.grind_surface(i.grind_audio_surface);
        // 8259CFB8: powerslide, foot_brake and deck_brake read the physics table.
        let physics = self.physics_surface(i.rumble_physics_surface);
        let mut out = Vec::new();
        for entry in &self.entries {
            let (fires, x, surface) = match entry.source {
                Source::Skeleton => (true, b40.body_contacts_92.impact.iter().sum(), [1.0; 2]),
                Source::Landing => (
                    // 0x8259C9B0: B60 0x392B || 0x392C first.
                    i.scoring_ground_or_air && newly.contains(&true),
                    (0..4).filter(|&w| newly[w]).map(|w| b40.wheel_impact_52[w]).sum(),
                    [1.0; 2],
                ),
                Source::Powerslide => {
                    let lateral = i.deck_rows[0].iter().zip(i.linear_velocity).map(|(a, b)| a * b).sum::<f32>();
                    (on_board && wheels_down, (lateral.abs() * 0.1).min(1.0), physics)
                }
                Source::Wheel => (on_board && wheels_down, i.ground_speed, wheel0),
                Source::Truck => (on_board && (i.truck_contacts[0] || i.truck_contacts[1]), i.ground_speed, grind),
                Source::Deck => (on_board && i.deck_contact, i.ground_speed, grind),
                Source::FootBrake => (i.state_flags[0] || i.state_flags[1], i.ground_speed, physics),
                Source::DeckBrake => (i.state_flags[2], i.ground_speed, physics),
            };
            if let Some((low, high, decay)) = fires.then(|| entry.evaluate(x, surface)).flatten() {
                out.push((entry.channel, low, high, decay));
            }
        }
        out
    }
}

/// The per-pad rumble channels and the 60 Hz mixer.
#[derive(Default)]
struct Mixer {
    /// (channel, [low, high], per-frame multiplier).
    channels: Vec<(u8, [f32; 2], f32)>,
}

impl Mixer {
    /// `0x82694D38`: find or create the channel and store its values.
    fn set(&mut self, channel: u8, low: f32, high: f32, decay: f32) {
        let values = [low.clamp(0.0, 1.0), high.clamp(0.0, 1.0)];
        let multiplier = (1.0 - decay / 60.0).clamp(0.0, 1.0);
        // 0x82694D38: an 8-slot pool; a new channel is dropped when it is full.
        let free = self.channels.len() < 8;
        match self.channels.iter_mut().find(|c| c.0 == channel) {
            Some(c) => (c.1, c.2) = (values, multiplier),
            None if free => self.channels.push((channel, values, multiplier)),
            None => {}
        }
    }

    /// `0x82695010`: [low (motor 0), high (motor 1)], then every channel decays.
    fn step(&mut self) -> [f32; 2] {
        let motors = std::array::from_fn(|m| {
            let sum: f32 = self.channels.iter().map(|c| c.1[m]).sum();
            let max = self.channels.iter().map(|c| c.1[m]).fold(0.0, f32::max);
            sum.min(max + (1.0 - max) * 0.2).clamp(0.0, 1.0)
        });
        for c in &mut self.channels {
            c.1 = c.1.map(|v| v * c.2);
        }
        self.channels.retain(|c| c.1[0] >= 0.001 || c.1[1] >= 0.001);
        motors
    }
}

#[derive(Default)]
pub(super) struct State {
    loading: Option<Receiver<Result<(Table, AudioTuning), String>>>,
    loaded: Option<(Table, AudioTuning)>,
    conditioner: Conditioner,
    landed: [bool; 4],
    mixer: Mixer,
    clock: f32,
    /// The slot and motor values last sent.
    sent: Option<(usize, [f32; 2])>,
}

/// Set to drop every running channel and the conditioner's history (Party Play's handover: the next player's turn
/// starts with no rumble left over).
pub(crate) static RESET: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(super) fn update(
    mut observations: MessageReader<PlayerAudioObservation>,
    assets: Option<Res<super::PlayerAudioAssets>>,
    mut menu: Option<ResMut<crate::graphics_menu::Menu>>,
    input: Option<Res<crate::input::ControllerInput>>,
    time: Res<Time<Real>>,
    mut state: Local<State>,
) {
    let state = &mut *state;
    if RESET.swap(false, std::sync::atomic::Ordering::Relaxed) {
        (state.conditioner, state.landed, state.mixer) = (Conditioner::default(), [false; 4], Mixer::default());
    }
    if state.loaded.is_none() && state.loading.is_none() {
        if let Some(assets) = &assets {
            let (send, receive) = channel();
            let root = assets.0.clone();
            let _ = std::thread::Builder::new().name("rumble-load".into()).spawn(move || {
                let _ = send.send(load(&root));
            });
            state.loading = Some(receive);
        }
    }
    if let Some(Ok(result)) = state.loading.as_ref().map(Receiver::try_recv) {
        match result {
            Ok(loaded) => state.loaded = Some(loaded),
            Err(e) => warn!("Rumble: no retail rumble table ({e})"),
        }
        // Kept (and never re-polled) after a failure so the load isn't retried every frame.
        if state.loaded.is_some() {
            state.loading = None;
        }
    }
    // Settings > Controls > Vibration: retail's per-pad enable (+408) gates every channel store.
    let enabled = menu.as_ref().is_none_or(|m| m.vibration());
    if menu.as_ref().is_some_and(|m| m.vibration_buzz) {
        menu.as_mut().unwrap().vibration_buzz = false;
        // 0x8260DF40: the On confirmation pulse.
        state.mixer.set(0, 0.5, 0.5, 5.0);
    }
    for observation in observations.read() {
        let Some((table, tuning)) = &state.loaded else { continue };
        let inputs = &observation.retail;
        state.conditioner.update(inputs, tuning);
        let b40 = state.conditioner.output();
        let fired = table.tick(inputs, b40, state.landed);
        state.landed = b40.wheel_landed_68;
        if enabled && !inputs.paused {
            for (channel, low, high, decay) in fired {
                // ponytail: bug 10-17 #41 diagnostic (rumble at the wrong times / too long); remove once traced.
                if low.max(high) >= 0.2 && !state.mixer.channels.iter().any(|c| c.0 == channel) {
                    let skeleton: f32 = b40.body_contacts_92.impact.iter().sum();
                    info!(
                        "Rumble: channel {channel} starts at low {low:.2} high {high:.2}, decay {decay:.1}/s \
                         (ground speed {:.3}, board speed {:.3}, skeleton sum {skeleton:.3}, tick {})",
                        inputs.ground_speed, observation.board_speed, observation.tick
                    );
                }
                state.mixer.set(channel, low, high, decay);
            }
        }
    }
    state.clock = (state.clock + time.delta_secs()).min(0.25);
    let mut motors = None;
    while state.clock >= 1.0 / 60.0 {
        state.clock -= 1.0 / 60.0;
        motors = Some(state.mixer.step());
    }
    if let Some(motors) = motors {
        route(&mut state.sent, input.and_then(|i| i.device()), motors, crate::input::platform::set_rumble);
    }
}

/// Send the motors to the pad's slot; the slot it last drove (another pad, or one now gone) is zeroed.
fn route(sent: &mut Option<(usize, [f32; 2])>, slot: Option<usize>, motors: [f32; 2], mut set: impl FnMut(usize, [f32; 2])) {
    if let Some((old, _)) = sent.filter(|(old, m)| Some(*old) != slot && *m != [0.0; 2]) {
        set(old, [0.0; 2]);
    }
    *sent = match slot {
        Some(slot) if *sent != Some((slot, motors)) => {
            set(slot, motors);
            Some((slot, motors))
        }
        Some(_) => *sent,
        None => None,
    };
}

fn load(root: &Path) -> Result<(Table, AudioTuning), String> {
    let data = Collections::shared(root)?; // perf 10-18
    Ok((Table::load(&data)?, AudioTuning::from_collections(&data)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(y0: f32, y1: f32) -> Graph {
        Graph { x: std::array::from_fn(|i| i as f32 / 7.0), y: std::array::from_fn(|i| y0 + (y1 - y0) * i as f32 / 7.0) }
    }

    fn entry(channel: u8, source: Source) -> Entry {
        Entry {
            channel,
            source,
            enabled: true,
            use_surface: false,
            use_low_graph: true,
            use_high_graph: false,
            input_min: 0.5,
            input_max: 10.0,
            intensity: 0.5,
            decay: line(20.0, 10.0),
            high: line(0.0, 0.0),
            low: line(0.0, 1.0),
        }
    }

    fn table(entries: Vec<Entry>) -> Table {
        // Material m (1-based) is wheel class m-1 and grind class m-1.
        let surfaces = SurfaceMap::from_entries((0..95).map(|i| {
            let mut e = [0u32; 18];
            (e[1], e[4]) = (i, i);
            e
        }).collect());
        let mut wheel = [[0.0; 2]; 13];
        wheel[2] = [0.5, 2.0];
        let mut grind = [[0.0; 2]; 15];
        grind[14] = [0.21, 0.4];
        let mut physics = [[0.0; 2]; 8];
        physics[2] = [2.0, 0.5];
        Table { entries, wheel, grind, physics, surfaces }
    }

    #[test]
    fn graphs_interpolate_and_clamp() {
        let g = Graph { x: [0.0, 0.25, 0.5, 0.75, 1.0, 1.0, 1.0, 1.0], y: [1.0, 2.0, 4.0, 8.0, 16.0, 16.0, 16.0, 16.0] };
        assert_eq!(g.at(-1.0), 1.0);
        assert_eq!(g.at(0.375), 3.0);
        assert_eq!(g.at(0.875), 12.0);
        assert_eq!(g.at(5.0), 16.0);
    }

    #[test]
    fn the_evaluator_follows_0x8259dfb0() {
        let close = |got: Option<(f32, f32, f32)>, want: (f32, f32, f32)| {
            let got = got.expect("fires");
            assert!((got.0 - want.0).abs() < 1e-5 && (got.1 - want.1).abs() < 1e-5 && (got.2 - want.2).abs() < 1e-4, "{got:?}");
        };
        let mut e = entry(6, Source::Wheel);
        // At or below input_min: nothing.
        assert_eq!(e.evaluate(0.5, [1.0; 2]), None);
        // Midway: t = 0.5; low graph 0.5, high = t (no graph) 0.5, decay 15; intensity 0.5.
        close(e.evaluate(5.25, [1.0; 2]), (0.25, 0.25, 15.0));
        // Past input_max t clamps to 1.
        close(e.evaluate(100.0, [1.0; 2]), (0.5, 0.5, 10.0));
        // Surface multipliers are [high, low]; zero from both means no message.
        e.use_surface = true;
        close(e.evaluate(100.0, [0.5, 2.0]), (1.0, 0.25, 10.0));
        assert_eq!(e.evaluate(100.0, [0.0, 0.0]), None);
        e.enabled = false;
        assert_eq!(e.evaluate(100.0, [1.0; 2]), None);
    }

    #[test]
    fn rumble_stops_on_the_old_pad_when_the_pad_goes_away_or_changes() {
        let mut calls = Vec::new();
        let mut sent = None;
        route(&mut sent, Some(2), [0.5, 0.1], |s, m| calls.push((s, m)));
        route(&mut sent, Some(2), [0.5, 0.1], |s, m| calls.push((s, m))); // unchanged: no resend
        route(&mut sent, None, [0.4, 0.1], |s, m| calls.push((s, m))); // pad gone: zeroed
        route(&mut sent, Some(1), [0.3, 0.0], |s, m| calls.push((s, m)));
        route(&mut sent, Some(0), [0.3, 0.0], |s, m| calls.push((s, m))); // another pad: old zeroed
        assert_eq!(calls, [(2, [0.5, 0.1]), (2, [0.0; 2]), (1, [0.3, 0.0]), (1, [0.0; 2]), (0, [0.3, 0.0])]);
        assert_eq!(sent, Some((0, [0.3, 0.0])));
    }

    #[test]
    fn the_mixer_sums_with_the_max_cap_and_decays() {
        let mut m = Mixer::default();
        m.set(0, 0.5, 0.2, 6.0);
        m.set(3, 0.5, 0.0, 6.0);
        // sum 1.0, max 0.5 -> min(1.0, 0.5 + 0.5 * 0.2) = 0.6; high: only 0.2 -> min(0.2, 0.36).
        let [low, high] = m.step();
        assert!((low - 0.6).abs() < 1e-6 && (high - 0.2).abs() < 1e-6);
        // Each channel decays by 1 - 6/60 per frame and is freed below 0.001.
        assert!((m.channels[0].1[0] - 0.45).abs() < 1e-6);
        m.set(0, 0.5, 0.5, 5.0); // the Vibration On pulse overwrites channel 0
        assert_eq!(m.channels[0].1, [0.5, 0.5]);
        for _ in 0..200 {
            m.step();
        }
        assert!(m.channels.is_empty());
        assert_eq!(m.step(), [0.0, 0.0]);
        // The pool has 8 slots; a ninth channel is dropped.
        for ch in 0..9 {
            m.set(ch, 0.5, 0.5, 1.0);
        }
        assert_eq!(m.channels.len(), 8);
    }

    #[test]
    fn the_event_table_fires_from_the_published_inputs() {
        let entries = EVENTS.iter().map(|&(_, channel, source)| entry(channel, source)).collect();
        let t = table(entries);
        let mut i = RetailAudioInputs::default();
        let mut b40 = ConditionerOutput::default();
        let fired = |i: &RetailAudioInputs, b40: &ConditionerOutput, landed| {
            t.tick(i, b40, landed).iter().map(|f| f.0).collect::<Vec<_>>()
        };
        assert!(fired(&i, &b40, [false; 4]).is_empty());
        // Rolling on the board, wheels down, wheel 0 on material 3 (class 2).
        i.feet_in_deck_box = [true, false];
        i.wheel_count = 4;
        i.ground_speed = 8.0;
        i.wheel_audio_surfaces[0] = 3;
        assert_eq!(fired(&i, &b40, [false; 4]), [6]);
        // Off the board nothing rolls; a foot brake (flag 52) and a tail drag (flag 54) still do.
        i.feet_in_deck_box = [false; 2];
        i.state_flags[0] = true;
        i.state_flags[2] = true;
        assert_eq!(fired(&i, &b40, [false; 4]), [9, 10]);
        i.state_flags = [false; 36];
        // Grinding: trucks and deck, keyed by the grind material (0 = class 14).
        i.feet_in_deck_box = [false, true];
        i.wheel_count = 0;
        i.truck_contacts = [true, false];
        i.deck_contact = true;
        assert_eq!(fired(&i, &b40, [false; 4]), [7, 8]);
        // A landing fires once, on the latch edge, and only with the Ground/Air collector live.
        b40.wheel_landed_68 = [true, true, false, false];
        b40.wheel_impact_52 = [0.5, 0.5, 0.0, 0.0];
        assert!(!fired(&i, &b40, [false; 4]).contains(&2));
        i.scoring_ground_or_air = true;
        assert!(fired(&i, &b40, [false; 4]).contains(&2));
        assert!(!fired(&i, &b40, [true, true, false, false]).contains(&2));
        // Body impacts drive the skeleton channel.
        b40.body_contacts_92.impact[3] = 0.9;
        assert!(fired(&i, &b40, [true; 4]).contains(&0));
        // Surface lookups: material 3 -> class 2, material 0 -> nothing for wheels, class 14 for grinds.
        assert_eq!(t.wheel_surface(3), [0.5, 2.0]);
        assert_eq!(t.wheel_surface(0), [0.0; 2]);
        assert_eq!(t.grind_surface(0), [0.21, 0.4]);
        // Physics table: 13 reads class 2's field, 8..12 read nothing.
        assert_eq!(t.physics_surface(13), [2.0, 0.5]);
        assert_eq!(t.physics_surface(9), [0.0; 2]);
    }

    /// The stock vault through `SKATE3_ASSET_ROOT`; skipped without it.
    #[test]
    fn stock_rumble_table_loads_the_retail_values() {
        let Some(root) = std::env::var_os("SKATE3_ASSET_ROOT").map(std::path::PathBuf::from) else {
            return;
        };
        let (table, _) = load(&root).unwrap();
        let get = |key: &str| &table.entries[EVENTS.iter().position(|e| e.0 == key).unwrap()];
        assert!(table.entries.iter().all(|e| e.enabled));
        let p = get("powerslide");
        assert_eq!((p.input_min, p.input_max, p.intensity, p.use_surface), (0.2, 1.0, 0.5, true));
        assert_eq!(p.decay.y, [10.0; 8]);
        let s = get("skeleton");
        assert_eq!((s.input_min, s.input_max, s.use_surface), (0.05, 1.0, false));
        assert!((s.low.at(0.0) - 0.55).abs() < 0.01 && (s.low.at(1.0) - 1.0).abs() < 0.01);
        let f = get("foot_brake");
        assert!(!f.use_low_graph && !f.use_high_graph);
        assert!(!get("deck_brake").use_surface);
        assert_eq!(table.wheel[0], [0.045, 0.035]);
        assert_eq!(table.wheel[11], [0.0, 2.5]);
        assert_eq!(table.grind[0], [0.4, 0.6]);
        assert_eq!(table.grind[14], [0.21, 0.4]);
        assert_eq!(table.physics[3], [0.4, 2.0]);
        assert_eq!(table.physics[5], [0.3, 3.0]);
    }
}
