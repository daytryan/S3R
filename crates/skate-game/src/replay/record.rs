//! The recorder core: presentation frames, quantised and delta-coded, in one-second zstd blocks, held in a ring
//! sized like retail's DataMan ring. Presentation only; never physics state or inputs (spec §1).
//!
//! Block layout (little endian, then zstd level 3): `u8 version, u16 frames`, then per frame `f64 time`, `u8 cut`,
//! `f32 fov`, camera, actors, props, the audio record, then (version 3) the weather: `u8 rain × 200` and a strike
//! flag with `f32 azimuth, f32 distance, u32 seed`. A position is millimetres and a rotation
//! smallest-three (u8 index + 3 x i16), each stored as zigzag varint residuals against a linear prediction from the
//! same slot's last two frames in the block; a bone's scale is stored only when it is not 1.
use crate::skate_audio::{BodyContacts, PlayerAudioObservation, RetailAudioInputs};
use bevy::prelude::*;
use skate_core::physics::phase::{PhysicsBody, PhysicsEvent};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// Stage 0 §1 [disc]: the 60 Hz sim tick 0x826D78B0 steps DataSysMan 0x82726908, whose 0x82dbc198 writes one
/// frame per call with no divider.
pub(crate) const RATE_HZ: u32 = 60;
/// research Q4 [disc, R2]: the DataMan ring's frame count (90 s at RATE_HZ).
pub(crate) const RING_FRAMES: usize = 5400;
/// research Q4 [disc, R2]: the ring's 11 MB budget.
pub(crate) const RING_BYTES: usize = 11 * 1024 * 1024;
/// One second at RATE_HZ.
pub(crate) const BLOCK_FRAMES: usize = RATE_HZ as usize;
/// 2: music_seconds dropped (review M8: never filled; the editor stops the music, music.rs state 5).
/// 3: the weather per frame (weather audio, plan Task 9); version 2 blocks still load, dry.
/// perf 10-19: 4: per actor, after its bones, `u16 joints` and the joints (a remote's distinct posed joints).
const VERSION: u8 = 4;
/// Corrupt data must not allocate without bound.
const MAX_ITEMS: usize = 1 << 16;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Actor {
    pub id: u32,
    pub root: Transform,
    /// Index 0 (the local skater): its render pose (native bone matrices). Others: their skin's bone-local
    /// transforms, as multiplayer::render binds them.
    pub bones: Vec<Mat4>,
    /// perf 10-19: a remote's distinct posed joints as bevy holds them (replay::ReplayJoints), `bones` then empty:
    /// no Mat4 round trip. SKATE_REPLAY_BONES_ONLY=0: every node below the root in `bones`, as before.
    pub joints: Vec<Transform>,
    pub visible: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Prop {
    pub id: u32,
    pub transform: Transform,
    pub visible: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RecFrame {
    /// The recording clock (seconds); the scripted API (skate2_challenge) seeks by it.
    pub time: f64,
    /// Index 0 = the local skater.
    pub actors: Vec<Actor>,
    pub camera: Transform,
    pub fov: f32,
    pub props: Vec<Prop>,
    /// The local skater's per-frame audio record (retail's PhysOut provider 0x827a11b0, which the audio bridge
    /// 0x824B0DA8 reads; Stage 0 §3, lead ruling): fed back into player_audio's input in playback.
    pub audio: Option<Box<PlayerAudioObservation>>,
    pub cut: bool,
    /// The rain weather audio heard (0, 0.5, 1).
    pub rain: f32,
    /// A lightning strike that flashed this frame: its thunder is replayed from here.
    pub strike: Option<crate::retail_render::LightningStrike>,
}
impl Default for RecFrame {
    fn default() -> Self {
        Self {
            time: 0.,
            actors: Vec::new(),
            camera: Transform::IDENTITY,
            fov: 1.,
            props: Vec::new(),
            audio: None,
            cut: false,
            rain: 0.,
            strike: None,
        }
    }
}

struct Block {
    bytes: Vec<u8>,
    frames: usize,
    serial: u64,
    /// review I9: zstd runs on the AsyncComputeTaskPool; until it lands the block keeps its decoded frames and
    /// counts no bytes.
    pending: Option<(bevy::tasks::Task<Vec<u8>>, Arc<Vec<RecFrame>>)>,
}

/// Frames in compressed one-second blocks plus the open block, encoded a frame at a time as it fills (review I9:
/// no 60-frame encode spike, no zstd on the main thread). Drops the oldest frame at RING_FRAMES and the oldest
/// blocks over RING_BYTES, whichever comes first (spec §1).
#[derive(Default)]
pub(crate) struct Ring {
    blocks: VecDeque<Block>,
    /// Frames of the front block already dropped.
    skip: usize,
    open: Vec<RecFrame>,
    encoder: Encoder,
    times: VecDeque<f64>,
    bytes: usize,
    serial: u64,
    cache: Mutex<Option<(u64, Arc<Vec<RecFrame>>)>>,
    /// A loaded save's distinct skater bone counts (review I4: checked against the live rig before it plays).
    rigs: Vec<usize>,
    /// Every recorded lightning strike with its frame's time, oldest first: weather audio re-arms thunder from it
    /// without decoding blocks.
    strikes: VecDeque<(f64, crate::retail_render::LightningStrike)>,
}

impl Ring {
    pub fn push(&mut self, frame: RecFrame) {
        self.settle(false);
        if self.len() >= RING_FRAMES {
            self.drop_front();
        }
        self.times.push_back(frame.time);
        self.strikes.extend(frame.strike.map(|s| (frame.time, s)));
        self.encoder.push(&frame);
        self.open.push(frame);
        if self.open.len() >= BLOCK_FRAMES {
            let raw = std::mem::take(&mut self.encoder).finish();
            let frames = Arc::new(std::mem::take(&mut self.open));
            self.serial += 1;
            let pool = bevy::tasks::AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::default);
            let task = pool.spawn(async move { compress(&raw) });
            self.blocks.push_back(Block {
                bytes: Vec::new(),
                frames: frames.len(),
                serial: self.serial,
                pending: Some((task, frames)),
            });
        }
    }

    /// Collects finished compressions (every one with `wait`), then holds the byte budget.
    fn settle(&mut self, wait: bool) {
        for block in &mut self.blocks {
            if block.pending.as_ref().is_some_and(|(task, _)| wait || task.is_finished())
                && let Some((task, _)) = block.pending.take()
            {
                block.bytes = bevy::tasks::block_on(task);
                self.bytes += block.bytes.len();
            }
        }
        while self.bytes > RING_BYTES && self.blocks.front().is_some_and(|b| b.pending.is_none()) {
            let block = self.blocks.pop_front().unwrap();
            self.bytes -= block.bytes.len();
            self.times.drain(..block.frames - self.skip);
            self.skip = 0;
        }
        self.drop_old_strikes();
    }

    fn drop_old_strikes(&mut self) {
        let oldest = self.times.front().copied().unwrap_or(f64::INFINITY);
        while self.strikes.front().is_some_and(|&(t, _)| t < oldest) {
            self.strikes.pop_front();
        }
    }

    /// The recorded strikes and their frames' times, oldest first.
    pub fn strikes(&self) -> impl Iterator<Item = (f64, crate::retail_render::LightningStrike)> + '_ {
        self.strikes.iter().copied()
    }

    /// Waits for every block still compressing (tests, size checks).
    pub fn flush(&mut self) {
        self.settle(true);
    }

    fn drop_front(&mut self) {
        self.times.pop_front();
        match self.blocks.front() {
            Some(block) => {
                self.skip += 1;
                if self.skip >= block.frames {
                    let block = self.blocks.pop_front().unwrap();
                    self.bytes -= block.bytes.len(); // 0 while it was still compressing
                    self.skip = 0;
                }
            }
            None => {
                self.open.remove(0);
                self.encoder = Encoder::default();
                self.open.iter().for_each(|f| self.encoder.push(f));
            }
        }
        self.drop_old_strikes();
    }

    /// Frames held.
    pub fn len(&self) -> usize {
        self.times.len()
    }
    pub fn is_empty(&self) -> bool {
        self.times.is_empty()
    }
    /// The recording time of frame `index` (0 = oldest).
    pub fn time(&self, index: usize) -> Option<f64> {
        self.times.get(index).copied()
    }
    pub fn times(&self) -> &VecDeque<f64> {
        &self.times
    }

    /// Frame `index` (0 = oldest); decodes its block, keeping the last decoded block.
    pub fn get(&self, index: usize) -> Option<RecFrame> {
        let mut at = index + self.skip;
        for block in &self.blocks {
            if at < block.frames {
                return self.decoded(block).and_then(|frames| frames.get(at).cloned());
            }
            at -= block.frames;
        }
        self.open.get(at).cloned()
    }

    fn decoded(&self, block: &Block) -> Option<Arc<Vec<RecFrame>>> {
        if let Some((_, frames)) = &block.pending {
            return Some(frames.clone());
        }
        let mut cache = self.cache.lock().ok()?;
        if let Some((serial, frames)) = cache.as_ref()
            && *serial == block.serial
        {
            return Some(frames.clone());
        }
        let frames = Arc::new(decode_block(&block.bytes).map_err(|e| error!("replay block: {e}")).ok()?);
        *cache = Some((block.serial, frames.clone()));
        Some(frames)
    }

    pub fn clear(&mut self) {
        *self = Self { serial: self.serial, ..Self::default() };
    }

    /// The compressed blocks' bytes (the open block and blocks still compressing count nothing yet).
    pub fn compressed_bytes(&self) -> usize {
        self.bytes
    }

    /// A loaded save's distinct skater bone counts (empty for the live recording).
    pub fn rigs(&self) -> &[usize] {
        &self.rigs
    }

    /// The whole recording as complete blocks (the open block encoded too), oldest first: what a save writes.
    pub fn export_blocks(&self) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        for (i, block) in self.blocks.iter().enumerate() {
            let skip = if i == 0 { self.skip } else { 0 };
            if skip == 0 && block.pending.is_none() {
                out.push(block.bytes.clone());
            } else {
                let frames = self.decoded(block).unwrap_or_default();
                out.push(encode_block(&frames[skip.min(frames.len())..]));
            }
        }
        if !self.open.is_empty() {
            out.push(encode_block(&self.open));
        }
        out
    }

    /// A ring holding exactly these blocks (a loaded save). Review I4: any undecodable block, a frame without the
    /// local skater, times that are not finite and rising, or more than RING_FRAMES frames is an error.
    pub fn from_blocks(blocks: Vec<Vec<u8>>) -> Result<Self, String> {
        let mut ring = Ring::default();
        let mut last = f64::NEG_INFINITY;
        for bytes in blocks {
            let frames = decode_block(&bytes)?;
            if frames.is_empty() {
                continue;
            }
            if ring.len() + frames.len() > RING_FRAMES {
                return Err(format!("more than {RING_FRAMES} frames"));
            }
            for frame in &frames {
                if frame.time <= last {
                    return Err("replay times do not rise".into());
                }
                last = frame.time;
                let skater = frame.actors.first().ok_or("a frame has no skater")?;
                if !ring.rigs.contains(&skater.bones.len()) {
                    ring.rigs.push(skater.bones.len());
                }
                ring.strikes.extend(frame.strike.map(|s| (frame.time, s)));
            }
            ring.times.extend(frames.iter().map(|f| f.time));
            ring.bytes += bytes.len();
            ring.serial += 1;
            ring.blocks.push_back(Block { bytes, frames: frames.len(), serial: ring.serial, pending: None });
        }
        Ok(ring)
    }
}

// ---------------------------------------------------------------- quantisation
fn mm(v: f32) -> i32 {
    (v * 1000.).round().clamp(i32::MIN as f32, i32::MAX as f32) as i32
}
/// Smallest-three: the largest component's index and the other three, x sqrt 2, as i16.
fn quantise_rotation(q: Quat) -> (u8, [i32; 3]) {
    let c = q.normalize().to_array();
    let largest = (0..4).max_by(|&a, &b| c[a].abs().total_cmp(&c[b].abs())).unwrap_or(3);
    let sign = if c[largest] < 0. { -1. } else { 1. };
    let mut out = [0; 3];
    for (slot, i) in (0..4).filter(|&i| i != largest).enumerate() {
        out[slot] = ((c[i] * sign * std::f32::consts::SQRT_2).clamp(-1., 1.) * 32767.).round() as i32;
    }
    (largest as u8, out)
}
fn rotation(largest: u8, q: [i32; 3]) -> Result<Quat, String> {
    if largest > 3 || q.iter().any(|v| v.abs() > 32767) {
        return Err("bad rotation".into());
    }
    let mut c = [0f32; 4];
    let mut sum = 0.;
    for (slot, i) in (0..4).filter(|&i| i != largest as usize).enumerate() {
        c[i] = q[slot] as f32 / 32767. / std::f32::consts::SQRT_2;
        sum += c[i] * c[i];
    }
    c[largest as usize] = (1. - sum).max(0.).sqrt();
    Ok(Quat::from_array(c).normalize())
}

/// Linear prediction from a slot's last two quantised values (constant velocity): smooth motion leaves residuals
/// near zero, which the varints and zstd shrink.
#[derive(Clone, Copy, Default)]
struct Predictor {
    seen: u8,
    last: [i32; 3],
    before: [i32; 3],
}
impl Predictor {
    fn predict(&self) -> [i32; 3] {
        match self.seen {
            0 => [0; 3],
            1 => self.last,
            _ => std::array::from_fn(|i| self.last[i].wrapping_mul(2).wrapping_sub(self.before[i])),
        }
    }
    fn push(&mut self, v: [i32; 3]) {
        self.before = self.last;
        self.last = v;
        self.seen = (self.seen + 1).min(2);
    }
}
#[derive(Clone, Copy, Default)]
struct Slot {
    position: Predictor,
    largest: u8,
    rotation: Predictor,
}

/// Delta state for one block: per slot the previous quantised values, in flat Vecs (review I9; was a HashMap).
#[derive(Default)]
struct Slots {
    camera: Slot,
    /// Per actor: root, then bones.
    actors: Vec<Vec<Slot>>,
    props: Vec<Slot>,
}
impl Slots {
    /// Slot identity (unchanged, so the format is too): camera 0, actor a's root SLOT_STRIDE(a+1) and bone b
    /// SLOT_STRIDE(a+1)+1+b, prop p PROP_SLOTS+p.
    fn at(&mut self, slot: usize) -> &mut Slot {
        fn grow(v: &mut Vec<Slot>, i: usize) -> &mut Slot {
            if v.len() <= i {
                v.resize(i + 1, Slot::default());
            }
            &mut v[i]
        }
        match slot {
            0 => &mut self.camera,
            s if s >= PROP_SLOTS => grow(&mut self.props, s - PROP_SLOTS),
            s => {
                let a = (s / SLOT_STRIDE).saturating_sub(1);
                if self.actors.len() <= a {
                    self.actors.resize_with(a + 1, Vec::new);
                }
                grow(&mut self.actors[a], s % SLOT_STRIDE)
            }
        }
    }
}

const SLOT_STRIDE: usize = 1024;
const PROP_SLOTS: usize = SLOT_STRIDE * 64;
/// Review I4: transforms one block may hold, so a corrupt block cannot ask for gigabytes.
/// perf 10-19: was 1 << 18 (60 frames x ~4370), which 9 dressed remotes recorded node by node (~500 each) passed:
/// the block failed to decode and that second of replay was lost. Every transform takes at least 8 raw bytes, so
/// MAX_RAW already bounds a block to this many; the cap no longer rejects a block MAX_RAW lets through.
const MAX_TRANSFORMS: usize = MAX_RAW as usize / 8;
/// Review I4: a block's decompressed size cap (a real one is well under 1 MiB).
const MAX_RAW: u64 = 8 << 20;

fn put_var(out: &mut Vec<u8>, v: i32) {
    let mut z = ((v << 1) ^ (v >> 31)) as u32;
    while z >= 0x80 {
        out.push(z as u8 | 0x80);
        z >>= 7;
    }
    out.push(z as u8);
}
fn put_residual(out: &mut Vec<u8>, predictor: &mut Predictor, v: [i32; 3]) {
    let p = predictor.predict();
    for i in 0..3 {
        put_var(out, v[i].wrapping_sub(p[i]));
    }
    predictor.push(v);
}
fn get_residual(r: &mut Reader, predictor: &mut Predictor) -> Result<[i32; 3], String> {
    let p = predictor.predict();
    let v = [p[0].wrapping_add(r.var()?), p[1].wrapping_add(r.var()?), p[2].wrapping_add(r.var()?)];
    predictor.push(v);
    Ok(v)
}

fn put_transform(out: &mut Vec<u8>, slots: &mut Slots, slot: usize, t: &Transform) {
    let state = slots.at(slot);
    put_residual(out, &mut state.position, [mm(t.translation.x), mm(t.translation.y), mm(t.translation.z)]);
    let (largest, q) = quantise_rotation(t.rotation);
    if largest != state.largest {
        state.rotation = Predictor::default();
        state.largest = largest;
    }
    out.push(largest);
    put_residual(out, &mut state.rotation, q);
    put_scale(out, t.scale);
}
fn get_transform(r: &mut Reader, slots: &mut Slots, slot: usize) -> Result<Transform, String> {
    let state = slots.at(slot);
    let p = get_residual(r, &mut state.position)?;
    let largest = r.u8()?;
    if largest != state.largest {
        state.rotation = Predictor::default();
        state.largest = largest;
    }
    let q = get_residual(r, &mut state.rotation)?;
    Ok(Transform {
        translation: Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32) / 1000.,
        rotation: rotation(largest, q)?,
        scale: get_scale(r)?,
    })
}
fn put_scale(out: &mut Vec<u8>, scale: Vec3) {
    if (scale - Vec3::ONE).abs().max_element() < 1e-4 {
        out.push(0);
    } else {
        out.push(1);
        for v in scale.to_array() {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
}
fn get_scale(r: &mut Reader) -> Result<Vec3, String> {
    Ok(match r.u8()? {
        0 => Vec3::ONE,
        1 => Some(Vec3::new(r.f32()?, r.f32()?, r.f32()?))
            .filter(|s| s.is_finite())
            .ok_or("bad scale")?,
        _ => return Err("bad scale flag".into()),
    })
}

/// One block's raw bytes, built a frame at a time (review I9).
pub(crate) struct Encoder {
    raw: Vec<u8>,
    slots: Slots,
    audio: Vec<u8>,
    count: u16,
}
impl Default for Encoder {
    fn default() -> Self {
        Self { raw: vec![VERSION, 0, 0], slots: Slots::default(), audio: Vec::new(), count: 0 }
    }
}
impl Encoder {
    pub fn push(&mut self, frame: &RecFrame) {
        let (out, slots) = (&mut self.raw, &mut self.slots);
        out.extend_from_slice(&frame.time.to_le_bytes());
        out.push(u8::from(frame.cut));
        out.extend_from_slice(&frame.fov.to_le_bytes());
        put_transform(out, slots, 0, &frame.camera);
        out.extend_from_slice(&(frame.actors.len() as u16).to_le_bytes());
        for (a, actor) in frame.actors.iter().enumerate() {
            let base = SLOT_STRIDE * (a + 1);
            out.extend_from_slice(&actor.id.to_le_bytes());
            out.push(u8::from(actor.visible));
            put_transform(out, slots, base, &actor.root);
            out.extend_from_slice(&(actor.bones.len() as u16).to_le_bytes());
            for (b, bone) in actor.bones.iter().enumerate() {
                let (scale, rotation, translation) = bone.to_scale_rotation_translation();
                put_transform(out, slots, base + 1 + b, &Transform { translation, rotation, scale });
            }
            out.extend_from_slice(&(actor.joints.len() as u16).to_le_bytes());
            for (j, joint) in actor.joints.iter().enumerate() {
                put_transform(out, slots, base + 1 + actor.bones.len() + j, joint);
            }
        }
        out.extend_from_slice(&(frame.props.len() as u32).to_le_bytes());
        for (p, prop) in frame.props.iter().enumerate() {
            out.extend_from_slice(&prop.id.to_le_bytes());
            out.push(u8::from(prop.visible));
            put_transform(out, slots, PROP_SLOTS + p, &prop.transform);
        }
        match &frame.audio {
            None => out.push(0),
            Some(audio) => {
                let mut blob = Vec::with_capacity(self.audio.len());
                audio.put(&mut blob);
                // Most of the record holds still between frames: XOR against the previous one, then zstd.
                if blob.len() == self.audio.len() {
                    out.push(2);
                    out.extend_from_slice(&(blob.len() as u32).to_le_bytes());
                    out.extend(blob.iter().zip(&self.audio).map(|(a, b)| a ^ b));
                } else {
                    out.push(1);
                    out.extend_from_slice(&(blob.len() as u32).to_le_bytes());
                    out.extend_from_slice(&blob);
                }
                self.audio = blob;
            }
        }
        out.push((frame.rain.clamp(0., 1.) * 200.).round() as u8);
        match frame.strike {
            None => out.push(0),
            Some(s) => {
                out.push(1);
                out.extend_from_slice(&s.azimuth.to_le_bytes());
                out.extend_from_slice(&s.distance_m.to_le_bytes());
                out.extend_from_slice(&s.seed.to_le_bytes());
            }
        }
        self.count += 1;
    }
    pub fn finish(mut self) -> Vec<u8> {
        self.raw[1..3].copy_from_slice(&self.count.to_le_bytes());
        self.raw
    }
}

fn compress(raw: &[u8]) -> Vec<u8> {
    zstd::bulk::compress(raw, 3).unwrap_or_default()
}

pub(crate) fn encode_block(frames: &[RecFrame]) -> Vec<u8> {
    let mut encoder = Encoder::default();
    frames.iter().for_each(|f| encoder.push(f));
    compress(&encoder.finish())
}

pub(crate) fn decode_block(bytes: &[u8]) -> Result<Vec<RecFrame>, String> {
    use std::io::Read;
    // Review I4: bounded, so a crafted block cannot inflate without limit.
    let mut raw = Vec::new();
    zstd::stream::Decoder::new(bytes)
        .and_then(|d| d.take(MAX_RAW + 1).read_to_end(&mut raw))
        .map_err(|e| format!("replay block: {e}"))?;
    if raw.len() as u64 > MAX_RAW {
        return Err("replay block too large".into());
    }
    let mut r = Reader { bytes: &raw, at: 0 };
    let version = r.u8()?;
    if !(2..=VERSION).contains(&version) {
        return Err("replay block version".into());
    }
    let count = limit(usize::from(r.u16()?), BLOCK_FRAMES)?;
    let mut frames = Vec::with_capacity(count);
    let mut slots = Slots::default();
    let mut previous_audio: Vec<u8> = Vec::new();
    let mut transforms = 0;
    let mut last = f64::NEG_INFINITY;
    for _ in 0..count {
        let time = r.f64()?;
        if !time.is_finite() || time <= last {
            return Err("replay times do not rise".into());
        }
        last = time;
        let cut = r.u8()? != 0;
        let fov = r.f32()?;
        if !(fov.is_finite() && fov > 0. && fov < std::f32::consts::PI) {
            return Err("bad camera field of view".into());
        }
        let camera = get_transform(&mut r, &mut slots, 0)?;
        let actor_count = limit(usize::from(r.u16()?), 63)?;
        let mut actors = Vec::with_capacity(actor_count);
        for a in 0..actor_count {
            let base = SLOT_STRIDE * (a + 1);
            let id = r.u32()?;
            let visible = r.u8()? != 0;
            let root = get_transform(&mut r, &mut slots, base)?;
            let bone_count = limit(usize::from(r.u16()?), SLOT_STRIDE - 1)?;
            transforms = limit(transforms + bone_count + 1, MAX_TRANSFORMS)?;
            let mut bones = Vec::with_capacity(bone_count);
            for b in 0..bone_count {
                let t = get_transform(&mut r, &mut slots, base + 1 + b)?;
                bones.push(Mat4::from_scale_rotation_translation(t.scale, t.rotation, t.translation));
            }
            let mut joints = Vec::new();
            if version >= 4 {
                let joint_count = limit(usize::from(r.u16()?), SLOT_STRIDE - 1 - bone_count)?;
                transforms = limit(transforms + joint_count, MAX_TRANSFORMS)?;
                joints.reserve(joint_count);
                for j in 0..joint_count {
                    joints.push(get_transform(&mut r, &mut slots, base + 1 + bone_count + j)?);
                }
            }
            actors.push(Actor { id, root, bones, joints, visible });
        }
        let prop_count = limit(r.u32()? as usize, MAX_ITEMS)?;
        transforms = limit(transforms + prop_count, MAX_TRANSFORMS)?;
        let mut props = Vec::with_capacity(prop_count.min(1024));
        for p in 0..prop_count {
            let id = r.u32()?;
            let visible = r.u8()? != 0;
            props.push(Prop { id, visible, transform: get_transform(&mut r, &mut slots, PROP_SLOTS + p)? });
        }
        let audio = match r.u8()? {
            0 => None,
            mode @ (1 | 2) => {
                let len = limit(r.u32()? as usize, 1 << 20)?;
                let mut blob = r.take(len)?.to_vec();
                if mode == 2 {
                    if previous_audio.len() != blob.len() {
                        return Err("audio delta without its reference".into());
                    }
                    blob.iter_mut().zip(&previous_audio).for_each(|(a, b)| *a ^= b);
                }
                let audio = PlayerAudioObservation::get(&mut Reader { bytes: &blob, at: 0 })?;
                previous_audio = blob;
                Some(Box::new(audio))
            }
            _ => return Err("bad audio flag".into()),
        };
        let (mut rain, mut strike) = (0., None);
        if version >= 3 {
            rain = f32::from(r.u8()?.min(200)) / 200.;
            strike = match r.u8()? {
                0 => None,
                1 => {
                    let (azimuth, distance_m, seed) = (r.f32()?, r.f32()?, r.u32()?);
                    // The storm's 1-4 km only: a distance near 0 would make the thunder's gain infinite.
                    use crate::weather_audio::thunder::{MAX_KM, MIN_KM};
                    if !(azimuth.is_finite() && (MIN_KM * 1000. ..=MAX_KM * 1000.).contains(&distance_m)) {
                        return Err("bad lightning strike".into());
                    }
                    Some(crate::retail_render::LightningStrike { azimuth, distance_m, seed })
                }
                _ => return Err("bad strike flag".into()),
            };
        }
        frames.push(RecFrame { time, actors, camera, fov, props, audio, cut, rain, strike });
    }
    if r.at != raw.len() {
        return Err("trailing bytes in replay block".into());
    }
    Ok(frames)
}

fn limit(n: usize, max: usize) -> Result<usize, String> {
    if n > max { Err(format!("count {n} over {max}")) } else { Ok(n) }
}

// ---------------------------------------------------------------- byte reader
pub(crate) struct Reader<'a> {
    pub bytes: &'a [u8],
    pub at: usize,
}
impl<'a> Reader<'a> {
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.bytes.len()).ok_or("replay data is truncated")?;
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Ok(slice)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], String> {
        Ok(self.take(N)?.try_into().unwrap_or([0; N]))
    }
    pub fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    /// A zigzag LEB128 i32.
    fn var(&mut self) -> Result<i32, String> {
        let mut z = 0u32;
        for shift in (0..35).step_by(7) {
            let byte = self.u8()?;
            z |= u32::from(byte & 0x7f).checked_shl(shift).unwrap_or(0);
            if byte & 0x80 == 0 {
                return Ok((z >> 1) as i32 ^ -((z & 1) as i32));
            }
        }
        Err("bad varint".into())
    }
    pub fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(self.array()?))
    }
    pub fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.array()?))
    }
    pub fn i32(&mut self) -> Result<i32, String> {
        Ok(i32::from_le_bytes(self.array()?))
    }
    pub fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.array()?))
    }
    pub fn f32(&mut self) -> Result<f32, String> {
        Ok(f32::from_le_bytes(self.array()?))
    }
    pub fn f64(&mut self) -> Result<f64, String> {
        Ok(f64::from_le_bytes(self.array()?))
    }
}

// ---------------------------------------------------------------- the audio record
/// Positional binary encoding of the audio record. The struct literals below list every field, so a field added
/// to the record fails to compile here until it is recorded too.
pub(crate) trait Field: Sized {
    fn put(&self, out: &mut Vec<u8>);
    fn get(r: &mut Reader) -> Result<Self, String>;
}
macro_rules! scalar {
    ($($t:ty => $read:ident),*) => {$(
        impl Field for $t {
            fn put(&self, out: &mut Vec<u8>) { out.extend_from_slice(&self.to_le_bytes()); }
            fn get(r: &mut Reader) -> Result<Self, String> { r.$read() }
        }
    )*};
}
scalar!(u32 => u32, i32 => i32, u64 => u64);
/// Review I4: a loaded audio record never carries NaN or infinity into the mixer.
impl Field for f32 {
    fn put(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.to_le_bytes());
    }
    fn get(r: &mut Reader) -> Result<Self, String> {
        r.f32().map(|v| if v.is_finite() { v } else { 0. })
    }
}
impl Field for bool {
    fn put(&self, out: &mut Vec<u8>) {
        out.push(u8::from(*self));
    }
    fn get(r: &mut Reader) -> Result<Self, String> {
        Ok(r.u8()? != 0)
    }
}
impl Field for usize {
    fn put(&self, out: &mut Vec<u8>) {
        (*self as u64).put(out);
    }
    fn get(r: &mut Reader) -> Result<Self, String> {
        usize::try_from(r.u64()?).map_err(|e| e.to_string())
    }
}
impl Field for String {
    fn put(&self, out: &mut Vec<u8>) {
        (self.len() as u32).put(out);
        out.extend_from_slice(self.as_bytes());
    }
    fn get(r: &mut Reader) -> Result<Self, String> {
        let n = limit(r.u32()? as usize, 4096)?;
        String::from_utf8(r.take(n)?.to_vec()).map_err(|e| e.to_string())
    }
}
impl<T: Field> Field for Option<T> {
    fn put(&self, out: &mut Vec<u8>) {
        match self {
            None => out.push(0),
            Some(v) => {
                out.push(1);
                v.put(out);
            }
        }
    }
    fn get(r: &mut Reader) -> Result<Self, String> {
        Ok(if r.u8()? != 0 { Some(T::get(r)?) } else { None })
    }
}
impl<T: Field> Field for Vec<T> {
    fn put(&self, out: &mut Vec<u8>) {
        (self.len() as u32).put(out);
        self.iter().for_each(|v| v.put(out));
    }
    fn get(r: &mut Reader) -> Result<Self, String> {
        let n = limit(r.u32()? as usize, 4096)?;
        (0..n).map(|_| T::get(r)).collect()
    }
}
impl<T: Field, const N: usize> Field for [T; N] {
    fn put(&self, out: &mut Vec<u8>) {
        self.iter().for_each(|v| v.put(out));
    }
    fn get(r: &mut Reader) -> Result<Self, String> {
        let items = (0..N).map(|_| T::get(r)).collect::<Result<Vec<_>, _>>()?;
        items.try_into().map_err(|_| "array length".to_string())
    }
}
impl<A: Field, B: Field> Field for (A, B) {
    fn put(&self, out: &mut Vec<u8>) {
        self.0.put(out);
        self.1.put(out);
    }
    fn get(r: &mut Reader) -> Result<Self, String> {
        Ok((A::get(r)?, B::get(r)?))
    }
}
impl Field for PhysicsEvent {
    fn put(&self, out: &mut Vec<u8>) {
        match *self {
            PhysicsEvent::StateChanged { from, to } => {
                out.push(0);
                (from as u32).put(out);
                (to as u32).put(out);
            }
            PhysicsEvent::Landing => out.push(1),
            PhysicsEvent::Wipeout => out.push(2),
            PhysicsEvent::Contact { body } => out.push(if body == PhysicsBody::Board { 3 } else { 4 }),
        }
    }
    fn get(r: &mut Reader) -> Result<Self, String> {
        let state = |r: &mut Reader| r.u32().and_then(|v| v.try_into().map_err(|_| format!("physical state {v}")));
        Ok(match r.u8()? {
            0 => PhysicsEvent::StateChanged { from: state(r)?, to: state(r)? },
            1 => PhysicsEvent::Landing,
            2 => PhysicsEvent::Wipeout,
            3 => PhysicsEvent::Contact { body: PhysicsBody::Board },
            4 => PhysicsEvent::Contact { body: PhysicsBody::Rider },
            other => return Err(format!("physics event {other}")),
        })
    }
}
macro_rules! record {
    ($t:ident { $($f:ident),* $(,)? } $(, not_recorded: $($n:ident),*)?) => {
        impl Field for $t {
            fn put(&self, out: &mut Vec<u8>) { $(self.$f.put(out);)* }
            fn get(r: &mut Reader) -> Result<Self, String> {
                Ok($t { $($f: Field::get(r)?,)* $($($n: Default::default(),)*)? })
            }
        }
    };
}
record!(BodyContacts {
    contact, weighted_change, slide, material, specific_current, group_8_force, skater_force, other_skater,
    group_11_force,
});
record!(RetailAudioInputs {
    dt, ground_speed, com_velocity, position, wheel_count, air_time_in_state, air_time_until_landing,
    air_jump_height, state_flags, offboard_feet, footplant, footstep_strength, state_category, state,
    filtered_state, in_known_air, grinding, grind_family, grind_audio_surface, grind_impact_speed, grind_flag_323,
    air_440, jump_velocity_delta, footplant_448, footplant_surface, offboard_surface, offboard_309, offboard_310,
    offboard_311, skeleton_599, feet_in_deck_box, foot_local_velocity, foot_world_velocity, wheel_contacts,
    truck_contacts, deck_contact, wheel_contact_normals, wheel_audio_surfaces, wheel_seam_patterns,
    part_audio_surfaces, deck_slide_speed, deck_scrape, angular_velocity, linear_velocity, deck_tilt, motion_200,
    wheel_velocities, deck_rows, effective_deck_up, wheel_positions, camera, deck_position, deck_forward,
    combo_multiplier, paused, time_scale, ground_normal, turn, jump_strength, scorable_id, ragdoll_spin,
    limb_speeds, toe_positions, body_contacts, pump_absorption,
// ponytail: rumble-only inputs, kept out of the block format (a version bump if replays need rumble).
}, not_recorded: rumble_physics_surface, scoring_ground_or_air);
record!(PlayerAudioObservation {
    tick, state, board_speed, rider_speed, grounded, grinding, grind_family, grind_substate, grind_audio_surface,
    grind_impact_speed, wiping_out, landed, landing_impact_speed, landing_clean, landing_sketchy, landing_type,
    landing_spin, landing_sideways_speed, footstep_strength, footstep_bone, foot_push_speed, feet_supported,
    foot_surface, contact_count, powersliding, trick_id, trick_identifier, animation_name, riding_switch,
    riding_fakie, nollie, wheel_surface, events, frontend_sounds, volumes, map, skate2, district, retail,
}, not_recorded: remote_records, watched, traffic, riders, dmos);  // a replay plays its own skater only

#[cfg(test)]
#[path = "record_tests.rs"]
pub(crate) mod tests;
