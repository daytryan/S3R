//! U7: rain ducks the ambience bed (plan Task 8). Nature beds drop further than city ones. The weather driver
//! publishes the rain level; the audio worker (player_audio world.rs / game_side.rs) classifies the bed it asked
//! for, smooths the duck and hands it to SFXObj_Ambience's controller (skate-audio-core `ambience_duck`).
use std::{
    cell::Cell,
    sync::atomic::{AtomicU32, Ordering},
};

/// The rain level weather audio hears (0, 0.5, 1), as f32 bits: read by the live audio worker.
static RAIN: AtomicU32 = AtomicU32::new(0);

thread_local! {
    /// The replay export's worker thread: the exported frame's recorded rain, in place of the live one.
    static EXPORT_RAIN: Cell<Option<f32>> = const { Cell::new(None) };
}

fn level(rain: f32) -> f32 {
    if rain.is_finite() { rain.clamp(0., 1.) } else { 0. }
}

pub(crate) fn publish_rain(rain: f32) {
    RAIN.store(level(rain).to_bits(), Ordering::Relaxed);
}

/// On the replay export's worker thread: the rain of the frame it renders next (its ambience ducks to that).
pub(crate) fn export_rain(rain: f32) {
    EXPORT_RAIN.with(|r| r.set(Some(level(rain))));
}

fn rain() -> f32 {
    EXPORT_RAIN.with(Cell::get).unwrap_or_else(|| f32::from_bits(RAIN.load(Ordering::Relaxed)))
}

/// Skate 3's ambience beds by number (`NN_*.snr` in data/audio/ambienceresident.big; map.md §6).
const BEDS: [&str; 24] = [
    "",
    "01_dt_apt",
    "02_dt_less_busy",
    "03_dt_rez",
    "04_dt_main",
    "05_dt_parks",
    "06_dt_open",
    "07_univ_mt_high",
    "08_univ_mt_low",
    "09_univ_campus",
    "10_univ_housing",
    "11_indu_shipyard",
    "12_indu_drydock",
    "13_indu_quarry",
    "14_reclaimed_a",
    "15_indu_new_factory",
    "16_spillway",
    "17_space_park",
    "18_skate_school",
    "19_reclaimed_b",
    "20_indu_old_factory",
    "21_interior_arena_amb",
    "22_interior_tunnel_amb",
    "23_Press_start_screen",
];

pub(crate) fn bed_name(bed: u32) -> &'static str {
    BEDS.get(bed as usize).copied().unwrap_or("")
}

/// The plan's nature words, plus `_mt_` (the university mountain beds: wilderness, not traffic).
const NATURE: [&str; 8] = ["bird", "nature", "insect", "cricket", "wind", "forest", "park", "_mt_"];

/// The duck in dB for a bed at `rain` (0 none, 0.5 light, 1 heavy; linear between). Nature beds go to −6 / −15 dB,
/// city and unknown ones to −3 / −6 dB.
pub(crate) fn duck_db(bed: &str, rain: f32) -> f32 {
    let bed = bed.to_ascii_lowercase();
    let (light, heavy) = if NATURE.iter().any(|w| bed.contains(w)) { (-6., -15.) } else { (-3., -6.) };
    let r = rain.clamp(0., 1.);
    if r <= 0.5 { light * r * 2. } else { light + (heavy - light) * (r - 0.5) * 2. }
}

pub(crate) fn duck_scale(bed: &str, rain: f32) -> f32 {
    10f32.powf(duck_db(bed, rain) / 20.)
}

/// The duck the worker applies, moving towards the published rain's with τ 3 s.
pub(crate) struct AmbienceDuck(f32);
impl Default for AmbienceDuck {
    fn default() -> Self {
        Self(1.)
    }
}
impl AmbienceDuck {
    pub(crate) fn step(&mut self, bed: &str, dt: f32) -> f32 {
        self.step_with(bed, rain(), dt)
    }

    /// [`AmbienceDuck::step`] at `rain`. Lands exactly on the target (an f32 one-pole stalls just short of it), so
    /// once the rain stops the duck is 1.0 again and skipped (retail 1:1).
    pub(crate) fn step_with(&mut self, bed: &str, rain: f32, dt: f32) -> f32 {
        let target = duck_scale(bed, rain);
        self.0 += (target - self.0) * (1. - (-dt.max(0.) / 3.).exp());
        if (target - self.0).abs() < 1e-4 {
            self.0 = target;
        }
        self.0
    }
}
