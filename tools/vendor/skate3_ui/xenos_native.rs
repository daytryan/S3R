// xenos.py's decode_rgba (linearize_base_level + _decode_dxt / the A8R8G8B8 swizzle) in one call, byte for byte.
// Not rx2_native.rs: that decoder expands 5:6:5 as v*255/31 and rounds its 2/3 blends; this one keeps the vendored
// bit replication and floor division. Any input Python rejects returns 1 (the caller reruns Python for its message).

fn tiled_offset(x: usize, y: usize, width: usize, log_bpb: u32) -> usize {
    let aligned_width = (width + 31) & !31;
    let macro_ = ((x >> 5) + (y >> 5) * (aligned_width >> 5)) << (log_bpb + 7);
    let micro = ((x & 7) + ((y & 0xE) << 2)) << log_bpb;
    let offset = macro_ + ((micro & !0xF) << 1) + (micro & 0xF) + ((y & 1) << 4);
    let address = ((offset & !0x1FF) << 3)
        + ((y & 16) << 7)
        + ((offset & 0x1C0) << 2)
        + (((((y & 8) >> 2) + (x >> 3)) & 3) << 6)
        + (offset & 0x3F);
    address >> log_bpb
}

fn linearize(data: &[u8], width: usize, height: usize, bs: usize, bpb: usize, tiled: bool, pitch: usize) -> Option<Vec<u8>> {
    let bw = ((width + bs - 1) / bs).max(1);
    let bh = ((height + bs - 1) / bs).max(1);
    let mut linear = vec![0u8; bw * bh * bpb];
    let compressed = bs == 4;
    let copy = |src: &[u8], dst: &mut [u8]| {
        if compressed {
            for i in (0..bpb).step_by(2) {
                dst[i] = src[i + 1];
                dst[i + 1] = src[i];
            }
        } else {
            for i in 0..bpb {
                dst[i] = src[bpb - 1 - i];
            }
        }
    };
    if !tiled {
        let texels = if pitch != 0 { pitch } else { width };
        let row_pitch = bw.max((texels + bs - 1) / bs) * bpb;
        if data.len() < (bh - 1) * row_pitch + bw * bpb {
            return None;
        }
        for y in 0..bh {
            for x in 0..bw {
                let s = y * row_pitch + x * bpb;
                let t = (y * bw + x) * bpb;
                copy(&data[s..s + bpb], &mut linear[t..t + bpb]);
            }
        }
        return Some(linear);
    }
    let unit = if compressed { 128 } else { 32 };
    let tiled_width = ((width + unit - 1) & !(unit - 1)) / bs;
    let log_bpb = bpb.trailing_zeros();
    for y in 0..bh {
        for x in 0..bw {
            let s = tiled_offset(x, y, tiled_width, log_bpb) * bpb;
            if s + bpb > data.len() {
                return None;
            }
            let t = (y * bw + x) * bpb;
            copy(&data[s..s + bpb], &mut linear[t..t + bpb]);
        }
    }
    Some(linear)
}

fn expand_565(v: u32) -> [u32; 3] {
    let (r, g, b) = ((v >> 11) & 0x1F, (v >> 5) & 0x3F, v & 0x1F);
    [(r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2)]
}

fn decode(data: &[u8], width: usize, height: usize, format: u32, tiled: bool, pitch: usize, out: &mut [u8]) -> Option<()> {
    let (bs, bpb) = match format {
        18 => (4, 8),
        19 | 20 => (4, 16),
        6 => (1, 4),
        _ => return None,
    };
    let linear = linearize(data, width, height, bs, bpb, tiled, pitch)?;
    if format == 6 {
        for (o, l) in out.chunks_exact_mut(4).zip(linear.chunks_exact(4)) {
            o.copy_from_slice(&[l[2], l[1], l[0], l[3]]);
        }
        return Some(());
    }
    let (bw, bh) = (((width + 3) / 4).max(1), ((height + 3) / 4).max(1));
    for by in 0..bh {
        for bx in 0..bw {
            let block = &linear[(by * bw + bx) * bpb..][..bpb];
            let mut alphas: Option<[u8; 16]> = None;
            let mut color = 0;
            if format == 19 {
                let mut a = [0u8; 16];
                for (i, &v) in block[..8].iter().enumerate() {
                    a[2 * i] = (v & 0xF) * 17;
                    a[2 * i + 1] = (v >> 4) * 17;
                }
                alphas = Some(a);
                color = 8;
            } else if format == 20 {
                let (a0, a1) = (block[0] as u32, block[1] as u32);
                let mut palette = [a0, a1, 0, 0, 0, 0, 0, 255];
                if a0 > a1 {
                    for i in 1..7 {
                        palette[i as usize + 1] = ((7 - i) * a0 + i * a1) / 7;
                    }
                } else {
                    for i in 1..5 {
                        palette[i as usize + 1] = ((5 - i) * a0 + i * a1) / 5;
                    }
                }
                let bits = (0..6).fold(0u64, |acc, i| acc | (block[2 + i] as u64) << (8 * i));
                let mut a = [0u8; 16];
                for (p, value) in a.iter_mut().enumerate() {
                    *value = palette[((bits >> (3 * p)) & 7) as usize] as u8;
                }
                alphas = Some(a);
                color = 8;
            }
            let c0 = u16::from_le_bytes([block[color], block[color + 1]]) as u32;
            let c1 = u16::from_le_bytes([block[color + 2], block[color + 3]]) as u32;
            let bits = u32::from_le_bytes([block[color + 4], block[color + 5], block[color + 6], block[color + 7]]);
            let (e0, e1) = (expand_565(c0), expand_565(c1));
            let mut colors = [[e0[0], e0[1], e0[2], 255], [e1[0], e1[1], e1[2], 255], [0, 0, 0, 255], [0, 0, 0, 255]];
            if format != 18 || c0 > c1 {
                for i in 0..3 {
                    colors[2][i] = (2 * e0[i] + e1[i]) / 3;
                    colors[3][i] = (e0[i] + 2 * e1[i]) / 3;
                }
            } else {
                for i in 0..3 {
                    colors[2][i] = (e0[i] + e1[i]) / 2;
                }
                colors[3] = [0, 0, 0, 0];
            }
            for py in 0..4 {
                for px in 0..4 {
                    let (x, y) = (bx * 4 + px, by * 4 + py);
                    if x >= width || y >= height {
                        continue;
                    }
                    let pixel = py * 4 + px;
                    let c = colors[((bits >> (pixel * 2)) & 3) as usize];
                    let alpha = match alphas {
                        Some(a) => a[pixel],
                        None => c[3] as u8,
                    };
                    let o = (y * width + x) * 4;
                    out[o..o + 4].copy_from_slice(&[c[0] as u8, c[1] as u8, c[2] as u8, alpha]);
                }
            }
        }
    }
    Some(())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn skate_xenos_decode(src: *const u8, src_len: usize, width: usize, height: usize, format: u32,
                                            tiled: bool, pitch: usize, out: *mut u8, out_len: usize) -> i32 {
    if src.is_null() || out.is_null() || width == 0 || height == 0 || out_len != width * height * 4 {
        return 1;
    }
    let data = unsafe { std::slice::from_raw_parts(src, src_len) };
    let output = unsafe { std::slice::from_raw_parts_mut(out, out_len) };
    match decode(data, width, height, format, tiled, pitch, output) {
        Some(()) => 0,
        None => 1,
    }
}
