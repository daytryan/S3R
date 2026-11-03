//! AUDIO (kind 15): the sender's own per-frame skater audio record, the 0x220 bytes the title's record builder
//! `sub_827A1B78` writes into the audio frame for each skater (TU3; frame `+0xF0`, up to 4 records: 0x827A1220).
//! Retail plays other skaters with its own player code from their records (`sub_824F1FB0`), so the record is the
//! smallest replicated data that reproduces their sounds. A datagram carries the frames since the last one plus a
//! few before (loss), each as the words that changed from the frame before it (the first: from zero), so every
//! datagram decodes on its own: no baseline, no acks.
use crate::packed::Reader;

pub const AUDIO: u8 = 15;
/// 0x220 bytes as big-endian words (the guest's).
pub const RECORD_WORDS: usize = 0x220 / 4;
pub type Record = [u32; RECORD_WORDS];
const MASK: usize = RECORD_WORDS.div_ceil(8);
/// Frames a datagram holds at most.
pub const MAX_FRAMES: usize = 8;

/// `frames` (tick, record), oldest first, ticks rising by 1..=255: `[count][first tick u32]`, then per frame
/// `[tick step u8]` (not for the first) `[changed-word mask][changed words, u32 LE]`.
pub fn encode(frames: &[(u32, Record)]) -> Vec<u8> {
    let mut b = vec![frames.len() as u8];
    b.extend(frames.first().map_or(0, |f| f.0).to_le_bytes());
    let mut last = [0u32; RECORD_WORDS];
    for (n, (tick, record)) in frames.iter().enumerate() {
        if n > 0 {
            b.push(tick.wrapping_sub(frames[n - 1].0) as u8);
        }
        let mut mask = [0u8; MASK];
        for i in 0..RECORD_WORDS {
            if record[i] != last[i] {
                mask[i / 8] |= 1 << (i % 8);
            }
        }
        b.extend(mask);
        for i in (0..RECORD_WORDS).filter(|&i| record[i] != last[i]) {
            b.extend(record[i].to_le_bytes());
        }
        last = *record;
    }
    b
}

/// The newest frames of `frames` (oldest first) whose encoding fits `room` bytes, at most [`MAX_FRAMES`]; empty
/// when not even the newest fits (it always does: 17 + 544 bytes < MTU).
pub fn pack(frames: &[(u32, Record)], room: usize) -> Vec<u8> {
    let consecutive = frames.windows(2).rposition(|w| !(1..=255).contains(&w[1].0.wrapping_sub(w[0].0))).map_or(0, |i| i + 1);
    // ponytail: re-encodes per dropped frame, at most MAX_FRAMES times.
    for start in frames.len().saturating_sub(MAX_FRAMES).max(consecutive)..frames.len() {
        let b = encode(&frames[start..]);
        if b.len() <= room {
            return b;
        }
    }
    vec![]
}

pub fn decode(b: &[u8]) -> Option<Vec<(u32, Record)>> {
    let mut r = Reader(b);
    let count = r.byte()? as usize;
    if count == 0 || count > MAX_FRAMES {
        return None;
    }
    let mut tick = r.u32()?;
    let mut last = [0u32; RECORD_WORDS];
    let mut out = Vec::with_capacity(count);
    for n in 0..count {
        if n > 0 {
            let step = r.byte()?;
            if step == 0 {
                return None;
            }
            tick = tick.wrapping_add(u32::from(step));
        }
        let mask: [u8; MASK] = r.take()?;
        for i in (0..RECORD_WORDS).filter(|i| mask[i / 8] & (1 << (i % 8)) != 0) {
            last[i] = r.u32()?;
        }
        out.push((tick, last));
    }
    r.0.is_empty().then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn record(seed: u32) -> Record {
        let mut r = [0; RECORD_WORDS];
        // A few moving words (position, speeds) over a mostly constant record.
        for (i, w) in r.iter_mut().enumerate().take(60) {
            *w = if i < 12 { (seed + 1).wrapping_mul(2654435761).wrapping_add(i as u32) } else { 0x3F80_0000 + i as u32 };
        }
        r
    }

    #[test]
    fn frames_round_trip_each_from_the_one_before() {
        let frames: Vec<_> = (0..5).map(|n| (1000 + n * 2, record(n))).collect();
        let wire = encode(&frames);
        assert_eq!(decode(&wire).unwrap(), frames);
        // Deltas: only the 12 moving words after the first frame.
        assert_eq!(wire.len(), 1 + 4 + (MASK + 60 * 4) + 4 * (1 + MASK + 12 * 4));
    }

    #[test]
    fn pack_keeps_the_newest_frames_that_fit_and_never_spans_a_tick_jump() {
        let mut frames: Vec<_> = (0..MAX_FRAMES as u32 + 3).map(|n| (n, record(n))).collect();
        let all = decode(&pack(&frames, 1171)).unwrap();
        assert_eq!(all.len(), MAX_FRAMES);
        assert_eq!(all.last().unwrap().0, frames.last().unwrap().0);
        let tight = decode(&pack(&frames, 400)).unwrap();
        assert!(tight.len() < MAX_FRAMES && tight.last().unwrap().0 == frames.last().unwrap().0);
        // A reset tick (the sender reloaded its map) starts a new run.
        frames.push((3, record(99)));
        assert_eq!(decode(&pack(&frames, 1171)).unwrap(), vec![(3, record(99))]);
    }

    #[test]
    fn garbage_is_refused() {
        let wire = encode(&[(5, record(1)), (6, record(2))]);
        assert!(decode(&wire[..wire.len() - 1]).is_none(), "short");
        assert!(decode(&[wire.clone(), vec![0]].concat()).is_none(), "trailing");
        assert!(decode(&[0, 0, 0, 0, 0]).is_none(), "no frames");
        let mut zero_step = wire.clone();
        zero_step[5 + MASK + 60 * 4] = 0;
        assert!(decode(&zero_step).is_none(), "a frame must advance the tick");
    }
}
