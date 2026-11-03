// convert_ps3.downscale_lightmaps' box filter: numpy's float64 mean of each factor x factor block, rounded half to even.

/// RGBA8 `w`x`h` -> (w/factor)x(h/factor) into `out`. 1 = bad arguments (keep the Python).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skate_box_downscale(src: *const u8, w: usize, h: usize, factor: usize, out: *mut u8) -> i32 {
    if src.is_null() || out.is_null() || factor == 0 || w < factor || h < factor || w > 1 << 16 || h > 1 << 16 {
        return 1;
    }
    let (w2, h2) = (w / factor, h / factor);
    let src = unsafe { std::slice::from_raw_parts(src, w * h * 4) };
    let out = unsafe { std::slice::from_raw_parts_mut(out, w2 * h2 * 4) };
    let count = (factor * factor) as f64;
    for y in 0..h2 {
        for x in 0..w2 {
            let mut sum = [0u32; 4];
            for dy in 0..factor {
                let row = ((y * factor + dy) * w + x * factor) * 4;
                for (i, &v) in src[row..row + factor * 4].iter().enumerate() {
                    sum[i & 3] += v as u32;
                }
            }
            for c in 0..4 {
                out[(y * w2 + x) * 4 + c] = (sum[c] as f64 / count).round_ties_even() as u8;
            }
        }
    }
    0
}
