//! A speaker's jitter buffer: packets in, frames out in `seq` order when due, at an adaptive 40-200 ms delay; losses
//! concealed (or rebuilt from the next packet's FEC); late bursts and stale audio dropped, never replayed.
use crate::codec::{Decoder, Packet};
use std::collections::{BTreeMap, VecDeque};

const MIN_DEPTH_MS: u32 = 40;
const MAX_DEPTH_MS: u32 = 200;
/// A late burst keeps at most this much more than the stream's delay (and two frames).
const BURST_MS: u32 = 80;
/// A frame due this long ago (the listener stalled: a hitch) is dropped, not played late.
const STALE_MS: u64 = 200;
/// A `seq` this far (2 s in 10 ms ticks) from the stream's, either way: the sender restarted, the stream starts over.
const JUMP_TICKS: u32 = 200;
/// Arrivals the jitter is judged over (a second of 20 ms packets).
const WINDOW: usize = 50;

pub struct Jitter {
    decoder: Decoder,
    /// Received and not played yet, by `seq`, with when each arrived; all at or after `next`.
    packets: BTreeMap<u32, (Packet, u64)>,
    /// The next 10 ms tick to play and when it is due (local ms); `None`: idle, the next packet starts the stream.
    next: Option<(u32, u64)>,
    depth_ms: u32,
    /// The delay this stream plays at: [`Self::depth_ms`] as it started, later by each packet that came after its turn.
    delay_ms: u32,
    /// Recent arrival offsets (arrival − 10 · seq): their spread is the jitter.
    offsets: VecDeque<i64>,
}

impl Jitter {
    pub fn new() -> Self {
        Self { decoder: Decoder::new(), packets: BTreeMap::new(), next: None, depth_ms: MIN_DEPTH_MS, delay_ms: MIN_DEPTH_MS, offsets: VecDeque::new() }
    }

    pub fn push(&mut self, p: Packet, arrival_ms: u64) {
        if let Some((next, due)) = self.next {
            if p.seq.abs_diff(next) > JUMP_TICKS {
                *self = Self::new();
            } else if p.seq < next {
                // Its turn has passed (more jitter than the delay): the stream plays on that much later, up to the deepest
                // delay, so the ones after it are heard (audit 10-30: the delay was set only as a stream started).
                let missed = due.saturating_sub(10 * u64::from(next - p.seq));
                let later = arrival_ms.saturating_sub(missed).min(u64::from(MAX_DEPTH_MS.saturating_sub(self.delay_ms)));
                self.next = Some((next, due + later));
                self.delay_ms += later as u32;
                return;
            }
        }
        self.offsets.push_back(arrival_ms as i64 - 10 * i64::from(p.seq));
        if self.offsets.len() > WINDOW {
            self.offsets.pop_front();
        }
        let spread = self.offsets.iter().max().unwrap() - self.offsets.iter().min().unwrap();
        self.depth_ms = (spread.clamp(0, i64::from(MAX_DEPTH_MS)) as u32 + 20).clamp(MIN_DEPTH_MS, MAX_DEPTH_MS);
        if self.next.is_none() {
            self.next = Some((p.seq, arrival_ms + u64::from(self.depth_ms)));
            self.delay_ms = self.depth_ms;
        }
        let limit = (self.delay_ms.max(BURST_MS) + 2 * u32::from(p.frame_ms)) / 10;
        self.packets.insert(p.seq, (p, arrival_ms));
        // A late burst: keep the newest audio, at most the stream's delay (80 ms at least) and two frames of it (a packet can still be
        // waiting to play when the two after it arrive together: 60 ms packets up to 30 ms early or late).
        while self.queued_ticks() > limit && self.packets.len() > 1 {
            self.packets.pop_first();
            if let (Some((next, _)), Some(&first)) = (&mut self.next, self.packets.keys().next()) {
                *next = first;
            }
        }
    }

    /// One frame of audio when it is due: the packet for it, or (lost) its FEC from the packet after it, or 10 ms of
    /// concealment. `None` when nothing is due, or nothing is left to play (the speaker stopped).
    pub fn pull(&mut self, now_ms: u64) -> Option<Vec<f32>> {
        let (mut next, mut due) = self.next?;
        if due + STALE_MS < now_ms {
            // The listener stalled (a hitch): what arrived more than the delay and 200 ms ago is dropped, the rest plays
            // on from now.
            let old = now_ms.saturating_sub(u64::from(self.depth_ms) + STALE_MS);
            self.packets.retain(|_, (_, at)| *at >= old);
            let Some(&seq) = self.packets.keys().next() else {
                self.next = None;
                return None;
            };
            (next, due) = (seq, now_ms);
        }
        if due > now_ms {
            return None;
        }
        let (audio, ticks) = if let Some((p, _)) = self.packets.remove(&next) {
            (self.decoder.decode(Some(&p.opus), u32::from(p.frame_ms), false), u32::from(p.frame_ms) / 10)
        } else if let Some((&later, (p, _))) = self.packets.iter().next() {
            if later - next == u32::from(p.frame_ms) / 10 {
                // Exactly one frame missing: the packet after it carries it as FEC.
                (self.decoder.decode(Some(&p.opus), u32::from(p.frame_ms), true), later - next)
            } else {
                (self.decoder.decode(None, 10, false), 1)
            }
        } else {
            self.next = None;
            return None;
        };
        self.next = Some((next.saturating_add(ticks), due + u64::from(ticks) * 10));
        Some(audio)
    }

    /// The delay a stream starts at (40..=200 ms), from the recent jitter.
    pub fn depth_ms(&self) -> u32 {
        self.depth_ms
    }

    /// The audio waiting (10 ms ticks): from the next tick to the end of the newest packet.
    fn queued_ticks(&self) -> u32 {
        match (self.next, self.packets.last_key_value()) {
            (Some((next, _)), Some((&seq, (p, _)))) => seq.saturating_add(u32::from(p.frame_ms) / 10).saturating_sub(next),
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{
        Encoder,
        tests::{rms, tones},
    };

    /// `n` 20 ms packets of the test tone from `seq`, frame `i` at `amp(i)`.
    fn packets(seq: u32, n: usize, amp: impl Fn(usize) -> f32) -> Vec<Packet> {
        let mut encoder = Encoder::new();
        (0..n).map(|i| Packet { seq: seq + 2 * i as u32, frame_ms: 20, opus: encoder.encode(&tones(960 * i, 20, amp(i)), 20, 24_000) }).collect()
    }

    /// A receiver in 10 ms ticks to `end` ms: each (arrival, packet) pushed in its tick, then every frame due pulled.
    /// The frames, with the tick each came out in.
    fn run(jitter: &mut Jitter, arrivals: &[(u64, Packet)], end: u64) -> Vec<(u64, Vec<f32>)> {
        let mut out = vec![];
        for t in (0..end).step_by(10) {
            for (at, p) in arrivals.iter().filter(|(at, _)| (t..t + 10).contains(at)) {
                jitter.push(p.clone(), *at);
            }
            while let Some(frame) = jitter.pull(t) {
                out.push((t, frame));
            }
        }
        out
    }

    #[test]
    fn reordered_packets_play_in_order() {
        // Six frames, each 6 dB louder than the last; 1 and 4 arrive after the frame behind them.
        let ladder = packets(0, 6, |i| 0.02 * 2f32.powi(i as i32));
        let arrivals: Vec<_> = [(0, 0), (40, 2), (45, 1), (60, 3), (100, 5), (105, 4)].map(|(at, i)| (at, ladder[i].clone())).into();
        let played: Vec<f32> = run(&mut Jitter::new(), &arrivals, 400).iter().map(|(_, f)| rms(f)).collect();
        assert_eq!(played.len(), 6);
        assert!(played.windows(2).all(|w| w[1] > w[0]), "louder each frame, so in order: {played:?}");
    }

    #[test]
    fn a_lost_packet_is_concealed_not_silent() {
        let tone = packets(0, 10, |_| 0.3);
        let arrivals: Vec<_> = tone.iter().enumerate().filter(|(i, _)| *i != 5).map(|(i, p)| (20 * i as u64, p.clone())).collect();
        let played = run(&mut Jitter::new(), &arrivals, 500);
        let ms: usize = played.iter().map(|(_, f)| f.len() / 48).sum();
        assert_eq!(ms, 200, "the whole 200 ms, the lost 20 ms included");
        assert!(played.iter().all(|(_, f)| rms(f) > 0.3 * rms(&tones(0, 20, 0.3))), "no silent frame");
    }

    #[test]
    fn a_late_burst_is_dropped_not_replayed() {
        // Nothing for 2 s, then 2 s of packets at once; only the last six (120 ms) are loud.
        let burst = packets(0, 100, |i| if i >= 94 { 0.3 } else { 0.01 });
        let mut jitter = Jitter::new();
        for p in &burst {
            jitter.push(p.clone(), 2000);
        }
        let played = run(&mut jitter, &[], 3000);
        let ms: usize = played.iter().map(|(_, f)| f.len() / 48).sum();
        assert!(ms <= 80 + 2 * 20, "{ms} ms of a 2 s backlog played");
        assert!(!played.is_empty() && played.iter().all(|(_, f)| rms(f) > 0.05), "the newest audio");
    }

    /// The listener stalls for a second (a hitch): then it plays on at once from what just arrived, not the second it
    /// missed, and not as a burst.
    #[test]
    fn a_stalled_listener_plays_on_from_now() {
        let tone = packets(0, 150, |i| if i >= 90 { 0.3 } else { 0.02 });
        let mut jitter = Jitter::new();
        let mut after = vec![];
        for t in (0..2400).step_by(10) {
            for p in tone.iter().filter(|p| (t..t + 10).contains(&(10 * u64::from(p.seq)))) {
                jitter.push(p.clone(), t);
            }
            if (1000..2000).contains(&t) {
                continue;
            }
            while let Some(frame) = jitter.pull(t) {
                if t >= 2000 {
                    after.push((t, rms(&frame)));
                }
            }
        }
        assert!(after.first().is_some_and(|(t, _)| *t <= 2010), "plays on at once: {:?}", after.first());
        assert!(after.iter().all(|(_, level)| *level > 0.1), "only what just arrived");
        assert_eq!(after.iter().filter(|(t, _)| *t == after[0].0).count(), 1, "no burst of backlog");
    }

    #[test]
    fn depth_stays_between_40_and_80_ms() {
        let tone = packets(0, 150, |_| 0.2);
        let steady: Vec<_> = tone.iter().enumerate().map(|(i, p)| (20 * i as u64, p.clone())).collect();
        let mut jitter = Jitter::new();
        run(&mut jitter, &steady, 3200);
        assert_eq!(jitter.depth_ms(), 40, "no jitter: the least delay");
        // Arrivals up to 30 ms early or late, reordering some.
        let mut x = 0x2545_f491_4f6c_dd1du64;
        let mut arrivals: Vec<_> = tone
            .iter()
            .enumerate()
            .map(|(i, p)| {
                (x ^= x << 13, x ^= x >> 7, x ^= x << 17);
                (20 * i as u64 + 30 + x % 61 - 30, p.clone())
            })
            .collect();
        arrivals.sort_by_key(|(at, _)| *at);
        let mut jitter = Jitter::new();
        let mut depths = vec![];
        for t in (0..3200).step_by(10) {
            for (at, p) in arrivals.iter().filter(|(at, _)| (t..t + 10).contains(at)) {
                jitter.push(p.clone(), *at);
            }
            while jitter.pull(t).is_some() {}
            depths.push(jitter.depth_ms());
        }
        assert!(depths.iter().all(|d| (40..=80).contains(d)), "{depths:?}");
        assert!(*depths.last().unwrap() > 40, "deeper for the jitter");
    }

    /// 60 ms packets (a congested link, or more than 24 listeners) up to 30 ms early or late: every one is heard. A
    /// packet can still be waiting to play when the two after it arrive together; the late-burst trim keeps it.
    #[test]
    fn a_jittery_60_ms_stream_plays_every_frame() {
        let mut encoder = Encoder::new();
        let mut packet = |seq: u32, amp: f32| Packet { seq, frame_ms: 60, opus: encoder.encode(&tones(0, 60, amp), 60, 12_000) };
        // A first spurt, 30 ms late then 30 ms early: that spread sets the deepest delay, 80 ms.
        let mut arrivals = vec![(30, packet(0, 0.1)), (30, packet(6, 0.1))];
        // After a pause, a spurt of eight: its first 30 ms late, the rest 30 ms early.
        for i in 0..8u32 {
            let seq = 120 + 6 * i;
            arrivals.push((10 * u64::from(seq) + if i == 0 { 30 } else { 0 } - if i == 0 { 0 } else { 30 }, packet(seq, 0.1)));
        }
        let played = run(&mut Jitter::new(), &arrivals, 3000);
        let sizes: Vec<usize> = played.iter().filter(|(t, _)| *t >= 1000).map(|(_, f)| f.len() / 48).collect();
        assert_eq!(sizes, [60; 8], "every packet decoded whole: none trimmed, none concealed");
    }

    /// Audit 10-30: a Wi-Fi player's packets up to 120 ms apart in lateness through one long Open Mic stretch. Once
    /// one comes after its turn the stream plays on that much later, so the rest are heard whole; before, the delay was
    /// fixed as the stream started (at most 80 ms) and every packet past it was concealed.
    #[test]
    fn a_long_jittery_stream_plays_on_later_and_every_frame_is_heard() {
        let tone = packets(0, 200, |_| 0.2);
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        let mut arrivals: Vec<_> = tone
            .iter()
            .enumerate()
            .map(|(i, p)| {
                (x ^= x << 13, x ^= x >> 7, x ^= x << 17);
                // 0-120 ms behind the earliest; the first is the earliest, the eleventh the latest.
                let behind = match i { 0 => 0, 10 => 120, _ => x % 121 };
                (100 + 20 * i as u64 + behind, p.clone())
            })
            .collect();
        arrivals.sort_by_key(|(at, _)| *at);
        let played = run(&mut Jitter::new(), &arrivals, 4500);
        let after: Vec<usize> = played.iter().filter(|(t, _)| *t >= 600).map(|(_, f)| f.len() / 48).collect();
        assert!(after.len() >= 175 && after.iter().all(|&ms| ms == 20), "every frame whole once settled: {after:?}");
    }

    #[test]
    fn a_speaker_heard_mid_sentence_starts_at_once() {
        let arrivals: Vec<_> = packets(90_000, 20, |_| 0.3).into_iter().enumerate().map(|(i, p)| (5000 + 20 * i as u64, p)).collect();
        let played = run(&mut Jitter::new(), &arrivals, 6000);
        assert!(played.first().is_some_and(|(t, _)| *t - 5000 <= 80), "first audio at {:?}", played.first().map(|p| p.0));
    }

    #[test]
    fn a_restarted_sender_is_heard_again() {
        // Quiet from a clock at 90_000, then the sender restarts (a Microphone change): loud from seq 0.
        let before = packets(90_000, 50, |_| 0.02).into_iter().enumerate().map(|(i, p)| (20 * i as u64, p));
        let after = packets(0, 20, |_| 0.3).into_iter().enumerate().map(|(i, p)| (1000 + 20 * i as u64, p));
        let arrivals: Vec<_> = before.chain(after).collect();
        let played = run(&mut Jitter::new(), &arrivals, 2000);
        let first_loud = played.iter().find(|(_, f)| rms(f) > 0.1).map(|(t, _)| *t);
        assert!(first_loud.is_some_and(|t| t - 1000 <= 80), "heard again at {first_loud:?}");
    }
}
