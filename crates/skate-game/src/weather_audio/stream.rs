//! One clip playing: Vorbis decoded a packet at a time from the bytes in memory (lewton's packet API over an ogg page
//! reader). The headers are parsed once, when the bank loads ([`headers`]), so starting over (a loop's wrap, a thunder
//! voice reused) is a byte seek to the first audio page: no header parse and no allocation on the mixer thread.
//! Mono plays on both sides.
use super::assets::{Clip, ClipData};
use lewton::{
    audio::{PreviousWindowRight, read_audio_packet_generic},
    header::{IdentHeader, SetupHeader, read_header_ident, read_header_setup},
    samples::InterleavedSamples,
};
use ogg::PacketReader;
use std::{
    io::{Cursor, SeekFrom},
    sync::Arc,
};

/// A clip's parsed Vorbis headers and where its audio starts.
pub(crate) struct Headers {
    pub ident: IdentHeader,
    setup: SetupHeader,
    /// Byte offset of the first audio page (Vorbis starts audio on a fresh page after the setup header).
    audio_start: u64,
}

/// Parse the three Vorbis header packets (on the main thread, at load).
pub(crate) fn headers(bytes: &Arc<[u8]>) -> Result<Headers, String> {
    let mut reader = PacketReader::new(Cursor::new(bytes.clone()));
    let mut packet = || reader.read_packet().map_err(|e| e.to_string())?.ok_or_else(|| "truncated Vorbis headers".to_string());
    let ident = read_header_ident(&packet()?.data).map_err(|e| format!("{e:?}"))?;
    let _comment = packet()?;
    let setup = read_header_setup(&packet()?.data, ident.audio_channels, (ident.blocksize_0, ident.blocksize_1))
        .map_err(|e| format!("{e:?}"))?;
    let audio_start = reader.into_inner().position();
    Ok(Headers { ident, setup, audio_start })
}

type Reader = PacketReader<Cursor<Arc<[u8]>>>;

pub(crate) struct LoopVoice {
    clip: Clip,
    reader: Option<Reader>,
    pwr: PreviousWindowRight,
    /// Frames decoded from the stream's start, when known (unknown after a granule seek until a page ends).
    pos: Option<u64>,
    /// The decoded packet (interleaved) and the next sample in it. For a PCM clip, `at` indexes the clip itself.
    buf: Vec<f32>,
    at: usize,
}

impl LoopVoice {
    /// Starts `start_fraction` of the way in (to the Ogg page at or before it: a seek, not a decode).
    pub(crate) fn new(clip: &Clip, start_fraction: f32) -> Option<Self> {
        let start = (f64::from(start_fraction.clamp(0., 1.)) * clip.frames as f64) as u64;
        let mut voice =
            Self { clip: clip.clone(), reader: None, pwr: PreviousWindowRight::new(), pos: Some(0), buf: Vec::new(), at: 0 };
        match &clip.data {
            ClipData::Ogg(bytes, headers) => {
                let mut reader = PacketReader::new(Cursor::new(bytes.clone()));
                if start > 0 && reader.seek_absgp(None, start).is_ok() {
                    voice.pos = None;
                } else {
                    reader.seek_bytes(SeekFrom::Start(headers.audio_start)).ok()?;
                }
                voice.reader = Some(reader);
            }
            #[cfg(test)]
            ClipData::Pcm(_) => voice.at = (start.min(clip.frames.saturating_sub(1)) as usize) * clip.channels,
        }
        Some(voice)
    }

    /// Back to the clip's first sample, without allocating.
    pub(crate) fn rewind(&mut self) {
        if let (Some(reader), ClipData::Ogg(_, headers)) = (self.reader.as_mut(), &self.clip.data) {
            let _ = reader.seek_bytes(SeekFrom::Start(headers.audio_start));
        }
        self.pwr = PreviousWindowRight::new();
        self.pos = Some(0);
        self.buf.clear();
        self.at = 0;
    }

    /// The next decoded packet's samples (empty for the first packet after a start, as Vorbis overlaps windows);
    /// None at the stream's end.
    fn packet(&mut self) -> Option<Vec<f32>> {
        let ClipData::Ogg(_, headers) = &self.clip.data else { return None };
        let reader = self.reader.as_mut()?;
        // Packets that do not decode (a header page a granule seek landed on) are skipped.
        loop {
            let packet = reader.read_packet().ok().flatten()?;
            let Ok(samples) =
                read_audio_packet_generic::<InterleavedSamples<f32>>(&headers.ident, &headers.setup, &packet.data, &mut self.pwr)
            else {
                continue;
            };
            let mut samples = samples.samples;
            let channels = self.clip.channels.max(1);
            // The last packet decodes past the stream's end; its page's granule says where the end is.
            if packet.last_in_stream()
                && let Some(pos) = self.pos
            {
                samples.truncate((packet.absgp_page().saturating_sub(pos) as usize * channels).min(samples.len()));
            }
            let frames = (samples.len() / channels) as u64;
            self.pos = if packet.last_in_page() { Some(packet.absgp_page()) } else { self.pos.map(|p| p + frames) };
            return Some(samples);
        }
    }

    /// The next stereo frame; None once a one-shot has finished (or a clip will not decode).
    pub(crate) fn next_frame(&mut self) -> Option<[f32; 2]> {
        let channels = self.clip.channels;
        #[cfg(test)]
        if let ClipData::Pcm(pcm) = &self.clip.data {
            if self.at + channels > pcm.len() {
                if !self.clip.looped {
                    return None;
                }
                self.at = 0;
            }
            let frame = [pcm[self.at], pcm[self.at + channels - 1]];
            self.at += channels;
            return Some(frame);
        }
        let mut restarted = false;
        // Empty packets (the first after a start) are skipped; a clip that never yields a sample ends.
        let mut empty = 0;
        while self.at + channels > self.buf.len() {
            match self.packet() {
                // ponytail: lewton hands each packet over in a new Vec (~21 ms of audio); our own buffers are reused.
                Some(samples) => {
                    empty = if samples.is_empty() { empty + 1 } else { 0 };
                    if empty > 64 {
                        return None;
                    }
                    self.buf = samples;
                    self.at = 0;
                }
                None if self.clip.looped && !restarted => {
                    restarted = true;
                    self.rewind();
                }
                None => return None,
            }
        }
        let frame = [self.buf[self.at], self.buf[self.at + channels - 1]];
        self.at += channels;
        Some(frame)
    }

    /// Adds `gain`-scaled stereo into `out` (interleaved). False when a one-shot has finished.
    pub(crate) fn mix_into(&mut self, out: &mut [f32], gain: [f32; 2]) -> bool {
        for frame in out.chunks_exact_mut(2) {
            let Some([l, r]) = self.next_frame() else { return false };
            frame[0] += l * gain[0];
            frame[1] += r * gain[1];
        }
        true
    }
}
