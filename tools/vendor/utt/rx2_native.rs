// rx2_fast.py's _decode in native code: Xbox 360 untiling and DXT1/DXT3/DXT5/ATI2 block decoding, byte for byte the
// numpy result (same integer rounding, swapped selector bytes, edge cropping and zeroed missing blocks).

fn untile(src: &[u8], width: i64, pitch: i64) -> Vec<u8> {
    let units = src.len() as i64 / pitch;
    let log = (pitch >> 2) + ((pitch >> 1) >> (pitch >> 2));
    let aligned = (width + 31) & !31;
    let mut source = vec![-1i64; units as usize];
    for offset in 0..units / width * width {
        let b = offset << log;
        let t = ((b & !4095) >> 3) + ((b & 1792) >> 2) + (b & 63);
        let m = t >> (7 + log);
        let mut x = (((m % (aligned >> 5)) << 2) + ((((t >> (5 + log)) & 2) + (b >> 6)) & 3)) << 3;
        x += ((((t >> 1) & !15) + (t & 15)) & ((pitch << 3) - 1)) >> log;
        let mut y = (((m / (aligned >> 5)) << 2) + ((t >> (6 + log)) & 1) + ((b & 2048) >> 10)) << 3;
        y += (((t & (((pitch << 6) - 1) & !31)) + ((t & 15) << 1)) >> (3 + log)) & !1;
        y += (t & 16) >> 4;
        let dest = y * width + x;
        if x < width && dest < units {
            source[dest as usize] = offset; // ascending offsets: the last write is np.maximum.at's maximum
        }
    }
    let p = pitch as usize;
    let mut out = vec![0u8; src.len()];
    for (dest, &from) in source.iter().enumerate() {
        if from >= 0 {
            let from = from as usize * p;
            out[dest * p..dest * p + p].copy_from_slice(&src[from..from + p]);
        }
    }
    out
}

fn rgb(v: u32) -> [u32; 3] {
    [((v >> 11) & 31) * 255 / 31, ((v >> 5) & 63) * 255 / 63, (v & 31) * 255 / 31]
}

fn colour(b: &[u8], transparent: bool, px: &mut [[u8; 4]; 16]) {
    let c0 = (b[0] as u32) << 8 | b[1] as u32;
    let c1 = (b[2] as u32) << 8 | b[3] as u32;
    let (a, z) = (rgb(c0), rgb(c1));
    let four = c0 > c1 || !transparent;
    let mut pal = [[0u8, 0, 0, 255]; 4];
    for k in 0..3 {
        pal[0][k] = a[k] as u8;
        pal[1][k] = z[k] as u8;
        pal[2][k] = (if four { (2 * a[k] + z[k] + 1) / 3 } else { (a[k] + z[k]) / 2 }) as u8;
        pal[3][k] = (if four { (a[k] + 2 * z[k] + 1) / 3 } else { 0 }) as u8;
    }
    if transparent && !four {
        pal[3][3] = 0;
    }
    for (p, pixel) in px.iter_mut().enumerate() {
        *pixel = pal[((b[[5, 4, 7, 6][p / 4]] >> (2 * (p % 4))) & 3) as usize];
    }
}

fn alpha(b: &[u8]) -> [u8; 16] {
    let (a, z) = (b[1] as u32, b[0] as u32);
    let mut table = [a as u8, z as u8, 0, 0, 0, 0, 0, 0];
    for i in 0..6u32 {
        let low = if i < 4 { ((4 - i) * a + (i + 1) * z) / 5 } else if i == 4 { 0 } else { 255 };
        table[i as usize + 2] = (if a > z { ((6 - i) * a + (i + 1) * z) / 7 } else { low }) as u8;
    }
    let bits = [3usize, 2, 5, 4, 7, 6].iter().enumerate().fold(0u64, |bits, (k, &j)| bits | (b[j] as u64) << (8 * k));
    std::array::from_fn(|j| table[((bits >> (3 * j)) & 7) as usize])
}

fn decode(data: &[u8], w: usize, h: usize, kind: u32, out: &mut [u8]) {
    let size = if kind == 1 { 8 } else { 16 };
    let bw = (w + 3) / 4;
    let available = (bw * ((h + 3) / 4)).min(data.len() / size);
    for block in 0..bw * ((h + 3) / 4) {
        let mut px = [[0u8; 4]; 16];
        if block < available {
            let b = &data[block * size..block * size + size];
            match kind {
                1 => colour(b, true, &mut px),
                2 => {
                    let (r, g) = (alpha(&b[..8]), alpha(&b[8..]));
                    for p in 0..16 {
                        px[p] = [r[p], g[p], 0, 255];
                    }
                }
                _ => {
                    colour(&b[8..], false, &mut px);
                    if kind == 3 {
                        for p in 0..16 {
                            px[p][3] = ((b[(p / 2) ^ 1] >> (4 * (p & 1))) & 15) * 17;
                        }
                    } else {
                        let a = alpha(&b[..8]);
                        for p in 0..16 {
                            px[p][3] = a[p];
                        }
                    }
                }
            }
        }
        let (by, bx) = (block / bw * 4, block % bw * 4);
        for (p, pixel) in px.iter().enumerate() {
            let (y, x) = (by + p / 4, bx + p % 4);
            if y < h && x < w {
                out[(y * w + x) * 4..][..4].copy_from_slice(pixel);
            }
        }
    }
}

/// rx2_fast._decode(data, w, h, kind, tiled) into `out` (w*h*4 bytes). 1 = unsupported arguments (keep the Python).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skate_rx2_decode(data: *const u8, len: usize, w: usize, h: usize, kind: u32, tiled: bool,
                                          out: *mut u8) -> i32 {
    if data.is_null() || out.is_null() || ![1, 2, 3, 5].contains(&kind) || w == 0 || h == 0 || w > 1 << 16 || h > 1 << 16 {
        return 1;
    }
    let src = unsafe { std::slice::from_raw_parts(data, len) };
    let output = unsafe { std::slice::from_raw_parts_mut(out, w * h * 4) };
    let size = if kind == 1 { 8 } else { 16 };
    if tiled {
        decode(&untile(src, ((w + 3) / 4) as i64, size), w, h, kind, output);
    } else {
        decode(src, w, h, kind, output);
    }
    0
}
