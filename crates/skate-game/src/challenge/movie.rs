//! Full-screen movies for the tutorial ports: EA `.vp6` decoded by an ffmpeg pipe (rawvideo rgba) on a background
//! thread, its audio as a PCM WAV the data scripts decoded, shown on a UI image above everything. The caller pauses
//! the game while one plays. Without ffmpeg the movie reports itself finished at once, with a warning.
use crate::skate_audio::StreamPcm;
use bevy::{
    asset::RenderAssetUsages,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use std::{
    io::Read,
    path::Path,
    process::{Child, Stdio},
    sync::{
        Mutex,
        mpsc::{Receiver, TryRecvError, sync_channel},
    },
    time::Instant,
};

/// ffmpeg scales every movie to this; Skate 2's are 1280x720 already.
const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;
/// Output frame rate ffmpeg resamples to, so the frame index is the clock times this.
const FPS: f64 = 30000. / 1001.;

pub(crate) struct Movie {
    // Mutex only to make the player Sync for a Bevy resource; one thread ever reads it.
    frames: Mutex<Receiver<Vec<u8>>>,
    child: Child,
    image: Handle<Image>,
    root: Entity,
    audio: Option<Entity>,
    // ponytail: wall clock started with the audio; the audio sink's own position if drift shows.
    started: Instant,
    shown: u64,
    ended: bool,
}

impl Movie {
    /// Starts `video` (and `wav`, when it exists) full screen on `camera`'s UI.
    pub(crate) fn start(
        commands: &mut Commands,
        images: &mut Assets<Image>,
        audio: &mut Assets<StreamPcm>,
        camera: Entity,
        video: &Path,
        wav: Option<&Path>,
    ) -> Result<Self, String> {
        let mut child = skate_data::audio::ffmpeg::command()
            .args(["-v", "error", "-nostdin", "-i"])
            .arg(video)
            .args(["-an", "-vf", &format!("scale={WIDTH}:{HEIGHT}"), "-r", "30000/1001", "-f", "rawvideo", "-pix_fmt", "rgba", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("cannot run ffmpeg for {}: {e}", video.display()))?;
        let mut stdout = child.stdout.take().ok_or("ffmpeg has no stdout")?;
        let (send, frames) = sync_channel::<Vec<u8>>(8);
        std::thread::Builder::new()
            .name("movie-decode".into())
            .spawn(move || {
                loop {
                    let mut frame = vec![0u8; (WIDTH * HEIGHT * 4) as usize];
                    if stdout.read_exact(&mut frame).is_err() || send.send(frame).is_err() {
                        return;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        let image = images.add(Image::new_fill(
            Extent3d { width: WIDTH, height: HEIGHT, depth_or_array_layers: 1 },
            TextureDimension::D2,
            &[0, 0, 0, 255],
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        ));
        let root = commands
            .spawn((
                Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() },
                BackgroundColor(Color::BLACK),
                ImageNode::new(image.clone()),
                UiTargetCamera(camera),
                GlobalZIndex(60),
                Pickable::IGNORE,
            ))
            .id();
        let audio = match wav.filter(|p| p.is_file()).map(read_wav).transpose() {
            Ok(pcm) => pcm.map(|pcm| commands.spawn((AudioPlayer(audio.add(pcm)), PlaybackSettings::DESPAWN)).id()),
            Err(error) => {
                warn!("{}: movie audio unavailable ({error})", video.display());
                None
            }
        };
        Ok(Self { frames: Mutex::new(frames), child, image, root, audio, started: Instant::now(), shown: 0, ended: false })
    }

    /// Shows the frame the clock is at; true once the video has run out.
    pub(crate) fn advance(&mut self, images: &mut Assets<Image>) -> bool {
        let due = (self.started.elapsed().as_secs_f64() * FPS) as u64;
        let mut latest = None;
        let frames = self.frames.get_mut().unwrap();
        while self.shown <= due && !self.ended {
            match frames.try_recv() {
                Ok(frame) => {
                    latest = Some(frame);
                    self.shown += 1;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => self.ended = true,
            }
        }
        if let (Some(frame), Some(image)) = (latest, images.get_mut(&self.image)) {
            image.data = Some(frame);
        }
        self.ended
    }

    pub(crate) fn stop(mut self, commands: &mut Commands, images: &mut Assets<Image>) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        commands.entity(self.root).despawn();
        if let Some(audio) = self.audio {
            commands.entity(audio).try_despawn();
        }
        images.remove(&self.image);
    }
}

/// A 16-bit PCM WAV (what the data scripts write).
pub(crate) fn read_wav(path: &Path) -> Result<StreamPcm, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF WAVE file".into());
    }
    let (mut format, mut at) = (None, 12);
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = bytes.get(at + 8..at + 8 + size).ok_or("truncated chunk")?;
        match id {
            b"fmt " if body.len() >= 16 => {
                let u16_at = |o: usize| u16::from_le_bytes([body[o], body[o + 1]]);
                let rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
                if u16_at(0) != 1 || u16_at(14) != 16 {
                    return Err("only 16-bit PCM".into());
                }
                format = Some((u16_at(2), rate));
            }
            b"data" => {
                let (channels, rate) = format.ok_or("data before fmt")?;
                let samples = body.chunks_exact(2).map(|s| i16::from_le_bytes([s[0], s[1]])).collect();
                return Ok(StreamPcm::from_interleaved(samples, channels, rate));
            }
            _ => {}
        }
        at += 8 + size + (size & 1);
    }
    Err("no data chunk".into())
}
