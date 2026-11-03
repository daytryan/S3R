//! Rain and thunder audio (docs/superpowers/specs/2026-10-06-weather-audio-design.md): the user's CC0 recordings,
//! mastered by tools/prepare_weather_audio.py, mixed host-side by [`mixer::Mixer`] on the audio mixer thread.
//!
//! The `drive` system (Update, after the weather) decides each frame what the rain falls on, how covered the camera
//! is, the wheels' speed and the thunder to come, and hands it to the mixer through [`WeatherLink`]; in a replay it
//! takes the rain and strikes from the recording instead (replay.rs).
mod assets;
mod census;
mod duck;
mod mixer;
pub(crate) mod replay;
mod stream;
pub(crate) mod thunder;
mod wheels;

#[cfg(test)]
mod preview;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_util;

pub(crate) use assets::Bank;
pub(crate) use duck::{AmbienceDuck, bed_name, export_rain};

use bevy::{audio::AddAudioSource, prelude::*};
use census::Census;
use mixer::{Params, Thunder};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

/// The bank's folder under the installation's assets.
pub(crate) const FOLDER: &str = "weather-audio";
/// Seconds between census parts: a whole census every half second, at most 16 rays a frame and about 100 a second
/// (the mixer smooths the steps).
const CENSUS_EVERY: f32 = 0.125;
/// Below this a wheel is on the ground (retail_wheel_fx GROUNDED).
const GROUNDED: f32 = 0.055;

/// What the game hands the mixer thread: this frame's parameters, strikes to queue, and whether to drop the thunder
/// still pending.
#[derive(Default)]
pub(crate) struct Link {
    pub params: Params,
    pub strikes: [Option<(Thunder, f32)>; 4],
    pub cancel: bool,
}
impl Link {
    fn queue(&mut self, strike: (Thunder, f32)) {
        match self.strikes.iter_mut().find(|s| s.is_none()) {
            Some(slot) => *slot = Some(strike),
            // ponytail: strikes are 10 s or more apart; a full queue only happens if the mixer thread stalls.
            None => self.strikes[0] = Some(strike),
        }
    }
}

#[derive(Resource, Clone, Default)]
pub(crate) struct WeatherLink(pub Arc<Mutex<Link>>);

/// The weather mix as a never-ending Bevy audio source (played ONCE: a looped source buffers forever).
#[derive(Asset, TypePath)]
pub(crate) struct WeatherMix {
    bank: Arc<Bank>,
    link: Arc<Mutex<Link>>,
    seed: u64,
}

/// Frames per parameter pick-up on the mixer thread (10.7 ms).
const DECODE_FRAMES: usize = 512;

pub(crate) struct WeatherDecoder {
    mixer: mixer::Mixer,
    link: Arc<Mutex<Link>>,
    buf: Vec<f32>,
    at: usize,
}
impl WeatherDecoder {
    fn refill(&mut self) {
        // try_lock: the mixer thread never waits on the game; on contention the last parameters stand.
        if let Ok(mut link) = self.link.try_lock() {
            if std::mem::take(&mut link.cancel) {
                self.mixer.cancel_pending();
            }
            self.mixer.set(link.params);
            for (thunder, delay) in link.strikes.iter_mut().filter_map(Option::take) {
                self.mixer.strike(thunder, delay);
            }
        }
        self.mixer.render(&mut self.buf);
        self.at = 0;
    }
}
impl Iterator for WeatherDecoder {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.at >= self.buf.len() {
            self.refill();
        }
        let sample = self.buf[self.at];
        self.at += 1;
        Some(sample)
    }
}
impl bevy::audio::Source for WeatherDecoder {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        2
    }
    fn sample_rate(&self) -> u32 {
        mixer::RATE
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}
impl bevy::audio::Decodable for WeatherMix {
    type DecoderItem = f32;
    type Decoder = WeatherDecoder;
    fn decoder(&self) -> WeatherDecoder {
        WeatherDecoder {
            mixer: mixer::Mixer::new(self.bank.clone(), self.seed),
            link: self.link.clone(),
            buf: vec![0.; DECODE_FRAMES * 2],
            at: DECODE_FRAMES * 2,
        }
    }
}

/// `headless`: the driver only (tests), no recordings and no audio output.
pub(crate) struct WeatherAudioPlugin {
    pub headless: bool,
}
impl Plugin for WeatherAudioPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WeatherLink>()
            .add_message::<crate::retail_render::LightningStrike>()
            .add_systems(Update, drive.after(crate::retail_render::WeatherSet));
        if !self.headless {
            app.add_audio_source::<WeatherMix>().add_systems(Startup, spawn);
        }
    }
}

fn spawn(
    mut commands: Commands,
    config: Option<Res<crate::config::Config>>,
    link: Res<WeatherLink>,
    mut sources: ResMut<Assets<WeatherMix>>,
) {
    let Some(config) = config else { return };
    let bank = Bank::load(&config.asset_root.join(FOLDER));
    if bank.is_empty() {
        return;
    }
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64);
    let handle = sources.add(WeatherMix { bank: Arc::new(bank), link: link.0.clone(), seed });
    commands.spawn((Name::new("Weather audio"), AudioPlayer(handle), PlaybackSettings::ONCE));
}

/// The driver's memory between frames.
#[derive(Default)]
struct Drive {
    since_census: f32,
    census: Census,
    /// The census being cast, its next part and its ring's turn.
    building: Census,
    part: usize,
    phase: f32,
    skater_covered: bool,
    rng: u64,
    replay: replay::ReplayThunder,
}

#[allow(clippy::too_many_arguments)]
fn drive(
    time: Res<Time<Real>>,
    menu: Option<Res<crate::graphics_menu::Menu>>,
    retail: Option<Res<crate::retail_render::RetailScene>>,
    customiser: Option<Res<crate::customiser::Customiser>>,
    physics: Option<Res<crate::physics::GamePhysics>>,
    mut recording: Option<ResMut<crate::replay::Replay>>,
    cameras: Query<&GlobalTransform, With<Camera3d>>,
    mut strikes: MessageReader<crate::retail_render::LightningStrike>,
    link: Res<WeatherLink>,
    screens: (Option<Res<crate::boot::BootActive>>, Option<Res<crate::tip_loader::LoadingHold>>),
    mut state: Local<Drive>,
) {
    let live_strikes: Vec<_> = strikes.read().copied().collect();
    let playback = recording.as_ref().is_some_and(|r| r.active);
    let mut rain = crate::retail_render::rain_level(menu.as_deref(), retail.as_deref(), customiser.as_deref());
    let retail_scene = retail.as_ref().is_some_and(|r| r.0);
    // graphics_menu::gameplay_active: a menu (the Start crossbar too) is a pause.
    let mut paused = menu.as_ref().is_some_and(|m| m.open);
    let mut queued = Vec::new();
    let mut cancel = false;
    match recording.as_mut() {
        Some(replay) if playback => {
            // The recording's weather, not the live one's.
            let (playing, speed) = (replay.playing, replay.speed);
            if let Some((index, now)) = replay.playhead() {
                rain = replay.weather_at(index).map_or(0., |w| w.1);
                let ring = replay.frames();
                let step = state.replay.step(index, now, playing, speed, |i| ring.time(i), ring.strikes());
                cancel = step.cancel;
                queued = step.strikes;
            }
            paused |= !playing;
        }
        replay => {
            // Leaving a replay: its thunder still pending must not land in live play.
            cancel = state.replay.reset();
            if let Some(replay) = replay {
                replay.note_weather(rain, live_strikes.first().copied());
            }
            if rain >= 1. && retail_scene {
                queued = live_strikes.iter().map(|s| (replay::thunder_of(*s), thunder::delay_s(s.distance_m))).collect();
            } else {
                // Review Focus 2: no thunder after the heavy rain stops or off the retail maps.
                cancel = true;
            }
        }
    }
    // The boot and the loading screens own the screen: no rain and no thunder behind them (as music.rs holds the
    // music off).
    if screens.0.is_some_and(|b| b.0) || screens.1.is_some_and(|h| h.0) {
        rain = 0.;
        cancel = true;
        queued.clear();
    }
    duck::publish_rain(rain);

    let camera = cameras.iter().next();
    state.since_census += time.delta_secs();
    if let (Some(physics), Some(camera)) = (physics.as_ref(), camera)
        && state.since_census >= CENSUS_EVERY
        && rain > 0.
    {
        let _perf = crate::performance::Scope::new("weather_census");
        state.since_census = 0.;
        if state.part == 0 {
            state.rng = state.rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            state.phase = (state.rng >> 40) as f32 / (1u64 << 24) as f32 * std::f32::consts::TAU;
            state.building = Census::default();
        }
        let (part, phase) = (state.part, state.phase);
        state.building.sample_part(physics.world(), camera.translation(), camera.right().into(), phase, part);
        state.part += 1;
        if state.part == Census::PARTS {
            state.part = 0;
            state.census = state.building;
            let deck = physics.board.bodies()[skate_core::physics::board::BodyId::Deck.index()].rates.position;
            state.skater_covered = census::covered(physics.world(), Vec3::new(deck.x, deck.y, deck.z));
        }
    }
    let wheel_speed = match physics.as_ref() {
        Some(p) if !playback && !state.skater_covered && p.riding.wheel_lines.minimum_distance < GROUNDED => {
            let v = p.board.bodies()[skate_core::physics::board::BodyId::Deck.index()].rates.linear_velocity;
            Vec3::new(v.x, v.y, v.z).length()
        }
        _ => 0.,
    };
    // Not retail (user 10-14, feature #10): the Rain Volume slider scales the rain under SFX.
    let master = menu.as_ref().map_or(1., |m| {
        let a = m.audio();
        f32::from(a.volumes[crate::music::SFX].min(10)) / 10. * f32::from(a.rain.min(10)) / 10.
    }) * crate::music::duck();
    let camera_yaw = camera.map_or(0., |c| {
        let f = c.forward();
        f.z.atan2(f.x)
    });
    let params = Params {
        rain,
        surfaces: census::water_boost(state.census.surfaces(), rain),
        cover: state.census.cover,
        roof_metal: state.census.roof_metal,
        paused,
        master,
        wheel_speed,
        camera_yaw,
    };
    let Ok(mut link) = link.0.lock() else { return };
    link.params = params;
    if cancel {
        link.cancel = true;
        link.strikes = [None; 4];
    }
    for strike in queued {
        link.queue(strike);
    }
}
