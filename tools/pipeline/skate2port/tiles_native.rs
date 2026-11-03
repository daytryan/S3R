// The per-triangle loops of cas_tiles.rasterise and s1render.rasterise (Edit Skater tiles) and their bilinear texture
// lookup (sample), bit for bit: the same f64
// operations in the same order as the numpy code, and numpy's matmul (OpenBLAS) for the barycentric blends: one row is
// a plain sum (gemv), more rows an FMA chain over k (the dgemm kernel), measured on 3.6 million products.

fn blend(rows: usize, w: [f64; 3], a: &[f64], b: &[f64], c: &[f64], out: &mut [f64]) {
    for j in 0..out.len() {
        out[j] = if rows == 1 {
            w[0] * a[j] + w[1] * b[j] + w[2] * c[j]
        } else {
            w[2].mul_add(c[j], w[1].mul_add(b[j], w[0] * a[j]))
        };
    }
}

// numpy's float remainder by 1 (python semantics): fmod, then shifted into [0, 1).
fn rem1(x: f64) -> f64 {
    let m = x % 1.0;
    if m != 0.0 { if m < 0.0 { m + 1.0 } else { m } } else { 0.0 }
}

// cas_tiles.sample's bilinear wrap lookup in an HxWx4 f32 texture: all four channels at (u, v).
fn sample4(tex: &[f32], h: i64, w: i64, u: f64, v: f64) -> [f64; 4] {
    let x = rem1(u) * w as f64 - 0.5;
    let y = rem1(v) * h as f64 - 0.5;
    let (x0, y0) = (x.floor() as i64, y.floor() as i64);
    let (fx, fy) = (x - x0 as f64, y - y0 as f64);
    let at = |yy: i64, xx: i64| ((yy.rem_euclid(h) * w + xx.rem_euclid(w)) * 4) as usize;
    let (a, b, c, d) = (at(y0, x0), at(y0, x0 + 1), at(y0 + 1, x0), at(y0 + 1, x0 + 1));
    let mut out = [0.0; 4];
    for ch in 0..4 {
        let g = |i: usize| tex[i + ch] as f64;
        out[ch] = g(a) * (1.0 - fx) * (1.0 - fy) + g(b) * fx * (1.0 - fy) + g(c) * (1.0 - fx) * fy + g(d) * fx * fy;
    }
    out
}

fn sample(tex: &[f32], h: i64, w: i64, ch: usize, u: f64, v: f64) -> f64 {
    sample4(tex, h, w, u, v)[ch]
}

struct Hit { at: usize, z: f64, bw: [f64; 3] }

/// cas_tiles.rasterise for one mesh: tris are the kept triangles (int64 x3); cover_tex (HxWx4 f32) is the alpha test
/// (channel `cover_ch` sampled at uv, or at uv1 when cover_uv1) or null. Updates depth/mid/uv/uv1/nrm (n x n) in place.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tiles_perspective(
    n: i64, sx: *const f64, sy: *const f64, z: *const f64, vn: *const f64, uv: *const f64, uv1: *const f64, nverts: i64,
    tris: *const i64, ntris: i64, cull: i32, bias: f64,
    cover_tex: *const f32, cover_h: i64, cover_w: i64, cover_ch: i64, cover_uv1: i32, m_i: i32,
    depth: *mut f64, mid: *mut i32, guv: *mut f64, guv1: *mut f64, gnrm: *mut f64,
) {
    let nv = nverts as usize;
    let (sx, sy, z) = unsafe { (std::slice::from_raw_parts(sx, nv), std::slice::from_raw_parts(sy, nv), std::slice::from_raw_parts(z, nv)) };
    let (vn, uv, uv1) = unsafe { (std::slice::from_raw_parts(vn, nv * 3), std::slice::from_raw_parts(uv, nv * 2), std::slice::from_raw_parts(uv1, nv * 2)) };
    let tris = unsafe { std::slice::from_raw_parts(tris, ntris as usize * 3) };
    let cells = (n * n) as usize;
    let (depth, mid) = unsafe { (std::slice::from_raw_parts_mut(depth, cells), std::slice::from_raw_parts_mut(mid, cells)) };
    let (guv, guv1, gnrm) = unsafe {
        (std::slice::from_raw_parts_mut(guv, cells * 2), std::slice::from_raw_parts_mut(guv1, cells * 2), std::slice::from_raw_parts_mut(gnrm, cells * 3))
    };
    let cover = if cover_tex.is_null() { None } else {
        Some(unsafe { std::slice::from_raw_parts(cover_tex, (cover_h * cover_w * 4) as usize) })
    };
    let mut hits: Vec<Hit> = Vec::new();
    let mut tuv: Vec<[f64; 4]> = Vec::new();
    for t in 0..ntris as usize {
        let tri = [tris[3 * t] as usize, tris[3 * t + 1] as usize, tris[3 * t + 2] as usize];
        let xs = [sx[tri[0]], sx[tri[1]], sx[tri[2]]];
        let ys = [sy[tri[0]], sy[tri[1]], sy[tri[2]]];
        let x0 = 0.max(xs[0].min(xs[1]).min(xs[2]) as i64);
        let x1 = (n - 1).min(xs[0].max(xs[1]).max(xs[2]).ceil() as i64);
        let y0 = 0.max(ys[0].min(ys[1]).min(ys[2]) as i64);
        let y1 = (n - 1).min(ys[0].max(ys[1]).max(ys[2]).ceil() as i64);
        if x0 > x1 || y0 > y1 { continue; }
        let d = (ys[1] - ys[2]) * (xs[0] - xs[2]) + (xs[2] - xs[1]) * (ys[0] - ys[2]);
        if d.abs() < 1e-9 || (cull != 0 && d > 0.0 && cover.is_none()) { continue; }
        let (a0, b0, a1, b1) = (ys[1] - ys[2], xs[2] - xs[1], ys[2] - ys[0], xs[0] - xs[2]);
        let iz = [1.0 / z[tri[0]], 1.0 / z[tri[1]], 1.0 / z[tri[2]]];
        hits.clear();
        for py in y0..=y1 {
            let yy = py as f64 + 0.5;
            for px in x0..=x1 {
                let xx = px as f64 + 0.5;
                let w0 = (a0 * (xx - xs[2]) + b0 * (yy - ys[2])) / d;
                let w1 = (a1 * (xx - xs[2]) + b1 * (yy - ys[2])) / d;
                let w2 = 1.0 - w0 - w1;
                if !(w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0) { continue; }
                let (p0, p1, p2) = (w0 * iz[0], w1 * iz[1], w2 * iz[2]);
                let s = p0 + p1 + p2;
                let zz = 1.0 / s - bias;
                let at = (py * n + px) as usize;
                if !(zz < depth[at]) { continue; }
                hits.push(Hit { at, z: zz, bw: [p0 / s, p1 / s, p2 / s] });
            }
        }
        if hits.is_empty() { continue; }
        let rows = hits.len();
        tuv.clear();
        for h in &hits {
            let mut o = [0.0; 4];
            blend(rows, h.bw, &uv[2 * tri[0]..2 * tri[0] + 2], &uv[2 * tri[1]..2 * tri[1] + 2], &uv[2 * tri[2]..2 * tri[2] + 2], &mut o[..2]);
            blend(rows, h.bw, &uv1[2 * tri[0]..2 * tri[0] + 2], &uv1[2 * tri[1]..2 * tri[1] + 2], &uv1[2 * tri[2]..2 * tri[2] + 2], &mut o[2..]);
            tuv.push(o);
        }
        let keep: Vec<bool> = match cover {
            None => vec![true; rows],
            Some(tex) => tuv.iter().map(|o| {
                let (u, v) = if cover_uv1 != 0 { (o[2], o[3]) } else { (o[0], o[1]) };
                sample(tex, cover_h, cover_w, cover_ch as usize, u, v) >= 0.5
            }).collect(),
        };
        let kept = keep.iter().filter(|k| **k).count();
        if kept == 0 { continue; }
        for (i, h) in hits.iter().enumerate() {
            if !keep[i] { continue; }
            depth[h.at] = h.z;
            mid[h.at] = m_i;
            guv[2 * h.at..2 * h.at + 2].copy_from_slice(&tuv[i][..2]);
            guv1[2 * h.at..2 * h.at + 2].copy_from_slice(&tuv[i][2..]);
            blend(kept, h.bw, &vn[3 * tri[0]..3 * tri[0] + 3], &vn[3 * tri[1]..3 * tri[1] + 3], &vn[3 * tri[2]..3 * tri[2] + 3],
                  &mut gnrm[3 * h.at..3 * h.at + 3]);
        }
    }
}

/// s1render.rasterise for one mesh (orthographic, nearest z wins, both faces): v is the view-space position (x3).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tiles_orthographic(
    n: i64, sx: *const f64, sy: *const f64, v: *const f64, vn: *const f64, uv: *const f64, nverts: i64,
    tris: *const i64, ntris: i64, m_i: i16, depth: *mut f64, mid: *mut i16, guv: *mut f64, gnrm: *mut f64,
) {
    let nv = nverts as usize;
    let (sx, sy) = unsafe { (std::slice::from_raw_parts(sx, nv), std::slice::from_raw_parts(sy, nv)) };
    let (v, vn, uv) = unsafe { (std::slice::from_raw_parts(v, nv * 3), std::slice::from_raw_parts(vn, nv * 3), std::slice::from_raw_parts(uv, nv * 2)) };
    let tris = unsafe { std::slice::from_raw_parts(tris, ntris as usize * 3) };
    let cells = (n * n) as usize;
    let (depth, mid) = unsafe { (std::slice::from_raw_parts_mut(depth, cells), std::slice::from_raw_parts_mut(mid, cells)) };
    let (guv, gnrm) = unsafe { (std::slice::from_raw_parts_mut(guv, cells * 2), std::slice::from_raw_parts_mut(gnrm, cells * 3)) };
    let mut hits: Vec<Hit> = Vec::new();
    for t in 0..ntris as usize {
        let tri = [tris[3 * t] as usize, tris[3 * t + 1] as usize, tris[3 * t + 2] as usize];
        let xs = [sx[tri[0]], sx[tri[1]], sx[tri[2]]];
        let ys = [sy[tri[0]], sy[tri[1]], sy[tri[2]]];
        let x0 = 0.max(xs[0].min(xs[1]).min(xs[2]) as i64);
        let x1 = (n - 1).min(xs[0].max(xs[1]).max(xs[2]).ceil() as i64);
        let y0 = 0.max(ys[0].min(ys[1]).min(ys[2]) as i64);
        let y1 = (n - 1).min(ys[0].max(ys[1]).max(ys[2]).ceil() as i64);
        if x0 > x1 || y0 > y1 { continue; }
        let d = (ys[1] - ys[2]) * (xs[0] - xs[2]) + (xs[2] - xs[1]) * (ys[0] - ys[2]);
        if d.abs() < 1e-9 { continue; }
        let (a0, b0, a1, b1) = (ys[1] - ys[2], xs[2] - xs[1], ys[2] - ys[0], xs[0] - xs[2]);
        let vz = [v[3 * tri[0] + 2], v[3 * tri[1] + 2], v[3 * tri[2] + 2]];
        hits.clear();
        for py in y0..=y1 {
            let yy = py as f64 + 0.5;
            for px in x0..=x1 {
                let xx = px as f64 + 0.5;
                let w0 = (a0 * (xx - xs[2]) + b0 * (yy - ys[2])) / d;
                let w1 = (a1 * (xx - xs[2]) + b1 * (yy - ys[2])) / d;
                let w2 = 1.0 - w0 - w1;
                if !(w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0) { continue; }
                let zz = w0 * vz[0] + w1 * vz[1] + w2 * vz[2];
                let at = (py * n + px) as usize;
                if !(zz > depth[at]) { continue; }
                hits.push(Hit { at, z: zz, bw: [w0, w1, w2] });
            }
        }
        let rows = hits.len();
        for h in &hits {
            depth[h.at] = h.z;
            mid[h.at] = m_i;
            blend(rows, h.bw, &uv[2 * tri[0]..2 * tri[0] + 2], &uv[2 * tri[1]..2 * tri[1] + 2], &uv[2 * tri[2]..2 * tri[2] + 2],
                  &mut guv[2 * h.at..2 * h.at + 2]);
            blend(rows, h.bw, &vn[3 * tri[0]..3 * tri[0] + 3], &vn[3 * tri[1]..3 * tri[1] + 3], &vn[3 * tri[2]..3 * tri[2] + 3],
                  &mut gnrm[3 * h.at..3 * h.at + 3]);
        }
    }
}

/// cas_tiles.sample / s1render.sample: out (n x 4 f64) = the HxWx4 f32 texture at each uv (n x 2 f64).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tiles_sample(tex: *const f32, h: i64, w: i64, uv: *const f64, n: i64, out: *mut f64) {
    let (tex, uv, out) = unsafe {
        (std::slice::from_raw_parts(tex, (h * w * 4) as usize), std::slice::from_raw_parts(uv, n as usize * 2),
         std::slice::from_raw_parts_mut(out, n as usize * 4))
    };
    for i in 0..n as usize {
        out[4 * i..4 * i + 4].copy_from_slice(&sample4(tex, h, w, uv[2 * i], uv[2 * i + 1]));
    }
}

/// old_skate_clothing's duplicate check: whether any of the n RGB u8 images `theirs` (each `pixels` x 3, like `mine`) has
/// under 1% of pixels with a channel more than 40 apart from `mine` (numpy: (|a-b|.max(axis=2) > 40).mean() < .01).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tiles_twin(mine: *const u8, theirs: *const u8, n: i64, pixels: i64) -> i32 {
    let px = pixels as usize;
    let (mine, theirs) = unsafe { (std::slice::from_raw_parts(mine, px * 3), std::slice::from_raw_parts(theirs, n as usize * px * 3)) };
    for t in theirs.chunks_exact(px * 3) {
        let mut far = 0usize;
        for (a, b) in mine.chunks_exact(3).zip(t.chunks_exact(3)) {
            if a.iter().zip(b).any(|(x, y)| x.abs_diff(*y) > 40) {
                far += 1;
                if far as f64 / pixels as f64 >= 0.01 { break; }  // the share only grows
            }
        }
        if (far as f64 / pixels as f64) < 0.01 { return 1; }
    }
    0
}
