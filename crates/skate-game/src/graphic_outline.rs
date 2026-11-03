//! The garment's outline on the Graphic Creator's board: the worn item's secondary-UV triangles that fall
//! inside the side's rectangle, rasterised to a coverage mask at the board's size. The covered pixels next
//! to an uncovered one are the edges, drawn faintly so the player sees where the garment's side ends. Pure.

pub(crate) struct OutlineMask {
    pub size: (u32, u32),
    /// 1 = on an edge of the UV coverage.
    pub edges: Vec<u8>,
}
impl OutlineMask {
    /// The edges as straight-alpha white at `alpha`, for the board.
    pub fn rgba(&self, alpha: u8) -> Vec<u8> {
        self.edges.iter().flat_map(|e| if *e == 1 { [255, 255, 255, alpha] } else { [0; 4] }).collect()
    }
}

/// `triangles_uv2`: secondary UVs, V up. `rect`: the side's `[top, bottom, left, right]`, V up.
pub(crate) fn outline(triangles_uv2: &[[[f32; 2]; 3]], rect: [f32; 4], size: (u32, u32)) -> OutlineMask {
    let [top, bottom, left, right] = rect;
    let (w, h) = (size.0 as usize, size.1 as usize);
    let mut covered = vec![false; w * h];
    let to_pixel = |[u, v]: [f32; 2]| [(u - left) / (right - left) * w as f32, (top - v) / (top - bottom) * h as f32];
    for tri in triangles_uv2 {
        let p = tri.map(to_pixel);
        let area = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[2][0] - p[0][0]) * (p[1][1] - p[0][1]);
        if area.abs() < 1e-9 {
            continue;
        }
        let lo = |i: usize| p.iter().map(|q| q[i]).fold(f32::MAX, f32::min).floor().max(0.) as usize;
        let hi = |i: usize, n: usize| (p.iter().map(|q| q[i]).fold(f32::MIN, f32::max).ceil().max(0.) as usize).min(n);
        let (x0, x1, y0, y1) = (lo(0), hi(0, w), lo(1), hi(1, h));
        for y in y0..y1 {
            for x in x0..x1 {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let e = |a: [f32; 2], b: [f32; 2]| ((b[0] - a[0]) * (py - a[1]) - (b[1] - a[1]) * (px - a[0])) / area;
                if e(p[1], p[2]) >= 0. && e(p[2], p[0]) >= 0. && e(p[0], p[1]) >= 0. {
                    covered[y * w + x] = true;
                }
            }
        }
    }
    // An edge: covered, with an uncovered 4-neighbour inside the canvas (the canvas's own border is not one).
    let at = |x: isize, y: isize| x < 0 || y < 0 || x >= w as isize || y >= h as isize || covered[y as usize * w + x as usize];
    let edges = (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as isize, (i / w) as isize);
            u8::from(covered[i] && !(at(x - 1, y) && at(x + 1, y) && at(x, y - 1) && at(x, y + 1)))
        })
        .collect();
    OutlineMask { size, edges }
}

/// Every triangle of a `.glb` model in its secondary UVs (TEXCOORD_1), V up.
pub(crate) fn glb_triangles(bytes: &[u8]) -> Option<Vec<[[f32; 2]; 3]>> {
    let u32_at = |o: usize| Some(u32::from_le_bytes(bytes.get(o..o + 4)?.try_into().ok()?) as usize);
    if bytes.get(0..4)? != b"glTF" {
        return None;
    }
    let json_len = u32_at(12)?;
    let json: serde_json::Value = serde_json::from_slice(bytes.get(20..20 + json_len)?).ok()?;
    let bin = 20 + json_len + 8;
    let accessor = |i: &serde_json::Value| -> Option<(usize, usize, usize, u64)> {
        let a = &json["accessors"][i.as_u64()? as usize];
        let view = &json["bufferViews"][a["bufferView"].as_u64()? as usize];
        let start = bin + view["byteOffset"].as_u64().unwrap_or(0) as usize + a["byteOffset"].as_u64().unwrap_or(0) as usize;
        let kind = a["componentType"].as_u64()?;
        let width = match kind {
            5121 => 1,
            5123 => 2,
            5125 | 5126 => 4,
            _ => return None,
        } * match a["type"].as_str()? {
            "SCALAR" => 1,
            "VEC2" => 2,
            _ => return None,
        };
        let stride = view["byteStride"].as_u64().map_or(width, |s| s as usize);
        Some((start, stride, a["count"].as_u64()? as usize, kind))
    };
    let f32_at = |o: usize| Some(f32::from_le_bytes(bytes.get(o..o + 4)?.try_into().ok()?));
    let mut out = Vec::new();
    for primitive in json["meshes"].as_array()?.iter().flat_map(|m| m["primitives"].as_array().into_iter().flatten()) {
        if primitive["mode"].as_u64().unwrap_or(4) != 4 {
            continue;
        }
        let Some((start, stride, count, 5126)) = accessor(&primitive["attributes"]["TEXCOORD_1"]) else { continue };
        let uv = |k: usize| Some([f32_at(start + k * stride)?, 1. - f32_at(start + k * stride + 4)?]);
        let indices: Vec<usize> = match accessor(&primitive["indices"]) {
            Some((s, st, n, kind)) => (0..n)
                .map(|k| {
                    let o = s + k * st;
                    Some(match kind {
                        5121 => *bytes.get(o)? as usize,
                        5123 => u16::from_le_bytes(bytes.get(o..o + 2)?.try_into().ok()?) as usize,
                        _ => u32_at(o)?,
                    })
                })
                .collect::<Option<_>>()?,
            None => (0..count).collect(),
        };
        for t in indices.chunks_exact(3) {
            if t.iter().all(|i| *i < count) {
                out.push([uv(t[0])?, uv(t[1])?, uv(t[2])?]);
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECT: [f32; 4] = [1., 0., 0., 1.];
    fn square(lo: f32, hi: f32) -> Vec<[[f32; 2]; 3]> {
        vec![[[lo, lo], [hi, lo], [hi, hi]], [[lo, lo], [hi, hi], [lo, hi]]]
    }

    #[test]
    fn outline_of_a_square_is_its_border() {
        let m = outline(&square(0.25, 0.75), RECT, (16, 16));
        let edge = |x: usize, y: usize| m.edges[y * 16 + x] == 1;
        // Pixels 4..12 are covered; the ring of them is the edge, the inside and outside are not.
        assert!((4..12).all(|i| edge(i, 4) && edge(i, 11) && edge(4, i) && edge(11, i)));
        assert!((5..11).all(|x| (5..11).all(|y| !edge(x, y))));
        assert!(!edge(3, 8) && !edge(12, 8) && !edge(0, 0));
        assert_eq!(m.edges.iter().filter(|e| **e == 1).count(), 28);
        // Filling the whole canvas draws no frame round it.
        assert!(outline(&square(-1., 2.), RECT, (8, 8)).edges.iter().all(|e| *e == 0));
    }

    #[test]
    fn outline_ignores_triangles_outside_the_side() {
        let m = outline(&square(1.5, 2.5), RECT, (16, 16));
        assert!(m.edges.iter().all(|e| *e == 0));
        assert_eq!(m.rgba(64).len(), 16 * 16 * 4);
    }

    /// The installed tee (SKATE_ASSET_ROOT): its front's outline is drawn, inside the canvas.
    #[test]
    #[ignore = "needs the installed customisation set"]
    fn a_tees_front_outline_is_inside_the_canvas() {
        let root = std::path::PathBuf::from(std::env::var("SKATE_ASSET_ROOT").expect("SKATE_ASSET_ROOT"));
        let sets = root.join("private/customisation/sets");
        let set = std::fs::read_dir(&sets).unwrap().flatten().find(|e| e.path().join("library").is_dir()).unwrap().path();
        let bytes = std::fs::read(set.join("library/801095192c838106_v4.glb")).unwrap();
        let triangles = glb_triangles(&bytes).unwrap();
        assert!(triangles.len() > 100);
        let m = outline(&triangles, [0.98, 0.48, 0.13, 0.37], (246, 512));
        let n = m.edges.iter().filter(|e| **e == 1).count();
        assert!(n > 200, "edges {n}");
        assert_eq!(m.edges.len(), 246 * 512);
    }
}
