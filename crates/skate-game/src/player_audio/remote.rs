//! The other skaters' sounds (item 30b): retail plays every skater in the audio frame with its own player code
//! (`sub_824F1FB0`), from the record the title's builder `sub_827A1B78` writes per skater. Each player sends its own
//! record (the audio worker's [`OUTBOX`]) over skate-net's AUDIO kind; the nearest three that arrive ride the next
//! observations as records 1..3 ([`RemoteAudio::next_records`], written by `game_side`).
use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use bevy::prelude::*;
use skate_net::audio::{MAX_FRAMES, Record};

/// The worker pushes the local record of each frame here while [`SHARING`] (the session is live).
pub(crate) static OUTBOX: Mutex<VecDeque<(u32, Record)>> = Mutex::new(VecDeque::new());
pub(crate) static SHARING: AtomicBool = AtomicBool::new(false);
/// Remote records per audio frame: retail's frame holds 4 skater records, the local one first (cmplwi r18,4 at
/// 0x827A1220; buffer 2176 = 4 x 0x220).
pub(crate) const REMOTE_SLOTS: usize = 3;
/// `+148`: the per-wheel landed event bits the builder sets on a landing frame (0x827A21B0..), word index.
pub(crate) const LANDED_WORD: usize = 148 / 4;
/// Frames held before a skater is heard, and the most kept: ~3 frames arrive per 50 ms datagram.
// ponytail: a fixed 3..8-frame jitter buffer (50..133 ms); adaptive if a real link shows underruns.
const PRIME: usize = 3;
/// 10-29 link simulation (Wi-Fi spikes, frame hitches): 8 keeps two datagrams that arrive together, where 6 threw
/// frames away (~25% fewer skips and held frames, ~3 ms more delay).
const MOST: usize = 8;
/// A skater not heard from this long is dropped (its voices end).
const STALE: Duration = Duration::from_secs(1);
/// The most frames a late skater's last record is held for (200 ms); a stalled sender's slot then goes quiet, as a
/// dropped skater's does, instead of repeating its last frame up to [`STALE`].
const HOLD: u32 = 12;
/// How much nearer (m) a skater out of the slots must be than the farthest slotted one to take its slot: two skaters
/// about as near no longer swap it back and forth.
const SWAP_MARGIN: f32 = 5.;
/// The farthest (m) a skater's root moves between two datagrams that is still played as movement; more is a teleport.
const JUMP: f32 = 10.;
const SEND_EVERY: Duration = Duration::from_millis(49);

#[derive(Default)]
struct Heard {
    queue: VecDeque<Record>,
    last: Option<Record>,
    primed: bool,
    at: Option<Instant>,
    /// The root the newest queued frame was placed at: the next datagram's frames go on from it ([`RemoteAudio::receive`]).
    root: Option<Vec3>,
    /// Frames the last record has been held for in a row ([`HOLD`]).
    held: u32,
    /// Frames played ([`RemoteAudio::next_records`]), and the frame each one-shot bit last rose ([`limit`]).
    played: u64,
    rose: [Option<u64>; 32],
}

#[derive(Resource, Default)]
pub(crate) struct RemoteAudio {
    /// Our newest frames, oldest first (what the next datagram resends).
    recent: VecDeque<(u32, Record)>,
    sent: Option<u32>,
    last_send: Option<Instant>,
    heard: BTreeMap<u64, Heard>,
    /// The actor in each record slot; slot k plays with controller nibble k + 1.
    slots: [Option<u64>; REMOTE_SLOTS],
    /// A slot whose skater changed sits one frame out: the title's player for its nibble ends, and the new skater
    /// starts on a fresh one, not on the last one's grind or air (edges it never made).
    fresh: [bool; REMOTE_SLOTS],
    /// Spectating: the camera on the screen (match_core::ObserverView), which the slots go nearest to instead of our
    /// own skater.
    pub(crate) listener: Option<Vec3>,
    /// Spectating: the skater the camera shows (match_core::Watching), which always has a slot and is what the world
    /// is heard from ([`RemoteAudio::watched_record`]).
    pub(crate) watched: Option<u64>,
    /// Where the watched skater is drawn, heard or not (muted, or its audio stopped).
    watched_at: Option<Vec3>,
}

fn position(r: &Record) -> Vec3 {
    Vec3::new(f32::from_bits(r[0]), f32::from_bits(r[1]), f32::from_bits(r[2]))
}

/// What a received record word may hold. The sender is untrusted: its record is rebuilt word by word from what the
/// builder `sub_827A1B78` writes (stores traced from 0x827A1B78..0x827A2E84, bits and ranges from
/// `builder_probe` over random PhysOut bundles); anything else is zero.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Word {
    /// Not written by the builder.
    Zero,
    /// A world position lane (+0, +48, the wheels +208..+255): see [`clean`].
    Position,
    /// A float, finite and within ±the bound (NaN/inf → 0).
    Float(f32),
    /// A float in 0..=the bound.
    Unit(f32),
    /// An integer field: the bits the builder can set.
    Bits(u32),
    /// A signed integer in min..=max (else `min`).
    Int(i32, i32),
}

/// ponytail: one generous bound for the non-position floats (speeds, times, forces, angles): it keeps every value
/// physics makes and stops inf/1e38 reaching the player code; per-field bounds if one is ever heard to matter.
const LOOSE: f32 = 10_000.;
/// How far the board, its wheels and the second position (+48) may be from the skater's real position.
const NEAR: f32 = 5.;

/// The whitelist, by byte offset. Probed (`game_side::builder_probe`, 2000 random bundles): words the builder never
/// wrote are `Zero`, its bit words keep only the bits it set, and the integer fields keep the ranges it (or the
/// conditioner it copies, `sub_82772748`) produces.
pub(crate) const fn word(i: usize) -> Word {
    use Word::*;
    match i * 4 {
        // +0 position, +48 position, the wheels +208..+255 (x, y, z; w a float).
        0x00..=0x08 | 0x30..=0x38 | 0xD0..=0xD8 | 0xE0..=0xE8 | 0xF0..=0xF8 | 0x100..=0x108 => Position,
        // The vectors' w lanes, +16 facing, +32 COM velocity, +64 facing, +80 velocity, +272 deck spin, +496.
        0x0C | 0x3C | 0xDC | 0xEC | 0xFC | 0x10C => Float(LOOSE),
        0x10..=0x2C | 0x40..=0x5C | 0x110..=0x11C | 0x1F0..=0x1FC => Float(LOOSE),
        // +96 grind family (B40+28: −1 until the first grind, then Grinds+136).
        0x60 => Int(-1, 255),
        // +108 |COM velocity| and +140 |Skeleton+288| (sign cleared), the rest of +100..+144 signed.
        0x6C | 0x8C => Unit(LOOSE),
        0x64..=0x90 => Float(LOOSE),
        // +148..+172: the packed flags.
        0x94 => Bits(0xFFFD_5FFF),
        0x98 => Bits(0xBFF0_00E0),
        0x9C => Bits(0x0000_00FF),
        0xA0 => Bits(0x8000_0001),
        0xA4 => Bits(0x0000_0003),
        // 0x42FF probed; 0x200/0x400/0x800 the scoring paths (0x827A204C, 0x827A2084, 0x827A20FC).
        0xA8 => Bits(0x0000_4EFF),
        0xAC => Bits(0xE000_0000),
        // +180/+184 audio trick ids (eSk8AudioTricks layout +164/+172; −1 none).
        0xB4 => Int(-1, 38),
        0xB8 => Int(-1, 35),
        // +188 spin bucket: 0, 1, 2, 4 or 8 (0x827A2118..2164).
        0xBC => Int(0, 8),
        // +192 the scoring entry's 64-bit key (0x827A2058), a lookup key: any value is one that is not found.
        0xC0 | 0xC4 => Bits(u32::MAX),
        // +200/+202 the foot surface halves (the bridge masks them, 0x7F).
        0xC8 => Bits(u32::MAX),
        // +288..+316 |foot and spin speeds| (sign cleared).
        0x120..=0x13C => Unit(LOOSE),
        // +320.. the conditioner's body contacts (B40+92..+207, copied whole): impacts are 0..1 (sub_82BD60C8's fsel),
        // slides, materials (tag & 0x7F), the groin/face bytes, forces; the other skater is ours to know, not theirs.
        0x140..=0x15C => Unit(1.),
        0x160..=0x17C => Float(LOOSE),
        0x180..=0x19C => Int(0, 0x7F),
        0x1A0 => Bits(0x0101_0000),
        0x1A4 | 0x1A8 | 0x1B0 => Float(LOOSE),
        0x1AC => Int(-1, -1),
        // +436.. wheel materials (0..143), seams ((tag >> 12) & 0xF), truck and deck materials.
        0x1B4..=0x1C0 | 0x1D4..=0x1DC | 0x200 => Int(0, 143),
        0x1C4..=0x1D0 => Int(0, 15),
        // +480 deck slide speed, +484 scrape, +516, +524.
        0x1E0 | 0x1E4 | 0x204 | 0x20C => Float(LOOSE),
        // +520 Air+0xE4's low half (0x827A2A5C), +528 Interaction+0x24 (0x827A2BE0).
        0x208 => Bits(0xFFFF_0000),
        0x210 => Bits(u32::MAX),
        _ => Zero,
    }
}

/// A received record rebuilt from [`word`]'s whitelist: never copied through. Its position is where the sender's
/// skater really is in this session (`at`, its BODY/POSE root, which is what is drawn), not what the record claims;
/// its other positions keep their offset from the record's own +0, at most [`NEAR`] per axis.
pub(crate) fn clean(raw: &Record, at: Vec3) -> Record {
    let mut out = [0u32; skate_net::audio::RECORD_WORDS];
    let claimed = position(raw);
    for (i, w) in out.iter_mut().enumerate() {
        let f = f32::from_bits(raw[i]);
        let finite = |f: f32| if f.is_finite() { f } else { 0. };
        *w = match word(i) {
            Word::Zero => 0,
            Word::Position => {
                let axis = i % 4;
                let offset = if i < 4 { 0. } else { (finite(f) - finite(claimed[axis])).clamp(-NEAR, NEAR) };
                (at[axis] + offset).to_bits()
            }
            Word::Float(bound) => finite(f).clamp(-bound, bound).to_bits(),
            Word::Unit(bound) => finite(f).clamp(0., bound).to_bits(),
            Word::Bits(mask) => raw[i] & mask,
            Word::Int(min, max) => {
                let v = raw[i] as i32;
                (if (min..=max).contains(&v) { v } else { min }) as u32
            }
        };
    }
    out
}

impl RemoteAudio {
    /// Takes in frames from `incoming` (actor, its frames oldest first), each rebuilt by [`clean`] at the actor's real
    /// position (`at`: its BODY/POSE root; an actor without one is not heard); keeps the nearest [`REMOTE_SLOTS`] to
    /// our own record's position in their slots (a chosen skater keeps its slot until another is [`SWAP_MARGIN`]
    /// nearer; the watched skater always has one).
    pub(crate) fn receive(
        &mut self,
        incoming: impl IntoIterator<Item = (u64, Vec<(u32, Record)>)>,
        live: &[u64],
        at: &BTreeMap<u64, Vec3>,
        now: Instant,
    ) {
        for (actor, frames) in incoming {
            let Some(&real) = at.get(&actor).filter(|p| p.is_finite()) else { continue };
            if frames.is_empty() {
                continue;
            }
            let heard = self.heard.entry(actor).or_default();
            // Each frame its own place, on the way from the last datagram's root to this one's: one root for the whole
            // datagram stepped the sound ~0.5 m 20 times a second at 10 m/s.
            let from = heard.root.filter(|r| r.distance(real) <= JUMP).unwrap_or(real);
            let n = frames.len() as f32;
            // Not Vec3::lerp (a(1 - s) + bs): this form is exact when nothing moved and on the newest frame.
            let along = |i: usize| from + (real - from) * ((i + 1) as f32 / n);
            heard.queue.extend(frames.into_iter().enumerate().map(|(i, (_, r))| clean(&r, along(i))));
            heard.root = Some(real);
            // Only a slotted skater's queue is played down (next_records); every other one is kept as short.
            while heard.queue.len() > MOST {
                heard.queue.pop_front();
            }
            heard.at = Some(now);
        }
        self.heard.retain(|id, h| live.contains(id) && h.at.is_some_and(|at| now.duration_since(at) < STALE));
        self.watched_at = self.watched.and_then(|w| at.get(&w).copied()).filter(|p| p.is_finite());
        let Some(me) = self.recent.back().map(|(_, r)| self.listener.unwrap_or(position(r))) else {
            self.slots = [None; REMOTE_SLOTS];
            return;
        };
        let watched = self.watched;
        let near: BTreeMap<u64, f32> = self
            .heard
            .iter()
            .filter_map(|(&id, h)| {
                let d = position(h.queue.back().or(h.last.as_ref())?).distance(me);
                Some((id, if Some(id) == watched { -1. } else { d }))
            })
            .collect();
        let before = self.slots;
        for slot in &mut self.slots {
            if slot.is_some_and(|id| !near.contains_key(&id)) {
                *slot = None;
            }
        }
        let mut out: Vec<(f32, u64)> =
            near.iter().filter(|(id, _)| !self.slots.contains(&Some(**id))).map(|(&id, &d)| (d, id)).collect();
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (d, id) in out {
            if let Some(free) = self.slots.iter_mut().find(|s| s.is_none()) {
                *free = Some(id);
                continue;
            }
            let Some((k, far)) =
                self.slots.iter().enumerate().filter_map(|(k, s)| Some((k, near[&(*s)?]))).max_by(|a, b| a.1.total_cmp(&b.1))
            else {
                break;
            };
            // Nearest first: once one is not clearly nearer, none after it is.
            if d + SWAP_MARGIN >= far && Some(id) != watched {
                break;
            }
            self.slots[k] = Some(id);
        }
        for (k, (slot, was)) in self.slots.iter().zip(before).enumerate() {
            if let Some(id) = slot.filter(|id| Some(*id) != was) {
                // A skater (re)entering a slot starts from its own frames, never a record held from before, and from its
                // newest: those queued while it had no slot would keep its sound up to MOST frames behind its skater.
                if let Some(h) = self.heard.get_mut(&id) {
                    (h.last, h.held) = (None, 0);
                    while h.queue.len() > PRIME {
                        h.queue.pop_front();
                    }
                }
                self.fresh[k] = was.is_some();
            }
        }
    }

    /// One frame's remote records, (controller nibble, record), each slot's next frame. An empty queue holds the last
    /// record as it was for up to [`HOLD`] frames, then the slot goes quiet, and waits for [`PRIME`] frames again.
    pub(crate) fn next_records(&mut self) -> Vec<(u8, Box<Record>)> {
        let mut out = vec![];
        for (k, slot) in self.slots.iter().enumerate() {
            if std::mem::take(&mut self.fresh[k]) {
                continue;
            }
            let Some(heard) = slot.and_then(|id| self.heard.get_mut(&id)) else { continue };
            if !heard.primed && heard.queue.len() >= PRIME {
                heard.primed = true;
            }
            while heard.queue.len() > MOST {
                heard.queue.pop_front();
            }
            // Unprimed, the queue only fills: popping it there threw a frame away each tick.
            let next = if heard.primed { heard.queue.pop_front() } else { None };
            let next = match next {
                Some(record) => {
                    heard.held = 0;
                    record
                }
                None => {
                    heard.primed = false;
                    // A late frame holds the last record as it was. Clearing its +148 flags dropped air, grind and
                    // trick for a frame, and the player code's contacts fire on those edges (sub_824B90D8): a landing
                    // and a pop mid-air, a grind end and a new grind's clank mid-grind.
                    // UNTRACED: what retail's own replicated records do on a late packet.
                    heard.held += 1;
                    if heard.held > HOLD {
                        continue;
                    }
                    let Some(record) = heard.last else { continue };
                    record
                }
            };
            let mut next = next;
            limit(heard, &mut next);
            heard.last = Some(next);
            out.push((k as u8 + 1, Box::new(next)));
        }
        out
    }

    /// The watched skater's record as last played ([`RemoteAudio::next_records`]), the frame's listener; before its
    /// first one (a slot just taken sits a frame out, or primes), its next frame, so the world is never heard from
    /// our parked skater on a switch. Not heard at all (muted, or its audio stopped): our own record moved to where it
    /// is drawn, so the listener (game_side::listen_from) and ambience go there.
    pub(crate) fn watched_record(&self) -> Option<Box<Record>> {
        let heard = self.heard.get(&self.watched?).and_then(|h| h.last.or_else(|| h.queue.front().copied()));
        heard
            .or_else(|| {
                let (mut ours, at) = (self.recent.back()?.1, self.watched_at?);
                ours[..3].copy_from_slice(&at.to_array().map(f32::to_bits));
                Some(ours)
            })
            .map(Box::new)
    }

    /// Our frames from the worker; the datagram is due when [`SEND_EVERY`] has passed and a frame is new.
    fn outgoing(&mut self, now: Instant) -> Option<Vec<(u32, Record)>> {
        let mut outbox = OUTBOX.lock().unwrap_or_else(|e| e.into_inner());
        self.recent.extend(outbox.drain(..));
        drop(outbox);
        while self.recent.len() > MAX_FRAMES {
            self.recent.pop_front();
        }
        let newest = self.recent.back()?.0;
        if self.sent == Some(newest) || self.last_send.is_some_and(|at| now.duration_since(at) < SEND_EVERY) {
            return None;
        }
        self.sent = Some(newest);
        self.last_send = Some(now);
        Some(self.recent.iter().copied().collect())
    }
}

/// `+148` bits a frame plays once as they rise: the per-wheel landed latches (bits 22..19, `sub_82772FD8`) and the
/// truck/deck contacts (bits 10..8), with the fewest frames between two rises of the same bit a skater makes: a
/// latch rises again only after > 5 air frames and a landing (0x827A21B0..), so 15 frames (0.25 s) is a hop.
/// ponytail: per-bit gaps; retail's own per-region cooldowns (`obj+260`, `sub_824BC188`) already pace body impacts.
const ONE_SHOTS: [(u32, u64); 2] = [(0x0078_0000, 15), (0x0000_0700, 6)];

/// Holds a one-shot bit down when it rises sooner than a skater can make it rise again: a record can set every
/// landing every frame, the game only every quarter second.
fn limit(heard: &mut Heard, record: &mut Record) {
    heard.played += 1;
    let before = heard.last.map_or(0, |r| r[LANDED_WORD]);
    for (mask, gap) in ONE_SHOTS {
        for bit in (0..32).filter(|b| mask & (1 << b) != 0) {
            let rising = record[LANDED_WORD] & (1 << bit) != 0 && before & (1 << bit) == 0;
            if !rising {
                continue;
            }
            if heard.rose[bit].is_some_and(|at| heard.played - at < gap) {
                record[LANDED_WORD] &= !(1 << bit);
            } else {
                heard.rose[bit] = Some(heard.played);
            }
        }
    }
}

/// Sends our records and takes in the others' (PreUpdate, after the session's receive).
pub(crate) fn exchange(
    mut net: ResMut<crate::multiplayer::Multiplayer>,
    mut remote: ResMut<RemoteAudio>,
    observer: Option<Res<crate::match_core::ObserverView>>,
    watching: Option<Res<crate::match_core::Watching>>,
) {
    let active = net.active();
    SHARING.store(active, Ordering::Relaxed);
    let Some(lobby) = net.session_mut().filter(|_| active) else {
        if remote.recent.len() + remote.heard.len() > 0 || remote.slots.iter().any(Option::is_some) {
            *remote = RemoteAudio::default();
        }
        OUTBOX.lock().unwrap_or_else(|e| e.into_inner()).clear();
        return;
    };
    let now = Instant::now();
    if let Some(frames) = remote.outgoing(now) {
        lobby.send_audio(&frames);
    }
    let local = lobby.local;
    // C6: a muted or blocked skater is never live, so never heard (online safety 3.12; player_list).
    let live: Vec<u64> = lobby.actors.keys().copied().filter(|&id| id != local && !crate::player_list::quiet(id)).collect();
    // Where each skater is drawn (its newest BODY or POSE root), not where its audio record says it is.
    let at: BTreeMap<u64, Vec3> = lobby
        .actors
        .iter()
        .filter_map(|(&id, a)| {
            let newest = [a.body.latest(), a.pose.latest()].into_iter().flatten().max_by_key(|r| r.received)?;
            Some((id, Vec3::from_array(newest.state.position())))
        })
        .collect();
    let incoming: Vec<_> = lobby
        .actors
        .iter_mut()
        .filter(|(id, _)| **id != local)
        .map(|(&id, a)| (id, a.audio.drain(..).collect::<Vec<_>>()))
        .collect();
    remote.listener = observer.and_then(|o| o.0).map(|view| view.translation);
    // The skater that camera shows (observer_camera frames Watching::shown).
    remote.watched = remote.listener.and(watching.and_then(|w| w.shown));
    remote.receive(incoming, &live, &at, now);
}

#[cfg(test)]
mod tests {
    use super::*;

    const LANDED: u32 = 0x0078_0000;

    fn at(x: f32) -> Record {
        let mut r = [0; skate_net::audio::RECORD_WORDS];
        r[0] = x.to_bits();
        r[LANDED_WORD] = LANDED;
        r
    }

    fn real(xs: &[(u64, f32)]) -> BTreeMap<u64, Vec3> {
        xs.iter().map(|&(id, x)| (id, Vec3::new(x, 0., 0.))).collect()
    }

    #[test]
    fn the_nearest_three_keep_their_slots_and_a_late_frame_holds_its_record() {
        let now = Instant::now();
        let mut remote = RemoteAudio::default();
        remote.recent.push_back((1, at(0.)));
        let frames = |x: f32| (0..PRIME as u32).map(|t| (t, at(x))).collect::<Vec<_>>();
        let live = [1, 2, 3, 4];
        let mut where_ = real(&[(1, 5.), (2, 40.), (3, 10.), (4, 20.)]);
        remote.receive([(1, frames(5.)), (2, frames(40.)), (3, frames(10.)), (4, frames(20.))], &live, &where_, now);
        assert_eq!(remote.slots, [Some(1), Some(3), Some(4)], "nearest three; 40 m left out");
        // 2 comes closer than 4: 4 leaves, 2 takes its slot; 1 and 3 keep theirs.
        where_.insert(2, Vec3::new(2., 0., 0.));
        remote.receive([(2, frames(2.))], &live, &where_, now);
        assert_eq!(remote.slots, [Some(1), Some(3), Some(2)]);
        // Slot 3 changed skater: it sits a frame out, so 2 starts on a fresh player, not on 4's.
        let first = remote.next_records();
        assert_eq!(first.iter().map(|(n, _)| *n).collect::<Vec<_>>(), [1, 2]);
        assert!(first.iter().all(|(_, r)| r[LANDED_WORD] == LANDED), "a frame plays as sent");
        assert_eq!(remote.next_records().iter().map(|(n, _)| *n).collect::<Vec<_>>(), [1, 2, 3]);
        // 1 and 3 run dry first.
        for _ in 2..PRIME {
            remote.next_records();
        }
        // Run dry: the last record repeats as it was. Zeroing its +148 flags made every late frame a landing (air
        // ending), a pop or a grind clank: the player code's contacts fire on those edges.
        let dry = remote.next_records();
        assert_eq!(dry.len(), 3);
        assert!(dry.iter().all(|(_, r)| r[LANDED_WORD] == LANDED && f32::from_bits(r[0]) > 0.));
        // A skater gone from the session is dropped at once.
        remote.receive([], &[1, 3], &where_, now);
        assert_eq!(remote.slots, [Some(1), Some(3), None]);
    }

    /// Q5: a griefer's record claims to be beside us; it is chosen, and heard, where its skater really is.
    #[test]
    fn the_nearest_are_chosen_by_where_skaters_really_are_not_by_their_records() {
        let now = Instant::now();
        let mut remote = RemoteAudio::default();
        remote.recent.push_back((1, at(0.)));
        let frames = |x: f32| (0..PRIME as u32).map(|t| (t, at(x))).collect::<Vec<_>>();
        // Actors 1..3 claim 0.1 m but are 200 m away; 4 is 10 m away and honest.
        let where_ = real(&[(1, 200.), (2, 201.), (3, 202.), (4, 10.)]);
        remote.receive([(1, frames(0.1)), (2, frames(0.1)), (3, frames(0.1)), (4, frames(10.))], &[1, 2, 3, 4], &where_, now);
        assert_eq!(remote.slots[0], Some(4), "the honest skater nearest for real takes the first slot");
        let played = remote.next_records();
        let mine = played.iter().find(|(n, _)| *n == 1).unwrap();
        assert_eq!(f32::from_bits(mine.1[0]), 10.);
        for (_, r) in &played {
            assert!(position(r).x >= 10., "every record plays where its skater is: {}", position(r).x);
        }
        // Without a known position an actor is not heard at all.
        let mut fresh = RemoteAudio::default();
        fresh.recent.push_back((1, at(0.)));
        fresh.receive([(9, frames(0.))], &[9], &BTreeMap::new(), now);
        assert!(fresh.slots.iter().all(Option::is_none));
    }

    /// An unslotted skater's frames used to pile up for as long as it kept sending (only slots were played down).
    #[test]
    fn a_skater_out_of_the_slots_never_grows_its_queue() {
        let now = Instant::now();
        let mut remote = RemoteAudio::default();
        remote.recent.push_back((1, at(0.)));
        let ids = [1u64, 2, 3, 4];
        let where_ = real(&[(1, 1.), (2, 2.), (3, 3.), (4, 50.)]);
        for t in 0..1000u32 {
            let batch: Vec<_> = ids.iter().map(|&id| (id, (0..8).map(|k| (t * 8 + k, at(0.))).collect())).collect();
            remote.receive(batch, &ids, &where_, now);
            remote.next_records();
        }
        assert!(remote.heard[&4].queue.len() <= MOST, "{}", remote.heard[&4].queue.len());
    }

    /// User 10-24 (build 10): spectating, the slots go to the skaters nearest the camera on the screen (`listener`),
    /// not to the ones round our own parked skater: the watched skater 300 m off is heard.
    #[test]
    fn spectating_the_skater_by_the_camera_is_heard() {
        let now = Instant::now();
        let mut remote = RemoteAudio::default();
        remote.recent.push_back((1, at(0.)));
        let frames = |x: f32| (0..PRIME as u32).map(|t| (t, at(x))).collect::<Vec<_>>();
        let where_ = real(&[(1, 5.), (2, 10.), (3, 15.), (4, 300.)]);
        remote.listener = Some(Vec3::new(297., 1.5, 0.));
        remote.receive([(1, frames(5.)), (2, frames(10.)), (3, frames(15.)), (4, frames(300.))], &[1, 2, 3, 4], &where_, now);
        assert!(remote.slots.contains(&Some(4)), "{:?}", remote.slots);
        remote.listener = None;
        remote.receive([], &[1, 2, 3, 4], &where_, now);
        let mut slots = remote.slots;
        slots.sort();
        assert_eq!(slots, [Some(1), Some(2), Some(3)], "our own camera again: the three round our skater");
    }

    /// Q5: a record that lands every other frame plays a landing at most every quarter second.
    #[test]
    fn landings_rise_no_faster_than_a_skater_can_land() {
        let now = Instant::now();
        let mut remote = RemoteAudio::default();
        remote.recent.push_back((1, at(0.)));
        let where_ = real(&[(1, 1.)]);
        let mut rises = 0;
        let mut was = 0;
        // Three frames a datagram, as they arrive.
        for t in (0..120u32).step_by(PRIME) {
            let frames = (t..t + PRIME as u32)
                .map(|t| {
                    let mut r = at(1.);
                    r[LANDED_WORD] = if t % 2 == 0 { LANDED } else { 0 };
                    (t, r)
                })
                .collect();
            remote.receive([(1, frames)], &[1], &where_, now);
            for _ in 0..PRIME {
                for (_, r) in remote.next_records() {
                    rises += (r[LANDED_WORD] & !was & LANDED != 0) as u32;
                    was = r[LANDED_WORD];
                }
            }
        }
        // ~2 s of play: at most 120 / 15 landings, not 60.
        assert!(rises > 0 && rises <= 120 / 15 + 1, "{rises} landings");
    }

    /// 3.1: no raw word reaches the player code. Adversarial records (random bits, NaN/inf, extreme ints) through
    /// the receive path come out with every float finite and bounded, every position beside the real one, every
    /// integer field in the builder's range, every word the builder never writes zero; fast and panic-free.
    #[test]
    fn adversarial_records_are_rebuilt_from_the_whitelist() {
        let started = Instant::now();
        let special = [0u32, u32::MAX, 0x7FFF_FFFF, 0x8000_0000, 0x7FC0_0000, 0x7F80_0000, 0xFF80_0000, 0x7F7F_FFFF, 143, 144];
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let real_at = Vec3::new(100., 2., -50.);
        let mut remote = RemoteAudio::default();
        remote.recent.push_back((1, at(0.)));
        let where_ = real(&[(1, 100.), (2, 101.), (3, 102.), (4, 103.)]);
        for t in 0..2000u32 {
            let batch: Vec<_> = (1..=4u64)
                .map(|id| {
                    let frames = (0..3)
                        .map(|k| {
                            let r: Record = std::array::from_fn(|_| match next() % 3 {
                                0 => next() as u32,
                                1 => special[(next() % special.len() as u64) as usize],
                                _ => 0,
                            });
                            (t * 3 + k, r)
                        })
                        .collect();
                    (id, frames)
                })
                .collect();
            remote.receive(batch, &[1, 2, 3, 4], &where_, now_plus(started, t));
            let played = remote.next_records();
            assert!(!played.is_empty() || t < 2);
            for (_, r) in played.iter().map(|(n, r)| (n, **r)).chain([(&0, clean(&std::array::from_fn(|_| next() as u32), real_at))]) {
                for (i, &w) in r.iter().enumerate() {
                    let f = f32::from_bits(w);
                    match word(i) {
                        Word::Zero => assert_eq!(w, 0, "+{:#x}", 4 * i),
                        Word::Position => assert!(f.is_finite() && f.abs() < 200., "+{:#x}: {f}", 4 * i),
                        Word::Float(b) => assert!(f.is_finite() && f.abs() <= b, "+{:#x}: {f}", 4 * i),
                        Word::Unit(b) => assert!(f.is_finite() && (0. ..=b).contains(&f), "+{:#x}: {f}", 4 * i),
                        Word::Bits(m) => assert_eq!(w & !m, 0, "+{:#x}", 4 * i),
                        Word::Int(lo, hi) => assert!((lo..=hi).contains(&(w as i32)), "+{:#x}: {}", 4 * i, w as i32),
                    }
                }
            }
        }
        // A record's +0 is the real position; a far claim for its board is pulled to within NEAR.
        let mut liar = at(1.0e30);
        liar[12] = (-7.0e37f32).to_bits();
        let kept = clean(&liar, real_at);
        assert_eq!(position(&kept), real_at);
        assert_eq!(f32::from_bits(kept[12]), real_at.x - NEAR);
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
    }

    /// Frames numbered by their +108 (|COM velocity|, kept as sent) to follow them through the queue.
    fn numbered(t: u32, x: f32) -> (u32, Record) {
        let mut r = at(x);
        r[0x6C / 4] = (t as f32).to_bits();
        (t, r)
    }

    fn numbers(played: &[(u8, Box<Record>)], nibble: u8) -> Vec<f32> {
        played.iter().filter(|(n, _)| *n == nibble).map(|(_, r)| f32::from_bits(r[0x6C / 4])).collect()
    }

    /// Bug 2 (10-29): unprimed, each frame popped the queue's oldest and threw it away, so a skater catching up never
    /// filled its buffer: frames 0 and 1 were lost and frame 2 played alone.
    #[test]
    fn catching_up_eats_no_frames() {
        let now = Instant::now();
        let mut remote = RemoteAudio::default();
        remote.recent.push_back((1, at(0.)));
        let where_ = real(&[(1, 1.)]);
        remote.receive([(1, vec![numbered(0, 1.), numbered(1, 1.)])], &[1], &where_, now);
        assert!(remote.next_records().is_empty() && remote.next_records().is_empty(), "under PRIME: nothing yet");
        remote.receive([(1, vec![numbered(2, 1.)])], &[1], &where_, now);
        let played: Vec<_> = (0..3).flat_map(|_| remote.next_records()).collect();
        assert_eq!(numbers(&played, 1), [0., 1., 2.]);
    }

    /// Bug 3 (10-29): every frame of a datagram played at the newest root, so the sound stepped ~0.5 m 20 times a second
    /// at 10 m/s. Each frame is placed on the way from the last datagram's root to this one's; a teleport jumps.
    #[test]
    fn each_frame_plays_at_its_own_place() {
        let now = Instant::now();
        let mut remote = RemoteAudio::default();
        remote.recent.push_back((1, at(0.)));
        let batch = |t: u32| (t..t + 3).map(|t| numbered(t, 0.)).collect::<Vec<_>>();
        let mut xs = vec![];
        for (t, x) in [(0, 0.), (3, 3.), (6, 300.)] {
            remote.receive([(1, batch(t))], &[1], &real(&[(1, x)]), now);
            xs.extend((0..3).flat_map(|_| remote.next_records()).map(|(_, r)| position(&r).x));
        }
        assert_eq!(xs, [0., 0., 0., 1., 2., 3., 300., 300., 300.]);
    }

    /// Bug 1 (10-29): a late frame repeated the last record with its whole +148 word zeroed: air (bit 31), grind (25)
    /// and trick (24, 23) fell for a frame and rose again, and the player code's contacts fire on those edges (a
    /// landing and a pop mid-air, a grind end and a new grind's clank mid-grind).
    #[test]
    fn a_late_frame_keeps_air_grind_and_trick() {
        let now = Instant::now();
        let mut remote = RemoteAudio::default();
        remote.recent.push_back((1, at(0.)));
        let flags = 0x8000_0000 | 0x0200_0000 | 0x0100_0000;
        let frames = (0..PRIME as u32)
            .map(|t| {
                let (t, mut r) = numbered(t, 1.);
                r[LANDED_WORD] = flags;
                (t, r)
            })
            .collect();
        remote.receive([(1, frames)], &[1], &real(&[(1, 1.)]), now);
        let played: Vec<_> = (0..PRIME + 2).flat_map(|_| remote.next_records()).collect();
        assert_eq!(played.len(), PRIME + 2);
        assert!(played.iter().all(|(_, r)| r[LANDED_WORD] == flags), "{:x?}", played.iter().map(|(_, r)| r[LANDED_WORD]).collect::<Vec<_>>());
    }

    /// Bug 5 (10-29): with four skaters about as near, the nearest three swapped a slot back and forth as they moved;
    /// a skater takes a slot only when clearly nearer than the farthest slotted one.
    #[test]
    fn skaters_about_as_near_keep_their_slots() {
        let now = Instant::now();
        let mut remote = RemoteAudio::default();
        remote.recent.push_back((1, at(0.)));
        let frames = |x: f32| (0..PRIME as u32).map(|t| (t, at(x))).collect::<Vec<_>>();
        let all = |where_: &BTreeMap<u64, Vec3>| (1..=4u64).map(|id| (id, frames(where_[&id].x))).collect::<Vec<_>>();
        let mut where_ = real(&[(1, 10.), (2, 11.), (3, 12.), (4, 13.)]);
        remote.receive(all(&where_), &[1, 2, 3, 4], &where_, now);
        assert_eq!(remote.slots, [Some(1), Some(2), Some(3)]);
        for x in [11.5, 13., 11.8, 12.5] {
            where_.insert(4, Vec3::new(x, 0., 0.));
            remote.receive(all(&where_), &[1, 2, 3, 4], &where_, now);
            assert_eq!(remote.slots, [Some(1), Some(2), Some(3)], "4 at {x} m");
        }
        where_.insert(4, Vec3::new(2., 0., 0.));
        remote.receive(all(&where_), &[1, 2, 3, 4], &where_, now);
        assert_eq!(remote.slots, [Some(1), Some(2), Some(4)], "clearly nearer: it takes the farthest slot");
    }

    /// Audit 10-30: a skater heard but out of the slots (the nearest three, or not yet watched) keeps up to MOST
    /// frames; taking a slot it played that backlog first, its sound up to 133 ms behind its skater for good. It plays
    /// from its newest PRIME frames.
    #[test]
    fn a_skater_taking_a_slot_plays_its_newest_frames() {
        let now = Instant::now();
        let mut remote = RemoteAudio::default();
        remote.recent.push_back((1, at(0.)));
        let others = |where_: &BTreeMap<u64, Vec3>| (1..=3u64).map(|id| (id, vec![numbered(0, where_[&id].x)])).collect::<Vec<_>>();
        let mut where_ = real(&[(1, 10.), (2, 11.), (3, 12.), (4, 20.)]);
        let mut batch = others(&where_);
        batch.push((4, (0..9).map(|t| numbered(t, 20.)).collect()));
        remote.receive(batch, &[1, 2, 3, 4], &where_, now);
        assert_eq!(remote.slots, [Some(1), Some(2), Some(3)], "4 is heard, out of the slots");
        where_.insert(4, Vec3::new(2., 0., 0.));
        let mut batch = others(&where_);
        batch.push((4, (9..12).map(|t| numbered(t, 2.)).collect()));
        remote.receive(batch, &[1, 2, 3, 4], &where_, now);
        assert_eq!(remote.slots, [Some(1), Some(2), Some(4)], "clearly nearer: it takes the farthest slot");
        let played: Vec<_> = (0..4).flat_map(|_| remote.next_records()).collect();
        assert_eq!(numbers(&played, 3), [9., 10., 11.], "its newest frames, not the backlog");
    }

    /// Bug 6 (10-29): a stalled sender's last record repeated (flags zeroed) for the whole second until STALE dropped
    /// it. It holds as it was, briefly, then its slot goes quiet.
    #[test]
    fn a_stalled_sender_holds_briefly_then_goes_quiet() {
        let now = Instant::now();
        let mut remote = RemoteAudio::default();
        remote.recent.push_back((1, at(0.)));
        remote.receive([(1, (0..PRIME as u32).map(|t| numbered(t, 1.)).collect())], &[1], &real(&[(1, 1.)]), now);
        let played: Vec<_> = (0..60).flat_map(|_| remote.next_records()).collect();
        assert!(played.len() > PRIME && played.len() < 30, "{} frames in a second", played.len());
        let last = &played[PRIME - 1].1;
        assert!(played[PRIME..].iter().all(|(_, r)| r == last), "a held frame is the last one as it was");
    }

    /// Bug 4 (10-29): spectating, the watched skater always has a slot (the camera is on it), and its record is what
    /// the world is heard from (painter, listener), not our parked skater.
    #[test]
    fn spectating_the_world_is_heard_from_the_watched_skater() {
        let now = Instant::now();
        let mut remote = RemoteAudio::default();
        remote.recent.push_back((1, at(0.)));
        let frames = |x: f32| (0..PRIME as u32).map(|t| (t, at(x))).collect::<Vec<_>>();
        let where_ = real(&[(1, 1.), (2, 2.), (3, 3.), (4, 30.)]);
        remote.listener = Some(Vec3::ZERO);
        remote.watched = Some(4);
        remote.receive((1..=4u64).map(|id| (id, frames(where_[&id].x))), &[1, 2, 3, 4], &where_, now);
        assert!(remote.slots.contains(&Some(4)), "{:?}", remote.slots);
        remote.next_records();
        let watched = remote.watched_record().expect("the watched skater's record");
        assert_eq!(position(&watched).x, 30.);
        let mut observation = crate::skate_audio::PlayerAudioObservation::default();
        observation.retail.position = [0., 0., 0.];
        assert_eq!(observation.heard_at(), [0., 0., 0.], "our own camera: our skater");
        observation.watched = Some(watched);
        assert_eq!(observation.heard_at(), [30., 0., 0.]);
    }

    /// Review 10-30: switching to a skater that takes another's slot sits that slot out a frame (`fresh`) with no
    /// record played yet, and the world was heard from our parked skater for that frame.
    #[test]
    fn switching_the_watched_skater_never_hears_from_our_own() {
        let now = Instant::now();
        let mut remote = RemoteAudio::default();
        remote.recent.push_back((1, at(0.)));
        let frames = |x: f32| (0..PRIME as u32).map(|t| (t, at(x))).collect::<Vec<_>>();
        let where_ = real(&[(1, 1.), (2, 2.), (3, 3.), (4, 30.)]);
        remote.listener = Some(Vec3::ZERO);
        remote.watched = Some(4);
        remote.receive((1..=4u64).map(|id| (id, frames(where_[&id].x))), &[1, 2, 3, 4], &where_, now);
        assert!(!remote.slots.contains(&Some(3)), "{:?}", remote.slots);
        remote.next_records();
        remote.watched = Some(3);
        remote.receive((1..=4u64).map(|id| (id, frames(where_[&id].x))), &[1, 2, 3, 4], &where_, now);
        assert!(remote.slots.contains(&Some(3)), "{:?}", remote.slots);
        remote.next_records();
        let watched = remote.watched_record().expect("the new watched skater's record on its first frame");
        assert_eq!(position(&watched).x, 3.);
    }

    /// Audit 10-30: watching a muted player (never live), or one whose audio stopped, the world was heard from our
    /// parked skater; it is heard from where the watched one is drawn.
    #[test]
    fn a_watched_skater_with_no_audio_is_still_where_the_world_is_heard_from() {
        let now = Instant::now();
        let mut remote = RemoteAudio::default();
        remote.recent.push_back((1, at(0.)));
        remote.listener = Some(Vec3::new(48., 0., 0.));
        remote.watched = Some(9);
        remote.receive([], &[], &real(&[(9, 50.)]), now);
        let watched = remote.watched_record().expect("a record at the watched skater");
        assert_eq!(position(&watched), Vec3::new(50., 0., 0.));
        remote.watched = None;
        remote.receive([], &[], &real(&[(9, 50.)]), now);
        assert_eq!(remote.watched_record(), None, "not spectating");
    }

    fn now_plus(start: Instant, t: u32) -> Instant {
        start + Duration::from_millis(u64::from(t) * 16)
    }
}

