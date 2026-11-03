//! Skate 1 / Skate 2 colour grading on their converted maps (data: skate2port/old_grading.py).
//! Both games finish with a 32^3 colour cube picked by area and blended over 1.8 s; Skate 2 first adds
//! its bloom (bright pass, box downsamples, 9-tap blurs, level combine, lens ghosts) and last draws the
//! frame through its fisheye lens and vignette, all ported from its own pixel shaders with the constants
//! its GPU command buffer carried. Maps without a grading file are untouched: the tone pass gets
//! `enabled = 0` and a black bloom.
use bevy::{
    asset::embedded_asset,
    core_pipeline::FullscreenShader,
    prelude::*,
    render::{
        Render, RenderApp, RenderStartup, RenderSystems,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_resource::{binding_types::*, *},
        renderer::{RenderContext, RenderDevice, RenderQueue},
    },
};
use std::sync::{Arc, Mutex};

const BLEND_SECONDS: f32 = 1.8; // Skate 2's cube blend state, read from the running game
const CUBE: u32 = 32;
const VIGNETTE: usize = 512;

#[derive(serde::Deserialize)]
struct Meta {
    lookup: Lookup,
    bloom: bool,
    zones: Vec<Zone>,
    grid: Option<Grid>,
    #[serde(default)]
    lens: Option<Lens>,
    #[serde(default)]
    film: Option<Film>,
}
/// Skate 1's post-processing (old_grading.py skate1_film): its grunge texture and `s1`, the vault data its exe packs
/// into the composite (E8DC) and final pass (B699240D) every frame. The file's other film keys are for older builds.
#[derive(serde::Deserialize)]
struct Film {
    texture: String,
    size: [u32; 2],
    /// A fixed exposure in place of Skate 1's meter.
    #[serde(default)]
    exposure: Option<f32>,
    s1: Skate1,
}
/// skate2port/GRADING_AUDIT_S1.md has the source of every value.
#[derive(serde::Deserialize, Clone)]
struct Skate1 {
    /// B699240D: adjust_hv_ratio, warp_vignette_power (warp, vignette), hv_upscaler, hv_warp_multiplier, vignette_multiplier.
    adjust: [f32; 2],
    power: [f32; 2],
    upscale: [f32; 2],
    warp: [f32; 2],
    vignette: f32,
    /// The grade sets (0x8273fc10), far (A) and near (B): 11 curves of t each, as [x0..x3, y0..y3].
    far: [[f32; 8]; 11],
    near: [[f32; 8]; 11],
    /// Grunge radius, feather (uploaded as 1/x), centre x, y; NIS near, far (metres); bloom threshold, multiply x, y.
    curves: [[f32; 8]; 9],
    /// VisualDirector (0x826f7618): t by speed (16 x, then 16 y), eased toward with this weight per 1/60 s.
    speed: [f32; 32],
    smoothing: f32,
    /// Far depth of field from, to (metres), bDOFLerpScale.
    dof: [f32; 3],
    /// maxCoC = x + (y - x)*clamp((exposure - z)/(w - z), 0, 1), exposure in Skate 1 units.
    coc: [f32; 4],
    /// Auto exposure target, min, max, rate, in Skate 1 units (its exposure 1 = the engine's 2.5): row 'tonemap'.
    meter: [f32; 4],
    /// The same from row 5133EA95, which 0x8273ede4 reads while the camera is inside one of `volumes` (0x822ada80).
    #[serde(default)]
    meter_volume: Option<[f32; 4]>,
    /// skate.ems audio volumes with eVolumeType 1 and eNotifyType & 1: centre, extents, forward.
    #[serde(default)]
    volumes: Vec<[f32; 9]>,
    /// E8DC's deck ambient occlusion (vault deck_ao): XLen, ZLen, Radius, HeightFallOff, Colour r, g, b. Absent: off.
    #[serde(default)]
    deck_ao: Option<[f32; 7]>,
    /// E8DC/3D17's camera motion blur (postfx main): motion_blur_intensity, motion_blur_enable (g_bDOF). Absent: off.
    #[serde(default)]
    motion_blur: Option<[f32; 2]>,
}
#[derive(serde::Deserialize)]
struct Lens {
    warp: [f32; 2],
    vignette: String,
}
#[derive(serde::Deserialize)]
struct Lookup {
    scale: f32,
    offset: f32,
}
#[derive(serde::Deserialize)]
struct Zone {
    cube: usize,
    #[serde(default)]
    bloom: Option<Bloom>,
    #[serde(default)]
    exposure: Option<[f32; 4]>,
    #[serde(default)]
    fog: Option<Fog>,
    #[serde(default)]
    sky: Option<[f32; 2]>,
    /// The district's title label (ID_DISTRICT_*), for Skate 2's HUD.
    #[serde(default)]
    district: Option<String>,
}
#[derive(serde::Deserialize)]
struct Fog {
    ramp: [f32; 4],
    colour: [f32; 4],
}

/// A Skate 2 zone's exposure (target, min, max, damping), fog ramp and colour (world-shader units),
/// and sky (sun scale, multiplier relative to the dome's own).
#[derive(Clone, Copy, PartialEq)]
struct Preset {
    exposure: Vec4,
    fog_ramp: Vec4,
    fog_colour: Vec4,
    sky: Vec2,
}
impl Preset {
    fn mix(self, to: Self, t: f32) -> Self {
        Self {
            exposure: self.exposure.lerp(to.exposure, t),
            fog_ramp: self.fog_ramp.lerp(to.fog_ramp, t),
            fog_colour: self.fog_colour.lerp(to.fog_colour, t),
            sky: self.sky.lerp(to.sky, t),
        }
    }
}
#[derive(serde::Deserialize, Clone, Copy, Default)]
struct Bloom {
    threshold: f32,
    scale: f32,
    level0: f32,
    level1: f32,
    saturation: f32,
}
#[derive(serde::Deserialize)]
struct Grid {
    x0: f32,
    z0: f32,
    cell: f32,
    width: usize,
    height: usize,
}

pub(super) struct Data {
    cubes: Vec<u8>,
    lookup: Vec2,
    bloom: bool,
    zones: Vec<(usize, Bloom)>,
    /// Per zone, when the map has them (Skate 2's city).
    presets: Vec<Option<Preset>>,
    districts: Vec<Option<String>>,
    grid: Option<(Grid, Vec<u8>)>,
    /// Fisheye (centre scale, r^4 scale) and the 512-texel RGBA vignette gradient.
    lens: Option<(Vec2, Vec<u8>)>,
    /// Skate 1's post-processing and its RGBA grunge texture.
    film: Option<(Skate1, Vec<u8>, UVec2)>,
    /// A fixed exposure for the whole map (Skate 1), instead of the auto meter.
    fixed_exposure: Option<f32>,
}
impl Data {
    fn zone(&self, at: Vec3) -> usize {
        let Some((g, cells)) = &self.grid else { return 0 };
        let x = ((at.x - g.x0) / g.cell).floor().clamp(0., (g.width - 1) as f32) as usize;
        let z = ((at.z - g.z0) / g.cell).floor().clamp(0., (g.height - 1) as f32) as usize;
        (cells[z * g.width + x] as usize).min(self.zones.len() - 1)
    }
}

/// The district title label (ID_DISTRICT_*) of the zone under the camera, on maps whose grading names them.
#[derive(Resource, Default, PartialEq)]
pub(crate) struct District(pub Option<String>);

/// The map's grading and the zone blend: `from` fades into `to` as `blend` goes 0 -> 1.
#[derive(Resource, Clone, ExtractResource, Default)]
pub(super) struct Grading {
    data: Option<Arc<Data>>,
    from: usize,
    to: usize,
    blend: f32,
    /// The sky dome material, its own multiplier and sun size, and the sky values last written to it.
    sky: Option<(AssetId<super::RetailSkyMaterial>, Vec2, Vec2)>,
    /// [`ShotPost`], copied each frame.
    post: ShotPost,
    /// [`ScreenFade`], copied each frame.
    fade: f32,
    /// Skate 2's blenders restart from the value on screen (set-target 0x8268ec70): the fog/sky preset and the
    /// bloom when the zone last changed, and the seconds since.
    since: f32,
    preset_from: Option<Preset>,
    bloom_from: Option<Bloom>,
    /// Skate 1's VisualDirector intensity t and the camera position its speed is measured from.
    film_t: f32,
    camera_at: Option<Vec3>,
    /// Skate 1's camera facing last frame, and whether the camera cut this frame (motion blur restarts).
    camera_facing: Option<Vec3>,
    cut: bool,
    /// Skate 1's deck frame (deck_from_world; None: no deck AO) and the skater stencil's capsules
    /// (end a, radius; end b, 0), from [`skate1_skater`].
    deck: Option<Mat4>,
    skater: [Vec4; 12],
}

/// A Skate 2 cutscene shot's post-effect overrides (challenge/cutscene.rs); the defaults outside scenes.
#[derive(Resource, Clone, Copy, PartialEq)]
pub(crate) struct ShotPost {
    /// FISHEYE_MULTIPLIER over the map's lens, 1 without.
    pub lens: f32,
    /// TONEMAP_VALUE, a fixed exposure (retail_exposure.rs `hold`), 0 without.
    pub exposure: f32,
    /// BLOOM_THRESHOLD and BLOOM_INTENSITY, 0 without.
    pub bloom: Vec2,
    /// SUN_DIRECTION, in the sky data's own axes (retail_character.rs), 0 without.
    pub sun: Vec3,
    /// A scene's camera drives the view (Skate 3 maps take retail's scene lens then, [`skate3_lens`]).
    pub scene: bool,
}
impl Default for ShotPost {
    fn default() -> Self {
        Self { lens: 1., exposure: 0., bloom: Vec2::ZERO, sun: Vec3::ZERO, scene: false }
    }
}
/// Skate 2's screen fade (globals+0x7bd8/+0x7bf8), 0..1: BloomToWhiteTransitionEffect (0x82666840) ramps it
/// around NIS cuts, and the bright pass's threshold drops by twice it, so the picture blooms out to white.
#[derive(Resource, Clone, Copy, Default, PartialEq)]
pub(crate) struct ScreenFade(pub f32);

impl Grading {
    /// The grading to draw.
    fn drawn(&self) -> Option<&Arc<Data>> {
        self.data.as_ref()
    }
    /// Skate 2's fog and sky blenders tick 0.2/s (0x823c4a88; set up by 0x8268e868 / 0x826a5d08), so they fade
    /// over 5 s. Its exposure meter reads the new zone's auto_exposure row directly (0x82325a48; the row's
    /// blender writes a dummy nothing reads), so the tuning switches at once.
    fn preset(&self, data: &Data) -> Option<Preset> {
        let to = data.presets.get(self.to).copied().flatten()?;
        let mut p = self.preset_from.unwrap_or(to).mix(to, (self.since / 5.).min(1.));
        p.exposure = to.exposure;
        Some(p)
    }
    /// Skate 2's five bloom blenders (0x8268ef20): bloom_threshold (layout offset 0) ticks 4/s, the rest
    /// 0.5/s, so 0.25 s and 2 s; the bright pass's scale follows the threshold (1 / (1 - t), 0x82374598).
    /// Skate 1 keeps the cube's blend.
    fn bloom(&self, data: &Data) -> Bloom {
        let zone = |z: usize| data.zones[z.min(data.zones.len() - 1)].1;
        let to = zone(self.to);
        let skate2 = data.lens.is_some();
        let (from, t, s) = match skate2 {
            true => (self.bloom_from.unwrap_or(to), (self.since / 0.25).min(1.), (self.since / 2.).min(1.)),
            false => (zone(self.from), self.blend, self.blend),
        };
        let mix = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let threshold = mix(from.threshold, to.threshold, t);
        Bloom {
            threshold,
            scale: if skate2 { 1. / (1. - threshold) } else { mix(from.scale, to.scale, t) },
            level0: mix(from.level0, to.level0, s),
            level1: mix(from.level1, to.level1, s),
            saturation: mix(from.saturation, to.saturation, s),
        }
    }
}

pub(super) fn install(app: &mut App) {
    embedded_asset!(app, "retail_grading.wgsl");
    app.init_resource::<Grading>()
        .init_resource::<District>()
        .init_resource::<ShotPost>()
        .init_resource::<ScreenFade>()
        .add_plugins(ExtractResourcePlugin::<Grading>::default())
        .add_systems(Startup, load)
        .add_systems(PreUpdate, load.after(crate::map_transition::MapTransitionSet).run_if(super::world_changed))
        .add_systems(PreUpdate, load.run_if(look_changed).after(crate::map_transition::MapTransitionSet))
        .add_systems(Update, follow)
        .add_systems(PostUpdate, skate1_skater.after(bevy::transform::TransformSystems::Propagate));
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render
            .add_systems(RenderStartup, initialize)
            .add_systems(Render, upload.in_set(RenderSystems::PrepareResources));
    }
}

/// Not retail (user 10-14, feature #9): the grading file a map shows with Settings' look (None: the map's own game),
/// and whether it is borrowed from another game. Skate 3 draws no grading; Skate 2's look is its Center park's single
/// zone (INFERRED as representative), Skate 1's is San Vanelona's first district (financial).
fn source(stem: &str, look: Option<u8>) -> Option<(String, bool)> {
    let game = if stem == "Skate_1_San_Vanelona" { 1 } else if stem == "New_San_Vanelona" || stem.starts_with("Skate_2_") { 2 } else { 3 };
    match look.unwrap_or(game) {
        g if g == game => Some((stem.to_owned(), false)),
        1 => Some(("Skate_1_San_Vanelona".into(), true)),
        2 => Some(("Skate_2_Center".into(), true)),
        _ => None,
    }
}

fn look_changed(menu: Option<Res<crate::graphics_menu::Menu>>, mut last: Local<Option<Option<u8>>>) -> bool {
    let look = menu.and_then(|m| m.look());
    last.replace(look).is_some_and(|was| was != look)
}

fn read(config: &crate::config::Config, retail: bool, look: Option<u8>) -> Option<Data> {
    let stem = config.map_path.as_ref()?.file_stem()?.to_string_lossy().into_owned();
    let (name, borrowed) = source(&stem, look)?;
    let root = config.asset_root.join("private/colour-grading");
    let meta: Meta = serde_json::from_slice(&std::fs::read(root.join(format!("{name}.json"))).ok()?).ok()?;
    let cubes = std::fs::read(root.join(format!("{name}.cubes"))).ok()?;
    let count = cubes.len() / (CUBE.pow(3) as usize * 4);
    let wants_grid = meta.grid.is_some();
    let grid = match meta.grid {
        Some(g) => {
            let cells = std::fs::read(root.join(format!("{name}.zones"))).ok()?;
            (cells.len() == g.width * g.height).then_some((g, cells))
        }
        None => None,
    };
    let zones: Vec<_> = meta.zones.iter().map(|z| (z.cube, z.bloom.unwrap_or_default())).collect();
    let presets: Vec<_> = meta
        .zones
        .iter()
        .map(|z| match (z.exposure, &z.fog, z.sky) {
            (Some(e), Some(f), Some(s)) if e.iter().chain(&f.ramp).chain(&f.colour).chain(&s).all(|v| v.is_finite()) && e[1] > 0. && e[2] >= e[1] => {
                // The frame state carries fog colour at the world's 2.5 exposure baseline (as AuthoredFog)
                // and ramp.w = 1 tells the world shader to use it over the material's.
                let c = Vec4::from(f.colour);
                Some(Preset { exposure: Vec4::from(e), fog_ramp: Vec4::from(f.ramp).with_w(1.), fog_colour: (c.truncate() * 2.5).extend(c.w), sky: Vec2::from(s) })
            }
            _ => None,
        })
        .collect();
    let lens = meta.lens.as_ref().and_then(|l| {
        let rgb: Vec<u8> = (0..l.vignette.len() / 2).map(|i| u8::from_str_radix(l.vignette.get(2 * i..2 * i + 2)?, 16).ok()).collect::<Option<_>>()?;
        (rgb.len() == VIGNETTE * 3).then(|| (Vec2::from(l.warp), rgb.chunks_exact(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect()))
    });
    let film = meta.film.as_ref().and_then(|f| {
        let rgba = std::fs::read(root.join(&f.texture)).ok()?;
        let size = UVec2::from(f.size);
        (rgba.len() == (size.x * size.y * 4) as usize && size.min_element() > 0 && f.s1.dof[1] > f.s1.dof[0])
            .then(|| (f.s1.clone(), rgba, size))
    });
    let valid = retail && count > 0 && cubes.len() == count * CUBE.pow(3) as usize * 4 && !zones.is_empty()
        && zones.iter().all(|(c, _)| *c < count) && (!wants_grid || grid.is_some()) && (meta.lens.is_none() || lens.is_some())
        && (meta.film.is_none() || film.is_some());
    if !valid {
        warn!("COLOUR_GRADING: ignoring malformed grading for {name}");
        return None;
    }
    info!("COLOUR_GRADING: {name}: {count} cubes, {} zones, bloom {}", zones.len(), meta.bloom);
    let districts = meta.zones.into_iter().map(|z| z.district).collect();
    // Another game's look on this map: one zone everywhere, and none of its places (districts, fog and sky presets,
    // Skate 1's interior volumes), which belong to its own city.
    let (grid, presets, districts, mut film) = if borrowed {
        (None, vec![None; presets.len()], Vec::new(), film)
    } else {
        (grid, presets, districts, film)
    };
    if borrowed {
        if let Some((s1, _, _)) = film.as_mut() {
            s1.volumes.clear();
        }
    }
    let fixed_exposure = meta.film.as_ref().and_then(|f| f.exposure).filter(|e| e.is_finite() && *e > 0.);
    Some(Data { cubes, lookup: Vec2::new(meta.lookup.scale, meta.lookup.offset), bloom: meta.bloom, zones, presets, districts, grid, lens, film, fixed_exposure })
}

/// On a new world, and when Settings > Screen Effects flips: off leaves every map as Skate 3 draws it.
#[allow(clippy::too_many_arguments)]
fn load(
    config: Res<crate::config::Config>,
    retail: Res<super::RetailScene>,
    mut grading: ResMut<Grading>,
    mut district: ResMut<District>,
    mut fog: ResMut<super::AuthoredFog>,
    mut sky_materials: ResMut<Assets<super::RetailSkyMaterial>>,
    menu: Option<Res<crate::graphics_menu::Menu>>,
) {
    // Undo what the zones wrote, so turning the effects off mid-map restores Skate 3's sky and fog.
    if let Some((id, base, _)) = grading.sky {
        if let Some(m) = sky_materials.get_mut(id) {
            (m.params.settings.z, m.params.settings.w) = (base.x, base.y);
        }
    }
    if grading.data.as_ref().is_some_and(|d| d.presets.iter().any(Option::is_some)) {
        *fog = super::AuthoredFog::default();
    }
    *grading = Grading { data: read(&config, retail.0, menu.and_then(|m| m.look())).map(Arc::new), blend: 1., ..default() };
    district.set_if_neq(District(None));
}

/// Pick the camera's zone; on a change fade from the zone on screen into the new one. Skate 2 zones
/// also fade the auto-exposure tuning, the world fog and the sky dome's multiplier and sun size.
fn follow(
    time: Res<Time>,
    mut grading: ResMut<Grading>,
    cameras: Query<&GlobalTransform, With<Camera3d>>,
    mut exposure: ResMut<super::exposure::Settings>,
    mut fog: ResMut<super::AuthoredFog>,
    skies: Query<&MeshMaterial3d<super::RetailSkyMaterial>>,
    mut sky_materials: ResMut<Assets<super::RetailSkyMaterial>>,
    mut district: ResMut<District>,
    post: Res<ShotPost>,
    fade: Res<ScreenFade>,
) {
    grading.post = *post;
    grading.fade = fade.0;
    exposure.hold = post.exposure;
    let Some(data) = grading.data.clone() else { return };
    if let Some(camera) = cameras.iter().next() {
        let zone = data.zone(camera.translation());
        if zone != grading.to {
            // ponytail: a change mid-fade restarts the cube from the incoming zone; the games' blend state
            // has a single from/to pair too. Skate 2's fog, sky and bloom restart from the value on screen,
            // and start settled on the zone the map loads in (its blenders' constructor 0x826a5d08 sets t = 1).
            let settled = data.lens.is_some() && grading.since == 0.;
            grading.preset_from = grading.preset(&data);
            grading.bloom_from = Some(grading.bloom(&data));
            grading.from = if settled { zone } else { grading.to };
            grading.to = zone;
            grading.blend = if settled { 1. } else { 0. };
            grading.since = if settled { f32::INFINITY } else { 0. };
        }
        district.set_if_neq(District(data.districts.get(zone).cloned().flatten()));
    }
    grading.blend = (grading.blend + time.delta_secs() / BLEND_SECONDS).min(1.);
    grading.since += time.delta_secs();
    if let Some((s1, ..)) = data.film.as_ref() {
        // Skate 1's VisualDirector (0x826f7618, ramp settled at 1, its second input's curve at 0): t = g16(speed),
        // eased by the smoothing weight each 1/60 s; the map starts settled. ponytail: the speed is a velocity's
        // length (0x826f7254), here the camera's, which follows the skater.
        let at = cameras.iter().next().map(GlobalTransform::translation);
        let dt = time.delta_secs();
        let speed = match (at, grading.camera_at) {
            (Some(a), Some(b)) if dt > 0. => a.distance(b) / dt,
            _ => 0.,
        };
        let target = skate1_curve(&s1.speed, speed);
        grading.film_t = match grading.camera_at {
            Some(_) => target + (grading.film_t - target) * s1.smoothing.powf(dt * 60.),
            None => target,
        };
        // ponytail: the engine's own cut test (Skate 1's is not traced; its MB rows are stored at 0x82742314): a jump of 5 m or
        // a 45 degree turn in one frame (teleports, camera switches) restarts the motion blur instead of streaking.
        let facing = cameras.iter().next().map(|c| *c.forward());
        grading.cut = match (at, grading.camera_at, facing, grading.camera_facing) {
            (Some(a), Some(b), Some(f), Some(g)) => a.distance(b) > 5. || f.dot(g) < std::f32::consts::FRAC_1_SQRT_2,
            _ => true,
        };
        grading.camera_at = at;
        grading.camera_facing = facing;
    }
    // Skate 2's maps meter exposure as Skate 2 does, Skate 1's as Skate 1 does (retail_exposure.wgsl).
    // The meters, fog and sky are the map's lighting, not a Settings > World effect: they always follow the map.
    exposure.skate2 = data.lens.is_some();
    exposure.skate1 = data.film.is_some();
    if let Some(e) = data.fixed_exposure {
        // The meter clamps to min..max: equal bounds hold it at e.
        exposure.tuning = Vec4::new(0., e, e, 0.);
    }
    // Skate 1's meter (0x8273ef80); SKATE_FIXED_EXPOSURE keeps its fixed tuning.
    if let Some((s1, ..)) = data.film.as_ref().filter(|_| exposure.tuning.x > 0.) {
        let at = cameras.iter().next().map(GlobalTransform::translation);
        let inside = at.is_some_and(|p| s1.volumes.iter().any(|v| skate1_inside(v, p)));
        let [target, min, max, rate] = s1.meter_volume.filter(|_| inside).unwrap_or(s1.meter);
        exposure.tuning = Vec4::new(target, min * 2.5, max * 2.5, rate);
    }

    let Some(preset) = grading.preset(&data) else { return };
    // SKATE_FIXED_EXPOSURE keeps its fixed (0, 2.5, 2.5, 0) tuning.
    if exposure.tuning.x > 0. {
        // Skate 2's meter (0x82325a48) is no scene meter: the CPU reads back byte 0 (alpha) of the 32x16 bloom
        // chain's slot from two frames ago, and that alpha is the down pass's constant 1 times the blur weights.
        // Every capture (Xenia, readback_resolve = "none": the read sees stale memory) sits at the zone's max
        // (Mega 1.8, Downtown 2.0, spot 1.7). Pinned there, min = max (INFERRED from the captures; real
        // hardware's readback would drive it to the min, which no footage shows). GRADING_AUDIT_S2.md 6.7.
        let e = preset.exposure;
        exposure.tuning = Vec4::new(e.x, e.z, e.z, e.w);
    }
    // Checked every frame: a scene spawn's sky resets AuthoredFog to the map's default fog. Written only when it
    // differs, so change detection doesn't fire every frame (perf 10-10 #7).
    fog.set_if_neq(super::AuthoredFog { ramp: preset.fog_ramp, colour: preset.fog_colour });
    let Some(id) = skies.iter().next().map(|m| m.id()) else { return };
    let base = match grading.sky {
        Some((sky, _, applied)) if sky == id && applied == preset.sky => return,
        Some((sky, base, _)) if sky == id => base,
        _ => match sky_materials.get(id) {
            Some(m) => Vec2::new(m.params.settings.z, m.params.settings.w),
            None => return,
        },
    };
    if let Some(m) = sky_materials.get_mut(id) {
        m.params.settings.z = base.x * preset.sky.y;
        m.params.settings.w = preset.sky.x;
    }
    grading.sky = Some((id, base, preset.sky));
}

/// Tone-pass group 1: bloom, the two cubes being blended, lens vignette, sampler, parameters, then Skate 1's
/// composite and depth-of-field targets (black elsewhere).
pub(super) fn tone_layout() -> BindGroupLayoutDescriptor {
    BindGroupLayoutDescriptor::new(
        "retail grading",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                texture_3d(TextureSampleType::Float { filterable: true }),
                texture_3d(TextureSampleType::Float { filterable: true }),
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                uniform_buffer::<[Vec4; PARAMS]>(false),
                texture_2d(TextureSampleType::Float { filterable: true }),
                texture_2d(TextureSampleType::Float { filterable: true }),
            ),
        ),
    )
}

/// Tone-pass parameters (retail_tone.wgsl `grade`), also the bloom passes'; [47] is Skate 3's shot bloom
/// ([`skate3_bloom`]): on, the bright pass's offset, level 1's horizontal gain.
const PARAMS: usize = 48;

/// 0x823a0df8: a PointNegGraphData's points (all x, then all y), linear between them and flat outside.
fn skate1_curve(c: &[f32], t: f32) -> f32 {
    let (x, y) = c.split_at(c.len() / 2);
    if t < x[0] {
        return y[0];
    }
    for i in 1..x.len() {
        if t < x[i] {
            return if x[i] <= x[i - 1] { y[i] } else { y[i - 1] + (y[i] - y[i - 1]) * (t - x[i - 1]) / (x[i] - x[i - 1]) };
        }
    }
    y[y.len() - 1]
}

/// 0x822bd6b8: is `at` inside a skate.ems volume? Equal extents are a sphere; otherwise the extents lie along forward,
/// up' = forward x side and side = Y x forward, and the point is inside below 1 (a zero extent drops its term).
fn skate1_inside(v: &[f32; 9], at: Vec3) -> bool {
    let d = at - Vec3::new(v[0], v[1], v[2]);
    let [ex, ey, ez] = [v[3], v[4], v[5]];
    if ex == ey && ey == ez {
        return d.length() < ex;
    }
    let f = Vec3::new(v[6], v[7], v[8]);
    let s = Vec3::Y.cross(f).normalize();
    let u = f.cross(s).normalize();
    let term = |a: f32, e: f32| if e > 0. { a * a / (e * e) } else { 0. };
    (term(f.dot(d), ex) + term(u.dot(d), ey) + term(s.dot(d), ez)).sqrt() < 1.
}

/// One grade set as 0x8273fc10 packs it, from its curves' values (offset, contrast, pivot, desaturation, saturate
/// intensity, saturate colour r/g/b, post modulate, post brightness): FxMatrix rows contrast x ((1 - s) I + s lum),
/// lum (.3, .6, .1), translation offset + pivot (1 - contrast); saturate colour and intensity; PostModulate and
/// PostBrightness over PostQuantise (the exe uploads both times PostQuantise and E8DC divides it back out).
fn skate1_pack([offset, contrast, pivot, s, intensity, r, g, b, modulate, bright]: [f32; 10]) -> [Vec4; 5] {
    let lum = Vec3::new(0.3, 0.6, 0.1);
    let row = |axis: Vec3| ((axis * (1. - s) + lum * s) * contrast).extend(offset + pivot * (1. - contrast));
    [row(Vec3::X), row(Vec3::Y), row(Vec3::Z), Vec4::new(r, g, b, intensity), Vec4::new(modulate, bright, 0., 0.)]
}

/// Skate 1 at intensity t, as its exe packs it each frame (0x82744be8): bloom threshold and multiply [2], final pass
/// [4] [5], far depth of field [6], far and near grade sets [7..12) [12..17), NIS distances and grunge radius /
/// feather [17], grunge centre and vignette multiplier [18], maxCoC ramp [19].
fn skate1_params(s1: &Skate1, t: f32, params: &mut [Vec4; PARAMS]) {
    let c = s1.curves.map(|c| skate1_curve(&c, t));
    let set = |curves: &[[f32; 8]; 11]| {
        let v = curves.map(|c| skate1_curve(&c, t));
        skate1_pack(std::array::from_fn(|i| v[i]))
    };
    params[2] = Vec4::new(c[6], 0., c[7], c[8]);
    params[4] = Vec4::new(s1.adjust[0], s1.adjust[1], s1.power[0], s1.power[1]);
    params[5] = Vec4::new(s1.upscale[0], s1.upscale[1], s1.warp[0], s1.warp[1]);
    params[6] = Vec3::from(s1.dof).extend(1.);
    params[7..12].copy_from_slice(&set(&s1.far));
    params[12..17].copy_from_slice(&set(&s1.near));
    params[17] = Vec4::new(c[4], c[5], c[0], 1. / c[1].max(1e-4));
    params[18] = Vec4::new(c[2], c[3], s1.vignette, 0.);
    params[19] = Vec4::from(s1.coc);
    // E8DC's DeckAO_* (0x8273c2bc..: this+0x4e0..0x4f0, set by 0x82740190) and g_MotionBlur_Intensity, g_bDOF;
    // [33].w (deck AO on) and the deck frame [24..28) are set in `upload`, the view matrices by retail_exposure.rs.
    let [x, z, radius, falloff, r, g, b] = s1.deck_ao.unwrap_or([0., 0., 0., 0., 1., 1., 1.]);
    params[32] = Vec4::new(x, z, radius, falloff);
    params[33] = Vec4::new(r, g, b, 0.);
    params[34] = s1.motion_blur.map_or(Vec4::ZERO, |[intensity, on]| Vec4::new(intensity, on, 0., 0.));
}

/// Skate 1's deck frame and skater stencil from the rendered skeleton (after transform propagation).
/// E8DC's deck AO works in the deck matrix's space (0x82740190 builds it each frame from a body matrix and the camera;
/// the body is not identified): x across, y up, z along the deck. INFERRED: the frame here is the board's from its
/// wheel bones, its origin on the wheels' contact plane (wheel centres less WheelRadius), so the AO's y < .04 test
/// reaches the ground but not the deck. Off the board (category 500) Skate 1's enable (0x82747c10) is not decoded;
/// the AO is off there. The stencil Skate 1's skater writes (s = saturate(2 stencil - 1)) is a proxy: capsules of
/// SKATER_RADIUS around the body's and the board's bones.
fn skate1_skater(
    mut grading: ResMut<Grading>,
    skater: Option<Res<crate::physics::SkaterRuntime>>,
    physics: Option<Res<crate::physics::GamePhysics>>,
    animation: Option<Res<crate::animation::AnimationStatus>>,
    nodes: Query<&GlobalTransform>,
) {
    const SKATER_RADIUS: f32 = 0.25; // ponytail: proxy for the stencil; a skater mask pass if it shows
    let film = grading.drawn().is_some_and(|d| d.film.is_some());
    let (Some(skater), Some(animation)) = (skater.filter(|_| film), animation.filter(|a| a.ready)) else {
        if grading.deck.is_some() || grading.skater != [Vec4::ZERO; 12] {
            (grading.deck, grading.skater) = (None, [Vec4::ZERO; 12]);
        }
        return;
    };
    let names = &skater.animation.evaluator.frames.bone_names;
    let mut at = std::collections::HashMap::new();
    for (entity, bone, _) in animation.online_bindings() {
        if let (Some(name), Ok(node)) = (names.get(bone), nodes.get(entity)) {
            at.insert(name.to_ascii_uppercase(), node.translation());
        }
    }
    let p = |n: &str| at.get(n).copied();
    let wheels = [p("LEFT_WHEELFRONT"), p("RIGHT_WHEELFRONT"), p("LEFT_WHEELBACK"), p("RIGHT_WHEELBACK")];
    let trucks = p("TRUCK_FRONT").zip(p("TRUCK_BACK")).map(|(a, b)| (a + b) / 2.);
    let on_board = skater.player_input.physical.state.category_12 != 500;
    grading.deck = match (wheels, trucks, physics) {
        ([Some(lf), Some(rf), Some(lb), Some(rb)], Some(trucks), Some(physics)) if on_board => {
            deck_frame([lf, rf, lb, rb], trucks, physics.wheel_radius())
        }
        _ => None,
    };
    let (f, b) = (wheels[0].zip(wheels[1]).map(|(l, r)| (l + r) / 2.), wheels[2].zip(wheels[3]).map(|(l, r)| (l + r) / 2.));
    let segments = [
        (p("HIPS"), p("HEAD")),
        (p("HIPS"), p("LEFTFOOT")),
        (p("HIPS"), p("RIGHTFOOT")),
        (p("SPINE2"), p("LEFTHAND")),
        (p("SPINE2"), p("RIGHTHAND")),
        (f, b),
    ];
    for (i, (a, b)) in segments.into_iter().enumerate() {
        (grading.skater[2 * i], grading.skater[2 * i + 1]) = match a.zip(b) {
            Some((a, b)) => (a.extend(SKATER_RADIUS), b.extend(0.)),
            None => (Vec4::ZERO, Vec4::ZERO),
        };
    }
}

/// deck_from_world for wheel centres (left front, right front, left back, right back), the trucks' midpoint (which
/// side is up) and the wheel radius: z toward the front axle, y away from the wheels toward the trucks, x = y cross z,
/// origin the wheels' midpoint less y times the radius.
fn deck_frame([lf, rf, lb, rb]: [Vec3; 4], trucks: Vec3, radius: f32) -> Option<Mat4> {
    let mid = (lf + rf + lb + rb) / 4.;
    let z = ((lf + rf) - (lb + rb)).try_normalize()?;
    let mut y = z.cross((lf + lb) - (rf + rb)).try_normalize()?;
    if y.dot(trucks - mid) < 0. {
        y = -y;
    }
    let x = y.cross(z);
    let world = Mat4::from_cols(x.extend(0.), y.extend(0.), z.extend(0.), (mid - y * radius).extend(1.));
    Some(world.inverse()).filter(|m| m.is_finite())
}

const PASSES: [&str; 7] = ["bright", "down", "blur_h0", "blur_h1", "blur_v1", "combine", "ghosts"];

#[derive(Resource)]
pub(super) struct Gpu {
    tone_layout: BindGroupLayoutDescriptor,
    pass_layout: BindGroupLayoutDescriptor,
    passes: [CachedRenderPipelineId; 7],
    /// passes[0] (bright) for multisampled depth.
    bright_msaa: CachedRenderPipelineId,
    sampler: Sampler,
    params: Buffer,
    black: TextureView,
    white: TextureView,
    empty_cube: TextureView,
    // (the loaded Data, its cube views, its vignette)
    cubes: Mutex<(Option<Arc<Data>>, Vec<TextureView>, Option<TextureView>)>,
    // Bloom levels: L0a, L0b at 1/4 of the view, L1a, L1b at 1/16.
    targets: Mutex<Option<(UVec2, Vec<TextureView>)>>,
    // (cube from, to, bloom, Skate 1's blur chain, Skate 1's film passes, camera cut)
    frame: Mutex<(usize, usize, bool, bool, bool, bool)>,
    // The bloom passes' groups by scene source view (ping-pong) at the targets' size, as retail_exposure.rs's bindings.
    bloom_groups: Mutex<Vec<(TextureViewId, UVec2, Vec<BindGroup>)>>,
    // perf 10-19: the tone pass's groups 1 ([`Self::group`]) by their six texture views; built each frame before.
    tone_groups: Mutex<Vec<([TextureViewId; 6], BindGroup)>>,
    // Skate 1's composite (E8DC, full size) and its two half-size depth-of-field targets.
    films: Mutex<Option<(UVec2, [TextureView; 3])>>,
    // Skate 1's motion blur: last frame's clip_from_world.
    previous: Mutex<Option<Mat4>>,
}

fn initialize(
    mut commands: Commands,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    cache: Res<PipelineCache>,
    assets: Res<AssetServer>,
    fullscreen: Res<FullscreenShader>,
) {
    let pass_layout = BindGroupLayoutDescriptor::new(
        "retail bloom",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                storage_buffer_read_only::<Vec4>(false),
                uniform_buffer::<[Vec4; 3]>(false),
                uniform_buffer::<[Vec4; PARAMS]>(false),
            ),
        ),
    );
    let shader = assets.load(bevy::asset::AssetPath::from(bevy::asset::embedded_path!("retail_grading.wgsl")).with_source("embedded"));
    // The bright pass also reads the scene depth (group 1, Skate 1's sky alpha), multisampled with MSAA.
    let pipeline = |entry: &str, depth: Option<bool>| {
        cache.queue_render_pipeline(RenderPipelineDescriptor {
            label: Some(format!("retail bloom {entry}").into()),
            layout: [Some(pass_layout.clone()), depth.map(super::exposure::depth_layout)].into_iter().flatten().collect(),
            vertex: fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: shader.clone(),
                shader_defs: if depth == Some(true) { vec!["DEPTH_MSAA".into()] } else { vec![] },
                entry_point: Some(entry.to_owned().into()),
                targets: vec![Some(ColorTargetState {
                    // Skate 2's bloom targets are 8-bit: every pass clamps to 0..1.
                    format: TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
                ..default()
            }),
            ..default()
        })
    };
    let passes = PASSES.map(|entry| pipeline(entry, (entry == "bright").then_some(false)));
    let bright_msaa = pipeline("bright", Some(true));
    let texture = |label, dimension, size: u32, texel: [u8; 4]| {
        device
            .create_texture_with_data(
                &queue,
                &TextureDescriptor {
                    label: Some(label),
                    size: Extent3d { width: 1, height: 1, depth_or_array_layers: size },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension,
                    format: TextureFormat::Rgba8Unorm,
                    usage: TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                TextureDataOrder::LayerMajor,
                &texel,
            )
            .create_view(&TextureViewDescriptor::default())
    };
    commands.insert_resource(Gpu {
        tone_layout: tone_layout(),
        pass_layout,
        passes,
        bright_msaa,
        sampler: device.create_sampler(&SamplerDescriptor {
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..default()
        }),
        params: device.create_buffer(&BufferDescriptor {
            label: Some("retail grading params"),
            size: (PARAMS * 16) as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }),
        black: texture("retail no bloom", TextureDimension::D2, 1, [0, 0, 0, 255]),
        white: texture("retail no vignette", TextureDimension::D2, 1, [255; 4]),
        empty_cube: texture("retail no cube", TextureDimension::D3, 1, [0, 0, 0, 255]),
        cubes: default(),
        targets: default(),
        frame: default(),
        bloom_groups: default(),
        tone_groups: default(),
        films: default(),
        previous: default(),
    });
}

/// Not retail: Skate 2's bloom levels at this share of the recovered values, whose glow washed the maps out.
const S2_BLOOM_GAIN: f32 = 0.5;
/// A zone's bright pass [threshold, scale, level 0, level 1], Skate 2's levels at [`S2_BLOOM_GAIN`].
fn zone_bloom(bloom: &Bloom, skate2: bool) -> Vec4 {
    let gain = if skate2 { S2_BLOOM_GAIN } else { 1. };
    Vec4::new(bloom.threshold, bloom.scale, bloom.level0 * gain, bloom.level1 * gain)
}

/// The bright pass's [threshold, scale, level 0, level 1]: a shot's bloom (0x8259f460 -> 0x82353364) is 0.5 x
/// BLOOM_THRESHOLD less twice the screen fade, scaled by 1 / (1 - threshold) (0x82374598). The intensity multiplies
/// several of the pass's constants and is taken as a gain on both levels. A shot without BLOOM_OVERRIDE is baked as
/// (0, 0); a threshold at or below 0 is a real key (Own the Spot intros flash white at -0.2).
fn shot_bloom(zone: Vec4, shot: Vec2, fade: f32) -> Vec4 {
    let mut p = zone;
    if shot != Vec2::ZERO {
        let (threshold, gain) = ((0.5 * shot.x).min(0.99), shot.y);
        p = Vec4::new(threshold, 1. / (1. - threshold), zone.z * gain, zone.w * gain);
    }
    if fade > 0. {
        // ponytail: the zones' threshold gets the same fade term; the frame setup applies it to whichever is live.
        p.x -= 2. * fade;
        p.y = 1. / (1. - p.x);
    }
    p
}

/// Skate 3's ICE-path bloom (0x827f3d20, a NIS camera's post state): t' = 0.5 x BLOOM_THRESHOLD - 2 x the screen fade,
/// the record's 1.4 / 0.5 without BLOOM_OVERRIDE (0x82678780). The setter (0x827ef1f0) takes d = 1 - t', 1 above 1 and
/// 10 below 0.1, scale 1 / d, and the bright pass adds max(-t', 0) to every pixel (bloom_dof_tap4_minusthreshold), so a
/// threshold at or below 0 whites the frame out. f_BloomMultiply = (4I, 4I(1 - k), 0, 0), k 0: the combine's level
/// weights and level 1's horizontal blur gain (0x827efd98's b2); level 2 is x 0. Returns params [2] and [47]
/// (.superpowers/sdd/2026-10-09-bug-list/item10-flashes.md, part A).
/// ponytail: the ICE path only, under a shot's override or a bloom fade; Skate 3's every-frame vault bloom
/// (bloom_default etc., 0x827f5450) isn't ported, nor its ghosts, and its targets' sizes aren't traced (Skate 2's).
fn skate3_bloom(shot: Vec2, fade: f32) -> (Vec4, Vec4) {
    let (threshold, intensity) = if shot != Vec2::ZERO { (shot.x, shot.y) } else { (1.4, 0.5) };
    let t = 0.5 * threshold - 2. * fade;
    let d = 1. - t;
    let d = if d > 1. { 1. } else if d < 0.1 { 10. } else { d };
    let gain = 4. * intensity;
    (Vec4::new(t, 1. / d, gain, gain), Vec4::new(1., (-t).max(0.), gain, 0.))
}

/// Skate 3's scene lens ([1]: lookup offset unused, lens flag, centre scale, r^4 scale): entering a ViewScene
/// (0x82836AB8, its +0xA5 set by both scene classes) raises the render-settings override (world+0x7E00), so the postfx
/// flags (0x827F4D28) carry 0x20 and 0x827F2828 keeps the postfx row 316A03931530B5E6's f_Fisheye_Warp (0.72, 0.9)
/// instead of the identity: uv' = .5 + (uv - .5)(0.9 + 0.72 r^4) while a scene's camera drives the view, none in play.
/// The take's FISHEYE (1 throughout) scales the r^4 term, as Skate 2's FISHEYE_MULTIPLIER. The flag is 0.25: the warp
/// without Skate 2's vignette, which Skate 3 maps have no gradient for. ponytail: the enter call's caller, whose 4th
/// argument is the flag, isn't traced for intros.
fn skate3_lens(post: &ShotPost) -> Vec4 {
    if post.scene { Vec4::new(0., 0.25, 0.9, 0.72 * post.lens) } else { Vec4::ZERO }
}

/// Whether Skate 3's shot bloom runs: on a map without Skate 1/2 grading, while a shot overrides the bloom or a
/// bloom transition holds the screen fade.
fn skate3_bloom_on(graded: bool, shot: Vec2, fade: f32) -> bool {
    !graded && (shot != Vec2::ZERO || fade > 0.)
}

/// Settings > World's effects (graphics_menu::EFFECTS bits): [3].z draws the colour cubes and Skate 1's grade sets,
/// [3].w adds the bloom (its passes are skipped when off, [`passes`]). Lens & Vignette clears Skate 2's
/// lens flag (fisheye and vignette share it) and Skate 1's warp, grunge and vignette; Depth of Field Skate 1's far
/// lerp scale; Motion Blur its enable.
fn gate_effects(params: &mut [Vec4; PARAMS], bits: u8) {
    let on = |row: u8| bits & 1 << row != 0;
    params[3].z = on(0) as u32 as f32;
    params[3].w = on(1) as u32 as f32;
    if !on(2) {
        params[1].y = 0.;
        (params[5].z, params[5].w) = (0., 0.);
        params[17].w = 0.;
        params[18].z = 0.;
    }
    if !on(3) {
        params[6].z = 0.;
    }
    if !on(4) {
        params[34].y = 0.;
    }
}

/// Which optional pass chains run (perf 10-10 #4): (bloom, Skate 1's depth of field/motion blur). With Bloom off the
/// tone and E8DC passes multiply the bloom by [3].w = 0, so its 7 passes are skipped and black is bound; Skate 1's meter
/// recomputes the bright pass's alpha from the scene (retail_exposure.wgsl), so it needs none of them. With Depth of
/// Field and Motion Blur off E8DC's alpha, max(far, blur), is 0 and the final pass's mix takes the composite alone, so
/// 66ED/3D17/9B58 are skipped. E8DC itself is the picture and always runs; Lens is the final pass's.
fn passes(bloom: bool, bits: u8) -> (bool, bool) {
    (bloom && bits & 0b10 != 0, bits & 0b1_1000 != 0)
}

fn upload(grading: Res<Grading>, gpu: Res<Gpu>, device: Res<RenderDevice>, queue: Res<RenderQueue>) {
    let mut cubes = gpu.cubes.lock().unwrap();
    let same = match (&cubes.0, grading.drawn()) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    };
    if !same {
        let size = (CUBE.pow(3) * 4) as usize;
        let views = grading.drawn().into_iter().flat_map(|d| d.cubes.chunks_exact(size)).map(|bytes| {
            device
                .create_texture_with_data(
                    &queue,
                    &TextureDescriptor {
                        label: Some("retail colour cube"),
                        size: Extent3d { width: CUBE, height: CUBE, depth_or_array_layers: CUBE },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: TextureDimension::D3,
                        format: TextureFormat::Rgba8Unorm,
                        usage: TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    },
                    TextureDataOrder::LayerMajor,
                    bytes,
                )
                .create_view(&TextureViewDescriptor::default())
        });
        // The lens slot holds Skate 2's vignette gradient or Skate 1's grunge texture (a map has one or neither).
        let lens = grading.drawn().and_then(|d| {
            d.lens.as_ref().map(|(_, rgba)| (rgba, UVec2::new(VIGNETTE as u32, 1))).or(d.film.as_ref().map(|f| (&f.1, f.2)))
        });
        let vignette = lens.map(|(rgba, size)| {
            device
                .create_texture_with_data(
                    &queue,
                    &TextureDescriptor {
                        label: Some("retail lens vignette"),
                        size: Extent3d { width: size.x, height: size.y, depth_or_array_layers: 1 },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: TextureDimension::D2,
                        format: TextureFormat::Rgba8Unorm,
                        usage: TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    },
                    TextureDataOrder::LayerMajor,
                    rgba,
                )
                .create_view(&TextureViewDescriptor::default())
        });
        *cubes = (grading.drawn().cloned(), views.collect(), vignette);
        gpu.tone_groups.lock().unwrap().clear(); // perf 10-19: they hold the old views
    }
    let mut params = [Vec4::ZERO; PARAMS];
    let mut frame = (0, 0, false, false, false, true);
    if let Some(data) = grading.drawn() {
        let (from, to) = (data.zones[grading.from.min(data.zones.len() - 1)], data.zones[grading.to.min(data.zones.len() - 1)]);
        let t = grading.blend;
        let bloom = grading.bloom(data);
        params[0] = Vec4::new(1., data.bloom as u32 as f32, t, data.lookup.x);
        let lens = data.lens.as_ref().map_or(Vec2::ZERO, |(warp, _)| *warp);
        // ponytail: the multiplier scales the lens's strength settings (0x8259ffd0); taken as linear in the r^4
        // term, as their combination into g_Fisheye_Warp isn't traced. The centre scale is left alone, as the exe does.
        params[1] = Vec4::new(data.lookup.y, data.lens.is_some() as u32 as f32, lens.x, lens.y * grading.post.lens);
        params[2] = shot_bloom(zone_bloom(&bloom, data.lens.is_some()), grading.post.bloom, grading.fade);
        let film = data.film.as_ref();
        params[3] = Vec4::new(bloom.saturation, film.is_some() as u32 as f32, 0., 0.);
        if let Some((s1, ..)) = film {
            skate1_params(s1, grading.film_t, &mut params);
            if let (Some(deck), Some(_)) = (grading.deck, s1.deck_ao) {
                params[24..28].copy_from_slice(&deck.to_cols_array_2d().map(Vec4::from));
                params[33].w = 1.;
            }
            params[35..47].copy_from_slice(&grading.skater);
        }
        let bits = crate::graphics_menu::EFFECT_BITS.load(std::sync::atomic::Ordering::Relaxed);
        let (bloom, blur) = passes(data.bloom, bits);
        frame = (from.0, to.0, bloom, blur, film.is_some(), grading.cut);
        gate_effects(&mut params, bits);
    } else if skate3_bloom_on(false, grading.post.bloom, grading.fade) {
        // The old games' final pass adds it (scene x 1.41 + bloom before the cube, postfx_fast_uber); no cube or lens.
        (params[2], params[47]) = skate3_bloom(grading.post.bloom, grading.fade);
        params[0].x = 1.;
        params[3].x = 1.; // the ICE path's saturation 1: the bright pass's matrix is the identity
        params[1] = skate3_lens(&grading.post);
        let bits = crate::graphics_menu::EFFECT_BITS.load(std::sync::atomic::Ordering::Relaxed);
        gate_effects(&mut params, bits);
        params[3].z = 0.;
        frame = (0, 0, passes(true, bits).0, false, false, true);
    } else if grading.post.scene {
        // A scene on a map without Skate 1/2 grading: only its lens, no cube or bloom.
        params[1] = skate3_lens(&grading.post);
        gate_effects(&mut params, crate::graphics_menu::EFFECT_BITS.load(std::sync::atomic::Ordering::Relaxed));
    }
    *gpu.frame.lock().unwrap() = frame;
    let bytes: Vec<u8> = params.iter().flat_map(|v| v.to_array()).flat_map(f32::to_le_bytes).collect();
    queue.write_buffer(&gpu.params, 0, &bytes);
}

impl Gpu {
    /// This frame draws Skate 1's film (grade[3].y): its passes read the scene depth.
    pub(super) fn film(&self) -> bool {
        self.frame.lock().unwrap().4
    }
    /// Run the bloom passes for this view (when the map has bloom and it's on, [`passes`]) and return the bloom for
    /// [`Self::group`].
    /// `depth`: the tone pass's depth group and whether it is multisampled (the bright pass's group 1).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn frame(
        &self,
        context: &RenderContext,
        encoder: &mut CommandEncoder,
        cache: &PipelineCache,
        source: &TextureView,
        size: UVec2,
        exposure: &Buffer,
        settings: &Buffer,
        depth: (&BindGroup, bool),
    ) -> TextureView {
        let (_, _, bloom, ..) = *self.frame.lock().unwrap();
        let mut pipelines = self.passes.map(|id| cache.get_render_pipeline(id));
        if depth.1 {
            pipelines[0] = cache.get_render_pipeline(self.bright_msaa);
        }
        let mut result = self.black.clone();
        if bloom && pipelines.iter().all(Option::is_some) {
            let views = self.targets(context.render_device(), size);
            let [l0a, l0b, l1a, l1b] = [&views[0], &views[1], &views[2], &views[3]];
            // (pass, input, second input, output)
            let steps = [
                (0, source, source, l0a),
                (1, l0a, l0a, l1a),
                (2, l0a, l0a, l0b),
                (3, l1a, l1a, l1b),
                (4, l1b, l1b, l1a),
                (5, l0b, l1a, l0a),
                (6, l0a, l0a, l0b),
            ];
            let mut cached = self.bloom_groups.lock().unwrap();
            cached.retain(|(_, s, _)| *s == size);
            let index = match cached.iter().position(|(id, ..)| *id == source.id()) {
                Some(index) => index,
                None => {
                    let layout = cache.get_bind_group_layout(&self.pass_layout);
                    let groups = steps.iter().map(|(_, input, second, _)| {
                        context.render_device().create_bind_group(
                            "retail bloom",
                            &layout,
                            &BindGroupEntries::sequential((
                                *input,
                                *second,
                                &self.sampler,
                                exposure.as_entire_binding(),
                                settings.as_entire_binding(),
                                self.params.as_entire_binding(),
                            )),
                        )
                    });
                    let groups = groups.collect();
                    if cached.len() == 4 {
                        cached.remove(0);
                    }
                    cached.push((source.id(), size, groups));
                    cached.len() - 1
                }
            };
            // perf 11-01: timed in --trace-gpu captures like the passes around them.
            use bevy::render::diagnostic::RecordDiagnostics;
            let diagnostics = context.diagnostic_recorder();
            for ((pass, _, _, output), group) in steps.into_iter().zip(&cached[index].2) {
                let mut render = super::exposure::pass(context, encoder, RenderPassDescriptor {
                    label: Some("retail bloom"),
                    color_attachments: &[Some(RenderPassColorAttachment {
                        view: output,
                        depth_slice: None,
                        resolve_target: None,
                        ops: Operations::default(),
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                let span = diagnostics.pass_span(&mut render, "retail_bloom");
                render.set_render_pipeline(pipelines[pass].unwrap());
                render.set_bind_group(0, group, &[]);
                if pass == 0 {
                    render.set_bind_group(1, depth.0, &[]);
                }
                render.draw(0..3, 0..1);
                span.end(&mut render);
            }
            result = l0b.clone();
        }
        result
    }

    /// The tone pass's group 1, with Skate 1's composite and depth-of-field targets when given.
    pub(super) fn group(&self, device: &RenderDevice, cache: &PipelineCache, bloom: &TextureView, film: Option<(&TextureView, &TextureView)>) -> BindGroup {
        let (from, to, ..) = *self.frame.lock().unwrap();
        let cubes = self.cubes.lock().unwrap();
        let cube = |i: usize| cubes.1.get(i).unwrap_or(&self.empty_cube);
        let (film, dof) = film.unwrap_or((&self.black, &self.black));
        let vignette = cubes.2.as_ref().unwrap_or(&self.white);
        let create = || {
            device.create_bind_group(
                "retail grading",
                &cache.get_bind_group_layout(&self.tone_layout),
                &BindGroupEntries::sequential((
                    bloom,
                    cube(from),
                    cube(to),
                    vignette,
                    &self.sampler,
                    self.params.as_entire_binding(),
                    film,
                    dof,
                )),
            )
        };
        // perf 10-19: the sampler and parameter buffer never change, so equal views are an equal group. Eight hold
        // Skate 1's five a frame; replaced cubes and resized targets age out. SKATE_POST_GROUP_CACHE=0: every call.
        if !crate::config::perf_on("SKATE_POST_GROUP_CACHE") {
            return create();
        }
        let key = [bloom, cube(from), cube(to), vignette, film, dof].map(TextureView::id);
        let mut groups = self.tone_groups.lock().unwrap();
        if let Some((_, group)) = groups.iter().find(|(k, _)| *k == key) {
            return group.clone();
        }
        if groups.len() == 8 {
            groups.remove(0);
        }
        let group = create();
        groups.push((key, group.clone()));
        group
    }

    /// On Skate 1's map, its film passes before the final one (`passes`: the tone shader's skate1_film, skate1_down,
    /// skate1_blur_h, skate1_blur_v for this view's MSAA; `groups`: the tone pass's groups 0 and 2): E8DC's composite
    /// with deck AO, its alpha the depth of field or motion blur amount, into a full-size 8-bit target, then 66ED, 3D17
    /// and 9B58 at half size (Skate 1's +0xf4/+0xf8 targets are one half-size surface, 0x8273ba38). Returns the final
    /// pass's group 1; None elsewhere.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn skate1(
        &self,
        context: &RenderContext,
        encoder: &mut CommandEncoder,
        cache: &PipelineCache,
        queue: &RenderQueue,
        bloom: &TextureView,
        size: UVec2,
        passes: [&RenderPipeline; 4],
        groups: (&BindGroup, &BindGroup),
        clip_from_world: Mat4,
    ) -> Option<BindGroup> {
        let (.., blur, film, cut) = *self.frame.lock().unwrap();
        if !film {
            return None;
        }
        // E8DC/3D17's g_MotionBlurMatrix maps this frame's pixels to last frame's; a cut starts over.
        let previous = self.previous.lock().unwrap().replace(clip_from_world).filter(|_| !cut).unwrap_or(clip_from_world);
        let bytes = |m: Mat4| m.to_cols_array().into_iter().flat_map(f32::to_le_bytes).collect::<Vec<u8>>();
        queue.write_buffer(&self.params, 20 * 16, &bytes(clip_from_world.inverse()));
        queue.write_buffer(&self.params, 28 * 16, &bytes(previous));
        let [full, a, b] = self.films(context.render_device(), size);
        let black = &self.black;
        // (pass, composite, depth-of-field input, output)
        let steps = [(0, black, black, &full), (1, &full, black, &a), (2, &full, &a, &b), (3, &full, &b, &a)];
        use bevy::render::diagnostic::RecordDiagnostics;
        let diagnostics = context.diagnostic_recorder();
        for (pass, film, dof, output) in steps.into_iter().take(if blur { 4 } else { 1 }) {
            let group = self.group(context.render_device(), cache, bloom, Some((film, dof)));
            let mut render = super::exposure::pass(context, encoder, RenderPassDescriptor {
                label: Some("retail skate 1 film"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: output,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations::default(),
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            let span = diagnostics.pass_span(&mut render, "retail_skate1_film");
            render.set_render_pipeline(passes[pass]);
            render.set_bind_group(0, groups.0, &[]);
            render.set_bind_group(1, &group, &[]);
            render.set_bind_group(2, groups.1, &[]);
            render.draw(0..3, 0..1);
            span.end(&mut render);
        }
        Some(self.group(context.render_device(), cache, bloom, Some((&full, if blur { &a } else { black }))))
    }

    fn films(&self, device: &RenderDevice, size: UVec2) -> [TextureView; 3] {
        let mut films = self.films.lock().unwrap();
        if films.as_ref().is_none_or(|(s, _)| *s != size) {
            let half = (size / 2).max(UVec2::ONE);
            let views = [size, half, half].map(|s| {
                device
                    .create_texture(&TextureDescriptor {
                        label: Some("retail skate 1 film"),
                        size: Extent3d { width: s.x, height: s.y, depth_or_array_layers: 1 },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: TextureDimension::D2,
                        format: TextureFormat::Rgba8Unorm,
                        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::RENDER_ATTACHMENT,
                        view_formats: &[],
                    })
                    .create_view(&TextureViewDescriptor::default())
            });
            *films = Some((size, views));
            self.tone_groups.lock().unwrap().clear(); // perf 10-19: they hold the old targets
        }
        films.as_ref().unwrap().1.clone()
    }

    fn targets(&self, device: &RenderDevice, size: UVec2) -> Vec<TextureView> {
        let mut targets = self.targets.lock().unwrap();
        if targets.as_ref().is_none_or(|(s, _)| *s != size) {
            let l0 = (size / 4).max(UVec2::ONE);
            let l1 = (l0 / 4).max(UVec2::ONE);
            let views = [l0, l0, l1, l1]
                .map(|s| {
                    device
                        .create_texture(&TextureDescriptor {
                            label: Some("retail bloom level"),
                            size: Extent3d { width: s.x, height: s.y, depth_or_array_layers: 1 },
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: TextureDimension::D2,
                            format: TextureFormat::Rgba8Unorm,
                            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::RENDER_ATTACHMENT,
                            view_formats: &[],
                        })
                        .create_view(&TextureViewDescriptor::default())
                })
                .to_vec();
            *targets = Some((size, views));
            self.tone_groups.lock().unwrap().clear(); // perf 10-19: they hold the old targets
        }
        targets.as_ref().unwrap().1.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// User 10-14, feature #9: any game's look on any map; the map's own game is its own file.
    #[test]
    fn each_games_look_reads_its_own_grading() {
        let s = |stem, look| source(stem, look).map(|(n, b)| (n, b));
        assert_eq!(s("Skate_2_Maloof", None), Some(("Skate_2_Maloof".into(), false)));
        assert_eq!(s("Skate_2_Maloof", Some(2)), Some(("Skate_2_Maloof".into(), false)));
        assert_eq!(s("New_San_Vanelona", Some(3)), None, "Skate 3's look draws no grading");
        assert_eq!(s("Downtown", Some(3)), Some(("Downtown".into(), false)), "a Skate 3 map has no file: nothing drawn");
        assert_eq!(s("Downtown", Some(2)), Some(("Skate_2_Center".into(), true)));
        assert_eq!(s("New_San_Vanelona", Some(1)), Some(("Skate_1_San_Vanelona".into(), true)));
    }

    #[test]
    fn skate3_shot_bloom_is_the_ice_path_setter() {
        let close = |a: Vec4, b: Vec4| assert!((a - b).abs().max_element() < 1e-5, "{a} != {b}");
        // A cut's flash (BLOOM_THRESHOLD -0.2): t' = -0.1, d = 1.1 clamps to 1, every pixel passes plus 0.1.
        let (bright, s3) = skate3_bloom(Vec2::new(-0.2, 1.), 0.);
        close(bright, Vec4::new(-0.1, 1., 4., 4.));
        close(s3, Vec4::new(1., 0.1, 4., 0.));
        // Between cuts (1.4): t' = 0.7, scale 1 / 0.3.
        close(skate3_bloom(Vec2::new(1.4, 1.), 0.).0, Vec4::new(0.7, 1. / 0.3, 4., 4.));
        // Past 1.8 (2.6): d = -0.3 < 0.1 takes d = 10, scale 0.1 (the literal, not 1 / d).
        close(skate3_bloom(Vec2::new(2.6, 1.), 0.).0, Vec4::new(1.3, 0.1, 4., 4.));
        // No BLOOM_OVERRIDE: the record's 1.4 / 0.5.
        close(skate3_bloom(Vec2::ZERO, 0.).0, Vec4::new(0.7, 1. / 0.3, 2., 2.));
        // The screen fade takes twice its level off the threshold.
        let (bright, s3) = skate3_bloom(Vec2::new(-0.2, 1.), 0.25);
        close(bright, Vec4::new(-0.6, 1., 4., 4.));
        assert!((s3.y - 0.6).abs() < 1e-6);
        // Only a Skate 3 map (no Skate 1/2 grading) under a shot's override or a bloom fade runs it.
        assert!(skate3_bloom_on(false, Vec2::new(1.4, 1.), 0.));
        assert!(skate3_bloom_on(false, Vec2::ZERO, 0.5));
        assert!(!skate3_bloom_on(false, Vec2::ZERO, 0.), "gameplay: no Skate 3 world bloom yet");
        assert!(!skate3_bloom_on(true, Vec2::new(1.4, 1.), 0.), "a Skate 1/2 map draws its own");
    }

    /// v12: Skate 3's lens only while a scene's camera drives the view (the take's FISHEYE is 1 in and out of scenes),
    /// under Skate 2's vignette threshold, and off with Lens & Vignette.
    #[test]
    fn skate3_scenes_warp_through_retails_lens_and_play_does_not() {
        assert_eq!(skate3_lens(&ShotPost::default()), Vec4::ZERO, "gameplay: no lens");
        let lens = skate3_lens(&ShotPost { scene: true, ..default() });
        assert_eq!((lens.z, lens.w), (0.9, 0.72));
        assert!(lens.y > 0. && lens.y < 0.5, "the warp's flag, not the vignette's: {}", lens.y);
        let mut gated = [Vec4::ZERO; PARAMS];
        gated[1] = lens;
        gate_effects(&mut gated, 0);
        assert_eq!(gated[1].y, 0., "Lens & Vignette off takes it away");
    }

    #[test]
    fn a_shot_bloom_at_or_below_zero_is_the_flash_not_no_override() {
        let zone = Vec4::new(0.4, 1. / 0.6, 0.5, 0.25);
        // Own the Spot intros key BLOOM_THRESHOLD down to -0.2 at each cut: the bright pass lets the whole frame through.
        let flash = shot_bloom(zone, Vec2::new(-0.2, 1.), 0.);
        assert_eq!(flash.x, -0.1);
        assert!((flash.y - 1. / 1.1).abs() < 1e-6);
        assert_eq!(shot_bloom(zone, Vec2::new(0., 1.), 0.).x, 0., "threshold 0 is an override too");
        assert_eq!(shot_bloom(zone, Vec2::ZERO, 0.), zone, "no BLOOM_OVERRIDE (both columns 0): the zone's");
        let held = shot_bloom(zone, Vec2::new(2.6, 0.5), 0.);
        assert_eq!((held.x, held.z, held.w), (0.99, 0.25, 0.125), "half the key, capped; the intensity a gain on both levels");
        assert_eq!(shot_bloom(zone, Vec2::ZERO, 0.25).x, 0.4 - 0.5, "the screen fade lowers whichever is live");
    }

    #[test]
    fn each_world_effect_clears_only_its_own_params() {
        let full = || {
            let mut p = [Vec4::ONE; PARAMS];
            p[3] = Vec4::new(0.7, 1., 0., 0.);
            p
        };
        let mut all = full();
        gate_effects(&mut all, 0b1_1111);
        assert_eq!((all[3].z, all[3].w), (1., 1.), "grading and bloom drawn");
        assert_eq!((all[1].y, all[6].z, all[34].y, all[3].y, all[3].x), (1., 1., 1., 1., 0.7), "nothing else touched");
        let mut none = full();
        gate_effects(&mut none, 0);
        assert_eq!((none[3].z, none[3].w), (0., 0.));
        assert_eq!([none[1].y, none[5].z, none[5].w, none[17].w, none[18].z, none[6].z, none[34].y], [0.; 7]);
        assert_eq!((none[3].y, none[3].x, none[2], none[33]), (1., 0.7, Vec4::ONE, Vec4::ONE), "S1 path, saturation, bloom curve, deck AO stay");
        let mut no_blur = full();
        gate_effects(&mut no_blur, 0b0_1111);
        assert_eq!((no_blur[34].y, no_blur[6].z, no_blur[1].y), (0., 1., 1.));
    }

    #[test]
    fn effect_bits_skip_only_pass_chains_nothing_reads() {
        // Defaults draw everything (perf 10-10 #4: the default look is unchanged).
        assert_eq!(passes(true, 0b1_1111), (true, true));
        assert_eq!(passes(false, 0b1_1111), (false, true), "a map without bloom never ran its chain");
        assert_eq!(passes(true, 0b1_1101), (false, true), "Bloom off: its 7 passes skip, on Skate 1 too");
        assert_eq!(passes(true, 0b0_0111), (true, false), "DoF and Motion Blur off: Skate 1's blur chain skips");
        assert_eq!(passes(true, 0b0_1111), (true, true), "Depth of Field alone keeps it");
        assert_eq!(passes(true, 0b1_0111), (true, true), "Motion Blur alone keeps it");
        assert_eq!(passes(true, 0b1_1011), (true, true), "Lens never reaches the blur chain");
        // The chain is skippable because its only reader mixes by the composite's alpha, max(far, blur), which these
        // two params zero.
        let mut p = [Vec4::ONE; PARAMS];
        gate_effects(&mut p, 0b0_0111);
        assert_eq!((p[6].z, p[34].y), (0., 0.));
    }

    #[test]
    fn zone_lookup_reads_the_cell_under_the_camera_and_clamps_outside() {
        let grid = Grid { x0: -10., z0: 100., cell: 8., width: 3, height: 2 };
        let data = Data {
            cubes: vec![],
            lookup: Vec2::ONE,
            bloom: false,
            zones: vec![(0, Bloom::default()); 3],
            presets: vec![],
            districts: vec![],
            grid: Some((grid, vec![0, 1, 2, 2, 1, 0])),
            lens: None,
            film: None,
            fixed_exposure: None,
        };
        assert_eq!(data.zone(Vec3::new(-9., 0., 101.)), 0);
        assert_eq!(data.zone(Vec3::new(7., 50., 101.)), 2);
        assert_eq!(data.zone(Vec3::new(-2., 0., 109.)), 1);
        assert_eq!(data.zone(Vec3::new(1000., 0., 1000.)), 0);
        assert_eq!(data.zone(Vec3::new(-1000., 0., -1000.)), 0);
    }

    #[test]
    fn skate2_blends_fog_over_5s_bloom_over_2s_and_switches_exposure_at_once() {
        let bloom = |v: f32| Bloom { threshold: v, scale: 1. / (1. - v), level0: v, level1: v, saturation: v };
        let preset = |v: f32| Preset { exposure: Vec4::splat(v), fog_ramp: Vec4::splat(v), fog_colour: Vec4::splat(v), sky: Vec2::splat(v) };
        let data = Data {
            cubes: vec![],
            lookup: Vec2::ONE,
            bloom: true,
            zones: vec![(0, bloom(0.)), (0, bloom(0.5))],
            presets: vec![Some(preset(0.)), Some(preset(1.))],
            districts: vec![],
            grid: None,
            lens: Some((Vec2::ZERO, vec![])),
            film: None,
            fixed_exposure: None,
        };
        let at = |since: f32| Grading { to: 1, since, preset_from: Some(preset(0.)), bloom_from: Some(bloom(0.)), ..default() };
        let (p, b) = (at(2.5).preset(&data).unwrap(), at(1.).bloom(&data));
        assert_eq!((p.exposure.x, p.fog_ramp.x, p.sky.x), (1., 0.5, 0.5));
        assert_eq!((b.threshold, b.scale, b.level0, b.level1, b.saturation), (0.5, 2., 0.25, 0.25, 0.25));
        assert_eq!(at(0.125).bloom(&data).threshold, 0.25);
        // Skate 2's glow at S2_BLOOM_GAIN; Skate 1's as recovered.
        let b = Bloom { threshold: 0.5, scale: 2., level0: 1.5, level1: 1., saturation: 2. };
        assert_eq!(zone_bloom(&b, true), Vec4::new(0.5, 2., 0.75, 0.5));
        assert_eq!(zone_bloom(&b, false), Vec4::new(0.5, 2., 1.5, 1.));
    }

    #[test]
    fn skate1_packs_its_grade_sets_as_its_exe_does() {
        // The 'game' row's curves at rest (t = 0.3554) and what the exe's own packing (0x82744be8, run in an
        // emulator) uploaded from them: far set A, near set B.
        let close = |a: [Vec4; 5], b: [[f32; 4]; 5]| a.iter().zip(b).all(|(a, b)| (*a - Vec4::from(b)).abs().max_element() < 2e-6);
        let far = skate1_pack([0., 0.996245, 0.901543, 0.012039, 0., 1., 1.005907, 1.005244, 1.006419, 0.]);
        assert!(close(far, [
            [0.987849, 0.007196, 0.001199, 0.003386],
            [0.003598, 0.991447, 0.001199, 0.003386],
            [0.003598, 0.007196, 0.98545, 0.003386],
            [1., 1.005907, 1.005244, 0.],
            [256.637 / 255., 0., 0., 0.],
        ]));
        let near = skate1_pack([0., 1.022468, 0.541067, 0.007594, 0., 1.005573, 1., 0.99536, 1., 0.013537]);
        assert!(close(near, [
            [1.017033, 0.004659, 0.000776, -0.012157],
            [0.002329, 1.019362, 0.000776, -0.012157],
            [0.002329, 0.004659, 1.01548, -0.012157],
            [1.005573, 1., 0.99536, 0.],
            [1., 3.452046 / 255., 0., 0.],
        ]));
        // 0x823a0df8: flat before the first and after the last point, linear between; a zero-width step jumps.
        let c = [0., 0.5, 0.5, 1., 1., 2., 4., 8.];
        assert_eq!([-1., 0.25, 0.5, 0.75, 2.].map(|t| skate1_curve(&c, t)), [1., 1.5, 4., 6., 8.]);
    }

    #[test]
    fn skate1_deck_frame_sits_on_the_wheels_contact_plane() {
        // Board along world +z, wheels 3 cm up (radius 3 cm), trucks above them; then the same board upside down.
        let wheels = |y: f32| [Vec3::new(0.1, y, 0.3), Vec3::new(-0.1, y, 0.3), Vec3::new(0.1, y, -0.3), Vec3::new(-0.1, y, -0.3)];
        let deck = deck_frame(wheels(0.03), Vec3::new(0., 0.07, 0.), 0.03).unwrap();
        let near = |a: Vec3, b: Vec3| (a - b).abs().max_element() < 1e-6;
        assert!(near(deck.transform_point3(Vec3::ZERO), Vec3::ZERO));
        assert!(near(deck.transform_point3(Vec3::new(0.2, 0.5, 0.3)), Vec3::new(0.2, 0.5, 0.3)));
        let flipped = deck_frame(wheels(1.), Vec3::new(0., 0.96, 0.), 0.03).unwrap();
        assert!(near(flipped.transform_point3(Vec3::new(0., 1.03, 0.)), Vec3::ZERO));
        assert!(near(flipped.transform_vector3(Vec3::Y), -Vec3::Y));
        assert!(deck_frame([Vec3::ZERO; 4], Vec3::Y, 0.03).is_none());
    }

    #[test]
    fn skate1_meter_volume_is_skate_ems_x_games_ellipsoid() {
        // The one qualifying skate.ems record: 150 along side (~x), 109 along forward (~z), 40 up.
        let v = [-39.422, 0.357, 1967.189, 109., 40., 150., 0.0348995, 0., 0.9993908];
        let c = Vec3::new(v[0], v[1], v[2]);
        assert!(skate1_inside(&v, c));
        assert!(skate1_inside(&v, c + Vec3::new(149., 0., 0.)));
        assert!(!skate1_inside(&v, c + Vec3::new(0., 0., 110.)));
        assert!(!skate1_inside(&v, c + Vec3::new(0., 41., 0.)));
        let sphere = [0., 0., 0., 5., 5., 5., 0., 0., 1.];
        assert!(skate1_inside(&sphere, Vec3::new(4.9, 0., 0.)) && !skate1_inside(&sphere, Vec3::new(0., 5., 0.)));
    }
}
