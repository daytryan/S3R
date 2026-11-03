//! Version 5: one datagram per independently decodable stream update.
//! Position error <= 0.5 mm/axis within 32 m; larger offsets use full floats.
//! Quaternion uses smallest-three 10-bit components. Velocities use binary16.
use crate::{Body, Bone, Pose};
use half::f16;
pub const MTU: usize = 1200;
/// The protocol version: 07 = reliable match events (`@ev` kept until every `@score` has taken them), `@ready`; 08 =
/// challenge proposals voted on (`@accept`, the proposal token), host-assigned teams (`tb`, `@switch`, `Game::teams`);
/// 09 = Skate 2 contests' RepositionAI fade in the contest snapshot (`Stage::Move`, `nr` / `po` / `mk`); 10 = the
/// sender's gameplay camera rides in the pose stream as row [`CAMERA`] (what a spectator sees); 11 = the skater audio
/// records (`crate::audio::AUDIO`), so the other skaters are heard; 12 = HELLO acknowledges the ROSTER it holds (sent
/// only to members lacking it, coalesced: 10-12 audit B1); 13 = the match relay's payloads changed shape (10-12 matrix:
/// `Relay::End(EndMark)`, `RelayEv::Voted`, sequenced S2 contest events `Wire::E(u32, Event)` + `Snap.tk`,
/// `TurnOver.best`), which a 12 build drops unparsed and then waits on for good (10-12 end review I1); 14 = every
/// datagram carries its receiver's link token after the sequence ([`token`]), a HELLO is admitted only with the host's
/// cookie and a link gets probes only until it answers (online safety 3.4, P1, Q1). A 13 build is refused: it took
/// forged packets and streamed at whatever endpoint a roster named. 15 (perf 10-18) = a BODY / POSE carries the sender's
/// ACK of the receiver's streams ([`set_ack`]) and a changed narrow row goes as byte deltas of its quantized fields
/// against the baseline ([`delta`]): about 40% less movement upload. 16 (10-28): the `@session` carries the host's
/// Skate 2 physics (`ph`), which a 15 build would ignore, skating Skate 3 physics in a Skate 2 session. 17 (10-28):
/// Skate 1 / 2 modes take Skate 3's turn rules (S1 `skateroffboard` / `UsedSessionMarker`, S2 `Outcome::OffBoard`, the
/// token's first player): a 16 build would play the old rules in the same match. 18 (11-01): 150-character chat lines,
/// the spectate eye and the Skate 1 trick state's id / switch, which a 17 build's relay drops with their events.
pub const MAGIC: &[u8; 8] = b"SK8NET18";
/// The pose row holding the sender's camera (engine space, world), never a skeleton bone.
pub const CAMERA: u16 = 255;
/// MAGIC, session, actor, kind, sequence, then the link token (bytes 29..37).
pub const HEADER: usize = 37;
pub const BODY: u8 = 1;
pub const POSE: u8 = 2;
#[derive(Clone, Debug, PartialEq)]
pub struct Packed {
    /// Source capture time in milliseconds; forwarding never retimestamps it.
    pub captured: u64,
    pub root: [u8; 16],
    pub enabled: u64,
    pub rows: Vec<Vec<u8>>,
}
#[derive(Clone, Debug)]
pub struct BodyState {
    pub root: Pose,
    pub enabled: u64,
    pub bodies: Vec<Body>,
}
#[derive(Clone, Debug)]
pub struct PoseState {
    pub root: Pose,
    pub bones: Vec<Bone>,
}
pub fn header(session: u64, actor: u64, kind: u8, sequence: u32) -> Vec<u8> {
    let mut b = Vec::with_capacity(MTU);
    b.extend(MAGIC);
    b.extend(session.to_le_bytes());
    b.extend(actor.to_le_bytes());
    b.push(kind);
    b.extend(sequence.to_le_bytes());
    b.extend(0u64.to_le_bytes());
    b
}
/// The token the receiver gave the sender for this link (0: none yet).
pub fn token(b: &[u8]) -> u64 {
    b.get(29..HEADER).map_or(0, |t| u64::from_le_bytes(t.try_into().unwrap()))
}
pub(crate) fn stamp(b: &mut [u8], token: u64) {
    if let Some(t) = b.get_mut(29..HEADER) {
        t.copy_from_slice(&token.to_le_bytes());
    }
}
pub fn envelope(b: &[u8]) -> Option<(u64, u64, u8, u32)> {
    if b.len() < HEADER || b.len() > MTU || &b[..8] != MAGIC {
        return None;
    }
    Some((
        u64::from_le_bytes(b[8..16].try_into().ok()?),
        u64::from_le_bytes(b[16..24].try_into().ok()?),
        b[24],
        u32::from_le_bytes(b[25..29].try_into().ok()?),
    ))
}
/// The version digits of a datagram from another SK8NET build (`08` to our `09`), which [`envelope`] refuses: the
/// lobby says so instead of dropping it silently (online audit M7).
/// Two ASCII digits only: a stranger's other bytes never reach the screen (online safety Q12).
pub fn other_version(b: &[u8]) -> Option<[u8; 2]> {
    (b.len() >= 8 && b[..6] == MAGIC[..6] && b[..8] != MAGIC[..] && b[6..8].iter().all(u8::is_ascii_digit)).then(|| [b[6], b[7]])
}
pub struct Reader<'a>(pub &'a [u8]);
impl Reader<'_> {
    pub fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let b = self.0.get(..N)?.try_into().ok()?;
        self.0 = &self.0[N..];
        Some(b)
    }
    pub fn byte(&mut self) -> Option<u8> {
        Some(self.take::<1>()?[0])
    }
    pub fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take()?))
    }
    pub fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take()?))
    }
}
fn quat(q: [f32; 4]) -> [u8; 4] {
    let largest = (0..4)
        .max_by(|&a, &b| q[a].abs().total_cmp(&q[b].abs()))
        .unwrap();
    let sign = if q[largest] < 0. { -1. } else { 1. };
    let mut bits = largest as u32;
    let mut shift = 2;
    for (i, v) in q.iter().enumerate() {
        if i != largest {
            let n = ((v * sign * std::f32::consts::SQRT_2 * 0.5 + 0.5) * 1023.)
                .round()
                .clamp(0., 1023.) as u32;
            bits |= n << shift;
            shift += 10;
        }
    }
    bits.to_le_bytes()
}
fn unquat(b: [u8; 4]) -> [f32; 4] {
    let bits = u32::from_le_bytes(b);
    let largest = (bits & 3) as usize;
    let mut q = [0.; 4];
    let mut shift = 2;
    let mut sum = 0.;
    for (i, v) in q.iter_mut().enumerate() {
        if i != largest {
            *v = (((bits >> shift) & 1023) as f32 / 1023. - 0.5) * std::f32::consts::SQRT_2;
            sum += *v * *v;
            shift += 10;
        }
    }
    q[largest] = (1. - sum).max(0.).sqrt();
    let norm = q.iter().map(|v| v * v).sum::<f32>().sqrt();
    q.map(|v| v / norm)
}
fn root(p: Pose) -> [u8; 16] {
    let mut b = [0; 16];
    for i in 0..3 {
        b[i * 4..i * 4 + 4].copy_from_slice(&p.p[i].to_le_bytes());
    }
    b[12..].copy_from_slice(&quat(p.q));
    b
}
fn unroot(b: [u8; 16]) -> Option<Pose> {
    let p = Pose {
        p: std::array::from_fn(|i| f32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap())),
        q: unquat(b[12..].try_into().unwrap()),
    };
    p.valid().then_some(p)
}
fn row_pose(b: &mut Vec<u8>, p: Pose, origin: [f32; 3]) {
    let relative = std::array::from_fn::<_, 3, _>(|i| p.p[i] - origin[i]);
    let small = relative.iter().all(|v| v.abs() <= 32.767);
    b.push(u8::from(!small));
    for v in relative {
        if small {
            b.extend(((v * 1000.).round() as i16).to_le_bytes());
        } else {
            b.extend(v.to_le_bytes());
        }
    }
    b.extend(quat(p.q));
}
fn read_pose(r: &mut Reader, origin: [f32; 3]) -> Option<Pose> {
    let wide = r.byte()?;
    if wide > 1 {
        return None;
    }
    let mut p = [0.; 3];
    for i in 0..3 {
        p[i] = origin[i]
            + if wide == 0 {
                i16::from_le_bytes(r.take()?) as f32 / 1000.
            } else {
                f32::from_le_bytes(r.take()?)
            };
    }
    let p = Pose {
        p,
        q: unquat(r.take()?),
    };
    p.valid().then_some(p)
}
fn rates(b: &mut Vec<u8>, v: [f32; 3]) {
    for f in v {
        b.extend(f16::from_f32(f).to_bits().to_le_bytes());
    }
}
fn read_rates(r: &mut Reader, bound: f32) -> Option<[f32; 3]> {
    let mut v = [0.; 3];
    for f in &mut v {
        *f = f16::from_bits(u16::from_le_bytes(r.take()?)).to_f32();
        if !f.is_finite() || f.abs() > bound {
            return None;
        }
    }
    Some(v)
}
impl Packed {
    pub fn same_state(&self, other: &Self) -> bool {
        self.root == other.root && self.enabled == other.enabled && self.rows == other.rows
    }
    pub fn body(s: &BodyState) -> Option<Self> {
        if !s.root.valid()
            || s.bodies.len() != 33
            || (s.enabled & !((1 << 63) | (1 << 62))) >> 33 != 0
        {
            return None;
        }
        let mut rows = Vec::with_capacity(33);
        for body in &s.bodies {
            if !body.pose.valid()
                || !body
                    .velocity
                    .iter()
                    .all(|v| v.is_finite() && v.abs() <= 250.)
                || !body
                    .angular
                    .iter()
                    .all(|v| v.is_finite() && v.abs() <= 500.)
            {
                return None;
            }
            let mut row = Vec::with_capacity(29);
            row_pose(&mut row, body.pose, s.root.p);
            rates(&mut row, body.velocity);
            rates(&mut row, body.angular);
            rows.push(row);
        }
        Some(Self {
            captured: 0,
            root: root(s.root),
            enabled: s.enabled,
            rows,
        })
    }
    pub fn pose(s: &PoseState) -> Option<Self> {
        if !s.root.valid() || s.bones.len() > 32 {
            return None;
        }
        let mut rows = Vec::with_capacity(s.bones.len());
        let mut seen = vec![];
        for bone in &s.bones {
            if bone.index >= 256 || !bone.pose.valid() || seen.contains(&bone.index) {
                return None;
            }
            seen.push(bone.index);
            let mut row = vec![bone.index as u8];
            row_pose(&mut row, bone.pose, [0.; 3]);
            rows.push(row);
        }
        Some(Self {
            captured: 0,
            root: root(s.root),
            enabled: 0,
            rows,
        })
    }
    pub fn unpack_body(&self) -> Option<BodyState> {
        let root = unroot(self.root)?;
        let mut bodies = vec![];
        if self.rows.len() != 33 || (self.enabled & !((1 << 63) | (1 << 62))) >> 33 != 0 {
            return None;
        }
        for row in &self.rows {
            let mut r = Reader(row);
            let body = Body {
                pose: read_pose(&mut r, root.p)?,
                velocity: read_rates(&mut r, 250.)?,
                angular: read_rates(&mut r, 500.)?,
            };
            if !r.0.is_empty() {
                return None;
            }
            bodies.push(body);
        }
        Some(BodyState {
            root,
            enabled: self.enabled,
            bodies,
        })
    }
    pub fn unpack_pose(&self) -> Option<PoseState> {
        let root = unroot(self.root)?;
        if self.rows.len() > 32 || self.enabled != 0 {
            return None;
        }
        let mut bones = vec![];
        for row in &self.rows {
            let mut r = Reader(row);
            let index = u16::from(r.byte()?);
            if bones.iter().any(|b: &Bone| b.index == index) {
                return None;
            }
            bones.push(Bone {
                index,
                pose: read_pose(&mut r, [0.; 3])?,
            });
            if !r.0.is_empty() {
                return None;
            }
        }
        Some(PoseState { root, bones })
    }
    pub fn position(&self) -> [f32; 3] {
        unroot(self.root).map_or([0.; 3], |p| p.p)
    }
    /// The first `n` BODY rows' positions, the rest left undecoded (perf 10-18: the lobby's interest points).
    pub fn body_points(&self, n: usize) -> Vec<[f32; 3]> {
        let Some(root) = unroot(self.root) else { return vec![] };
        self.rows.iter().take(n).map_while(|row| read_pose(&mut Reader(row), root.p).map(|p| p.p)).collect()
    }
    /// A POSE's [`CAMERA`] row position (world), if the sender sent one: where it watches from.
    pub fn camera(&self) -> Option<[f32; 3]> {
        let row = self.rows.iter().find(|row| row.first() == Some(&(CAMERA as u8)))?;
        read_pose(&mut Reader(&row[1..]), [0.; 3]).map(|p| p.p)
    }
}
/// perf 10-18 (SK8NET15): a narrow row's quantized fields as integers: 3 positions (mm) and the quaternion's 3 10-bit
/// parts, then BODY's 6 rates (binary16 bits); with the fields a delta never changes (POSE's bone index, the
/// quaternion's largest part).
fn fields(row: &[u8], kind: u8) -> Option<([u8; 2], [i32; 12])> {
    let (tag, r) = if kind == POSE { (*row.first()?, &row[1..]) } else { (0, row) };
    if r.len() != if kind == BODY { 23 } else { 11 } || r[0] != 0 {
        return None;
    }
    let at = |i: usize| u16::from_le_bytes([r[i], r[i + 1]]);
    let q = u32::from_le_bytes(r[7..11].try_into().unwrap());
    let mut v = [0; 12];
    for i in 0..3 {
        v[i] = at(1 + 2 * i) as i16 as i32;
        v[3 + i] = ((q >> (2 + 10 * i)) & 1023) as i32;
    }
    if kind == BODY {
        for i in 0..6 {
            v[6 + i] = at(11 + 2 * i) as i32;
        }
    }
    Some(([tag, (q & 3) as u8], v))
}
fn field_count(kind: u8) -> usize {
    if kind == BODY { 12 } else { 6 }
}
fn row_of(kind: u8, tag: [u8; 2], v: &[i32; 12]) -> Option<Vec<u8>> {
    let mut row = Vec::with_capacity(24);
    if kind == POSE {
        row.push(tag[0]);
    }
    row.push(0);
    for &p in &v[..3] {
        row.extend(i16::try_from(p).ok()?.to_le_bytes());
    }
    let mut q = u32::from(tag[1]);
    for (i, &c) in v[3..6].iter().enumerate() {
        q |= u32::try_from(c).ok().filter(|c| *c < 1024)? << (2 + 10 * i);
    }
    row.extend(q.to_le_bytes());
    if kind == BODY {
        for &r in &v[6..] {
            row.extend(u16::try_from(r).ok()?.to_le_bytes());
        }
    }
    Some(row)
}
/// `row` against `base` (both narrow, the same fixed fields): one signed byte per field (within 127 steps); a row with
/// a field past that is "mixed": a bit per field (2 bytes BODY, 1 POSE) marks those sent as a 16-bit wrapping step.
/// (bytes, mixed), only when shorter than the row. Lossless, so baselines never drift.
fn compact(kind: u8, base: &[u8], row: &[u8]) -> Option<(Vec<u8>, bool)> {
    let ((t0, a), (t1, b)) = (fields(base, kind)?, fields(row, kind)?);
    let n = field_count(kind);
    if t0 != t1 {
        return None;
    }
    let (mut wide, mut out) = (0u16, Vec::with_capacity(24));
    for (i, (a, b)) in a[..n].iter().zip(&b).enumerate() {
        match i8::try_from(b - a) {
            Ok(d) => out.push(d as u8),
            Err(_) => {
                wide |= 1 << i;
                out.extend(((b - a) as u16).to_le_bytes());
            }
        }
    }
    if wide != 0 {
        out.splice(0..0, wide.to_le_bytes()[..n.div_ceil(8)].iter().copied());
    }
    (out.len() < row.len()).then_some((out, wide != 0))
}
/// [`compact`] back onto `row` (its base); `None` when the bytes run out or a field leaves its range.
fn expand(kind: u8, row: &[u8], mixed: bool, r: &mut Reader) -> Option<Vec<u8>> {
    let (tag, mut v) = fields(row, kind)?;
    let n = field_count(kind);
    let wide = match (mixed, n > 8) {
        (false, _) => 0,
        (true, true) => u16::from_le_bytes(r.take()?),
        (true, false) => u16::from(r.byte()?),
    };
    if wide >> n != 0 {
        return None;
    }
    for (i, v) in v[..n].iter_mut().enumerate() {
        if wide & (1 << i) == 0 {
            *v += i32::from(r.byte()? as i8);
        } else {
            let x = (*v as u16).wrapping_add(u16::from_le_bytes(r.take()?));
            // Positions are signed millimetres; the rest unsigned bits.
            *v = if i < 3 { i32::from(x as i16) } else { i32::from(x) };
        }
    }
    row_of(kind, tag, &v)
}
/// BODY / POSE: header, baseline seq, the ACK ([`set_ack`]), captured, root, enabled, row count, changed-row mask,
/// with a baseline the masks of rows sent [`compact`] and of those mixed, then the changed rows.
pub fn delta(
    session: u64,
    actor: u64,
    kind: u8,
    seq: u32,
    state: &Packed,
    base: Option<(u32, &Packed)>,
) -> Vec<u8> {
    let base = base.filter(|(_, b)| b.rows.len() == state.rows.len());
    let mut out = header(session, actor, kind, seq);
    out.extend(base.map_or(0, |b| b.0).to_le_bytes());
    out.extend([0; 8]);
    out.extend(state.captured.to_le_bytes());
    out.extend(state.root);
    out.extend(state.enabled.to_le_bytes());
    out.push(state.rows.len() as u8);
    let (mut mask, mut small, mut mixed, mut rows) = (0u64, 0u64, 0u64, Vec::with_capacity(MTU));
    for (i, row) in state.rows.iter().enumerate() {
        let Some((_, b)) = base else {
            mask |= 1 << i;
            rows.extend(row);
            continue;
        };
        if b.rows[i] == *row {
            continue;
        }
        mask |= 1 << i;
        match compact(kind, &b.rows[i], row) {
            Some((d, wide)) => {
                small |= 1 << i;
                mixed |= u64::from(wide) << i;
                rows.extend(d);
            }
            None => rows.extend(row),
        }
    }
    out.extend(mask.to_le_bytes());
    if base.is_some() {
        out.extend(small.to_le_bytes());
        out.extend(mixed.to_le_bytes());
    }
    out.extend(rows);
    debug_assert!(out.len() <= MTU);
    out
}
pub fn baseline(data: &[u8]) -> Option<u32> {
    u32::from_le_bytes(data.get(HEADER..HEADER + 4)?.try_into().ok()?).into()
}
/// perf 10-18 (SK8NET15): the ACK riding a BODY / POSE: what its sender holds of the receiver's own [BODY, POSE]
/// streams (lobby ACK, without its own datagram).
pub fn set_ack(data: &mut [u8], ack: [u32; 2]) {
    if let Some(a) = data.get_mut(HEADER + 4..HEADER + 12) {
        a[..4].copy_from_slice(&ack[0].to_le_bytes());
        a[4..].copy_from_slice(&ack[1].to_le_bytes());
    }
}
pub fn ack(data: &[u8]) -> Option<[u32; 2]> {
    let a = data.get(HEADER + 4..HEADER + 12)?;
    Some([u32::from_le_bytes(a[..4].try_into().ok()?), u32::from_le_bytes(a[4..].try_into().ok()?)])
}
pub fn apply(data: &[u8], base: Option<&Packed>) -> Option<Packed> {
    let (_, _, kind, _) = envelope(data)?;
    if kind != BODY && kind != POSE {
        return None;
    }
    let mut r = Reader(&data[HEADER..]);
    let base_seq = r.u32()?;
    r.take::<8>()?;
    let captured = r.u64()?;
    let root = r.take()?;
    unroot(root)?;
    let enabled = r.u64()?;
    let count = r.byte()? as usize;
    if (kind == BODY && count != 33) || (kind == POSE && count > 32) {
        return None;
    }
    let mask = r.u64()?;
    if mask >> count != 0 {
        return None;
    }
    let (small, mixed) = if base_seq != 0 { (r.u64()?, r.u64()?) } else { (0, 0) };
    if small & !mask != 0 || mixed & !small != 0 {
        return None;
    }
    let mut rows = if base_seq != 0 {
        let b = base?;
        if b.rows.len() != count {
            return None;
        }
        b.rows.clone()
    } else {
        if mask != (1u64 << count) - 1 {
            return None;
        }
        vec![vec![]; count]
    };
    for (i, row) in rows.iter_mut().enumerate() {
        if mask & (1 << i) == 0 {
            continue;
        }
        if small & (1 << i) != 0 {
            *row = expand(kind, row, mixed & (1 << i) != 0, &mut r)?;
            continue;
        }
        let start = r.0;
        if kind == POSE {
            r.byte()?;
        }
        let wide = *r.0.first()?;
        if wide > 1 {
            return None;
        }
        let len = 1 + if wide == 0 { 6 } else { 12 } + 4 + if kind == BODY { 12 } else { 0 };
        r.0 = r.0.get(len..)?;
        *row = start[..start.len() - r.0.len()].to_vec();
    }
    if !r.0.is_empty() {
        return None;
    }
    let result = Packed {
        captured,
        root,
        enabled,
        rows,
    };
    if kind == BODY {
        result.unpack_body()?;
    } else {
        result.unpack_pose()?;
    }
    Some(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn body(t: f32) -> Packed {
        spinning(t, -1.)
    }
    fn spinning(t: f32, z: f32) -> Packed {
        let pose = |p: [f32; 3], a: f32| Pose { p, q: [0., (a * 0.5).sin(), 0., (a * 0.5).cos()] };
        let bodies = (0..33)
            .map(|i| crate::Body {
                pose: pose([10. + t + i as f32 * 0.01, 1., t * 0.5], t + i as f32 * 0.1),
                velocity: [3. + t, 0., z],
                angular: [0., 1. + t, 0.],
            })
            .collect();
        Packed::body(&BodyState { root: pose([10. + t, 1., 0.], t), enabled: 7, bodies }).unwrap()
    }
    /// perf 10-18 (SK8NET15): a skater moving 50 ms on sends each row as byte deltas, losslessly (the receiver
    /// rebuilds the sender's exact rows) in under 70% of the bytes; a row that jumps goes whole; the ACK rides
    /// along.
    #[test]
    fn moving_rows_go_as_lossless_byte_deltas_with_the_ack() {
        let (a, b) = (body(0.), body(0.05));
        let full = delta(1, 2, BODY, 2, &b, None);
        let mut wire = delta(1, 2, BODY, 2, &b, Some((1, &a)));
        assert!(wire.len() * 10 < full.len() * 7, "{} vs {}", wire.len(), full.len());
        set_ack(&mut wire, [5, 9]);
        assert_eq!(ack(&wire), Some([5, 9]));
        assert_eq!(apply(&wire, Some(&a)), Some(b.clone()));
        // A teleport: whole rows, still exact.
        let far = body(40.);
        let jump = delta(1, 2, BODY, 3, &far, Some((2, &b)));
        assert_eq!(apply(&jump, Some(&b)), Some(far));
        // A rate through zero (its binary16 sign flips): that field takes 2 bytes, the row is still compact.
        let (c, d) = (spinning(0., -0.01), spinning(0.05, 0.01));
        let flip = delta(1, 2, BODY, 2, &d, Some((1, &c)));
        assert!(flip.len() * 10 < full.len() * 8, "{} vs {}", flip.len(), full.len());
        assert_eq!(apply(&flip, Some(&c)), Some(d));
        // Pose rows too.
        let pose = |t: f32| {
            let bones = (0..30).map(|i| crate::Bone { index: i, pose: Pose { p: [0., i as f32 * 0.03 + t * 0.1, 0.], q: [0., (t * 0.3).sin(), 0., (t * 0.3).cos()] } }).collect();
            Packed::pose(&PoseState { root: Pose { p: [0.; 3], q: [0., 0., 0., 1.] }, bones }).unwrap()
        };
        let (p, q) = (pose(0.), pose(0.05));
        let wire = delta(1, 2, POSE, 2, &q, Some((1, &p)));
        assert!(wire.len() * 10 < delta(1, 2, POSE, 2, &q, None).len() * 7);
        assert_eq!(apply(&wire, Some(&p)), Some(q));
        // Corrupt compact bytes never panic and never decode out of range.
        for n in HEADER..wire.len() {
            let mut bad = wire.clone();
            bad[n] ^= 0x80;
            let _ = apply(&bad, Some(&p));
        }
    }
}
