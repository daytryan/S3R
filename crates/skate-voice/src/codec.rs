//! Opus for voice: libopus (unsafe-libopus) encodes and opus-rs decodes (DECISIONS.md); the voice datagram's payload; the
//! frame size and bitrate a lobby's size calls for.

/// A voice datagram's payload at most: `[frame_ms]` and one Opus packet.
pub const MAX_PAYLOAD: usize = 400;

/// The Opus frame (ms) for a lobby of `listeners`: up to 8 → 20, up to 24 → 40, more → 60; while the lobby is congested,
/// 60 at any size (movement first).
pub fn frame_ms_for(listeners: usize, congested: bool) -> u32 {
    match listeners {
        _ if congested => 60,
        0..=8 => 20,
        9..=24 => 40,
        _ => 60,
    }
}

/// The bitrate (bps) to go with [`frame_ms_for`].
pub fn bitrate_for(listeners: usize, congested: bool) -> u32 {
    match listeners {
        _ if congested => 12_000,
        0..=8 => 24_000,
        9..=24 => 16_000,
        _ => 12_000,
    }
}

/// libopus: VoIP, in-band FEC for 10 % loss, DTX.
pub struct Encoder {
    opus: *mut unsafe_libopus::OpusEncoder,
    bitrate: u32,
}

// SAFETY: the encoder state is one heap block that only this value touches; it moves to the voice thread, never shared.
unsafe impl Send for Encoder {}

impl Encoder {
    pub fn new() -> Self {
        let mut error = 0;
        let opus = unsafe { unsafe_libopus::opus_encoder_create(48_000, 1, unsafe_libopus::OPUS_APPLICATION_VOIP, &mut error) };
        assert!(error == unsafe_libopus::OPUS_OK && !opus.is_null(), "libopus encoder: {error}");
        unsafe {
            unsafe_libopus::opus_encoder_ctl!(opus, unsafe_libopus::OPUS_SET_INBAND_FEC_REQUEST, 1);
            unsafe_libopus::opus_encoder_ctl!(opus, unsafe_libopus::OPUS_SET_PACKET_LOSS_PERC_REQUEST, 10);
            unsafe_libopus::opus_encoder_ctl!(opus, unsafe_libopus::OPUS_SET_DTX_REQUEST, 1);
        }
        Self { opus, bitrate: 0 }
    }
    /// One Opus packet for `pcm` (`48 * frame_ms` samples) at `bitrate`, sized to fit a payload; empty if libopus
    /// refuses it.
    pub fn encode(&mut self, pcm: &[f32], frame_ms: u32, bitrate: u32) -> Vec<u8> {
        if pcm.len() != 48 * frame_ms as usize {
            return vec![];
        }
        if bitrate != self.bitrate {
            unsafe { unsafe_libopus::opus_encoder_ctl!(self.opus, unsafe_libopus::OPUS_SET_BITRATE_REQUEST, bitrate as i32) };
            self.bitrate = bitrate;
        }
        let mut packet = [0u8; MAX_PAYLOAD - 1];
        let n = unsafe { unsafe_libopus::opus_encode_float(self.opus, pcm.as_ptr(), pcm.len() as i32, packet.as_mut_ptr(), packet.len() as i32) };
        packet[..n.max(0) as usize].to_vec()
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe { unsafe_libopus::opus_encoder_destroy(self.opus) };
    }
}

/// opus-rs. Packets come from other players, so one that makes it panic is never fatal: the decoder is replaced and the
/// frame is silence.
pub struct Decoder {
    opus: opus_rs::OpusDecoder,
}

impl Decoder {
    pub fn new() -> Self {
        Self { opus: opus_rs::OpusDecoder::new(48_000, 1).expect("48 kHz mono") }
    }
    /// `frame_ms` of audio: `packet` decoded, `None` (lost) concealed, or with `fec` this gap rebuilt from the in-band
    /// FEC of `packet`, the one after it. A packet that won't decode gives silence.
    pub fn decode(&mut self, packet: Option<&[u8]>, frame_ms: u32, fec: bool) -> Vec<f32> {
        let samples = 48 * frame_ms as usize;
        let mut out = vec![0f32; samples];
        let opus = &mut self.opus;
        let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match packet {
            Some(p) if fec => opus.decode_fec(p, samples, &mut out),
            Some(p) => opus.decode(p, samples, &mut out),
            None => opus.decode(&[], samples, &mut out),
        }));
        match decoded {
            Ok(Ok(n)) if n == samples => out,
            Ok(_) => vec![0.; samples],
            Err(_) => {
                *self = Self::new();
                vec![0.; samples]
            }
        }
    }
}

/// A voice datagram's content: `seq` (from the header: the frame's start in 10 ms ticks of the sender's voice clock) and
/// one Opus packet of `frame_ms`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    pub seq: u32,
    pub frame_ms: u8,
    pub opus: Vec<u8>,
}

impl Packet {
    /// `[frame_ms]` and the Opus packet.
    pub fn payload(&self) -> Vec<u8> {
        let mut payload = vec![self.frame_ms];
        payload.extend(&self.opus);
        payload
    }
    /// A received payload: `None` unless the frame is 20, 40 or 60 ms and Opus bytes follow, all within
    /// [`MAX_PAYLOAD`].
    pub fn parse(seq: u32, payload: &[u8]) -> Option<Packet> {
        let (&frame_ms, opus) = payload.split_first()?;
        (matches!(frame_ms, 20 | 40 | 60) && !opus.is_empty() && payload.len() <= MAX_PAYLOAD).then(|| Packet { seq, frame_ms, opus: opus.to_vec() })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    /// `ms` of `amp` x (a 300 Hz + 1 kHz mix), from sample `start`.
    pub(crate) fn tones(start: usize, ms: usize, amp: f32) -> Vec<f32> {
        let w = |f: f32, i: usize| (std::f32::consts::TAU * f * i as f32 / 48_000.).sin();
        (start..start + 48 * ms).map(|i| amp * 0.5 * (w(300., i) + w(1000., i))).collect()
    }

    #[test]
    fn frame_sizes_follow_the_lobby() {
        assert_eq!([8, 9, 24, 25].map(|n| frame_ms_for(n, false)), [20, 40, 40, 60]);
        assert_eq!([8, 9, 24, 25].map(|n| bitrate_for(n, false)), [24_000, 16_000, 16_000, 12_000]);
        assert_eq!((frame_ms_for(2, true), bitrate_for(2, true)), (60, 12_000), "congested: movement first");
    }

    #[test]
    fn a_round_trip_keeps_the_signal() {
        let (mut encoder, mut decoder) = (Encoder::new(), Decoder::new());
        let pcm = tones(0, 1000, 0.5);
        let out: Vec<f32> = pcm.chunks(960).flat_map(|frame| decoder.decode(Some(&encoder.encode(frame, 20, 24_000)), 20, false)).collect();
        let db = 20. * (rms(&out) / rms(&pcm)).log10();
        assert!(out.len() == pcm.len() && db.abs() <= 3., "{db} dB");
    }

    #[test]
    fn a_bad_payload_is_refused() {
        let good = Packet { seq: 7, frame_ms: 20, opus: vec![1, 2, 3] };
        assert_eq!(Packet::parse(7, &good.payload()), Some(good));
        assert_eq!(Packet::parse(7, &[0, 1, 2]), None, "frame_ms 0");
        assert_eq!(Packet::parse(7, &[30, 1, 2]), None, "frame_ms 30");
        assert_eq!(Packet::parse(7, &[20]), None, "no Opus bytes");
        assert_eq!(Packet::parse(7, &[]), None);
        let mut big = vec![20];
        big.resize(MAX_PAYLOAD + 1, 9);
        assert_eq!(Packet::parse(7, &big), None, "over 400 bytes");
    }

    /// A packet from another player is untrusted: no bytes panic the decoder, and every call gives a whole frame.
    #[test]
    fn random_packets_never_panic_the_decoder() {
        let mut decoder = Decoder::new();
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            (x ^= x << 13, x ^= x >> 7, x ^= x << 17);
            x
        };
        for i in 0..20_000 {
            let packet: Vec<u8> = (0..1 + next() % 399).map(|_| next() as u8).collect();
            let ms = [20, 40, 60][i % 3];
            let out = decoder.decode(Some(&packet), ms, i % 7 == 0);
            assert_eq!(out.len(), 48 * ms as usize);
            assert!(out.iter().all(|s| s.is_finite()));
        }
        assert_eq!(decoder.decode(None, 20, false).len(), 960, "and it still conceals");
    }
}
