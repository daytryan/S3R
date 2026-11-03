//! Packs retail lightmaps into shared pages at load. The world material batch key
//! includes the lightmap, so per-mesh lightmaps kept otherwise-identical materials
//! apart (New San Vanelona: 34,229 batches, 3,678 without the lightmap; University
//! 4,870 vs 664). Sampling is unchanged: lightmaps are read at mip 0, bilinear and
//! clamped, and each tile carries a replicated border wide enough for every tap.
use bevy::{
    image::{ImageSampler, ImageSamplerDescriptor},
    prelude::*,
    render::render_resource::TextureFormat,
};
use skate_data::skate_map::SkateMap;
use std::collections::HashMap;

/// Texture ids at or above this name atlas page `id - FIRST_PAGE`.
pub(super) const FIRST_PAGE: u32 = u32::MAX - 4095;
const PAGE: u32 = 4096;
/// Replicated border: bilinear taps at up to one texel outside the source stay inside it.
const PAD: u32 = 2;
/// Tile cells (source + border, replication filling the rest) are 4-texel
/// aligned, so no BC7 block mixes two lightmaps.
fn cell(n: u32) -> u32 { (n + 2 * PAD).next_multiple_of(4) }

#[derive(Default)]
pub(super) struct Atlas {
    /// Built off the caller's thread (`Page::image`).
    pub pages: Vec<Page>,
    /// Per material index: (page, scale, offset) for its lightmap UVs.
    remap: Vec<Option<(u32, [f32; 2], [f32; 2])>>,
}
impl Atlas {
    pub fn page(&self, material: usize) -> Option<u32> {
        self.remap.get(material).copied().flatten().map(|r| r.0)
    }
    /// `material` is the vertex's 0-based material index.
    pub fn uv(&self, material: usize, uv: [f32; 2]) -> [f32; 2] {
        match self.remap.get(material).copied().flatten() {
            Some((_, scale, offset)) => [offset[0] + uv[0] * scale[0], offset[1] + uv[1] * scale[1]],
            None => uv,
        }
    }
}

/// One atlas page: its used height and its tiles (texture id, top-left texel).
pub(super) struct Page {
    height: u32,
    tiles: Vec<(u32, [u32; 2])>,
}
impl Page {
    /// RGBA8, each tile's lightmap with its replicated border. Both lightmap shaders read .rgb only; opaque alpha
    /// lets BC7 take its opaque path, 5x faster to encode than the alpha one.
    pub fn pixels(&self, map: &SkateMap) -> Vec<u8> {
        self.rows(map, 0, self.height)
    }
    /// Page rows `from..to` of `pixels`.
    fn rows(&self, map: &SkateMap, from: u32, to: u32) -> Vec<u8> {
        let mut out = [0u8, 0, 0, 255].repeat((PAGE * (to - from)) as usize);
        for &(id, [x0, y0]) in &self.tiles {
            let t = &map.textures[id as usize - 1];
            let (w, h) = (t.width as i64, t.height as i64);
            let (first, last) = ((from as i64 - y0 as i64).max(0), (to as i64 - y0 as i64).min(cell(t.height) as i64));
            for y in first..last {
                let sy = (y - PAD as i64).clamp(0, h - 1);
                for x in 0..cell(t.width) as i64 {
                    let sx = (x - PAD as i64).clamp(0, w - 1);
                    let src = ((sy * w + sx) * 4) as usize;
                    let dst = (((y0 as i64 + y - from as i64) * PAGE as i64 + x0 as i64 + x) * 4) as usize;
                    out[dst..dst + 3].copy_from_slice(&t.rgba[src..src + 3]);
                }
            }
        }
        out
    }
    pub fn image(&self, map: &SkateMap) -> Image {
        // Encoded as BC7 a band of rows at a time (blocks are independent and row-major, so the bytes are the
        // whole page's): eight workers composing whole pages held up to 8 x 64 MB of RGBA at once, the load's peak on
        // University (memory 10-12). The pages are opaque, so the opaque profile texture_image would pick.
        let mut image = if super::compresses(PAGE, self.height, 1) {
            const BAND: u32 = 64;
            let mut data = Vec::with_capacity((PAGE * self.height / 4) as usize);
            for from in (0..self.height).step_by(BAND as usize) {
                let to = (from + BAND).min(self.height);
                data.extend(intel_tex_2::bc7::compress_blocks(
                    &intel_tex_2::bc7::opaque_ultra_fast_settings(),
                    &intel_tex_2::RgbaSurface { data: &self.rows(map, from, to), width: PAGE, height: to - from, stride: PAGE * 4 },
                ));
            }
            super::uploaded(data, PAGE, self.height, 1, 1, TextureFormat::Bc7RgbaUnorm)
        } else {
            super::texture_image(self.pixels(map), PAGE, self.height, 1, 1, TextureFormat::Rgba8Unorm)
        };
        // Same as a map lightmap (role 4): clamped, bilinear, mip 0 only.
        image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor::linear());
        image
    }
}

/// The lightmap record `m` samples, if its renderer can read it from a tile,
/// and whether that renderer is the retail one.
#[cfg(test)]
pub(super) fn source(m: &skate_data::skate_map::Material, tuning: &crate::retail_render::MaterialTuning) -> Option<(u32, bool)> {
    source_with(m, m.retail_definition.as_deref().and_then(crate::retail_render::Definition::parse).as_ref(), tuning)
}
/// `source` with `m`'s definition already parsed.
fn source_with(
    m: &skate_data::skate_map::Material,
    d: Option<&crate::retail_render::Definition>,
    tuning: &crate::retail_render::MaterialTuning,
) -> Option<(u32, bool)> {
    let d = d?;
    if !d.supported(tuning) {
        // PBR fallback (Bevy Lightmap) samples textures[1] at plain UVs. Its
        // lightmap is usually the object's, already atlased for the retail
        // materials beside it; sharing the tile avoids a second upload.
        return Some((m.textures[1], false));
    }
    // Families 1-13 only: water and ocean shaders offset lightmap UVs and read its size.
    (1..=13).contains(&d.family).then(|| (d.bindings.get("lightmap").map_or(m.textures[1], |b| b.texture), true))
}

/// `ids` is the renderer's per-material identity (representative material index);
/// materials sharing it share a lightmap, so they get the same tile.
#[cfg(test)]
pub(super) fn build(map: &SkateMap, texture_ids: &[u32], ids: &[usize], tuning: &crate::retail_render::MaterialTuning) -> Atlas {
    let parsed: Vec<_> = map.materials.iter().map(|m| m.retail_definition.as_deref().and_then(crate::retail_render::Definition::parse)).collect();
    build_with(map, texture_ids, ids, tuning, &parsed)
}
/// `build` with each material's definition already parsed (`definitions[i]`).
pub(super) fn build_with(
    map: &SkateMap,
    texture_ids: &[u32],
    ids: &[usize],
    tuning: &crate::retail_render::MaterialTuning,
    definitions: &[Option<crate::retail_render::Definition>],
) -> Atlas {
    let mut lightmap = vec![0u32; map.materials.len()];
    let mut retail = std::collections::HashSet::new();
    for (i, m) in map.materials.iter().enumerate() {
        if ids[i] != i {
            continue;
        }
        if let Some((raw, is_retail)) = source_with(m, definitions[i].as_ref(), tuning) {
            lightmap[i] = texture_ids.get(raw as usize).copied().unwrap_or(0);
            if is_retail {
                retail.insert(lightmap[i]);
            }
        }
    }
    // UV extent of every representative's vertices.
    let mut bounds = vec![[f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY]; map.materials.len()];
    for v in &map.geometry.vertices {
        let Some(&r) = (v.material as usize).checked_sub(1).and_then(|m| ids.get(m)) else { continue };
        if lightmap[r] == 0 {
            continue;
        }
        let b = &mut bounds[r];
        let [u, w] = v.lightmap_uv;
        *b = [b[0].min(u), b[1].min(w), b[2].max(u), b[3].max(w)];
    }
    // One tile per lightmap texture whose users all sample within a texel of it.
    let mut tiles: HashMap<u32, Option<[u32; 2]>> = HashMap::new();
    for r in 0..map.materials.len() {
        let id = lightmap[r];
        if id == 0 || !bounds[r][0].is_finite() {
            continue;
        }
        let t = &map.textures[id as usize - 1];
        let (w, h) = (t.width as f32, t.height as f32);
        let b = bounds[r];
        let inside = t.width > 0 && t.height > 0 && cell(t.width) <= PAGE && cell(t.height) <= PAGE
            && b[0] * w >= -1. && b[1] * h >= -1. && b[2] * w <= w + 1. && b[3] * h <= h + 1.;
        let tile = tiles.entry(id).or_insert(Some([0, 0]));
        if !inside {
            *tile = None;
        }
    }
    // Shelf packing, tallest first. Retail lightmaps go first: retail batches
    // split by page, so PBR-only tiles must not spread them over more pages.
    let mut order: Vec<u32> = tiles.iter().filter(|(_, t)| t.is_some()).map(|(&id, _)| id).collect();
    order.sort_by_key(|&id| (!retail.contains(&id), std::cmp::Reverse(map.textures[id as usize - 1].height), id));
    let mut placed: HashMap<u32, (u32, [u32; 2])> = HashMap::new();
    let mut extents: Vec<u32> = Vec::new(); // used height per page
    let (mut x, mut y, mut shelf) = (0u32, 0u32, 0u32);
    for id in order {
        let t = &map.textures[id as usize - 1];
        let (w, h) = (cell(t.width), cell(t.height));
        if x + w > PAGE {
            (x, y, shelf) = (0, y + shelf, 0);
        }
        if extents.is_empty() || y + h > PAGE {
            extents.push(0);
            (x, y, shelf) = (0, 0, 0);
        }
        let page = extents.len() as u32 - 1;
        placed.insert(id, (page, [x, y]));
        x += w;
        shelf = shelf.max(h);
        extents[page as usize] = extents[page as usize].max(y + h);
    }
    let mut pages: Vec<Page> = extents.iter().map(|&height| Page { height, tiles: Vec::new() }).collect();
    for (&id, &(page, origin)) in &placed {
        pages[page as usize].tiles.push((id, origin));
    }
    let mut remap = vec![None; map.materials.len()];
    for (i, &r) in ids.iter().enumerate() {
        if let Some(&(page, [x0, y0])) = placed.get(&lightmap[r]).filter(|_| lightmap[r] != 0) {
            let t = &map.textures[lightmap[r] as usize - 1];
            let (pw, ph) = (PAGE as f32, extents[page as usize] as f32);
            remap[i] = Some((page, [t.width as f32 / pw, t.height as f32 / ph],
                [(x0 + PAD) as f32 / pw, (y0 + PAD) as f32 / ph]));
        }
    }
    info!("SKATE_LIGHTMAP_ATLAS lightmaps={} pages={} excluded={}", placed.len(), extents.len(), tiles.len() - placed.len());
    Atlas { pages, remap }
}

#[cfg(test)]
mod tests {
    /// Clamp-to-edge bilinear at mip 0, as the world shader samples lightmaps.
    fn bilinear(rgba: &[u8], w: u32, h: u32, uv: [f32; 2]) -> [f32; 4] {
        let (x, y) = (uv[0] * w as f32 - 0.5, uv[1] * h as f32 - 0.5);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let texel = |tx: f32, ty: f32, c: usize| {
            let (tx, ty) = ((tx as i64).clamp(0, w as i64 - 1), (ty as i64).clamp(0, h as i64 - 1));
            f32::from(rgba[((ty * w as i64 + tx) * 4) as usize + c])
        };
        std::array::from_fn(|c| {
            let top = texel(x0, y0, c) * (1. - fx) + texel(x0 + 1., y0, c) * fx;
            let bottom = texel(x0, y0 + 1., c) * (1. - fx) + texel(x0 + 1., y0 + 1., c) * fx;
            top * (1. - fy) + bottom * fy
        })
    }

    /// Every vertex of a real map reads the same lightmap value through the atlas.
    #[test]
    #[ignore = "requires SKATE_MAP_INVENTORY (a .skate map); CPU only"]
    fn atlas_samples_match_source_lightmaps() {
        let map = skate_data::skate_map::SkateMap::load(std::path::Path::new(&std::env::var_os("SKATE_MAP_INVENTORY").unwrap())).unwrap();
        let textures = super::super::render_texture_ids(&map.textures);
        let ids = super::super::render_material_ids(&map.materials, &textures);
        let tuning = crate::retail_render::MaterialTuning::default();
        let atlas = super::build(&map, &textures, &ids, &tuning);
        let pixels: Vec<Vec<u8>> = atlas.pages.iter().map(|p| p.pixels(&map)).collect();
        // Perf 10-12 oracle: each page as build() composed them before, zeroed, the tiles' rgb, then opaque alpha.
        for (page, composed) in atlas.pages.iter().zip(&pixels) {
            let mut serial = vec![0u8; (super::PAGE * page.height * 4) as usize];
            for &(id, [x0, y0]) in &page.tiles {
                let t = &map.textures[id as usize - 1];
                let (w, h) = (t.width as i64, t.height as i64);
                for y in 0..super::cell(t.height) as i64 {
                    let sy = (y - super::PAD as i64).clamp(0, h - 1);
                    for x in 0..super::cell(t.width) as i64 {
                        let sx = (x - super::PAD as i64).clamp(0, w - 1);
                        let src = ((sy * w + sx) * 4) as usize;
                        let dst = (((y0 as i64 + y) * super::PAGE as i64 + x0 as i64 + x) * 4) as usize;
                        serial[dst..dst + 3].copy_from_slice(&t.rgba[src..src + 3]);
                    }
                }
            }
            serial.iter_mut().skip(3).step_by(4).for_each(|a| *a = 255);
            assert!(serial == *composed, "atlas page differs from the serial composition");
        }
        let (mut checked, mut worst) = (0usize, 0f32);
        for v in &map.geometry.vertices {
            let material = v.material as usize - 1;
            let Some(page) = atlas.page(material) else { continue };
            let id = textures[super::source(&map.materials[ids[material]], &tuning).unwrap().0 as usize];
            let t = &map.textures[id as usize - 1];
            let a = bilinear(&t.rgba, t.width, t.height, v.lightmap_uv);
            let b = bilinear(&pixels[page as usize], super::PAGE, atlas.pages[page as usize].height, atlas.uv(material, v.lightmap_uv));
            worst = a.iter().zip(&b).take(3).map(|(a, b)| (a - b).abs()).fold(worst, f32::max);
            checked += 1;
        }
        eprintln!("ATLAS_CHECK vertices={checked} pages={} worst_difference={worst}", atlas.pages.len());
        assert!(checked > 0);
        // Under one 8-bit step: only float rounding of the remapped coordinate.
        assert!(worst < 1., "worst difference {worst}");
    }
}
