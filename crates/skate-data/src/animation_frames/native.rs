//! Bank-backed lazy animation samples. Each requested clip is decoded once;
//! registration indexes every record without allocating every decoded frame.
use super::{ClipFrames, ReferencePose, SampleWords, validate_samples};
use crate::{
    abin::{Bank, Codec, PartEntry, Reader, RecordData},
    animation_banks::AnimationBanks,
};
use skate_core::animation::vbr::VbrDecoder;
use std::{
    collections::BTreeMap,
    sync::{Arc, OnceLock},
};

struct Entry<T> {
    bank: usize,
    offset: usize,
    decoded: OnceLock<Result<T, String>>,
}
impl<T> Entry<T> {
    fn new(bank: usize, offset: usize) -> Self {
        Self {
            bank,
            offset,
            decoded: OnceLock::new(),
        }
    }
}
pub(super) struct NativeFrames {
    banks: Vec<Arc<Bank>>,
    clips: BTreeMap<String, Entry<ClipFrames>>,
    poses: BTreeMap<(usize, u64), Entry<ReferencePose>>,
    pose_names: BTreeMap<(usize, String), u64>,
    /// Bones whose reference local is overwritten, by name, before a RAW clip composes on it (retail 0x826c0550).
    base: Vec<(String, SampleWords)>,
}
impl NativeFrames {
    pub(super) fn new(banks: &AnimationBanks) -> Result<Self, String> {
        let mut clips: BTreeMap<String, Entry<ClipFrames>> = BTreeMap::new();
        let mut poses = BTreeMap::new();
        let mut pose_names = BTreeMap::new();
        for (bank_index, bank) in banks.banks.iter().enumerate() {
            for record in bank.records() {
                let name = &record.header.name;
                let at = record.header.offset;
                match &record.data {
                    RecordData::Clip(_) => {
                        if clips
                            .get(name)
                            .is_some_and(|entry| entry.bank != bank_index)
                        {
                            return Err(format!(
                                "Ambiguous clip {name} in multiple animation banks"
                            ));
                        }
                        clips.insert(name.clone(), Entry::new(bank_index, at));
                    }
                    RecordData::Pose(_) => {
                        poses.insert((bank_index, at as u64), Entry::new(bank_index, at));
                        pose_names.insert((bank_index, name.clone()), at as u64);
                    }
                    _ => {}
                }
            }
        }
        Ok(Self {
            banks: banks.banks.clone(),
            clips,
            poses,
            pose_names,
            base: Vec::new(),
        })
    }
    pub(super) fn clip_count(&self) -> usize {
        self.clips.len()
    }
    pub(super) fn clip_names(&self) -> impl Iterator<Item = &str> {
        self.clips.keys().map(String::as_str)
    }
    pub(super) fn clip_bank(&self, name: &str) -> Result<usize, String> {
        self.clips
            .get(&name.to_ascii_uppercase())
            .map(|e| e.bank)
            .ok_or_else(|| format!("Missing stock animation clip {name}"))
    }
    pub(super) fn clip(&self, name: &str) -> Result<&ClipFrames, String> {
        let entry = self
            .clips
            .get(&name.to_ascii_uppercase())
            .ok_or_else(|| format!("Missing stock animation clip {name}"))?;
        entry
            .decoded
            .get_or_init(|| decode_clip(&self.banks[entry.bank], entry.offset, &self.base))
            .as_ref()
            .map_err(Clone::clone)
    }
    pub(super) fn set_base(&mut self, base: Vec<(String, SampleWords)>) {
        self.base = base;
        for entry in self.clips.values_mut() {
            entry.decoded = OnceLock::new();
        }
    }
    pub(super) fn reference_samples(&self, bank: usize) -> Result<Vec<SampleWords>, String> {
        raw_reference(self.banks.get(bank).ok_or("No such animation bank")?)
    }
    pub(super) fn named_pose(&self, bank: usize, name: &str) -> Result<&ReferencePose, String> {
        let at = self
            .pose_names
            .get(&(bank, name.to_ascii_uppercase()))
            .ok_or_else(|| format!("Missing reference pose {name} in bank {bank}"))?;
        self.reference_pose(bank, *at)
    }
    pub(super) fn reference_pose(
        &self,
        bank: usize,
        offset: u64,
    ) -> Result<&ReferencePose, String> {
        let entry = self
            .poses
            .get(&(bank, offset))
            .ok_or_else(|| format!("Missing reference pose {bank}:{offset:#x}"))?;
        entry
            .decoded
            .get_or_init(|| decode_pose(&self.banks[bank], entry.offset))
            .as_ref()
            .map_err(Clone::clone)
    }
}

fn decode_clip(bank: &Bank, offset: usize, base: &[(String, SampleWords)]) -> Result<ClipFrames, String> {
    let record = bank.record_at(offset).ok_or("Missing clip source record")?;
    let RecordData::Clip(clip) = &record.data else {
        return Err("Source record is not a clip".into());
    };
    let count = f32::from_bits(clip.frame_count_bits);
    if !count.is_finite() || count < 1.0 || count.fract() != 0.0 {
        return Err(format!("{}: invalid frame count", record.header.name));
    }
    let frames = if record.header.codec() == Codec::Raw {
        let mut reference = raw_reference(bank)?;
        let names = &bank.hierarchy().ok_or("No bank hierarchy")?.bone_names;
        for (name, words) in base {
            if let Some(bone) = names.iter().position(|n| n.eq_ignore_ascii_case(name)) {
                reference[bone] = *words;
            }
        }
        decode_raw(bank, &clip.parts, count as usize, Some(&reference))
    } else {
        require_vbr(record.header.codec())?;
        decode_parts(bank, &clip.parts, count as usize)
    }
    .map_err(|e| format!("{}: {e}", record.header.name))?;
    let bones = bank.hierarchy().ok_or("No bank hierarchy")?.bone_count as usize;
    let mut weights = vec![1.0f32.to_bits(); bones];
    if clip.channel_animation() {
        let reader = Reader::new(bank.bytes());
        for (entry, layout) in clip.parts.iter().zip(&bank.hierarchy().unwrap().parts) {
            let part = entry
                .part
                .as_ref()
                .ok_or("Channel animation part is absent")?;
            let range = part
                .channel_weights
                .as_ref()
                .ok_or("Channel animation weights are absent")?;
            for lane in 0..layout.bone_count as usize {
                let word = reader
                    .u32(range.start + lane * 4)
                    .map_err(|e| e.to_string())?;
                if !f32::from_bits(word).is_finite() {
                    return Err("Nonfinite channel weight".into());
                }
                weights[layout.sqt_offset as usize + lane] = word;
            }
        }
    }
    Ok(ClipFrames {
        name: record.header.name.clone(),
        source_offset: offset as u64,
        fps_bits: clip.fps_bits,
        loop_translation_bits: clip.loop_translation_words[..3].try_into().unwrap(),
        loop_rotation_bits: clip.loop_rotation_words,
        channel_animation: clip.channel_animation(),
        channel_weights: weights,
        frames,
    })
}
fn decode_pose(bank: &Bank, offset: usize) -> Result<ReferencePose, String> {
    let record = bank.record_at(offset).ok_or("Missing pose source record")?;
    let RecordData::Pose(pose) = &record.data else {
        return Err("Source record is not a pose".into());
    };
    let mut frames = if record.header.codec() == Codec::Raw {
        decode_raw(bank, &pose.parts, 1, None)
    } else {
        require_vbr(record.header.codec())?;
        decode_parts(bank, &pose.parts, 1)
    }
    .map_err(|e| format!("{}: {e}", record.header.name))?;
    Ok(ReferencePose {
        name: record.header.name.clone(),
        source_offset: offset as u64,
        samples: frames.remove(0),
    })
}
/// The bank's reference pose (named after its hierarchy): RAW clips animate only
/// some channels and keep this pose for the rest.
/// The bank's reference pose (the pose named as its hierarchy), in either codec: the rest shape for the bones a clip
/// doesn't carry.
fn bank_reference(bank: &Bank) -> Result<Vec<SampleWords>, String> {
    let records = bank.records();
    let hierarchy = records.iter().find(|r| r.header.type_id == 4).map(|r| &r.header.name);
    let poses = || records.iter().filter(|r| r.header.type_id == 3);
    let record = poses().find(|r| Some(&r.header.name) == hierarchy).or_else(|| poses().next()).ok_or("bank has no reference pose")?;
    let RecordData::Pose(pose) = &record.data else { return Err("reference record is not a pose".into()) };
    let mut frames = if record.header.codec() == Codec::Raw {
        decode_raw(bank, &pose.parts, 1, None)?
    } else {
        require_vbr(record.header.codec())?;
        decode_parts(bank, &pose.parts, 1)?
    };
    Ok(frames.remove(0))
}
fn raw_reference(bank: &Bank) -> Result<Vec<SampleWords>, String> {
    let records = bank.records();
    let raw_pose = |r: &&crate::abin::Record| {
        r.header.type_id == 3 && matches!(r.data, RecordData::Pose(_)) && r.header.codec() == Codec::Raw
    };
    // Gesture banks list posture delta poses before the reference; pick it by name.
    let hierarchy = records.iter().find(|r| r.header.type_id == 4).map(|r| &r.header.name);
    let record = records
        .iter()
        .filter(raw_pose)
        .find(|r| Some(&r.header.name) == hierarchy)
        .or_else(|| records.iter().find(raw_pose))
        .ok_or("RAW bank has no reference pose")?;
    let RecordData::Pose(pose) = &record.data else { unreachable!() };
    Ok(decode_raw(bank, &pose.parts, 1, None)?.remove(0))
}
/// RAW ("RD") parts, as in the Create-a-Skater banks: per part an 8-word header
/// (word 2 rotation bone mask, word 4 translation bone mask, word 6 high half
/// the per-frame stride, low half bone count - 1), then big-endian floats per
/// frame, per bone: (Skate 2 scenes: scale XYZ if word 0 masks it,) quaternion XYZW (Skate 2 scenes: one
/// packed word when word 7 says so) if animated, then translation XYZ if animated.
/// A clip's channels are local transforms applied after the bank's reference pose
/// (reference x clip); verified by CAS_STAND's upright body and board in hand.
fn decode_raw(
    bank: &Bank,
    parts: &[PartEntry],
    count: usize,
    base: Option<&Vec<SampleWords>>,
) -> Result<Vec<Vec<SampleWords>>, String> {
    let hierarchy = bank.hierarchy().ok_or("Missing hierarchy")?;
    let bones = hierarchy.bone_count as usize;
    let one = 1.0f32.to_bits();
    let identity: SampleWords = [one, one, one, 0, 0, 0, one, 0, 0, 0];
    let start = base.cloned().unwrap_or_else(|| vec![identity; bones]);
    if start.len() != bones {
        return Err("RAW reference pose bone count differs".into());
    }
    let mut frames = vec![start; count];
    let bytes = bank.bytes();
    let word = |at: usize| -> Result<u32, String> {
        bytes.get(at..at + 4).map(|b| u32::from_be_bytes(b.try_into().unwrap())).ok_or_else(|| "RAW data out of range".to_string())
    };
    // Entries follow hierarchy part order, omitting parts a record leaves out;
    // each entry's channel count names its part.
    let mut cursor = 0;
    for entry in parts {
        let Some(part) = entry.part.as_ref() else { continue };
        while hierarchy.parts.get(cursor).is_some_and(|l| l.bone_count as u16 != part.channel_count) {
            cursor += 1;
        }
        let layout = hierarchy.parts.get(cursor).ok_or("RAW part is not in the hierarchy")?;
        cursor += 1;
        let header = part.compression_header.start;
        let (rotations, translations, shape) = (word(header + 8)?, word(header + 16)?, word(header + 24)?);
        let stride = (shape >> 16) as usize;
        let lanes = layout.bone_count as usize;
        // Skate 2 scene banks (word 7 bit 24) pack each rotation into one word; see `packed_quaternion`.
        let packed = word(header + 28)? & 0x0100_0000 != 0;
        let rotation_bytes = if packed { 4 } else { 16 };
        let expected: usize = (0..lanes).map(|b| rotation_bytes * (rotations >> b & 1) as usize + 12 * (translations >> b & 1) as usize).sum();
        // Skate 2 scene banks also animate scale: word 0 is its bone mask, XYZ before the rotation.
        let scale_mask = word(header)?;
        let scaled: usize = (0..lanes).map(|b| 12 * (scale_mask >> b & 1) as usize).sum();
        // ponytail: only when the stride proves it, so Skate 3 banks keep decoding exactly as before.
        let scales = if scaled > 0 && stride == expected + scaled { scale_mask } else { 0 };
        let expected = if scales != 0 { expected + scaled } else { expected };
        if stride != expected || part.compressed_data.len() < stride * count {
            return Err(format!("{}: RAW stride {stride} disagrees with channel masks (expected {expected}, {lanes} lanes, header {:x?})", layout.name, (0..8).map(|i| word(header + 4 * i).unwrap_or(0)).collect::<Vec<_>>()));
        }
        let first = usize::try_from(layout.sqt_offset).map_err(|_| "Negative animation part offset")?;
        let relative = base.is_some();
        if first + lanes > bones {
            return Err("RAW part exceeds hierarchy".into());
        }
        for (index, frame) in frames.iter_mut().enumerate() {
            let mut at = part.compressed_data.start + index * stride;
            for lane in 0..lanes {
                let sample = &mut frame[first + lane];
                let f = |w: u32| f32::from_bits(w);
                // Reference transform, before this frame's channel replaces it.
                let reference = [f(sample[3]), f(sample[4]), f(sample[5]), f(sample[6])];
                let offset = [f(sample[7]), f(sample[8]), f(sample[9])];
                // Op 8 (kernel 0x82835d88): out.s = base.s * delta.s (the base is 1 outside a clip).
                if scales >> lane & 1 != 0 {
                    for c in 0..3 {
                        sample[c] = (f(sample[c]) * f(word(at)?)).to_bits();
                        at += 4;
                    }
                }
                if rotations >> lane & 1 != 0 {
                    let mut q = [0f32; 4];
                    if packed {
                        q = packed_quaternion(word(at)?);
                        at += 4;
                    } else {
                        for c in &mut q {
                            *c = f(word(at)?);
                            at += 4;
                        }
                    }
                    if relative {
                        q = quat_mul(reference, q);
                    }
                    for c in 0..4 {
                        sample[3 + c] = q[c].to_bits();
                    }
                }
                if translations >> lane & 1 != 0 {
                    let mut t = [0f32; 3];
                    for c in &mut t {
                        *c = f(word(at)?);
                        at += 4;
                    }
                    if relative {
                        let r = rotate(reference, t);
                        t = [offset[0] + r[0], offset[1] + r[1], offset[2] + r[2]];
                    }
                    for c in 0..3 {
                        sample[7 + c] = t[c].to_bits();
                    }
                }
            }
        }
    }
    for frame in &frames {
        validate_samples(frame, bones)?;
    }
    Ok(frames)
}
/// A Skate 2 scene-bank rotation in one word: bits 0-10 sin(angle/2) over 2047, bits 11-20 the axis's polar
/// angle from -Z (0) to +Z (1023), bits 21-31 its azimuth (a full turn over 2048, from -X towards -Y).
/// Fitted against the float reference pose of the same character (SLAPPY) in another bank: 119 bones agree
/// within 0.005.
fn packed_quaternion(word: u32) -> [f32; 4] {
    use std::f32::consts::PI;
    let s = (word & 0x7ff) as f32 / 2047.;
    let polar = PI * (1. - ((word >> 11) & 0x3ff) as f32 / 1023.);
    let azimuth = (word >> 21) as f32 / 2048. * 2. * PI;
    let (sp, cp) = polar.sin_cos();
    let (sa, ca) = azimuth.sin_cos();
    [-sp * ca * s, -sp * sa * s, cp * s, (1. - s * s).max(0.).sqrt()]
}
fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}
fn rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let p = quat_mul(quat_mul(q, [v[0], v[1], v[2], 0.]), [-q[0], -q[1], -q[2], q[3]]);
    [p[0], p[1], p[2]]
}
fn require_vbr(codec: Codec) -> Result<(), String> {
    if codec != Codec::Vbr {
        return Err(format!("Unsupported native animation codec {codec:?}"));
    }
    Ok(())
}
fn decode_parts(
    bank: &Bank,
    parts: &[PartEntry],
    count: usize,
) -> Result<Vec<Vec<SampleWords>>, String> {
    let hierarchy = bank.hierarchy().ok_or("Missing stock hierarchy")?;
    let bones = hierarchy.bone_count as usize;
    // A clip carries the hierarchy's first N parts in order; the parts after them must be ones this bank's clips don't
    // drive (flags bit 0 clear: PedestrianSkeletonPres' hands, face and twist helpers, owned by other banks). Their
    // bones keep the bank's reference pose. RETAIL data (peer-part-mapping.md); the retail fill is INFERRED.
    let missing = hierarchy.parts.get(parts.len()..).ok_or("Animation part count differs from hierarchy")?;
    if missing.iter().any(|p| p.flags & 1 != 0) {
        return Err("Animation part count differs from hierarchy".into());
    }
    let mut frames = vec![vec![[0; 10]; bones]; count];
    let mut assigned = vec![false; bones];
    for (entry, layout) in parts.iter().zip(&hierarchy.parts) {
        let part = entry
            .part
            .as_ref()
            .ok_or("Required stock animation part is absent")?;
        let mut decoder = VbrDecoder::new(
            bank.bytes(),
            part.offset,
            part.compression_header_relative as usize,
            part.compressed_data_relative as usize,
        )?;
        if decoder.frame_count() != count || decoder.channel_count() != layout.bone_count as usize {
            return Err(format!(
                "{}: codec frame/channel count disagrees with bank header",
                layout.name
            ));
        }
        let start =
            usize::try_from(layout.sqt_offset).map_err(|_| "Negative animation part offset")?;
        let end = start + layout.bone_count as usize;
        if end > bones || assigned[start..end].iter().any(|b| *b) {
            return Err("Overlapping animation parts".into());
        }
        assigned[start..end].fill(true);
        for (index, frame) in frames.iter_mut().enumerate() {
            frame[start..end].copy_from_slice(&decoder.decode_frame(index)?);
        }
    }
    if !missing.is_empty() {
        let reference = bank_reference(bank)?;
        for frame in &mut frames {
            for bone in (0..bones).filter(|&b| !assigned[b]) {
                frame[bone] = reference[bone];
            }
        }
        assigned.fill(true);
    }
    if assigned.iter().any(|v| !*v) {
        return Err("Incomplete animation hierarchy channels".into());
    }
    for frame in &frames {
        validate_samples(frame, bones)?;
    }
    Ok(frames)
}

#[cfg(test)]
mod packed_quaternion_tests {
    /// Words from ProsArrival_Slappy1's SLAPPY reference against WelcomeBack_Slappy1's float one.
    #[test]
    fn decodes_skate2_scene_reference_rotations() {
        for (word, want) in [
            (0x401f_f927, [0.0, 0.0, 0.1442, 0.9895]),
            (0xc2ae_6639, [-0.0503, 0.7671, -0.1215, 0.6279]),
            (0xa3d5_c6b4, [0.4501, 0.5453, 0.4501, 0.5453]),
            (0x9fff_fda7, [0.0, 0.0, 0.7071, 0.7071]),
        ] {
            let got = super::packed_quaternion(word);
            assert!(got.iter().zip(want).all(|(a, b)| (a - b).abs() < 0.005), "{word:#x}: {got:?}");
        }
    }
}

#[cfg(test)]
mod face_base_tests {
    use super::*;
    use std::path::PathBuf;

    /// Retail 0x826c0550 (the player's NIS base): named bones' base locals are overwritten before the clip composes
    /// on them; scale composes as op 8 (kernel 0x82835d88), base.s * delta.s.
    #[test]
    #[ignore = "requires SKATE3_ASSET_ROOT with private/skate2-tutorial"]
    fn face_base_overwrites_a_bones_reference_by_name() {
        let path = PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap())
            .join("private/skate2-tutorial/scene/anim/WelcomeBack_Player1.abin");
        let bank = Arc::new(Bank::load(&path).unwrap());
        let banks = AnimationBanks { banks: vec![bank.clone()], identities: vec!["t".into()] };
        let names = bank.hierarchy().unwrap().bone_names.clone();
        let at = |n: &str| names.iter().position(|b| b.eq_ignore_ascii_case(n)).unwrap();
        let (eye, lip) = (at("LEFTEYE"), at("LOWERLIP"));
        let plain = NativeFrames::new(&banks).unwrap();
        let clip = plain.clip_names().next().unwrap().to_owned();
        let reference = raw_reference(&bank).unwrap();
        let f = f32::from_bits;
        // T(1 mm along the parent's X) * LEFTEYE's base; LOWERLIP's base scale doubled.
        let mut moved = reference[eye];
        moved[7] = (f(moved[7]) + 0.001).to_bits();
        let mut scaled = reference[lip];
        for c in 0..3 {
            scaled[c] = (2. * f(scaled[c])).to_bits();
        }
        let mut faced = NativeFrames::new(&banks).unwrap();
        faced.set_base(vec![("LeftEye".into(), moved), ("LowerLip".into(), scaled)]);
        let (a, b) = (&plain.clip(&clip).unwrap().frames, &faced.clip(&clip).unwrap().frames);
        for (i, (x, y)) in a[0].iter().zip(&b[0]).enumerate() {
            if i == eye {
                assert!((f(y[7]) - f(x[7]) - 0.001).abs() < 1e-6, "LEFTEYE x {} vs {}", f(y[7]), f(x[7]));
                assert_eq!((&x[..7], &x[8..]), (&y[..7], &y[8..]), "LEFTEYE moves along X only");
            } else if i != lip {
                assert_eq!(x, y, "{}", names[i]);
            }
        }
        for (x, y) in a.iter().zip(b) {
            for c in 0..3 {
                assert!((f(y[lip][c]) - 2. * f(x[lip][c])).abs() < 1e-5, "LOWERLIP scale {} vs {}", f(y[lip][c]), f(x[lip][c]));
            }
        }
    }
}
