use crate::{
    AudioPlayer, Decodable, DefaultSpatialScale, GlobalVolume, PlaybackMode, PlaybackSettings,
    SpatialAudioSink, SpatialListener,
};
use alloc::sync::Arc;
use bevy_asset::{Asset, Assets};
use core::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError, mpsc::SyncSender};
use bevy_ecs::{prelude::*, system::SystemParam};
use bevy_math::Vec3;
use bevy_transform::prelude::GlobalTransform;
use rodio::{OutputStream, OutputStreamHandle, Sink, Source, SpatialSink};
use tracing::warn;

use crate::{AudioSink, AudioSinkPlayback};

/// Used internally to play audio on the current "audio device"
///
/// Not upstream (Skate 3 Remaster, 10-29): the [`OutputStream`] (`!Send`) lives on the [`watch`] thread, which opens
/// it again when the device is reset or unplugged, or the device it should be on changes; [`reopen_output`] then
/// plays every sink on the new one. Upstream opened it once and leaked it: a reset device stayed silent for good.
#[derive(Resource)]
pub(crate) struct AudioOutput {
    stream_handle: Option<OutputStreamHandle>,
}

/// Not upstream (Skate 3 Remaster, user 10-26): the output device by name, read once when [`AudioOutput`] opens.
static OUTPUT_DEVICE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

/// Names the output device the audio plugin opens (`None`, or a name not found: the system default). Call it before
/// the plugin builds; later calls change nothing.
pub fn set_output_device(name: Option<String>) {
    let _ = OUTPUT_DEVICE.set(name);
}

/// `wanted`'s index in `names`; `None` (the default) when it is not there.
fn choose_device(names: &[String], wanted: Option<&str>) -> Option<usize> {
    names.iter().position(|name| Some(name.as_str()) == wanted)
}

/// The output devices and their names.
fn output_devices() -> (Vec<rodio::cpal::Device>, Vec<String>) {
    use rodio::cpal::traits::{DeviceTrait, HostTrait};
    let devices: Vec<_> = rodio::cpal::default_host().output_devices().map(Iterator::collect).unwrap_or_default();
    let names = devices.iter().map(|d| d.name().unwrap_or_default()).collect();
    (devices, names)
}

/// The device the output should be on: the one named by [`set_output_device`] while it is there, else the system
/// default; `None` without any.
fn wanted_device() -> Option<String> {
    use rodio::cpal::traits::{DeviceTrait, HostTrait};
    if let Some(name) = OUTPUT_DEVICE.get().cloned().flatten()
        && choose_device(&output_devices().1, Some(&name)).is_some()
    {
        return Some(name);
    }
    rodio::cpal::default_host().default_output_device()?.name().ok()
}

/// `wanted` opened; `None` when it is not there or does not open (`say`: warn why).
fn open_exact(wanted: Option<&str>, say: bool) -> Option<(OutputStream, OutputStreamHandle)> {
    let (devices, names) = output_devices();
    let i = choose_device(&names, wanted)?;
    OutputStream::try_from_device(&devices[i])
        .inspect_err(|error| if say { warn!("Output device \"{}\" did not open ({error}).", names[i]) })
        .ok()
}

/// `wanted` opened, else the default (or any output that opens, as rodio's `try_default` picks); true when it is
/// `wanted`.
fn open(wanted: Option<&str>) -> Option<((OutputStream, OutputStreamHandle), bool)> {
    open_exact(wanted, true).map(|s| (s, true)).or_else(|| OutputStream::try_default().ok().map(|s| (s, false)))
}

/// How often the [`watch`] thread looks at the output.
const WATCH_EVERY: core::time::Duration = core::time::Duration::from_secs(2);

/// The handle the [`watch`] thread opened since [`reopen_output`] last looked (`Some(None)`: the output was lost and
/// nothing opened yet).
static REOPENED: Mutex<Option<Option<OutputStreamHandle>>> = Mutex::new(None);

/// Silence on the output's mixer that counts the samples pulled from it. cpal 0.15 ends a stream on its first error
/// (WASAPI's device invalidated or unplugged) and rodio only logs it, so a stream that stopped counting is lost.
struct Probe(Arc<AtomicU64>);

impl Iterator for Probe {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Some(0.)
    }
}

impl Source for Probe {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        1
    }
    fn sample_rate(&self) -> u32 {
        48_000
    }
    fn total_duration(&self) -> Option<core::time::Duration> {
        None
    }
}

/// Watch ticks in a row with nothing pulled before a stream counts as lost: one quiet tick is a device waking up
/// (Bluetooth power save) or a long stall, not a reset.
const DEAD_TICKS: u32 = 2;

#[derive(Debug, PartialEq)]
enum Step {
    Keep,
    /// The stream still plays but should be on `wanted` (a new default, or back from a fallback): open that beside it
    /// and swap only once it opened.
    Swap,
    /// The stream is lost (or none is open and a device is there): let it go and open `wanted`, else any output.
    Restart,
}

/// What the watch does: `open` is the device the playing stream was asked for and got (`None`: none is open; `""`:
/// a fallback), `dead` the ticks in a row it pulled nothing, `wanted` [`wanted_device`].
fn step(open: Option<&str>, dead: u32, wanted: Option<&str>) -> Step {
    match open {
        None if wanted.is_some() => Step::Restart,
        None => Step::Keep,
        Some(_) if dead >= DEAD_TICKS => Step::Restart,
        Some(on) if wanted.is_some_and(|w| w != on) => Step::Swap,
        Some(_) => Step::Keep,
    }
}

/// The output's own thread (device enumeration and opening stay off the main thread): opens the stream and keeps it,
/// sending the first handle to `first`, then every [`WATCH_EVERY`] opens it again when [`reopen_due`].
fn watch(first: SyncSender<Option<OutputStreamHandle>>) {
    let pulled = Arc::new(AtomicU64::new(0));
    let probe = |(stream, handle): (OutputStream, OutputStreamHandle)| {
        let _ = handle.play_raw(Probe(pulled.clone()));
        (stream, handle)
    };
    let wanted = wanted_device();
    let opened = open(wanted.as_deref());
    // A fallback is asked for nothing, so a later tick tries `wanted` again (Step::Swap).
    let mut asked = wanted.filter(|_| opened.as_ref().is_some_and(|(_, exact)| *exact));
    let mut stream = opened.map(|(s, _)| probe(s));
    let _ = first.send(stream.as_ref().map(|(_, handle)| handle.clone()));
    let (mut seen, mut dead) = (pulled.load(Ordering::Relaxed), 0);
    // The device a Swap last failed to open: warned once, not every tick.
    let mut failed: Option<String> = None;
    loop {
        std::thread::sleep(WATCH_EVERY);
        let now = pulled.load(Ordering::Relaxed);
        dead = if now == seen { dead + 1 } else { 0 };
        seen = now;
        let wanted = wanted_device();
        let was = stream.is_some().then(|| asked.clone().unwrap_or_default());
        match step(was.as_deref(), dead, wanted.as_deref()) {
            Step::Keep => continue,
            Step::Swap => {
                let Some(new) = open_exact(wanted.as_deref(), failed != wanted) else {
                    failed = wanted;
                    continue;
                };
                stream = Some(probe(new));
                asked = wanted;
            }
            Step::Restart => {
                // The old stream goes first: a reset device is let go before it is opened again.
                let had = stream.take().is_some();
                let opened = open(wanted.as_deref());
                asked = wanted.filter(|_| opened.as_ref().is_some_and(|(_, exact)| *exact));
                stream = opened.map(|(s, _)| probe(s));
                if !had && stream.is_none() {
                    continue; // Still nothing: tried again quietly next tick.
                }
            }
        }
        failed = None;
        warn!("Audio output reopened: {:?} (was {was:?}, opened: {}).", asked, stream.is_some());
        (seen, dead) = (pulled.load(Ordering::Relaxed), 0);
        *REOPENED.lock().unwrap_or_else(PoisonError::into_inner) = Some(stream.as_ref().map(|(_, handle)| handle.clone()));
    }
}

impl Default for AudioOutput {
    fn default() -> Self {
        let (first, opened) = std::sync::mpsc::sync_channel(1);
        let stream_handle = match std::thread::Builder::new().name("audio-output".into()).spawn(move || watch(first)) {
            // The main thread waits for the first stream as it waited on its own open.
            Ok(_) => opened.recv().ok().flatten(),
            // No thread: opened here once and leaked, as upstream did.
            Err(_) => open(wanted_device().as_deref()).map(|((stream, handle), _)| {
                core::mem::forget(stream);
                handle
            }),
        };
        if stream_handle.is_none() {
            warn!("No audio device found.");
        }
        Self { stream_handle }
    }
}

/// What a sink did, kept on its settings for the sink made again on a reopened output: volume, speed, pause, mute,
/// and a one-shot's place. A stream played Once (the game's live streams never end) would be skipped through sample
/// by sample, so it starts again, as loops do.
/// shortcut: a one-shot's `duration` stays whole, and a GlobalVolume other than 1 is applied once more; the game
/// uses neither, carry them if it ever does.
fn carry(settings: &mut PlaybackSettings, sink: &dyn AudioSinkPlayback) {
    settings.volume = sink.volume();
    settings.speed = sink.speed();
    settings.paused = sink.is_paused();
    settings.muted = sink.is_muted();
    if matches!(settings.mode, PlaybackMode::Despawn | PlaybackMode::Remove) {
        settings.start_position = Some(settings.start_position.unwrap_or_default() + sink.position());
    }
}

/// Puts the output the [`watch`] thread reopened in place and plays every sink again on it. A rodio sink stays on
/// the mixer it was made on, so each is dropped and [`play_queued_audio_system`] makes it again from its
/// [`AudioPlayer`] on the new output, with [`carry`]'s settings. Back from no output at all, the one-shots queued
/// meanwhile (menu sounds, dialog) are stale and dropped, not all played at once.
pub(crate) fn reopen_output(
    mut output: ResMut<AudioOutput>,
    mut playing: Query<(Entity, &mut PlaybackSettings, Option<&AudioSink>, Option<&SpatialAudioSink>)>,
    mut commands: Commands,
) {
    let Some(handle) = REOPENED.lock().unwrap_or_else(PoisonError::into_inner).take() else { return };
    let silent = output.stream_handle.is_none();
    output.stream_handle = handle;
    if output.stream_handle.is_none() {
        return;
    }
    for (entity, mut settings, sink, spatial) in &mut playing {
        let Some(sink) = sink.map(|s| s as &dyn AudioSinkPlayback).or(spatial.map(|s| s as &dyn AudioSinkPlayback)) else {
            if silent && stale(&settings) {
                commands.entity(entity).despawn();
            }
            continue;
        };
        carry(&mut settings, sink);
        commands
            .entity(entity)
            .remove::<(AudioSink, SpatialAudioSink, PlaybackDespawnMarker, PlaybackRemoveMarker)>();
    }
}

/// A one-shot that never got a sink while no output was open: played late, it would be out of place.
fn stale(settings: &PlaybackSettings) -> bool {
    matches!(settings.mode, PlaybackMode::Despawn)
}

/// Marker for internal use, to despawn entities when playback finishes.
#[derive(Component, Default)]
pub struct PlaybackDespawnMarker;

/// Marker for internal use, to remove audio components when playback finishes.
#[derive(Component, Default)]
pub struct PlaybackRemoveMarker;

#[derive(SystemParam)]
pub(crate) struct EarPositions<'w, 's> {
    pub(crate) query: Query<'w, 's, (Entity, &'static GlobalTransform, &'static SpatialListener)>,
}

impl<'w, 's> EarPositions<'w, 's> {
    /// Gets a set of transformed ear positions.
    ///
    /// If there are no listeners, use the default values. If a user has added multiple
    /// listeners for whatever reason, we will return the first value.
    pub(crate) fn get(&self) -> (Vec3, Vec3) {
        let (left_ear, right_ear) = self
            .query
            .iter()
            .next()
            .map(|(_, transform, settings)| {
                (
                    transform.transform_point(settings.left_ear_offset),
                    transform.transform_point(settings.right_ear_offset),
                )
            })
            .unwrap_or_else(|| {
                let settings = SpatialListener::default();
                (settings.left_ear_offset, settings.right_ear_offset)
            });

        (left_ear, right_ear)
    }

    pub(crate) fn multiple_listeners(&self) -> bool {
        self.query.iter().len() > 1
    }
}

/// Plays "queued" audio through the [`AudioOutput`] resource.
///
/// "Queued" audio is any audio entity (with an [`AudioPlayer`] component) that does not have an
/// [`AudioSink`]/[`SpatialAudioSink`] component.
///
/// This system detects such entities, checks if their source asset
/// data is available, and creates/inserts the sink.
pub(crate) fn play_queued_audio_system<Source: Asset + Decodable>(
    audio_output: Res<AudioOutput>,
    audio_sources: Res<Assets<Source>>,
    global_volume: Res<GlobalVolume>,
    query_nonplaying: Query<
        (
            Entity,
            &AudioPlayer<Source>,
            &PlaybackSettings,
            Option<&GlobalTransform>,
        ),
        (Without<AudioSink>, Without<SpatialAudioSink>),
    >,
    ear_positions: EarPositions,
    default_spatial_scale: Res<DefaultSpatialScale>,
    mut commands: Commands,
) where
    f32: rodio::cpal::FromSample<Source::DecoderItem>,
{
    let Some(stream_handle) = audio_output.stream_handle.as_ref() else {
        // audio output unavailable; cannot play sound
        return;
    };

    for (entity, source_handle, settings, maybe_emitter_transform) in &query_nonplaying {
        let Some(audio_source) = audio_sources.get(&source_handle.0) else {
            continue;
        };
        // audio data is available (has loaded), begin playback and insert sink component
        if settings.spatial {
            let (left_ear, right_ear) = ear_positions.get();

            // We can only use one `SpatialListener`. If there are more than that, then
            // the user may have made a mistake.
            if ear_positions.multiple_listeners() {
                warn!(
                    "Multiple SpatialListeners found. Using {}.",
                    ear_positions.query.iter().next().unwrap().0
                );
            }

            let scale = settings.spatial_scale.unwrap_or(default_spatial_scale.0).0;

            let emitter_translation = if let Some(emitter_transform) = maybe_emitter_transform {
                (emitter_transform.translation() * scale).into()
            } else {
                warn!("Spatial AudioPlayer with no GlobalTransform component. Using zero.");
                Vec3::ZERO.into()
            };

            let sink = match SpatialSink::try_new(
                stream_handle,
                emitter_translation,
                (left_ear * scale).into(),
                (right_ear * scale).into(),
            ) {
                Ok(sink) => sink,
                Err(err) => {
                    warn!("Error creating spatial sink: {err:?}");
                    continue;
                }
            };

            let decoder = audio_source.decoder();

            match settings.mode {
                PlaybackMode::Loop => match (settings.start_position, settings.duration) {
                    // custom start position and duration
                    (Some(start_position), Some(duration)) => sink.append(
                        decoder
                            .skip_duration(start_position)
                            .take_duration(duration)
                            .repeat_infinite(),
                    ),

                    // custom start position
                    (Some(start_position), None) => {
                        sink.append(decoder.skip_duration(start_position).repeat_infinite());
                    }

                    // custom duration
                    (None, Some(duration)) => {
                        sink.append(decoder.take_duration(duration).repeat_infinite());
                    }

                    // full clip
                    (None, None) => sink.append(decoder.repeat_infinite()),
                },
                PlaybackMode::Once | PlaybackMode::Despawn | PlaybackMode::Remove => {
                    match (settings.start_position, settings.duration) {
                        (Some(start_position), Some(duration)) => sink.append(
                            decoder
                                .skip_duration(start_position)
                                .take_duration(duration),
                        ),

                        (Some(start_position), None) => {
                            sink.append(decoder.skip_duration(start_position));
                        }

                        (None, Some(duration)) => sink.append(decoder.take_duration(duration)),

                        (None, None) => sink.append(decoder),
                    }
                }
            }

            let mut sink = SpatialAudioSink::new(sink);

            if settings.muted {
                sink.mute();
            }

            sink.set_speed(settings.speed);
            sink.set_volume(settings.volume * global_volume.volume);

            if settings.paused {
                sink.pause();
            }

            match settings.mode {
                PlaybackMode::Loop | PlaybackMode::Once => commands.entity(entity).insert(sink),
                PlaybackMode::Despawn => commands
                    .entity(entity)
                    // PERF: insert as bundle to reduce archetype moves
                    .insert((sink, PlaybackDespawnMarker)),
                PlaybackMode::Remove => commands
                    .entity(entity)
                    // PERF: insert as bundle to reduce archetype moves
                    .insert((sink, PlaybackRemoveMarker)),
            };
        } else {
            let sink = match Sink::try_new(stream_handle) {
                Ok(sink) => sink,
                Err(err) => {
                    warn!("Error creating sink: {err:?}");
                    continue;
                }
            };

            let decoder = audio_source.decoder();

            match settings.mode {
                PlaybackMode::Loop => match (settings.start_position, settings.duration) {
                    // custom start position and duration
                    (Some(start_position), Some(duration)) => sink.append(
                        decoder
                            .skip_duration(start_position)
                            .take_duration(duration)
                            .repeat_infinite(),
                    ),

                    // custom start position
                    (Some(start_position), None) => {
                        sink.append(decoder.skip_duration(start_position).repeat_infinite());
                    }

                    // custom duration
                    (None, Some(duration)) => {
                        sink.append(decoder.take_duration(duration).repeat_infinite());
                    }

                    // full clip
                    (None, None) => sink.append(decoder.repeat_infinite()),
                },
                PlaybackMode::Once | PlaybackMode::Despawn | PlaybackMode::Remove => {
                    match (settings.start_position, settings.duration) {
                        (Some(start_position), Some(duration)) => sink.append(
                            decoder
                                .skip_duration(start_position)
                                .take_duration(duration),
                        ),

                        (Some(start_position), None) => {
                            sink.append(decoder.skip_duration(start_position));
                        }

                        (None, Some(duration)) => sink.append(decoder.take_duration(duration)),

                        (None, None) => sink.append(decoder),
                    }
                }
            }

            let mut sink = AudioSink::new(sink);

            if settings.muted {
                sink.mute();
            }

            sink.set_speed(settings.speed);
            sink.set_volume(settings.volume * global_volume.volume);

            if settings.paused {
                sink.pause();
            }

            match settings.mode {
                PlaybackMode::Loop | PlaybackMode::Once => commands.entity(entity).insert(sink),
                PlaybackMode::Despawn => commands
                    .entity(entity)
                    // PERF: insert as bundle to reduce archetype moves
                    .insert((sink, PlaybackDespawnMarker)),
                PlaybackMode::Remove => commands
                    .entity(entity)
                    // PERF: insert as bundle to reduce archetype moves
                    .insert((sink, PlaybackRemoveMarker)),
            };
        }
    }
}

pub(crate) fn cleanup_finished_audio<T: Decodable + Asset>(
    mut commands: Commands,
    query_nonspatial_despawn: Query<
        (Entity, &AudioSink),
        (With<PlaybackDespawnMarker>, With<AudioPlayer<T>>),
    >,
    query_spatial_despawn: Query<
        (Entity, &SpatialAudioSink),
        (With<PlaybackDespawnMarker>, With<AudioPlayer<T>>),
    >,
    query_nonspatial_remove: Query<
        (Entity, &AudioSink),
        (With<PlaybackRemoveMarker>, With<AudioPlayer<T>>),
    >,
    query_spatial_remove: Query<
        (Entity, &SpatialAudioSink),
        (With<PlaybackRemoveMarker>, With<AudioPlayer<T>>),
    >,
) {
    for (entity, sink) in &query_nonspatial_despawn {
        if sink.sink.empty() {
            commands.entity(entity).despawn();
        }
    }
    for (entity, sink) in &query_spatial_despawn {
        if sink.sink.empty() {
            commands.entity(entity).despawn();
        }
    }
    for (entity, sink) in &query_nonspatial_remove {
        if sink.sink.empty() {
            commands.entity(entity).remove::<(
                AudioPlayer<T>,
                AudioSink,
                PlaybackSettings,
                PlaybackRemoveMarker,
            )>();
        }
    }
    for (entity, sink) in &query_spatial_remove {
        if sink.sink.empty() {
            commands.entity(entity).remove::<(
                AudioPlayer<T>,
                SpatialAudioSink,
                PlaybackSettings,
                PlaybackRemoveMarker,
            )>();
        }
    }
}

/// Run Condition to only play audio if the audio output is available
pub(crate) fn audio_output_available(audio_output: Res<AudioOutput>) -> bool {
    audio_output.stream_handle.is_some()
}

/// Updates spatial audio sinks when emitter positions change.
pub(crate) fn update_emitter_positions(
    mut emitters: Query<
        (&GlobalTransform, &SpatialAudioSink, &PlaybackSettings),
        Or<(Changed<GlobalTransform>, Changed<PlaybackSettings>)>,
    >,
    default_spatial_scale: Res<DefaultSpatialScale>,
) {
    for (transform, sink, settings) in emitters.iter_mut() {
        let scale = settings.spatial_scale.unwrap_or(default_spatial_scale.0).0;

        let translation = transform.translation() * scale;
        sink.set_emitter_position(translation);
    }
}

/// Updates spatial audio sink ear positions when spatial listeners change.
pub(crate) fn update_listener_positions(
    mut emitters: Query<(&SpatialAudioSink, &PlaybackSettings)>,
    changed_listener: Query<
        (),
        (
            Or<(
                Changed<SpatialListener>,
                Changed<GlobalTransform>,
                Changed<PlaybackSettings>,
            )>,
            With<SpatialListener>,
        ),
    >,
    ear_positions: EarPositions,
    default_spatial_scale: Res<DefaultSpatialScale>,
) {
    if !default_spatial_scale.is_changed() && changed_listener.is_empty() {
        return;
    }

    let (left_ear, right_ear) = ear_positions.get();

    for (sink, settings) in emitters.iter_mut() {
        let scale = settings.spatial_scale.unwrap_or(default_spatial_scale.0).0;

        sink.set_ears_position(left_ear * scale, right_ear * scale);
    }
}

#[cfg(test)]
mod output_device_tests {
    use super::*;

    /// 10-29: the output opened once and never again, so a reset device (HDMI, Bluetooth) went silent for good and
    /// a new Windows default was never followed.
    #[test]
    fn the_output_reopens_on_a_lost_or_moved_device() {
        assert_eq!(step(Some("Speakers"), 0, Some("Speakers")), Step::Keep, "playing where it should");
        assert_eq!(step(Some("Speakers"), 1, Some("Speakers")), Step::Keep, "one quiet tick: a device waking up");
        assert_eq!(step(Some("Speakers"), 2, Some("Speakers")), Step::Restart, "reset: cpal ends the stream");
        assert_eq!(step(Some("Speakers"), 0, Some("Headphones")), Step::Swap, "the default moved");
        assert_eq!(step(Some("Speakers"), 0, None), Step::Keep, "nothing to move to");
        assert_eq!(step(None, 0, Some("Speakers")), Step::Restart, "a device came back");
        assert_eq!(step(None, 0, None), Step::Keep);
    }

    /// Review 10-30: a fallback (the wanted device did not open) is asked for nothing, so the wanted one is tried
    /// again beside it, and it moves back once that opens instead of staying on the fallback for good.
    #[test]
    fn a_fallback_moves_back_to_the_wanted_device() {
        assert_eq!(step(Some(""), 0, Some("Headphones")), Step::Swap);
        assert_eq!(step(Some(""), 2, Some("Headphones")), Step::Restart, "a lost fallback is let go");
    }

    /// Review 10-30: one-shots queued while no output was open are dropped when one opens, not played in a burst;
    /// loops and live streams start.
    #[test]
    fn only_one_shots_queued_without_output_are_stale() {
        assert!(stale(&PlaybackSettings::DESPAWN));
        assert!(!stale(&PlaybackSettings::LOOP));
        assert!(!stale(&PlaybackSettings::ONCE));
        assert!(!stale(&PlaybackSettings::REMOVE), "nothing uses Remove; left to play as before");
    }

    /// The sink made again on a reopened output keeps what the old one did, and a one-shot (a movie's sound, dialog)
    /// carries on from where it was; a stream played Once (the game's live streams) is never skipped.
    #[test]
    fn a_reopened_sink_carries_on_from_where_it_was() {
        let (sink, mut out) = Sink::new_idle();
        sink.append(rodio::buffer::SamplesBuffer::new(1, 48_000, alloc::vec![0f32; 96_000]));
        let mut sink = AudioSink::new(sink);
        sink.set_volume(crate::Volume::Linear(0.5));
        for _ in 0..48_000 {
            out.next();
        }
        sink.pause();
        let mut once = PlaybackSettings::DESPAWN;
        carry(&mut once, &sink);
        assert!(once.paused);
        assert!((once.volume.to_linear() - 0.5).abs() < 1e-6);
        let at = once.start_position.expect("a one-shot's place").as_secs_f32();
        assert!((0.9..=1.0).contains(&at), "{at}");
        let mut live = PlaybackSettings::ONCE;
        carry(&mut live, &sink);
        assert_eq!(live.start_position, None);
    }

    #[test]
    fn the_named_output_is_chosen_else_default() {
        let names = ["Speakers".to_string(), "Headphones".to_string()];
        assert_eq!(choose_device(&names, Some("Headphones")), Some(1));
        assert_eq!(choose_device(&names, Some("Unplugged USB")), None, "gone: the default");
        assert_eq!(choose_device(&names, None), None);
    }
}
