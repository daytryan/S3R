// retail_collision_mesh.decode_clustered_mesh for the spawn search and the manifest counts: every triangle's three
// corners as float64 (the f32 values Python holds) plus the mesh's counts, without a Python object per triangle.
// Anything Python would reject returns 1: the caller then runs the Python decoder, which raises its own error.

fn u16_at(d: &[u8], o: usize) -> Option<u64> {
    Some(u16::from_be_bytes(d.get(o..o + 2)?.try_into().ok()?) as u64)
}
fn u32_at(d: &[u8], o: usize) -> Option<u64> {
    Some(u32::from_be_bytes(d.get(o..o + 4)?.try_into().ok()?) as u64)
}
fn i32_at(d: &[u8], o: usize) -> Option<i64> {
    Some(i32::from_be_bytes(d.get(o..o + 4)?.try_into().ok()?) as i64)
}
fn f32_at(d: &[u8], o: usize) -> Option<f32> {
    Some(f32::from_be_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

/// Python's _f32(x): struct.pack('<f') rounds to f32 and refuses a finite value that overflows.
fn to_f32(x: f64) -> Option<f64> {
    let y = x as f32;
    (y.is_finite() || !x.is_finite()).then_some(y as f64)
}

const HEADER: usize = 16;

fn vertices(c: &[u8], count: usize, compression: u8, g: f32, end: usize, unsigned16: bool) -> Option<Vec<[f64; 3]>> {
    let mut out = Vec::with_capacity(count);
    match compression {
        1 if unsigned16 => {
            // convert_ps3.decode_vertices: unsigned deltas, clipped to int32, float32 times float32(granularity)
            if HEADER + 12 + count * 6 > end {
                return None;
            }
            let base = [i32_at(c, HEADER)?, i32_at(c, HEADER + 4)?, i32_at(c, HEADER + 8)?];
            for i in 0..count {
                let o = HEADER + 12 + i * 6;
                let v: Option<Vec<f64>> = (0..3)
                    .map(|k| {
                        let n = (base[k] + u16_at(c, o + 2 * k)? as i64).clamp(-(1 << 31), (1 << 31) - 1);
                        let y = n as f32 * g;
                        y.is_finite().then_some(y as f64) // numpy would carry an inf on silently: let Python do it
                    })
                    .collect();
                let v = v?;
                out.push([v[0], v[1], v[2]]);
            }
            return Some(out);
        }
        1 => {
            if end < 28 {
                return None;
            }
            let base = [i32_at(c, HEADER)?, i32_at(c, HEADER + 4)?, i32_at(c, HEADER + 8)?];
            for i in 0..count {
                let o = HEADER + 12 + i * 6;
                if o + 6 > end {
                    return None;
                }
                let mut v = [0.0; 3];
                for k in 0..3 {
                    let delta = u16_at(c, o + 2 * k)? as u16 as i16 as i64;
                    v[k] = to_f32((base[k] + delta) as f64 * g as f64)?;
                }
                out.push(v);
            }
        }
        2 => {
            for i in 0..count {
                let o = HEADER + i * 12;
                if o + 12 > end {
                    return None;
                }
                let mut v = [0.0; 3];
                for k in 0..3 {
                    v[k] = to_f32(i32_at(c, o + 4 * k)? as f64 * g as f64)?;
                }
                out.push(v);
            }
        }
        0 => {
            for i in 0..count {
                let o = HEADER + i * 16;
                if o + 16 > end {
                    return None;
                }
                out.push([f32_at(c, o)? as f64, f32_at(c, o + 4)? as f64, f32_at(c, o + 8)? as f64]);
            }
        }
        _ => return None,
    }
    out.iter().flatten().all(|v| v.is_finite()).then_some(out)
}

struct Counts {
    vertices: u64,
    units: u64,
}

fn cluster(c: &[u8], g: f32, group_width: u8, surface_width: u8, unsigned16: bool, points: &mut Vec<[[f64; 3]; 3]>,
           counts: &mut Counts) -> Option<u8> {
    if c.len() < HEADER {
        return None;
    }
    let units = u16_at(c, 0)? as usize;
    let unit_bytes = u16_at(c, 2)? as usize;
    let blocks = u16_at(c, 4)? as usize;
    if u16_at(c, 8)? as usize != c.len() {
        return None;
    }
    let (count, compression) = (c[10] as usize, c[12]);
    let (start, end) = ((blocks + 1) * 16, (blocks + 1) * 16 + unit_bytes);
    if !(HEADER <= start && end <= c.len()) {
        return None;
    }
    let v = vertices(c, count, compression, g, start, unsigned16)?;
    let mut cursor = start;
    for _ in 0..units {
        if cursor >= end {
            return None;
        }
        let flags = c[cursor];
        cursor += 1;
        let triangles = match flags & 15 {
            1 => 1,
            2 => 2,
            _ => return None, // 3: triangle lists are refused by _triangulate_unit; others are unsupported
        };
        let indices = c.get(cursor..cursor + triangles + 2)?;
        cursor += triangles + 2;
        if cursor > end || indices.iter().any(|&i| i as usize >= count) {
            return None;
        }
        if flags & 0x20 != 0 {
            cursor += triangles + 2;
            if cursor > end {
                return None;
            }
        }
        for (flag, width) in [(0x40, group_width), (0x80, surface_width)] {
            if flags & flag != 0 {
                if !(1..=2).contains(&width) || cursor + width as usize > c.len() {
                    return None;
                }
                cursor += width as usize;
            }
        }
        let corner = |i: u8| v[i as usize];
        points.push([corner(indices[0]), corner(indices[1]), corner(indices[2])]);
        if triangles == 2 {
            points.push([corner(indices[3]), corner(indices[2]), corner(indices[1])]);
        }
    }
    if cursor != end {
        return None;
    }
    counts.vertices += count as u64;
    counts.units += units as u64;
    Some(compression)
}

fn mesh(d: &[u8], unsigned16: bool, points: &mut Vec<[[f64; 3]; 3]>, info: &mut [u64; 7]) -> Option<()> {
    if d.len() < 96 {
        return None;
    }
    let bounds: Vec<f32> = (0..3).map(|k| f32_at(d, 4 * k)).chain((0..3).map(|k| f32_at(d, 16 + 4 * k))).collect::<Option<_>>()?;
    if !bounds.iter().all(|b| b.is_finite()) || (0..3).any(|k| bounds[k] > bounds[k + 3]) {
        return None;
    }
    let expected = u32_at(d, 40)?;
    let (kd, table) = (u32_at(d, 48)?, u32_at(d, 52)?);
    let g = f32_at(d, 56)?;
    let (group_width, surface_width) = (d[62], d[63]);
    let (clusters, mesh_bytes) = (u32_at(d, 64)?, u32_at(d, 80)?);
    if !(g.is_finite() && g > 0.0) || !(96..=d.len() as u64).contains(&mesh_bytes) || !(kd >= 96 && kd + 48 <= table)
        || !(table >= 96 && table + clusters * 4 <= mesh_bytes) {
        return None;
    }
    let offsets: Vec<u64> = (0..clusters).map(|i| u32_at(d, (table + i * 4) as usize)).collect::<Option<_>>()?;
    if offsets.windows(2).any(|w| w[0] >= w[1]) {
        return None;
    }
    let mut counts = Counts { vertices: 0, units: 0 };
    let mut by_compression = [0u64; 3];
    for (i, &offset) in offsets.iter().enumerate() {
        let next = offsets.get(i + 1).copied().unwrap_or(mesh_bytes);
        if !(offset >= table + clusters * 4 && offset + HEADER as u64 <= next) {
            return None;
        }
        let size = u16_at(d, offset as usize + 8)?;
        if offset + size > next {
            return None;
        }
        let c = &d[offset as usize..(offset + size) as usize];
        let compression = cluster(c, g, group_width, surface_width, unsigned16, points, &mut counts)?;
        by_compression[compression as usize] += 1;
    }
    if points.len() as u64 != expected {
        return None;
    }
    *info = [expected, clusters, counts.vertices, counts.units, by_compression[0], by_compression[1], by_compression[2]];
    Some(())
}

/// One serialized ClusteredMesh -> `points` (capacity triangles x 3 corners x xyz float64) and `info` = [triangles,
/// clusters, vertices, units, clusters per compression 0/1/2]. 0 = decoded, 1 = let Python decode it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skate_collision_mesh(data: *const u8, len: usize, unsigned16: bool, points: *mut f64,
                                              capacity: usize, info: *mut u64) -> i32 {
    if data.is_null() || points.is_null() || info.is_null() {
        return 1;
    }
    let d = unsafe { std::slice::from_raw_parts(data, len) };
    let mut found = Vec::with_capacity(capacity);
    let mut counts = [0u64; 7];
    if mesh(d, unsigned16, &mut found, &mut counts).is_none() || found.len() > capacity {
        return 1;
    }
    let out = unsafe { std::slice::from_raw_parts_mut(points, found.len() * 9) };
    for (t, tri) in found.iter().enumerate() {
        for (k, corner) in tri.iter().enumerate() {
            out[t * 9 + k * 3..t * 9 + k * 3 + 3].copy_from_slice(corner);
        }
    }
    unsafe { std::slice::from_raw_parts_mut(info, 7) }.copy_from_slice(&counts);
    0
}
