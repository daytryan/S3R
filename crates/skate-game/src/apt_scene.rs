//! Retained movie traversal. All geometry and glyphs come from the owned APT.
use crate::{
    apt_movie::Movie,
    apt_vm::{Value, Vm},
};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Vertex {
    pub position: [f32; 2],
    pub uv: [f32; 2],
}
#[derive(Clone, Deserialize)]
pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub rgba: String,
    /// The texel rect [x0, y0, x1, y1] a placeholder quad stands for, when the art is padded (Skate 2's button
    /// bank: a 32 px button in a 64 px texture, its pad's direction arrows past it).
    #[serde(default)]
    pub frame: Option<[f32; 4]>,
}
#[derive(Clone, Deserialize)]
pub struct Shape {
    pub texture: Texture,
    pub triangles: Vec<[Vertex; 3]>,
    pub color: [f32; 4],
}
pub type Shapes = BTreeMap<i32, Vec<Shape>>;
#[derive(Clone, Debug, PartialEq)]
pub struct Draw {
    pub texture: String,
    pub size: [u32; 2],
    pub vertices: Vec<Vertex>,
    pub multiply: [f32; 4],
    pub add: [f32; 4],
}
/// By bits (perf 10-18: a host's draw key over draws it adds itself).
impl std::hash::Hash for Draw {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        (&self.texture, self.size, self.multiply.map(f32::to_bits), self.add.map(f32::to_bits)).hash(h);
        for v in &self.vertices {
            (v.position.map(f32::to_bits), v.uv.map(f32::to_bits)).hash(h);
        }
    }
}
fn compose(a: [f32; 6], b: [f32; 6]) -> [f32; 6] {
    [
        a[0] * b[0] + a[2] * b[1],
        a[1] * b[0] + a[3] * b[1],
        a[0] * b[2] + a[2] * b[3],
        a[1] * b[2] + a[3] * b[3],
        a[0] * b[4] + a[2] * b[5] + a[4],
        a[1] * b[4] + a[3] * b[5] + a[5],
    ]
}
fn transform(m: [f32; 6], v: &Vertex) -> Vertex {
    Vertex {
        position: [
            m[0] * v.position[0] + m[2] * v.position[1] + m[4],
            m[1] * v.position[0] + m[3] * v.position[1] + m[5],
        ],
        uv: v.uv,
    }
}
/// A clip's own placement: its authored matrix with the script's `_x`/`_y` (and `_xscale`/`_yscale`).
fn local(movie: &Movie, vm: &Vm, id: usize) -> [f32; 6] {
    let mut matrix = movie.instances[&id]
        .placement
        .as_ref()
        .map_or([1., 0., 0., 1., 0., 0.], |p| p.matrix);
    matrix[4] = vm.get(id, "_x").number() as f32;
    matrix[5] = vm.get(id, "_y").number() as f32;
    // _xscale/_yscale exist only once a script assigns them; authored scale
    // (and rotation) otherwise stay bit-exact.
    for (column, key) in [(0, "_xscale"), (1, "_yscale")] {
        if let Value::Number(percent) = vm.get(id, key) {
            let (a, b) = (matrix[column * 2], matrix[column * 2 + 1]);
            let length = a.hypot(b);
            let wanted = percent as f32 / 100.;
            if (length - wanted).abs() > 1e-5 {
                if length > 1e-6 {
                    matrix[column * 2] = a / length * wanted;
                    matrix[column * 2 + 1] = b / length * wanted;
                } else {
                    matrix[column * 2] = if column == 0 { wanted } else { 0. };
                    matrix[column * 2 + 1] = if column == 1 { wanted } else { 0. };
                }
            }
        }
    }
    matrix
}
/// Where clip `id` draws on the stage placed by `root` (as [`draw_at`]): its matrix composed up the `_parent` chain.
pub fn stage_matrix(movie: &Movie, vm: &Vm, id: usize, root: [f32; 6]) -> [f32; 6] {
    let mut chain = vec![id];
    while let Value::Object(parent) = vm.get(*chain.last().unwrap(), "_parent") {
        if !movie.instances.contains_key(&parent) || chain.contains(&parent) {
            break;
        }
        chain.push(parent);
    }
    chain.iter().rev().fold(root, |m, &clip| compose(m, local(movie, vm, clip)))
}
/// The two triangles of glyph `g` (from [`crate::apt_text::Font::place`], texture `size`) with the pen at `x` on
/// the line whose top is `y`, at text scale `[sx, sy]`; uvs normalised.
pub(crate) fn glyph_quad(font: &crate::apt_text::Font, g: &crate::apt_text::Glyph, size: [u32; 2], [x, y]: [f32; 2], [sx, sy]: [f32; 2]) -> [Vertex; 6] {
    let x0 = x + g.x_offset * sx;
    let y0 = y + (font.ascent - g.y_offset) * sy;
    let (x1, y1) = (x0 + g.width * sx, y0 + g.height * sy);
    let [u0, v0, u1, v1] = g.atlas_bounds;
    let points = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
    let uvs = [[u0, v0], [u1, v0], [u1, v1], [u0, v1]];
    let size = size.map(|s| s.max(1) as f32);
    [0, 1, 2, 0, 2, 3].map(|i| Vertex { position: points[i], uv: [uvs[i][0] / size[0], uvs[i][1] / size[1]] })
}
/// `m` applied to a stage point.
pub fn apply(m: [f32; 6], [x, y]: [f32; 2]) -> [f32; 2] {
    transform(m, &Vertex { position: [x, y], uv: [0.; 2] }).position
}
pub fn draw(movie: &Movie, vm: &Vm, shapes: &Shapes) -> Result<Vec<Draw>, String> {
    draw_at(movie, vm, shapes, [1., 0., 0., 1., 0., 0.])
}
/// perf 10-18: everything [`draw`] reads but its `shapes`, hashed: a movie whose key is unchanged draws the same list,
/// so a host can keep last frame's (with the key, whatever else its draws depend on: a placement, a hidden flag).
pub fn draw_key(movie: &Movie, vm: &Vm) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (vm.revision, movie.root).hash(&mut h);
    // Hosts fill the image and swatch slots directly, not through the VM.
    for (name, t) in &movie.images {
        (name, t.width, t.height, &t.rgba, t.frame.map(|f| f.map(f32::to_bits))).hash(&mut h);
    }
    for (name, c) in &movie.colours {
        (name, c.map(f32::to_bits)).hash(&mut h);
    }
    let ps = PS_ACTIVE.load(std::sync::atomic::Ordering::Relaxed);
    (ps, PS_GLYPHS.get().is_some(), crate::text_fallback::generation()).hash(&mut h);
    // User 10-26: a movie that has shown the creator tag redraws at each step of its rainbow sweep (20 a second).
    if matches!(vm.get_ref(movie.root, crate::apt_movie::GOLD_SHOWN), Some(Value::Bool(true))) {
        crate::creator::step().hash(&mut h);
    }
    h.finish()
}
/// A host's draw key: [`draw_key`]s with whatever else its draws depend on.
pub fn hash_of(value: impl std::hash::Hash) -> u64 {
    use std::hash::Hasher;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut h);
    h.finish()
}
/// SKATE_APT_REDRAW=1: hosts rebuild every movie's draws every frame (before perf 10-18).
pub fn redraw_always() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("SKATE_APT_REDRAW").is_ok_and(|v| v == "1"))
}
/// [`draw`] with the stage placed by `root` ([a, b, c, d, tx, ty], as a placement's matrix).
pub fn draw_at(movie: &Movie, vm: &Vm, shapes: &Shapes, root: [f32; 6]) -> Result<Vec<Draw>, String> {
    let mut draws = Vec::new();
    visit(
        movie,
        vm,
        shapes,
        movie.root,
        root,
        [1.; 4],
        [0.; 4],
        None,
        None,
        &mut draws,
    )?;
    Ok(draws)
}
fn visit(
    movie: &Movie,
    vm: &Vm,
    shapes: &Shapes,
    id: usize,
    parent: [f32; 6],
    pm: [f32; 4],
    pa: [f32; 4],
    inherited: Option<(&Texture, bool)>,
    tinted: Option<[f32; 4]>,
    out: &mut Vec<Draw>,
) -> Result<(), String> {
    if !vm.get_ref(id, "_visible").is_some_and(Value::truth) {
        return Ok(());
    }
    let instance = &movie.instances[&id];
    let matrix = compose(parent, local(movie, vm, id));
    if matrix.iter().any(|x| !x.is_finite()) {
        return Err(format!(
            "Nonfinite HUD transform on character {} ('{}': _x {:?} _y {:?} _xscale {:?} _yscale {:?})",
            instance.character,
            vm.get(id, "_name").text(),
            vm.get(id, "_x"),
            vm.get(id, "_y"),
            vm.get(id, "_xscale"),
            vm.get(id, "_yscale")
        ));
    }
    let color = instance
        .placement
        .as_ref()
        .map_or([255, 255, 255, 255, 0, 0, 0, 0], |p| p.color);
    let rgba = [1, 2, 3, 0];
    let mut multiply = std::array::from_fn(|i| pm[i] * color[rgba[i]] as f32 / 255.);
    let mut add = std::array::from_fn(|i| pa[i] + pm[i] * color[4 + rgba[i]] as f32 / 255.);
    let alpha = (vm.get(id, "_alpha").number() as f32 / 100.).clamp(0., 1.);
    multiply[3] *= alpha;
    add[3] *= alpha;
    if multiply[3] <= 0. && add[3] <= 0. {
        return Ok(());
    }
    // Native image slots: image_renderer quads renamed to their slot index, and
    // button_item2 clips whose `_test` names a controller glyph.
    // The flag marks a controller glyph: drawn 1:1 (centred) in its slot, not stretched to it.
    let name = vm.get_ref(id, "_name");
    let own = match (name, vm.get_ref(id, "_test")) {
        (_, Some(Value::Text(button))) => {
            // Direction variants (DPad_Up) share their pad's glyph on the disc.
            let button = button.trim_end_matches("_hud");
            glyph(movie, button).or_else(|| button.rsplit_once('_').and_then(|(pad, _)| glyph(movie, pad))).map(|t| (t, true))
        }
        (Some(Value::Text(name)), _) => movie.images.get(name).map(|t| (t, false)),
        _ => None,
    };
    let colour = match name {
        Some(Value::Text(name)) => movie.colours.get(name).copied(),
        _ => None,
    }
    .or(tinted);
    // An image_renderer's (or swatch's) quad shows only a loaded image or set
    // colour, never its authored placeholder fill.
    if own.is_none() && colour.is_none() {
        if let Value::Object(parent) = vm.get(id, "_parent") {
            if vm.get_ref(parent, "mRenderComponent") == Some(&Value::Object(id)) && matches!(vm.get_ref(parent, "ImageIndex"), Some(Value::Number(_))) {
                return Ok(());
            }
            // A native component's clip (component parameter `_type`): the engine draws the movie or image into it;
            // a solid-colour fill is only the editor placeholder (welcomescreen's yellow MovieComponent box,
            // team_management_boot's blue slots). Authored bitmap art in one (core_menu's banner) still draws.
            let solid = shapes
                .get(&movie.characters[&instance.character].id)
                .is_some_and(|units| units.iter().all(|u| u.texture.width == 1 && u.texture.height == 1));
            if solid
                && inherited.is_none()
                && matches!(vm.get_ref(parent, "_type"), Some(Value::Text(kind)) if kind == "MovieComponent" || kind == "ImageRenderComponent")
            {
                return Ok(());
            }
        }
    }
    let image = own.or(inherited);
    let character = &movie.characters[&instance.character];
    if character.type_name == "shape" {
        let shapes = shapes.get(&character.id).ok_or("Missing original shape geometry")?;
        if let Some((image, glyph)) = image {
            // The quad's own extent is the image rectangle.
            let points: Vec<_> = shapes.iter().flat_map(|s| s.triangles.iter().flatten()).map(|v| v.position).collect();
            let min = points.iter().fold([f32::MAX; 2], |m, p| [m[0].min(p[0]), m[1].min(p[1])]);
            let max = points.iter().fold([f32::MIN; 2], |m, p| [m[0].max(p[0]), m[1].max(p[1])]);
            let size = [(max[0] - min[0]).max(1e-6), (max[1] - min[1]).max(1e-6)];
            out.push(Draw {
                texture: image.rgba.clone(),
                size: [image.width, image.height],
                vertices: points
                    .iter()
                    .map(|p| {
                        let uv = [(p[0] - min[0]) / size[0], (p[1] - min[1]) / size[1]];
                        let mut position = *p;
                        // A framed image fills the quad with its frame, the rest of it drawn past at that scale.
                        if let Some(f) = image.frame.or_else(|| glyph.then(|| glyph_frame([image.width, image.height], size))) {
                            let texels = [image.width as f32, image.height as f32];
                            for i in 0..2 {
                                position[i] = min[i] + (uv[i] * texels[i] - f[i]) * size[i] / (f[i + 2] - f[i]);
                            }
                        }
                        transform(matrix, &Vertex { position, uv })
                    })
                    .collect(),
                multiply,
                add,
            });
        }
        for shape in shapes.iter().filter(|_| image.is_none()) {
            out.push(Draw {
                texture: shape.texture.rgba.clone(),
                size: [shape.texture.width, shape.texture.height],
                vertices: shape
                    .triangles
                    .iter()
                    .flatten()
                    .map(|v| transform(matrix, v))
                    .collect(),
                multiply: std::array::from_fn(|i| multiply[i] * colour.unwrap_or(shape.color)[i]),
                add,
            });
        }
    } else if let Some(text) = &character.text {
        let font = &movie.text_assets.fonts
            [&(text["font_id"].as_i64().ok_or("Invalid text font")? as i32)];
        // Native shadow text uses a black atlas pass followed by the sharp
        // futuraheavy glyphs translated +1 in text X (825D6B68/82CA1FD8).
        let passes = if let Some(foreground) = font.foreground {
            vec![
                (font, true, 0.),
                (&movie.text_assets.fonts[&foreground], false, 1.),
            ]
        } else {
            vec![(font, false, 0.)]
        };
        for (font, shadow, advance_x) in passes {
            let height = text["font_height"].as_f64().ok_or("Invalid text height")? as f32;
            let sx = font.scale[0] * height;
            let sy = font.scale[1] * height;
            let shown = match vm.get_ref(id, "_displayText") {
                Some(Value::Text(text)) => std::borrow::Cow::Borrowed(text.as_str()),
                other => std::borrow::Cow::Owned(other.cloned().unwrap_or_default().text()),
            };
            let (value, inline) = inline_glyphs(&crate::text_fallback::nfc(&shown), |name| tag_glyph(movie, name).is_some());
            let bounds = character.bounds.ok_or("Missing text bounds")?;
            let alignment = text["alignment"].as_u64().unwrap_or(0);
            let left = matches!(vm.get_ref(id, "autoSize"), Some(Value::Text(a)) if a == "left");
            let alignment = if left { 0 } else { alignment };
            let mut vertices = Vec::new();
            let mut system: Vec<Draw> = Vec::new();
            let wrap = crate::apt_text::wrap_width(text, bounds, &vm.get(id, "wordWrap"));
            let lines = font.lines(&value, height, wrap);
            // An inline controller glyph (`<B:A>`) draws 1:1, as button_item2's glyphs do, centred on the line.
            let image = |c: char| inline.get((c as u32).wrapping_sub(INLINE_GLYPH) as usize).and_then(|name| tag_glyph(movie, name));
            // Not retail (user 10-27): a fit field's one line shrinks into its room, centred on its old line.
            let (scale, lines) = match fit_room(movie, vm, id, bounds).filter(|_| wrap.is_none() && lines.len() == 1) {
                Some(room) => {
                    let measure = |s: &str, k: f32| {
                        s.chars().map(|c| image(c).map_or_else(|| font.width(&c.to_string(), height) * k, |t| t.width as f32)).sum::<f32>()
                    };
                    let (scale, line) = fitted(&lines[0], room, measure);
                    (scale, vec![line])
                }
                None => (1., lines),
            };
            let bounds = [bounds[0], bounds[1] + (1. - scale) * height * 0.5, bounds[2], bounds[3]];
            let (height, sx, sy) = (height * scale, sx * scale, sy * scale);
            for (row, line) in lines.iter().enumerate() {
                let width = line
                    .chars()
                    .map(|c| image(c).map_or_else(|| font.width(&c.to_string(), height), |t| t.width as f32))
                    .sum::<f32>();
                let mut x = bounds[0]
                    + advance_x
                    + font.offset[0] * sx
                    + match alignment {
                        1 => bounds[2] - bounds[0] - width,
                        2 => (bounds[2] - bounds[0] - width) * 0.5,
                        _ => 0.,
                    };
                let y = bounds[1] + font.offset[1] * sy + row as f32 * height * crate::apt_text::LINE_PITCH;
                for c in line.chars() {
                    if let Some(t) = image(c) {
                        if !shadow {
                            let [w, h] = [t.width as f32, t.height as f32];
                            let y0 = bounds[1] + (row as f32 + 0.5) * height * crate::apt_text::LINE_PITCH - h * 0.5;
                            let points = [[x, y0], [x + w, y0], [x + w, y0 + h], [x, y0 + h]];
                            let uvs = [[0., 0.], [1., 0.], [1., 1.], [0., 1.]];
                            out.push(Draw {
                                texture: t.rgba.clone(),
                                size: [t.width, t.height],
                                vertices: [0, 1, 2, 0, 2, 3]
                                    .map(|i| transform(matrix, &Vertex { position: points[i], uv: uvs[i] }))
                                    .to_vec(),
                                multiply,
                                add,
                            });
                        }
                        x += t.width as f32;
                    } else if let Some((g, texture, size)) = font.place(c) {
                        let quad = glyph_quad(font, &g, size, [x, y], [sx, sy]).map(|v| transform(matrix, &v));
                        if texture == font.texture {
                            vertices.extend(quad);
                        } else if size[0] > 0 {
                            // A system-font glyph (text_fallback): its own texture, tinted below like the atlas.
                            system.push(Draw { texture: texture.to_owned(), size, vertices: quad.to_vec(), multiply, add });
                        }
                        x = g.x_advance.mul_add(sx, x);
                    }
                }
            }
            let mut argb = u32::from_str_radix(
                text["color_argb"]
                    .as_str()
                    .ok_or("Missing text color")?
                    .trim_start_matches('#'),
                16,
            )
            .map_err(|e| e.to_string())?;
            // TextField.textColor replaces RGB; authored alpha is retained.
            if let Value::Number(rgb) = vm.get(id, "textColor") {
                argb = (argb & 0xff00_0000) | (rgb as u32 & 0x00ff_ffff);
            }
            // Not retail (user 10-25): the creator tag's colour (a rainbow sweep, 10-26), over whatever colour the movie
            // gives the field.
            if matches!(vm.get_ref(id, crate::apt_movie::GOLD_FIELD), Some(Value::Bool(true))) {
                let [r, g, b] = crate::creator::colour().map(u32::from);
                argb = (argb & 0xff00_0000) | (r << 16) | (g << 8) | b;
            }
            let color = [
                ((argb >> 16) & 255) as f32 / 255.,
                ((argb >> 8) & 255) as f32 / 255.,
                (argb & 255) as f32 / 255.,
                (argb >> 24) as f32 / 255.,
            ];
            let tint: [f32; 4] = std::array::from_fn(|i| if shadow && i < 3 { 0. } else { multiply[i] * color[i] });
            if !vertices.is_empty() {
                out.push(Draw { texture: font.texture.clone(), size: font.size, vertices, multiply: tint, add });
            }
            out.extend(system.into_iter().map(|d| Draw { multiply: tint, ..d }));
        }
    }
    // Flash masks: a clip placement is never drawn; the siblings up to its clip depth show only inside it.
    let mut masks: Vec<(i32, [f32; 4])> = Vec::new();
    for (depth, child) in instance.children.iter().chain(instance.dynamic.iter()) {
        masks.retain(|(until, _)| depth <= until);
        let mut drawn = Vec::new();
        visit(movie, vm, shapes, *child, matrix, multiply, add, image, colour, &mut drawn)?;
        let clip = movie.instances[child].placement.as_ref().map_or(-1, |p| p.clip_depth);
        if clip > 0 {
            let points = drawn.iter().flat_map(|d| &d.vertices).map(|v| v.position);
            let rect = points.fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |r, p| [r[0].min(p[0]), r[1].min(p[1]), r[2].max(p[0]), r[3].max(p[1])]);
            masks.push((clip, rect));
            continue;
        }
        for mut d in drawn {
            for (_, rect) in &masks {
                d.vertices = clip_rect(&d.vertices, *rect);
            }
            if !d.vertices.is_empty() {
                out.push(d);
            }
        }
    }
    Ok(())
}

/// A fit field's room (apt_text::fit): its own width, or its movie's FIT_WIDTH when narrower. None for other fields.
pub(crate) fn fit_room(movie: &Movie, vm: &Vm, id: usize, bounds: [f32; 4]) -> Option<f32> {
    if !matches!(vm.get_ref(id, crate::apt_movie::FIT_FIELD), Some(Value::Bool(true))) {
        return None;
    }
    let own = bounds[2] - bounds[0] - 4.;
    Some(match vm.get_ref(movie.root, crate::apt_movie::FIT_WIDTH) {
        Some(Value::Number(width)) => own.min(*width as f32),
        _ => own,
    })
}

/// The smallest a fit line gets, of its field's size, before it is cut.
const MIN_FIT: f32 = 0.6;
/// `line` fitted into `room`: the scale (down to [`MIN_FIT`]) and the line, cut with "…" when that is still too wide.
/// `width(line, scale)` measures it.
fn fitted(line: &str, room: f32, width: impl Fn(&str, f32) -> f32) -> (f32, String) {
    let full = width(line, 1.);
    if full <= room {
        return (1., line.to_owned());
    }
    // Shrinking alone fits it: no re-measure, whose rounding could land a hair over `room` and cut it.
    if room / full >= MIN_FIT {
        return (room / full, line.to_owned());
    }
    let (mut kept, mut shown): (Vec<char>, String) = (line.chars().collect(), line.to_owned());
    while width(&shown, MIN_FIT) > room && !kept.is_empty() {
        kept.pop();
        shown = format!("{}…", kept.iter().collect::<String>().trim_end());
    }
    (MIN_FIT, shown)
}

/// PlayStation button art (button_icons' built-in set, served by path through apt_canvas::texture_bytes), drawn instead of a movie's Xbox 360 glyph of the same name
/// while `PS_ACTIVE` (the active pad is a PlayStation pad; `input::pad_glyphs` keeps both).
pub(crate) static PS_GLYPHS: std::sync::OnceLock<BTreeMap<String, Texture>> = std::sync::OnceLock::new();
pub(crate) static PS_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The glyphs Skate 3's disc set lacks (button_icons::EXTRA_GLYPHS), as Xbox art served by path through
/// apt_canvas::texture_bytes. Bug 10-17 #30/#39: without them the slot drew its white placeholder.
static EXTRA_GLYPHS: std::sync::OnceLock<BTreeMap<String, Texture>> = std::sync::OnceLock::new();

/// A controller glyph by name: the active pad's own art, else the movie's, else our extra art.
fn glyph<'a>(movie: &'a Movie, name: &str) -> Option<&'a Texture> {
    PS_ACTIVE
        .load(std::sync::atomic::Ordering::Relaxed)
        .then(|| PS_GLYPHS.get().and_then(|set| set.get(name)))
        .flatten()
        .or_else(|| movie.images.get(&format!("button:{name}")))
        .or_else(|| {
            let texture = |n: &str| Texture { width: 40, height: 30, rgba: format!("runtime/buttons/{n}.rgba"), frame: None };
            let set = EXTRA_GLYPHS.get_or_init(|| crate::button_icons::EXTRA_GLYPHS.iter().map(|&n| (n.to_owned(), texture(n))).collect());
            set.get(name).or_else(|| set.get(&lit_arm(name)?))
        })
}

/// User 10-25: Edit Style's gesture rows (createskater mButtonImage `DPad_Up` .. `DPad_Down`) and the `<B:DPad_Up_hud>`
/// tags show their direction lit (our DPadUp .. art) where the disc has one plain pad; `DPad_UpDown`, `DPad_Arrows`
/// keep it.
fn lit_arm(name: &str) -> Option<String> {
    let arm = name.strip_prefix("DPad_")?;
    ["Up", "Down", "Left", "Right"].contains(&arm).then(|| format!("DPad{arm}"))
}

/// The glyph a `<B:Name>` tag names, resolved as button_item2's `_test` is: the string table's tags carry a `_hud`
/// suffix (`<B:L_Bumper_hud>`), a leading `~` (`<B:~A_hud>`) and direction variants (`<B:DPad_Right>`) that share
/// their pad's art on the disc.
pub(crate) fn tag_glyph<'a>(movie: &'a Movie, name: &str) -> Option<&'a Texture> {
    let name = name.trim_start_matches('~').trim_end_matches("_hud");
    glyph(movie, name).or_else(|| name.rsplit_once('_').and_then(|(pad, _)| glyph(movie, pad)))
}

/// First private-use code point standing for an inline glyph while a line is laid out.
const INLINE_GLYPH: u32 = 0xE000;
/// `text` with each `<B:Name>` controller-glyph tag (the string table's markup, "Press <B:Start> to continue")
/// replaced by one private-use character, and the glyph names in order.
// ponytail: wrapping measures a tag as the font's fallback glyph; the tagged boot strings are single lines.
/// A tag whose glyph the movie lacks (`has`) stays as written.
fn inline_glyphs(text: &str, has: impl Fn(&str) -> bool) -> (String, Vec<String>) {
    let (mut out, mut names, mut rest) = (String::new(), Vec::new(), text);
    while let Some(at) = rest.find("<B:") {
        let Some(end) = rest[at..].find('>') else { break };
        let name = &rest[at + 3..at + end];
        if has(name) {
            out.push_str(&rest[..at]);
            out.push(char::from_u32(INLINE_GLYPH + names.len() as u32).unwrap_or('?'));
            names.push(name.to_owned());
        } else {
            out.push_str(&rest[..at + end + 1]);
        }
        rest = &rest[at + end + 1..];
    }
    out.push_str(rest);
    (out, names)
}

/// The texel rect a frameless controller glyph stands for in a `quad`-sized slot: the slot's size centred
/// on the image, so the glyph draws at one texel per stage unit. Skate 3's glyphs are 40x30 and its Button
/// Item slot 32x32; stretching one to the other turned the round A/B buttons into ovals.
fn glyph_frame([w, h]: [u32; 2], quad: [f32; 2]) -> [f32; 4] {
    let (w, h) = (w as f32, h as f32);
    [(w - quad[0]) / 2., (h - quad[1]) / 2., (w + quad[0]) / 2., (h + quad[1]) / 2.]
}

/// Triangles (a flat list) clipped to an axis-aligned rect [x0, y0, x1, y1], uvs interpolated.
// ponytail: masks clip to their bounding box, exact for the rectangular masks the HUDs use; a shaped mask
// would need a stencil pass.
fn clip_rect(vertices: &[Vertex], rect: [f32; 4]) -> Vec<Vertex> {
    let mut out = Vec::new();
    for tri in vertices.chunks_exact(3) {
        let mut poly = tri.to_vec();
        for (axis, bound, keep_below) in [(0, rect[0], false), (0, rect[2], true), (1, rect[1], false), (1, rect[3], true)] {
            let inside = |v: &Vertex| if keep_below { v.position[axis] <= bound } else { v.position[axis] >= bound };
            let mut next = Vec::new();
            for (i, a) in poly.iter().enumerate() {
                let b = &poly[(i + 1) % poly.len()];
                if inside(a) {
                    next.push(a.clone());
                }
                if inside(a) != inside(b) {
                    let t = (bound - a.position[axis]) / (b.position[axis] - a.position[axis]);
                    let lerp = |p: [f32; 2], q: [f32; 2]| [p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t];
                    next.push(Vertex { position: lerp(a.position, b.position), uv: lerp(a.uv, b.uv) });
                }
            }
            poly = next;
            if poly.is_empty() {
                break;
            }
        }
        for i in 1..poly.len().saturating_sub(1) {
            out.extend([poly[0].clone(), poly[i].clone(), poly[i + 1].clone()]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// perf 10-18: hosts keep last frame's draws while the draw key repeats; a repeated key must mean the same draws.
    #[test]
    fn an_unchanged_draw_key_never_keeps_a_stale_frame() {
        let json = serde_json::json!({
            "characters": [
                {"id": 0, "type_name": "movie", "frames": [
                    {"controls": [{"type_name": "place_object2", "flags": 38, "depth": 1, "character_id": 1, "matrix": [1, 0, 0, 1, 10, 10], "name": "box"}]},
                    {"controls": [{"type_name": "place_object2", "flags": 5, "depth": 1, "matrix": [1, 0, 0, 1, 20, 10]}]},
                    {"controls": [{"type_name": "place_object2", "flags": 9, "depth": 1, "color_transform": [128, 255, 255, 255, 0, 0, 0, 0]}]}
                ]},
                {"id": 1, "type_name": "shape"}
            ],
            "actions": {},
            "language": {}
        });
        let vertex = |x: f32, y: f32| Vertex { position: [x, y], uv: [0., 0.] };
        let texture = Texture { width: 1, height: 1, rgba: "white".into(), frame: None };
        let shapes: Shapes = [(1, vec![Shape { texture, triangles: vec![[vertex(0., 0.), vertex(8., 0.), vertex(0., 8.)]], color: [1.; 4] }])].into();
        let mut movie = Movie::load(&json).unwrap();
        let mut vm = Vm::new();
        movie.initialize(&mut vm).unwrap();
        let (mut kept, mut repeats, mut changes) = (None::<(u64, Vec<Draw>)>, 0, 0);
        let mut check = |movie: &Movie, vm: &Vm| {
            for _ in 0..2 {
                let (key, draws) = (draw_key(movie, vm), draw(movie, vm, &shapes).unwrap());
                if let Some((k, d)) = &kept {
                    if *k == key {
                        assert_eq!(&draws, d, "a repeated key drew something else");
                        repeats += 1;
                    } else if draws != *d {
                        changes += 1;
                    }
                }
                kept = Some((key, draws));
            }
        };
        check(&movie, &vm);
        for _ in 0..3 {
            movie.advance(&mut vm).unwrap();
            check(&movie, &vm);
        }
        let root = movie.root;
        movie.method(&mut vm, root, "stop", &[]).unwrap();
        movie.advance(&mut vm).unwrap();
        check(&movie, &vm);
        let Value::Object(clip) = vm.get(root, "box") else { panic!("box") };
        let x = vm.get(clip, "_x");
        vm.set(clip, "_x", x).unwrap();
        check(&movie, &vm);
        vm.set(clip, "_x", Value::Number(55.)).unwrap();
        check(&movie, &vm);
        movie.colours.insert("box".into(), [1., 0., 0., 1.]);
        check(&movie, &vm);
        vm.set(clip, "_alpha", Value::Number(50.)).unwrap();
        check(&movie, &vm);
        movie.method(&mut vm, root, "gotoAndStop", &[Value::Number(2.)]).unwrap();
        check(&movie, &vm);
        assert!(repeats >= 10 && changes >= 5, "{repeats} repeats, {changes} changes");
    }

    /// A fit line (user 10-27) stays as it is when it fits, shrinks to its room, and past MIN_FIT is cut with "…".
    #[test]
    fn a_fit_line_shrinks_then_cuts() {
        let width = |s: &str, k: f32| s.chars().count() as f32 * 10. * k;
        assert_eq!(fitted("abcdefghij", 100., width), (1., "abcdefghij".to_owned()));
        assert_eq!(fitted("abcdefghij", 80., width), (0.8, "abcdefghij".to_owned()));
        let (scale, cut) = fitted("abcd efghij", 50., width);
        assert_eq!((scale, cut.as_str()), (MIN_FIT, "abcd ef…"), "8 characters at 6 px: 48 of 50");
        assert!(width(&cut, scale) <= 50.);
        // A measure that isn't exactly proportional (float rounding) never cuts a line that shrinking alone fits: the
        // server browser's "Ryan's Park - Freeskate - 3/50 - 45 ms" came out "... 45…" (render 10-27).
        let rounded = |s: &str, k: f32| width(s, k) + 1e-3;
        assert_eq!(fitted("abcdefghij", 80., rounded).1, "abcdefghij");
    }

    #[test]
    fn inline_glyph_tags_become_placeholders() {
        let (text, names) = inline_glyphs("<B:A> Skip <B:B> Cancel", |_| true);
        assert_eq!(text, "\u{E000} Skip \u{E001} Cancel");
        assert_eq!(names, ["A", "B"]);
        assert_eq!(inline_glyphs("no tags <B:open", |_| true), ("no tags <B:open".to_owned(), Vec::new()));
        // Without the glyph the tag stays text.
        assert_eq!(inline_glyphs("<B:Z> x <B:A>", |n| n == "A"), ("<B:Z> x \u{E000}".to_owned(), vec!["A".to_owned()]));
    }

    #[test]
    fn glyphs_draw_one_to_one_in_their_slot() {
        assert_eq!(glyph_frame([40, 30], [32., 32.]), [4., -1., 36., 31.]);
    }

    #[test]
    fn a_dpad_direction_gets_its_lit_arm() {
        for arm in ["Up", "Down", "Left", "Right"] {
            let lit = lit_arm(&format!("DPad_{arm}")).unwrap();
            assert!(crate::button_icons::EXTRA_GLYPHS.contains(&lit.as_str()), "{lit}");
        }
        assert_eq!(lit_arm("DPad_UpDown"), None, "two arms keep the plain pad");
        assert_eq!(lit_arm("DPad_Arrows"), None);
        assert_eq!(lit_arm("DPad"), None);
    }

    #[test]
    fn clipping_keeps_the_part_inside_the_mask() {
        let v = |x: f32, y: f32| Vertex { position: [x, y], uv: [x / 10., y / 10.] };
        let clipped = clip_rect(&[v(0., 0.), v(10., 0.), v(0., 10.)], [0., 0., 5., 10.]);
        assert!(clipped.iter().all(|p| p.position[0] <= 5. && (p.uv[0] - p.position[0] / 10.).abs() < 1e-6));
        let area: f32 = clipped
            .chunks_exact(3)
            .map(|t| {
                let [a, b, c] = [t[0].position, t[1].position, t[2].position];
                ((b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])).abs() / 2.
            })
            .sum();
        assert!((area - 37.5).abs() < 1e-3, "{area}"); // 50 - the 12.5 triangle right of x = 5
        assert!(clip_rect(&[v(0., 0.), v(1., 0.), v(0., 1.)], [5., 5., 6., 6.]).is_empty());
    }
}
