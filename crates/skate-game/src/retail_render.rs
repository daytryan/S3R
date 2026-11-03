//! Retail world shading. See docs/retail-renderer.md for provenance and gaps.
use bevy::{
    asset::{AssetPath, embedded_asset, embedded_path},
    prelude::*,
    render::{
        extract_component::ExtractComponent,
        render_resource::{AsBindGroup, ShaderType},
    },
    shader::ShaderRef,
};
use std::collections::BTreeMap;

pub(crate) struct RetailRenderPlugin;
#[cfg(test)]
#[path = "retail_shader_tests.rs"]
mod shader_tests;
// Use the same path derivation as embedded_asset!: alternate binary targets
// have a different crate namespace even though they share these source files.
fn retail_shader(path: &str) -> ShaderRef {
    AssetPath::from(embedded_path!(path))
        .with_source("embedded")
        .into()
}
impl Plugin for RetailRenderPlugin {
    fn build(&self, app: &mut App) {
        shadow::install(app);
        shadow_visibility::install(app);
        crate::skate_world::shadow_geometry::install(app); // perf 11-01: world lights see the shadow proxies' layer
        exposure::install(app);
        grading::install(app);
        weather::install(app);
        daynight::install(app);
        wheel_fx::install(app);
        backdrop::install(app);
        godray::install(app);
        // perf 10-18: SKATE_BG_FIRST=0 restores Bevy's pipeline-first batch set order (vendor/README.md).
        if std::env::var_os("SKATE_BG_FIRST").is_some_and(|v| v == "0") {
            bevy::core_pipeline::core_3d::BATCH_SETS_BY_BIND_GROUP.store(false, std::sync::atomic::Ordering::Relaxed);
        }
        if std::env::var_os("SKATE_WEATHERING_COMPARE").is_some_and(|v| v == "1") {
            app.add_systems(Update, compare_weathering);
        }
        app.add_plugins(crate::retail_character::CharacterLightingPlugin);
        if std::env::var_os("SKATE_DEBUG_FOLIAGE").is_some_and(|v| v == "1") {
            eprintln!(
                "SKATE_FOLIAGE_DEBUG: solid cyan tree-wall cards, magenta other foliage; alpha rejection disabled for foliage only"
            );
        }
        info!(
            "Skate 2 world lighting: {}",
            if *S2_LIGHTING { "retail (SKATE_S2_LIGHTING=0 for the Skate 3 path)" } else { "off, Skate 3 path (SKATE_S2_LIGHTING=0)" }
        );
        info!(
            "Skate 1 world lighting: {}",
            if *S1_LIGHTING { "retail (SKATE_S1_LIGHTING=0 for the Skate 3 path)" } else { "off, Skate 3 path (SKATE_S1_LIGHTING=0)" }
        );
        embedded_asset!(app, "retail_world.wgsl");
        bevy::shader::load_shader_library!(app, "retail_material_bindings.wgsl");
        bevy::shader::load_shader_library!(app, "retail_weather.wgsl");
        embedded_asset!(app, "retail_tone.wgsl");
        embedded_asset!(app, "retail_depth.wgsl");
        embedded_asset!(app, "retail_sky.wgsl");
        app.add_plugins((
            MaterialPlugin::<RetailWorldMaterial>::default(),
            MaterialPlugin::<RetailBlendMaterial>::default(),
            MaterialPlugin::<RetailSkyMaterial>::default(),
        ));
    }
    fn finish(&self, app: &mut App) {
        // After every plugin's build, so LightPlugin's upstream system exists to remove.
        shadow_visibility::finish(app);
        app.world_mut().schedule_scope(PostUpdate, |world, schedule| {
            let removed = schedule
                .remove_systems_in_set(
                    bevy::mesh::mark_3d_meshes_as_changed_if_their_assets_changed,
                    world,
                    bevy::ecs::schedule::ScheduleCleanupPolicy::RemoveSystemsOnly,
                )
                .expect("replace the mesh-changed walk");
            assert_eq!(removed, 1, "exactly one upstream mesh-changed walk");
        });
        app.add_systems(PostUpdate, mark_changed_meshes.after(bevy::asset::AssetEventSystems));
    }
}

/// Bevy's mark_3d_meshes_as_changed_if_their_assets_changed, minus static map batches. Bevy walks every Mesh3d whenever
/// any Mesh asset is modified, and a HUD minimap rewrites its 2D meshes each frame while skating: 0.44 ms a frame over
/// Skate 1 San Vanelona's ~9k batches (perf 10-10). Map batch meshes are never modified after the map loads.
fn mark_changed_meshes(
    mut meshes: Query<&mut Mesh3d, Without<shadow_visibility::StaticShadowCaster>>,
    mut events: MessageReader<AssetEvent<Mesh>>,
) {
    let changed: std::collections::HashSet<AssetId<Mesh>> = events
        .read()
        .filter_map(|e| match *e {
            AssetEvent::Modified { id } => Some(id),
            _ => None,
        })
        .collect();
    if changed.is_empty() {
        return;
    }
    for mut mesh in &mut meshes {
        if changed.contains(&mesh.0.id()) {
            mesh.set_changed();
        }
    }
}

// Replaces Bevy's directional shadow visibility: static map batches sit in a retained BVH, so the character-visibility
// and layer-28 player lights stop testing every map entity every frame (perf 10-10 #3).
#[path = "retail_shadow_visibility.rs"]
pub(crate) mod shadow_visibility;

// Diagnostic only: mutate materials once per keypress, never continuously.
// Separate bits preserve the original texture-presence flags for restoration.
fn compare_weathering(
    keys: Res<ButtonInput<KeyCode>>,
    mut mode: Local<u32>,
    mut materials: ResMut<Assets<RetailWorldMaterial>>,
    mut windows: Query<&mut Window, With<bevy::window::PrimaryWindow>>,
) {
    if !keys.just_pressed(KeyCode::F8) {
        return;
    }
    *mode = (*mode + 1) % 4;
    let label = match *mode {
        1 => "Repeating grime OFF; decals ON",
        2 => "Repeating grime ON; decals OFF",
        3 => "Repeating grime OFF; decals OFF",
        _ => "Normal grime and decals ON",
    };
    for (_, material) in materials.iter_mut() {
        let family = material.params.mode.x as u32;
        if !(1..=8).contains(&family) {
            continue;
        }
        let flags = material.params.mode.y as u32;
        material.params.mode.y = ((flags & !768) | (*mode << 8)) as f32;
    }
    for mut window in &mut windows {
        window.title = format!("Skate 3 - {label} [F8: next comparison]");
    }
    info!("WEATHERING_COMPARE: {label}");
}

#[path = "retail_exposure.rs"]
mod exposure;

#[path = "retail_grading.rs"]
mod grading;
pub(crate) use grading::{District, ScreenFade, ShotPost};

#[path = "retail_shadow.rs"]
mod shadow;
pub(crate) use shadow::{AuthoredFog, ShadowState};

#[derive(Resource)]
pub(crate) struct RetailScene(pub bool);

impl RetailScene {
    /// Package evidence, never the editable map name. Older Skate 2 exports
    /// can retain native provenance/sky/collision without material definitions.
    pub(crate) fn for_map(map: &skate_data::skate_map::SkateMap) -> bool {
        map.materials.iter().any(|m| m.retail_definition.is_some())
            || map
                .extensions
                .iter()
                .any(|e| matches!(&e.tag, b"WMET" | b"RWCM" | b"SKYB"))
            || map
                .geometry
                .collision
                .iter()
                .any(|c| c.native_edges.is_some())
            || map.rails.iter().any(|r| r.native.is_some())
    }
}

#[path = "retail_sky.rs"]
mod sky;
pub(crate) use sky::{RetailSkyMaterial, spawn_sky};

#[path = "retail_weather.rs"]
mod weather;
pub(crate) use weather::{LightningStrike, PropMaterial, WeatherSet, rain_level};
#[cfg(test)]
pub(crate) use weather::tests_api as weather_tests_api;

#[path = "retail_wheel_fx.rs"]
mod wheel_fx;

#[path = "retail_daynight.rs"]
mod daynight;
pub(crate) use daynight::DayLook;
pub(crate) use daynight::DAY as DAY_LOOK;

#[path = "retail_backdrop.rs"]
mod backdrop;

#[path = "retail_godray.rs"]
pub(crate) mod godray;
pub(crate) use backdrop::{OnlineHidden, spawn_backdrop};

/// #33: a mesh drawing a videoscreen material; while one is in view the tone pass refreshes the feed it samples.
#[derive(Component)]
pub(crate) struct JumbotronScreen;
pub(crate) use exposure::FEED;

#[derive(Component, ExtractComponent, Clone, Copy, ShaderType, Default)]
pub(crate) struct RetailTone {
    pub enabled: Vec4,
}

#[derive(Clone, Debug, ShaderType)]
pub(crate) struct WorldParams {
    // family, texture flags, alpha cutoff (-1 for opaque), exposure
    pub mode: Vec4,
    // Diagnostic solid foliage colour; w=0 keeps retail shading.
    pub foliage_debug: Vec4,
    // macro UV scale, opacity, detail UV scale, material multiplier
    pub surface: Vec4,
    // tree LM scale/floor/tint, proxy multiplier: reference day capture
    pub family: Vec4,
    pub fog_ramp: Vec4,
    pub fog_color: Vec4,
    pub shadow_color: Vec4,
    pub sun_direction: Vec4,
    // Wear-only visual tuning; artwork retains unit opacity.
    pub decal: Vec4,
    pub water: [Vec4; 4],
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
#[bind_group_data(RetailKey)]
#[data(0, WorldParams, binding_array(17))]
// ponytail: slab size trades opaque bind-group count against the cost of each slab switch
// in the depth-sorted transparent pass (wgpu tracks every slot on every switch).
// With equal blended materials shared (skate_world.rs), RTX 2050 FPS at 128/256:
// NSV Mega Compound 58/60, University 165/169, DownTown 137/144, Skate 1 San Vanelona 123/129.
// Unshared, 2048 dropped NSV to 20.
#[bindless(limit(bindless_slab()))]
pub(crate) struct RetailWorldMaterial {
    pub params: WorldParams,
    #[texture(1)]
    #[sampler(2)]
    pub diffuse: Option<Handle<Image>>,
    #[texture(3)]
    #[sampler(4)]
    pub lightmap: Option<Handle<Image>>,
    #[texture(5)]
    #[sampler(6)]
    pub normal: Option<Handle<Image>>,
    #[texture(7)]
    #[sampler(8)]
    pub detail: Option<Handle<Image>>,
    #[texture(9)]
    #[sampler(10)]
    pub macro_map: Option<Handle<Image>>,
    #[texture(11)]
    #[sampler(12)]
    pub decal: Option<Handle<Image>>,
    #[texture(13)]
    #[sampler(14)]
    pub specular: Option<Handle<Image>>,
    #[texture(15, dimension = "cube")]
    pub environment: Option<Handle<Image>>,
    // Skate 2 terrain's slope diffuse (retail_world.wgsl bit 32768); it samples with the diffuse sampler.
    #[texture(17)]
    pub diffuse2: Option<Handle<Image>>,
    #[storage(16, read_only, binding_array(18))]
    pub shadow_state: Handle<bevy::render::storage::ShaderStorageBuffer>,
    pub alpha: AlphaMode,
    pub two_sided: bool,
}
/// perf 10-18: SKATE_BINDLESS_SLAB=<n> sets the world material's slab size (default 256) to A/B sizes in one build
/// (vendor/README.md: the AsBindGroup derive takes an expression). Read once: the layout and the allocator agree.
fn bindless_slab() -> u32 {
    static SLAB: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *SLAB.get_or_init(|| std::env::var("SKATE_BINDLESS_SLAB").ok().and_then(|v| v.parse().ok()).filter(|&n| n > 1).unwrap_or(256))
}
impl From<&RetailWorldMaterial> for WorldParams {
    fn from(material: &RetailWorldMaterial) -> Self {
        material.params.clone()
    }
}
/// FPS pass 10-23: retail_depth.wgsl (prepass and shadow maps) tests the plain diffuse alpha against `mode.z`; the main
/// pass discards more for family 14 (scrolled UVs) and flag 2048 (alpha squared). Blended batches never enter the prepass.
pub(crate) fn camera_prepass_unsafe(alpha: AlphaMode, mode: Vec4) -> bool {
    !matches!(alpha, AlphaMode::Blend) && mode.z >= 0. && (mode.x == 14. || (mode.y as u32) & 2048 != 0)
}
/// Identity of the actual GPU inputs, independent of unused source metadata.
#[derive(PartialEq, Eq, Hash)]
pub(crate) struct WorldMaterialKey {
    params: [[u32; 4]; 13],
    images: [Option<AssetId<Image>>; 9],
    shadow: AssetId<bevy::render::storage::ShaderStorageBuffer>,
    alpha: [u32; 2],
    two_sided: bool,
}
impl RetailWorldMaterial {
    /// retail_world.wgsl reads vertex colours (decal UVs) only in this case.
    pub(crate) fn samples_decal_uv(&self) -> bool {
        (self.params.mode.y as u32 & 8) != 0 && matches!(self.params.mode.x as u32, 3 | 4)
    }
    pub(crate) fn camera_prepass_unsafe(&self) -> bool {
        camera_prepass_unsafe(self.alpha, self.params.mode)
    }
    pub(crate) fn batch_key(&self) -> Option<WorldMaterialKey> {
        // Blended geometry keeps its previous mesh centers and sorting groups.
        matches!(self.alpha, AlphaMode::Opaque | AlphaMode::Mask(_)).then(|| self.material_key())
    }
    /// Equal materials share one asset, blended ones included: their sorted draws then
    /// reuse a bindless slab instead of switching slabs on almost every draw.
    pub(crate) fn material_key(&self) -> WorldMaterialKey {
        let alpha = match self.alpha {
            AlphaMode::Opaque => [0, 0],
            AlphaMode::Mask(cutoff) => [1, cutoff.to_bits()],
            AlphaMode::Blend => [2, 0],
            AlphaMode::Premultiplied => [3, 0],
            AlphaMode::AlphaToCoverage => [4, 0],
            AlphaMode::Add => [5, 0],
            AlphaMode::Multiply => [6, 0],
        };
        let p = &self.params;
        WorldMaterialKey {
            params: [
                p.mode,
                p.foliage_debug,
                p.surface,
                p.family,
                p.fog_ramp,
                p.fog_color,
                p.shadow_color,
                p.sun_direction,
                p.decal,
                p.water[0],
                p.water[1],
                p.water[2],
                p.water[3],
            ]
            .map(|v| v.to_array().map(f32::to_bits)),
            images: [
                &self.diffuse,
                &self.lightmap,
                &self.normal,
                &self.detail,
                &self.macro_map,
                &self.decal,
                &self.specular,
                &self.environment,
                &self.diffuse2,
            ]
            .map(|h| h.as_ref().map(Handle::id)),
            shadow: self.shadow_state.id(),
            alpha,
            two_sided: self.two_sided,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RetailKey {
    two_sided: bool,
    /// The shaders' `alpha < mode.z` discard can fire. Without it the discard is compiled out: a shader that
    /// may discard loses early depth rejection, which cost the SSAO prepass 3.9 ms at Skate 1 Slappys (perf 10-10).
    alpha_test: bool,
    /// perf 11-01: the shader path retail_world.wgsl takes, game * 8 + family group ([`shader_path`]); 0: all paths.
    path: u8,
}
impl From<&RetailWorldMaterial> for RetailKey {
    fn from(m: &RetailWorldMaterial) -> Self {
        Self {
            two_sided: m.two_sided,
            // Alpha is at least 0, so a negative cutoff (opaque: -1) never discards.
            alpha_test: m.params.mode.z >= 0.,
            path: if crate::config::perf_on("SKATE_RETAIL_SPECIALIZE") { shader_path(m.params.mode) } else { 0 },
        }
    }
}
/// retail_world.wgsl's path for `mode` (family, flags): its game (1 Skate 1 by flag 16384, 2 Skate 2 by 1024, 3 Skate 3
/// by neither) times 8 plus its family group (1 water 14 / 30-33, 2 tree 9 / 10, 3 lamp 11 / 12, 4 the rest); 0 when
/// both game flags are set, which only the unspecialized shader reads as it is.
fn shader_path(mode: Vec4) -> u8 {
    let flags = mode.y as u32;
    let game = match (flags & 16384 != 0, flags & 1024 != 0) {
        (true, false) => 1,
        (false, true) => 2,
        (false, false) => 3,
        (true, true) => return 0,
    };
    let group = match mode.x as u32 {
        14 | 30..=33 => 1,
        9 | 10 => 2,
        11 | 12 => 3,
        _ => 4,
    };
    game * 8 + group
}
fn specialize_retail(
    descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
    key: RetailKey,
) {
    descriptor.primitive.cull_mode = cull_mode(key.two_sided);
    if let Some(fragment) = descriptor.fragment.as_mut().filter(|_| !key.alpha_test) {
        fragment.shader_defs.push("RETAIL_NO_ALPHA_TEST".into());
    }
    // FPS pass 10-23: SKATE_FPS_LM1=0 takes retail_world.wgsl's single lightmap tap everywhere again.
    if let Some(fragment) = descriptor.fragment.as_mut().filter(|_| !crate::config::perf_on("SKATE_FPS_LM1")) {
        fragment.shader_defs.push("RETAIL_LM_TAP_ALWAYS".into());
    }
    if let Some(fragment) = descriptor.fragment.as_mut().filter(|_| key.path != 0) {
        let group = ["RETAIL_GROUP_WATER", "RETAIL_GROUP_TREE", "RETAIL_GROUP_LAMP", "RETAIL_GROUP_WORLD"];
        fragment.shader_defs.push("RETAIL_SPECIALIZED".into());
        fragment.shader_defs.push(group[(key.path % 8 - 1) as usize].into());
        match key.path / 8 {
            1 => fragment.shader_defs.push("RETAIL_GAME_S1".into()),
            2 => fragment.shader_defs.push("RETAIL_GAME_S2".into()),
            _ => {}
        }
    }
    // perf 11-01: SKATE_EARLY_DISCARD=0 rejects cut-out tree and alphatest texels at the end of the shader again.
    if let Some(fragment) = descriptor.fragment.as_mut().filter(|_| !crate::config::perf_on("SKATE_EARLY_DISCARD")) {
        fragment.shader_defs.push("RETAIL_LATE_DISCARD".into());
    }
}
impl Material for RetailWorldMaterial {
    fn fragment_shader() -> ShaderRef {
        retail_shader("retail_world.wgsl")
    }
    fn prepass_fragment_shader() -> ShaderRef {
        retail_shader("retail_depth.wgsl")
    }
    fn alpha_mode(&self) -> AlphaMode {
        self.alpha
    }
    /// Without an alpha test the depth shader is compiled with RETAIL_NO_ALPHA_TEST and reads no material.
    fn depth_passes_skip_material(&self) -> bool {
        !RetailKey::from(self).alpha_test
    }
    fn specialize(
        _: &bevy::pbr::MaterialPipeline,
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        _: &bevy::mesh::MeshVertexBufferLayoutRef,
        key: bevy::pbr::MaterialPipelineKey<Self>,
    ) -> Result<(), bevy::render::render_resource::SpecializedMeshPipelineError> {
        specialize_retail(descriptor, key.bind_group_data);
        Ok(())
    }
}

/// Blended world batches: the same material with one small bind group each instead of a 256-material bindless slab.
/// The transparent pass is depth sorted, so its draws hop between materials, and wgpu re-tracks every slot of a slab
/// on each hop: ~5 ms of serial encode per frame looking over New San Vanelona's 705 blended materials (perf 10-10).
/// Opaque batches stay bindless; they are drawn binned by slab.
#[derive(Asset, TypePath, Debug, Clone)]
pub(crate) struct RetailBlendMaterial(pub RetailWorldMaterial);
impl AsBindGroup for RetailBlendMaterial {
    type Data = <RetailWorldMaterial as AsBindGroup>::Data;
    type Param = <RetailWorldMaterial as AsBindGroup>::Param;
    fn label() -> &'static str {
        "retail_blend_material"
    }
    fn bind_group_data(&self) -> Self::Data {
        self.0.bind_group_data()
    }
    fn unprepared_bind_group(
        &self,
        layout: &bevy::render::render_resource::BindGroupLayout,
        render_device: &bevy::render::renderer::RenderDevice,
        param: &mut bevy::ecs::system::SystemParamItem<'_, '_, Self::Param>,
        _: bool,
    ) -> Result<bevy::render::render_resource::UnpreparedBindGroup, bevy::render::render_resource::AsBindGroupError> {
        self.0.unprepared_bind_group(layout, render_device, param, true)
    }
    fn bind_group_layout_entries(
        render_device: &bevy::render::renderer::RenderDevice,
        _: bool,
    ) -> Vec<bevy::render::render_resource::BindGroupLayoutEntry> {
        RetailWorldMaterial::bind_group_layout_entries(render_device, true)
    }
}
impl Material for RetailBlendMaterial {
    fn fragment_shader() -> ShaderRef {
        RetailWorldMaterial::fragment_shader()
    }
    fn prepass_fragment_shader() -> ShaderRef {
        RetailWorldMaterial::prepass_fragment_shader()
    }
    fn alpha_mode(&self) -> AlphaMode {
        self.0.alpha
    }
    fn depth_passes_skip_material(&self) -> bool {
        self.0.depth_passes_skip_material()
    }
    fn specialize(
        _: &bevy::pbr::MaterialPipeline,
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        _: &bevy::mesh::MeshVertexBufferLayoutRef,
        key: bevy::pbr::MaterialPipelineKey<Self>,
    ) -> Result<(), bevy::render::render_resource::SpecializedMeshPipelineError> {
        specialize_retail(descriptor, key.bind_group_data);
        Ok(())
    }
}
fn cull_mode(two_sided: bool) -> Option<bevy::render::render_resource::Face> {
    // Retail foliage is two-sided; opaque world triangles retain winding.
    (!two_sided).then_some(bevy::render::render_resource::Face::Back)
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Binding {
    pub texture: u32,
    pub uv: u32,
    pub u: u32,
    pub v: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Definition {
    pub shader: String,
    pub family: u32,
    pub flags: u32,
    pub bindings: BTreeMap<String, Binding>,
    pub parameters: BTreeMap<String, Vec<String>>,
}
impl Definition {
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        struct Reader<'a>(&'a [u8]);
        impl<'a> Reader<'a> {
            fn take(&mut self, n: usize) -> Option<&'a [u8]> {
                let v = self.0.get(..n)?;
                self.0 = &self.0[n..];
                Some(v)
            }
            fn u32(&mut self) -> Option<u32> {
                Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
            }
            fn text(&mut self) -> Option<String> {
                let n = self.u32()? as usize;
                String::from_utf8(self.take(n)?.to_vec()).ok()
            }
        }
        let mut r = Reader(bytes);
        r.take(16)?;
        let shader = r.text()?;
        let stored_family = r.u32()?;
        // Older map packages retained the complete bindings but classified
        // non-flowing water as unknown. Upgrade without re-exporting geometry.
        let family = if matches!(
            shader.as_str(),
            "water.default" | "water.alpha" | "water.skatepark"
        ) {
            33
        } else {
            stored_family
        };
        let flags = r.u32()?;
        let mut bindings = BTreeMap::new();
        for _ in 0..r.u32()? {
            bindings.insert(
                r.text()?,
                Binding {
                    texture: r.u32()?,
                    uv: r.u32()?,
                    u: r.u32()?,
                    v: r.u32()?,
                },
            );
        }
        let mut parameters = BTreeMap::new();
        for _ in 0..r.u32()? {
            let name = r.text()?;
            let values = (0..r.u32()?)
                .map(|_| r.text())
                .collect::<Option<Vec<_>>>()?;
            // Names/GUIDs do not affect shading or batching.
            if !matches!(name.as_str(), "Name" | "AttribulatorMaterialName") {
                parameters.insert(name, values);
            }
        }
        r.text()?;
        if !r.0.is_empty() {
            return None;
        }
        // Skate 2's vault gives incandescent.backlit and .videoscreen the same
        // baseincandescent pixel shader as incandescent.default, and backlitzbias its
        // zbiasincandescentbacklit variant; environment.transparent (transparentenvironment)
        // and decal.default (decalzbias) are baseenvironment variants (GRADING_AUDIT_S2.md;
        // retail_world.wgsl bits 2048-8192).
        let family = match shader.as_str() {
            "incandescent.backlit" | "incandescent.videoscreen" | "incandescent.backlitzbias"
                if family == 0 && skate2_lighting(&parameters) =>
            {
                12
            }
            // #33: Skate 3's videoscreen draws with its baseincandescent pixel shader too (TU3 vault, 827E596C).
            "incandescent.videoscreen" if family == 0 => 12,
            "environment.transparent" | "decal.default"
                if family == 0 && skate2_lighting(&parameters) =>
            {
                1
            }
            // baseterrain / decalterrain (GRADING_AUDIT_S2.md §7) once the converter binds their slope maps; maps
            // converted before keep the fallback. decalterrain reads i_specular for both, so it has no specular2.
            "terrain.default"
                if family == 0
                    && skate2_lighting(&parameters)
                    && bindings.contains_key("diffuse2")
                    && bindings.contains_key("specular2") =>
            {
                1
            }
            "terrain.decal" if family == 0 && skate2_lighting(&parameters) && bindings.contains_key("diffuse2") => 3,
            _ => family,
        };
        Some(Self {
            shader,
            family,
            flags,
            bindings,
            parameters,
        })
    }
    /// Skate 2 package evidence (layer-page lightmap plus chromaticity page);
    /// Skate 3 uses "lightpage_", Skate 1 "tex_0x", and neither has chromaticity.
    pub fn skate2_lighting(&self) -> bool {
        skate2_lighting(&self.parameters)
    }
    pub fn scalar(&self, name: &str) -> Option<f32> {
        self.parameters
            .get(name)?
            .first()?
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite())
    }
    pub fn supported(&self, tuning: &MaterialTuning) -> bool {
        (1..=13).contains(&self.family)
            || match self.family {
                14 | 32 => tuning.rows.get(&self.shader).is_some_and(|r| !r.is_empty()),
                // Skate 2's ocean is Skate 3's with its own lightmap (retail_world.wgsl, brief 10-29); its still
                // water keeps the fallback, unchecked (brief 10-26).
                31 => tuning.pca_available && tuning.rows.get(&self.shader).is_some_and(|r| r.len() == 3),
                30 => tuning.rows.get(&self.shader).is_some_and(|r| r.len() == 4),
                33 => {
                    tuning.pca_available
                        && !self.skate2_lighting()
                        && self.bindings.contains_key("normal")
                        && self.bindings.contains_key("normal2")
                        && tuning.rows.get(&self.shader).is_some_and(|r| r.len() == 4)
                }
                _ => false,
            }
    }
    pub fn build(
        &self,
        m: &skate_data::skate_map::Material,
        tuning: &MaterialTuning,
        texture: &mut impl FnMut(u32, u8) -> Option<Handle<Image>>,
    ) -> RetailWorldMaterial {
        // Skate 2 terrain (retail_world.wgsl bit 32768): normal2 in the detail slot, terrain.default's specular2 in the
        // decal slot (decalterrain keeps its decal there), diffuse2 in its own.
        let terrain = matches!((self.family, self.shader.as_str()), (1, "terrain.default") | (3, "terrain.decal"));
        let mut fetch = |role: &str, fallback: u32, clamp: bool| {
            let b = self.bindings.get(role);
            texture(
                b.map_or(fallback, |b| b.texture),
                if clamp || b.is_some_and(|b| b.u == 1 && b.v == 1) {
                    if role == "decal" { 6 } else { 4 }
                } else {
                    3
                },
            )
        };
        // #33: the videoscreen's own texture ("test temp") is a placeholder retail replaces with the scene feed.
        let diffuse = if self.shader == "incandescent.videoscreen" {
            Some(exposure::FEED)
        } else {
            fetch("diffuse", m.textures[0], false)
        };
        let lightmap = fetch("lightmap", m.textures[1], true);
        let normal = fetch("normal", m.textures[2], false);
        let detail = fetch(
            if matches!(self.family, 31 | 33) || terrain {
                "normal2"
            } else {
                "detail"
            },
            0,
            false,
        );
        let macro_map = fetch("macrooverlay", 0, false);
        let decal = fetch(if terrain && self.family == 1 { "specular2" } else { "decal" }, 0, self.family == 3);
        let diffuse2 = if terrain { fetch("diffuse2", 0, false) } else { None };
        let specular = fetch("specular", 0, false);
        let environment = texture(self.bindings.get("environment").map_or(0, |b| b.texture), 5);
        let macro_scale = self.scalar("macroOverlayUVScale").unwrap_or(0.);
        let macro_opacity = self.scalar("macroOverlayOpacity").unwrap_or(0.);
        let detail_scale = self.scalar("detailNormalUVScale").unwrap_or(0.);
        // Skate 2 lighting (retail_world.wgsl bit 1024); Skate 3/1 materials are unchanged.
        let skate2 = self.skate2_lighting();
        // Skate 2 variants from their ucode and capture state: transparentenvironment (2048),
        // decalzbias (4096), zbiasincandescentbacklit (8192). Each alpha-tests its output
        // alpha at ALPHAREF 16/255 (GEQUAL); the shader tests that, not diffuse alpha.
        let clear = skate2 && self.shader == "environment.transparent";
        let zbias_decal = skate2 && self.shader == "decal.default";
        let zbias_glow = skate2 && self.shader == "incandescent.backlitzbias";
        let s2_alpha_test = clear || zbias_decal || zbias_glow;
        // Skate 1 lighting (retail_world.wgsl bit 16384; GRADING_AUDIT_S1.md part 5).
        let skate1 = !skate2 && skate1_lighting(&self.parameters);
        let flags = u32::from(normal.is_some())
            | (u32::from(detail.is_some() && detail_scale > 0. && !terrain) << 1)
            | (u32::from(
                macro_map.is_some()
                    && macro_scale > 0.
                    && (macro_opacity > 0. || self.family == 31),
            ) << 2)
            | (u32::from(decal.is_some()) << 3)
            | (u32::from(specular.is_some()) << 4)
            | (u32::from(lightmap.is_some()) << 5)
            | (u32::from(environment.is_some()) << 6)
            | (u32::from(detail.is_some()) << 7)
            // 256/512 are compare_weathering's diagnostic bits.
            | (u32::from(skate2) << 10)
            | (u32::from(clear) << 11)
            | (u32::from(zbias_decal) << 12)
            | (u32::from(zbias_glow) << 13)
            | (u32::from(skate1) << 14)
            | (u32::from(terrain) << 15);
        // scene.hlsl's retail world ALPHAREF is 30, not the portable 0.5.
        let cutoff = 30. / 255.;
        let debug_foliage = matches!(self.family, 9 | 10)
            && std::env::var_os("SKATE_DEBUG_FOLIAGE").is_some_and(|v| v == "1");
        let tree_wall = m
            .retail_definition
            .as_deref()
            .is_some_and(|bytes| bytes.windows(b"TreeWall".len()).any(|s| s == b"TreeWall"));
        let alpha = match (debug_foliage, m.alpha_mode) {
            (true, _) => AlphaMode::Opaque,
            (_, 1) => AlphaMode::Mask(cutoff),
            (_, 2) => AlphaMode::Blend,
            _ => AlphaMode::Opaque,
        };
        // decalzbias blends SRC_ALPHA / INV_SRC_ALPHA (RB_BLENDCONTROL0 0x07060706).
        // zbiasincandescentbacklit is opaque (ONE / ZERO) but its alpha test reads the
        // lightmap, which the depth prepass (retail_depth.wgsl) cannot: drawn in the blended
        // pass at alpha 1 instead, so no prepass depth survives a discarded pixel.
        let alpha = if self.family == 32
            || (matches!(self.family, 30 | 33) && self.shader.ends_with("alpha"))
            || zbias_decal
            || zbias_glow
        {
            AlphaMode::Blend
        } else {
            alpha
        };
        let mut water = [Vec4::ZERO; 4];
        if let Some(rows) = tuning.rows.get(&self.shader) {
            for (to, from) in water.iter_mut().zip(rows) {
                *to = Vec4::from_array(*from);
            }
        }
        if self.family == 31 && skate2 {
            // Skate 2's vault material_ocean default m_params (db.big): Skate 3's but for this level, 1.2 to 0.2.
            water[1].y = 1.2;
        }
        if self.family == 14 {
            water[1] = Vec4::new(
                self.scalar("uAnimationSpeed").unwrap_or(0.),
                self.scalar("vAnimationSpeed").unwrap_or(0.),
                0.,
                0.,
            );
        }
        RetailWorldMaterial {
            params: WorldParams {
                mode: Vec4::new(
                    self.family as f32,
                    flags as f32,
                    if s2_alpha_test {
                        16. / 255.
                    } else if m.alpha_mode == 1 && !debug_foliage {
                        cutoff
                    } else {
                        -1.
                    },
                    2.5,
                ),
                foliage_debug: if !debug_foliage {
                    Vec4::ZERO
                } else if tree_wall {
                    Vec4::new(0., 1., 1., 1.)
                } else {
                    Vec4::new(1., 0., 1., 1.)
                },
                // Terrain: diffuse2UVScale where the detail scale goes (baseterrain reads no detail map).
                surface: Vec4::new(
                    macro_scale,
                    macro_opacity,
                    if terrain { self.scalar("diffuse2UVScale").unwrap_or(1.) } else { detail_scale },
                    1.,
                ),
                family: match (skate2, self.shader.as_str()) {
                    (false, _) => Vec4::new(0.3435, 0.02, 1., 0.45),
                    // Skate 2 vault incandescent class: m_params.x lightmap blend, .y exposure.
                    (true, "incandescent.default") => Vec4::new(0., 10., 0., 0.),
                    (
                        true,
                        "incandescent.backlit" | "incandescent.backlituvscroll" | "incandescent.backlitzbias",
                    ) => Vec4::new(0.75, 1., 0., 0.),
                    (true, "incandescent.videoscreen") => Vec4::new(0., 1., 0., 0.),
                    // Vault class DDFD2CF3C65C265B (baseterrain, decalterrain) m_params[1]: the slope blend powers and offsets.
                    (true, "terrain.default" | "terrain.decal") if terrain => Vec4::new(8., 0.04, 8., -0.1),
                    // Skate 2 tree g_ViewDotLight.x (median of three captures; it varies
                    // with view and light) and .y floor, tree and proxy m_params.y.
                    (true, _) => Vec4::new(0.6547124, 0.2, 1., 0.4),
                },
                fog_ramp: Vec4::new(0., 0., 1., 0.),
                fog_color: Vec4::ZERO,
                shadow_color: Vec4::ZERO,
                sun_direction: Vec3::new(4., 7., 4.).normalize().extend(0.),
                decal: Vec4::new(
                    // decalenvironment lerps by the full decal alpha.
                    if skate2 {
                        1.
                    } else {
                        stain_opacity(
                            self.parameters
                                .get("decal")
                                .and_then(|v| v.first())
                                .map(String::as_str)
                                .unwrap_or(""),
                        )
                    },
                    0.,
                    0.,
                    0.,
                ),
                water,
            },
            diffuse,
            lightmap,
            normal,
            detail,
            macro_map,
            decal,
            specular,
            environment,
            diffuse2,
            shadow_state: shadow::BUFFER,
            alpha,
            two_sided: self.flags & 4 != 0,
        }
    }
}

// The source labels distinguish weathering from graphics. This is an explicit
// visual tuning choice, not a recovered native material constant. Keep arrows,
// logos, paint, scratches and edge wear at their authored alpha.
fn stain_opacity(texture_label: &str) -> f32 {
    let label = texture_label.to_ascii_lowercase();
    if [
        "grime",
        "grunge",
        "stain",
        "oildirt",
        "drainage",
        "ground_decals",
    ]
    .iter()
    .any(|word| label.contains(word))
    {
        0.35
    } else {
        1.
    }
}

/// SKATE_S2_LIGHTING=0 draws Skate 2 maps with the Skate 3 world path (bit 1024 and the
/// Skate 2 family upgrades off), to compare without a rebuild.
static S2_LIGHTING: std::sync::LazyLock<bool> =
    std::sync::LazyLock::new(|| s2_lighting_switch(std::env::var_os("SKATE_S2_LIGHTING").as_deref()));
fn s2_lighting_switch(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_none_or(|v| v != "0")
}

fn skate2_lighting(parameters: &BTreeMap<String, Vec<String>>) -> bool {
    if !*S2_LIGHTING {
        return false;
    }
    let first = |name: &str| {
        parameters
            .get(name)
            .and_then(|v| v.first())
            .map_or("", String::as_str)
    };
    first("lightmap").starts_with("layerpage_") && first("chromaticity").starts_with("chromo_")
}

/// SKATE_S1_LIGHTING=0 draws the Skate 1 map with the Skate 3 world path (bit 16384 off); same rule as Skate 2's.
static S1_LIGHTING: std::sync::LazyLock<bool> =
    std::sync::LazyLock::new(|| s2_lighting_switch(std::env::var_os("SKATE_S1_LIGHTING").as_deref()));

/// Skate 1 package evidence: its converted lightmaps are "tex_0x…" (Skate 3 "lightpage_", Skate 2 "layerpage_").
fn skate1_lighting(parameters: &BTreeMap<String, Vec<String>>) -> bool {
    *S1_LIGHTING
        && parameters
            .get("lightmap")
            .and_then(|v| v.first())
            .is_some_and(|l| l.starts_with("tex_0x"))
}

#[cfg(test)]
mod stain_tests {
    use super::stain_opacity;
    #[test]
    fn weathering_tuning_preserves_artwork() {
        for name in [
            "decal_WEAR_WaterStain_01",
            "subway_grunge03",
            "decal_GrimePuddle",
            "OT_Ground_decals",
        ] {
            assert_eq!(stain_opacity(name), 0.35);
        }
        for name in [
            "decal_Graphic_SP_UN_Shark_01",
            "decal_other_sp_arrowramps_01",
            "decal_Graphic_SP_UN_MegaRmp_01",
            "decal_Wear_GL_UN_MPedge_01",
            "decal_skateboardscratcheswood01",
            "",
        ] {
            assert_eq!(stain_opacity(name), 1.);
        }
    }
}

#[cfg(test)]
mod shader_path_tests {
    use super::*;
    /// perf 11-01 (SKATE_RETAIL_SPECIALIZE): a material's world-shader path names its game and family group; with
    /// both game flags it takes the shader that reads them.
    #[test]
    fn a_world_material_names_its_shader_path() {
        let mode = |fam: f32, flags: u32| Vec4::new(fam, flags as f32, 0., 1.);
        assert_eq!(shader_path(mode(9., 16384)), 8 + 2, "a Skate 1 tree");
        assert_eq!(shader_path(mode(31., 1024)), 16 + 1, "Skate 2 water");
        assert_eq!(shader_path(mode(12., 0)), 24 + 3, "a Skate 3 lamp");
        assert_eq!(shader_path(mode(1., 1024 | 32768)), 16 + 4, "Skate 2 terrain: the rest");
        assert_eq!(shader_path(mode(1., 16384 | 1024)), 0, "both games: every path");
    }
}

#[cfg(test)]
mod skate2_lighting_tests {
    use super::*;
    const S2: [(&str, &str); 2] = [
        ("lightmap", "layerpage_[0x0000008903e38704]_HighLOD_0007"),
        ("chromaticity", "chromo_[0x0000008903e38704]_HighLOD_0013"),
    ];
    fn build(shader: &str, family: u32, params: &[(&str, &str)]) -> (u32, WorldParams) {
        let (family, material) = material(shader, family, params);
        (family, material.params)
    }
    fn definition(shader: &str, family: u32, params: &[(&str, &str)]) -> Vec<u8> {
        bound_definition(shader, family, &[], params)
    }
    /// `definition` with texture bindings (role, map texture index), as map_writer writes them.
    fn bound_definition(shader: &str, family: u32, bindings: &[(&str, u32)], params: &[(&str, &str)]) -> Vec<u8> {
        let text = |b: &mut Vec<u8>, s: &str| {
            b.extend((s.len() as u32).to_le_bytes());
            b.extend(s.as_bytes());
        };
        let mut b = vec![0; 16];
        text(&mut b, shader);
        for v in [family, 0, bindings.len() as u32] {
            b.extend(v.to_le_bytes());
        }
        for (role, texture) in bindings {
            text(&mut b, role);
            for v in [*texture, 0, 0, 0] {
                b.extend(v.to_le_bytes());
            }
        }
        b.extend((params.len() as u32).to_le_bytes());
        for (k, v) in params {
            text(&mut b, k);
            b.extend(1u32.to_le_bytes());
            text(&mut b, v);
        }
        text(&mut b, "");
        b
    }
    fn material(shader: &str, family: u32, params: &[(&str, &str)]) -> (u32, RetailWorldMaterial) {
        material_with(shader, family, params, &MaterialTuning::default())
    }
    fn material_with(
        shader: &str,
        family: u32,
        params: &[(&str, &str)],
        tuning: &MaterialTuning,
    ) -> (u32, RetailWorldMaterial) {
        let b = definition(shader, family, params);
        let d = Definition::parse(&b).unwrap();
        let m = skate_data::skate_map::Material {
            name: String::new(),
            flags: 0,
            friction: 0.,
            restitution: 0.,
            color: [1.; 3],
            roughness: 1.,
            emissive: 0.,
            textures: [0; 5],
            indirect_strength: 1.,
            alpha_mode: 0,
            alpha_cutoff: 0.,
            audio: 0,
            physics: 0,
            pattern: 0,
            depth_layer: None,
            retail_definition: Some(b),
        };
        (d.family, d.build(&m, tuning, &mut |_, _| None))
    }
    #[test]
    fn skate2_lighting_follows_package_evidence_only() {
        let s3 = Vec4::new(0.3435, 0.02, 1., 0.45);
        // Skate 3 / Skate 1 lightmaps, or a layer page without chromaticity, keep everything.
        for params in [
            &[("lightmap", "lightpage_0x1"), ("decal", "decal_GrimePuddle")][..],
            &[("lightmap", "tex_0x00001234"), ("decal", "decal_GrimePuddle")],
            &[S2[0], ("decal", "decal_GrimePuddle")],
        ] {
            let (family, p) = build("environment.decal", 3, params);
            assert_eq!((family, p.mode.y as u32 & 1024), (3, 0));
            assert_eq!((p.family, p.decal.x), (s3, 0.35));
        }
        let (_, p) = build("environment.decal", 3, &[S2[0], S2[1], ("decal", "decal_GrimePuddle")]);
        assert_eq!(p.mode.y as u32 & 1024, 1024);
        assert_eq!((p.family, p.decal.x), (Vec4::new(0.6547124, 0.2, 1., 0.4), 1.));
        // Vault incandescent m_params; backlit/videoscreen join the baseincandescent family.
        for (shader, stored, family, params) in [
            ("incandescent.default", 12, 12, Vec4::new(0., 10., 0., 0.)),
            ("incandescent.backlit", 0, 12, Vec4::new(0.75, 1., 0., 0.)),
            ("incandescent.backlituvscroll", 14, 14, Vec4::new(0.75, 1., 0., 0.)),
        ] {
            assert_eq!(build(shader, stored, &S2).0, family, "{shader}");
            assert_eq!(build(shader, stored, &S2).1.family, params, "{shader}");
            assert_eq!(build(shader, stored, &[]).0, stored, "{shader}");
            assert_eq!(build(shader, stored, &[]).1.family, s3, "{shader}");
        }
        // Variants: environment.transparent and decal.default join family 1, backlitzbias 12;
        // each alpha-tests its output alpha at 16/255 and draws in the blended pass.
        let variants = 2048 | 4096 | 8192;
        for (shader, family, bit, params) in [
            ("environment.transparent", 1, 2048, Vec4::new(0.6547124, 0.2, 1., 0.4)),
            ("decal.default", 1, 4096, Vec4::new(0.6547124, 0.2, 1., 0.4)),
            ("incandescent.backlitzbias", 12, 8192, Vec4::new(0.75, 1., 0., 0.)),
        ] {
            let (f, p) = build(shader, 0, &S2);
            assert_eq!((f, p.mode.y as u32 & (1024 | variants), p.mode.z, p.family), (family, 1024 | bit, 16. / 255., params), "{shader}");
            let (f, p) = build(shader, 0, &[]);
            assert_eq!((f, p.mode.y as u32 & (1024 | variants), p.mode.z), (0, 0, -1.), "{shader}");
        }
    }
    /// #33: both games' videoscreens draw the scene feed through baseincandescent (TU3 827E596C, Skate 2 8268FFE0),
    /// at the vault's m_params (Skate 2 (0, 1) here; Skate 3's comes from render-parameters.json).
    #[test]
    fn videoscreens_sample_the_jumbotron_feed() {
        for params in [&S2[..], &[][..]] {
            let (family, m) = material("incandescent.videoscreen", 0, params);
            assert_eq!((family, m.diffuse.map(|h| h.id())), (12, Some(FEED.id())));
        }
        assert_eq!(build("incandescent.videoscreen", 0, &S2).1.family, Vec4::new(0., 1., 0., 0.));
        // Other incandescents keep their own texture (none in this fixture).
        for (shader, family) in [("incandescent.default", 12), ("incandescent.backlit", 0)] {
            assert_eq!(material(shader, family, &[]).1.diffuse, None, "{shader}");
        }
    }
    /// Brief 10-29: both games' oceans draw once the PCA is there. Skate 2's ocean_defaultPS (s2 render.big) is
    /// Skate 3's with its layer-page lightmap (xemu, 10-29). Its still water (water.default) keeps the fallback.
    #[test]
    fn skate2_ocean_draws_once_the_pca_is_there() {
        let tuning = MaterialTuning {
            rows: BTreeMap::from([("ocean.default".to_owned(), vec![[0.; 4]; 3])]),
            pca_available: true,
        };
        let drawn = |params: &[(&str, &str)]| {
            Definition::parse(&definition("ocean.default", 31, params)).unwrap().supported(&tuning)
        };
        assert!(drawn(&[("lightmap", "lightpage_0x1")]));
        assert!(drawn(&S2));
        // Skate 2's vault material_ocean default m_params differ from Skate 3's (render-parameters.json) in row 1 .y.
        let rows = vec![[0.25, 0.19475, 0.0875, 1.], [0.4, 0.2, 0.22, 44.], [14., 9., 1.5, 0.]];
        let tuning = MaterialTuning { rows: BTreeMap::from([("ocean.default".to_owned(), rows)]), pca_available: true };
        let level = |params: &[(&str, &str)]| {
            let water = material_with("ocean.default", 31, params, &tuning).1.params.water;
            (water[1].y, water[1].w)
        };
        assert_eq!(level(&[("lightmap", "lightpage_0x1")]), (0.2, 44.));
        assert_eq!(level(&S2), (1.2, 44.));
    }
    /// GRADING_AUDIT_S2.md §7: terrain.default (baseterrain) and terrain.decal (decalterrain) join the Skate 2
    /// families 1 / 3 with bit 32768 once the converter binds their slope maps; older maps keep the fallback.
    #[test]
    fn skate2_terrain_blends_its_slope_maps_once_they_are_bound() {
        let tuning = MaterialTuning::default();
        let roles = [("diffuse", 1), ("lightmap", 2), ("normal", 3), ("normal2", 4), ("macrooverlay", 5),
            ("specular", 6), ("diffuse2", 7), ("specular2", 8), ("decal", 9)];
        let scale = ("diffuse2UVScale", "0.03");
        let parse = |shader: &str, bound: &[&str], params: &[(&str, &str)]| {
            let bindings: Vec<_> = roles.iter().copied().filter(|(r, _)| bound.contains(r)).collect();
            let b = bound_definition(shader, 0, &bindings, params);
            (Definition::parse(&b).unwrap(), b)
        };
        let all = ["diffuse", "lightmap", "normal", "normal2", "macrooverlay", "specular", "diffuse2", "specular2"];
        let old = ["diffuse", "lightmap", "normal", "normal2", "macrooverlay", "specular"];
        let s2 = [S2[0], S2[1], scale];
        // Unbound slope maps (maps converted before), Skate 3 / no evidence, or no specular2 on terrain.default: fallback.
        for (shader, bound, params) in [
            ("terrain.default", &old[..], &s2[..]),
            ("terrain.default", &all[..], &[scale][..]),
            ("terrain.default", &all[..7], &s2[..]),
            ("terrain.decal", &old[..], &s2[..]),
        ] {
            let (d, _) = parse(shader, bound, params);
            assert_eq!(d.family, 0, "{shader} {bound:?}");
            assert!(!d.supported(&tuning), "{shader} {bound:?}");
        }
        let id = |t: u32| Handle::<Image>::Uuid(bevy::asset::uuid::Uuid::from_u128(t as u128), Default::default());
        let make = |d: &Definition, b: Vec<u8>| {
            let source = skate_data::skate_map::Material {
                name: String::new(), flags: 0, friction: 0., restitution: 0., color: [1.; 3], roughness: 1.,
                emissive: 0., textures: [0; 5], indirect_strength: 1., alpha_mode: 0, alpha_cutoff: 0., audio: 0,
                physics: 0, pattern: 0, depth_layer: None, retail_definition: Some(b),
            };
            d.build(&source, &tuning, &mut |t, _| (t != 0).then(|| id(t)))
        };
        // terrain.default: normal2 in the detail slot, specular2 in the decal slot, diffuse2 in its own;
        // vault m_params[1] (class DDFD2CF3C65C265B) in `family`, diffuse2UVScale in surface.z.
        let (d, b) = parse("terrain.default", &all[..], &s2[..]);
        assert_eq!(d.family, 1);
        assert!(d.supported(&tuning));
        let m = make(&d, b);
        let flags = m.params.mode.y as u32;
        assert_eq!(flags & (1024 | 32768 | 128 | 8 | 2), 1024 | 32768 | 128 | 8);
        assert_eq!((m.params.family, m.params.surface.z), (Vec4::new(8., 0.04, 8., -0.1), 0.03));
        assert_eq!(
            [&m.diffuse, &m.detail, &m.decal, &m.specular, &m.diffuse2].map(|h| h.clone()),
            [Some(id(1)), Some(id(4)), Some(id(8)), Some(id(6)), Some(id(7))]
        );
        assert_eq!((m.alpha, m.params.mode.z), (AlphaMode::Opaque, -1.));
        // terrain.decal: decalterrain reads i_specular for both; the decal keeps its slot and vertex-colour UVs.
        let (d, b) = parse("terrain.decal", &[&old[..], &["diffuse2", "decal"][..]].concat()[..], &s2[..]);
        assert_eq!(d.family, 3);
        let m = make(&d, b);
        assert_eq!(m.params.mode.y as u32 & (1024 | 32768 | 8), 1024 | 32768 | 8);
        assert_eq!([&m.decal, &m.diffuse2].map(|h| h.clone()), [Some(id(9)), Some(id(7))]);
        assert!(m.samples_decal_uv());
        // Other Skate 2 materials never get the bit or a diffuse2.
        let (_, p) = build("environment.decal", 3, &s2[..]);
        assert_eq!(p.mode.y as u32 & 32768, 0);
    }
    #[test]
    fn skate2_lighting_switch_is_on_unless_zero() {
        use std::ffi::OsStr;
        assert!(s2_lighting_switch(None));
        assert!(s2_lighting_switch(Some(OsStr::new("1"))));
        assert!(s2_lighting_switch(Some(OsStr::new(""))));
        assert!(!s2_lighting_switch(Some(OsStr::new("0"))));
    }
    #[test]
    fn skate1_lighting_follows_package_evidence_only() {
        let s3 = Vec4::new(0.3435, 0.02, 1., 0.45);
        let s1 = ("lightmap", "tex_0x33c23eae66843942");
        // Skate 1's package families (0, 2, 9, 10) gain bit 16384 and nothing else changes.
        for (shader, family) in [("environmentsimple.default", 2), ("environment.transparent", 0), ("tree.default", 9), ("animated.tree", 10)] {
            let (f, p) = build(shader, family, &[s1, ("decal", "decal_GrimePuddle")]);
            let (_, q) = build(shader, family, &[("lightmap", "lightpage_0x1"), ("decal", "decal_GrimePuddle")]);
            assert_eq!((f, p.mode.y as u32), (family, q.mode.y as u32 | 16384), "{shader}");
            assert_eq!((p.mode.x, p.mode.z, p.mode.w, p.family, p.decal.x, p.surface), (q.mode.x, q.mode.z, q.mode.w, s3, 0.35, q.surface), "{shader}");
        }
        // Skate 2 and Skate 3 never get it; Skate 2 keeps 1024.
        for (params, s2) in [(&[S2[0], S2[1]][..], 1024), (&[("lightmap", "lightpage_0x1")][..], 0), (&[S2[0]][..], 0), (&[][..], 0)] {
            assert_eq!(build("environment.decal", 3, params).1.mode.y as u32 & (1024 | 16384), s2);
        }
    }
}

#[cfg(test)]
#[test]
fn texture_quality_halves_big_textures_and_keeps_small_ones() {
    let face = |w: usize, h: usize| (0..w * h).flat_map(|i| [(i % 2 * 200) as u8, 10, 20, 255]).collect::<Vec<u8>>();
    let big = face(256, 128);
    assert_eq!(shrink(&big, 256, 128, 1, 0), (big.clone(), 256, 128), "High leaves it alone");
    let (half, w, h) = shrink(&big, 256, 128, 1, 1);
    assert_eq!((w, h, half.len()), (128, 64, 128 * 64 * 4));
    assert_eq!(&half[..4], &[100, 10, 20, 255], "box filtered, as the mip chain is");
    assert_eq!(shrink(&big, 256, 128, 1, 2).1, 64);
    assert_eq!(shrink(&big, 256, 128, 1, 3).1, 64, "never under 64 on the larger side");
    // BC7 needs 4-texel blocks: a halving that would break them (and fall back to 4x the memory) is skipped.
    assert_eq!(shrink(&face(256, 4), 256, 4, 1, 2).1, 256);
    assert_eq!(shrink(&face(256, 16), 256, 16, 1, 2).1, 64);
    let small = face(64, 64);
    assert_eq!(shrink(&small, 64, 64, 1, 2).0, small, "small textures keep their detail");
    let cube: Vec<u8> = (0..6).flat_map(|_| face(128, 128)).collect();
    let (faces, w, _) = shrink(&cube, 128, 128, 6, 1);
    assert_eq!((w, faces.len()), (64, 6 * 64 * 64 * 4), "every cube face halves");
}

/// One 2x2 box-filter step of an RGBA level.
fn halve(level: &[u8], w: usize, h: usize) -> Vec<u8> {
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let mut next = vec![0; nw * nh * 4];
    for y in 0..nh {
        for x in 0..nw {
            for c in 0..4 {
                let mut sum = 0u32;
                for dy in 0..2 {
                    for dx in 0..2 {
                        sum += level[((y * 2 + dy).min(h - 1) * w + (x * 2 + dx).min(w - 1)) * 4 + c] as u32;
                    }
                }
                next[(y * nw + x) * 4 + c] = ((sum + 2) / 4) as u8;
            }
        }
    }
    next
}

/// Native renderer box-filters decoded UNORM cube faces down to 1x1.
/// The same chain prevents aliasing of repeating world maps at distance.
pub(crate) fn mip_chain(rgba: &[u8], width: u32, height: u32, layers: u32) -> (Vec<u8>, u32) {
    let count = 32 - width.max(height).leading_zeros();
    let mut bytes = Vec::with_capacity(rgba.len() * 4 / 3 + 64);
    for face in rgba
        .chunks_exact((width * height * 4) as usize)
        .take(layers as usize)
    {
        let mut level = face.to_vec();
        let (mut w, mut h) = (width as usize, height as usize);
        bytes.extend_from_slice(&level);
        while w > 1 || h > 1 {
            let next = halve(&level, w, h);
            bytes.extend_from_slice(&next);
            level = next;
            (w, h) = ((w / 2).max(1), (h / 2).max(1));
        }
    }
    (bytes, count)
}

/// Settings > Graphics > Texture Quality: each layer halved `steps` times, the same texels as dropping the top mips,
/// stopping once the larger side would fall under 64 so small textures keep their detail.
pub(crate) fn shrink(rgba: &[u8], width: u32, height: u32, layers: u32, steps: u32) -> (Vec<u8>, u32, u32) {
    let (mut w, mut h) = (width as usize, height as usize);
    let mut faces: Vec<Vec<u8>> = rgba.chunks_exact(w * h * 4).take(layers as usize).map(<[u8]>::to_vec).collect();
    for _ in 0..steps {
        let aligned = w % 4 == 0 && h % 4 == 0;
        if w.max(h) / 2 < 64 || aligned && ((w / 2) % 4 != 0 || (h / 2) % 4 != 0) {
            break;
        }
        faces = faces.iter().map(|f| halve(f, w, h)).collect();
        (w, h) = ((w / 2).max(1), (h / 2).max(1));
    }
    (faces.concat(), w as u32, h as u32)
}

#[derive(Default)]
pub(crate) struct MaterialTuning {
    rows: BTreeMap<String, Vec<[f32; 4]>>,
    pca_available: bool,
}
impl MaterialTuning {
    pub(crate) fn load(root: &std::path::Path) -> Self {
        let path = root.join("private/render-parameters.json");
        match std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<BTreeMap<String, Vec<[f32; 4]>>>(&b).ok())
        {
            Some(rows) if rows.values().flatten().flatten().all(|x| x.is_finite()) => Self {
                rows,
                pca_available: shadow::pca_available(root),
            },
            _ => {
                warn!("Retail water/scroll tuning unavailable: {}", path.display());
                Self::default()
            }
        }
    }
}

pub(crate) fn world_changed(
    mut messages: MessageReader<crate::map_transition::WorldChanged>,
) -> bool {
    messages.read().count() != 0
}

#[cfg(test)]
#[test]
fn a_modified_mesh_marks_its_users_but_static_map_batches_sit_out() {
    let mut app = App::new();
    app.add_plugins(bevy::asset::AssetPlugin::default()).init_asset::<Mesh>().add_systems(PostUpdate, mark_changed_meshes.after(bevy::asset::AssetEventSystems));
    let mesh = app.world_mut().resource_mut::<Assets<Mesh>>().add(Mesh::from(Cuboid::default()));
    let moving = app.world_mut().spawn(Mesh3d(mesh.clone())).id();
    let map = app.world_mut().spawn((Mesh3d(mesh.clone()), shadow_visibility::StaticShadowCaster)).id();
    app.update();
    app.update();
    let tick = app.world().read_change_tick();
    app.world_mut().resource_mut::<Assets<Mesh>>().get_mut(&mesh);
    app.update();
    let changed = |e: Entity| app.world().entity(e).get_ref::<Mesh3d>().unwrap().last_changed().is_newer_than(tick, app.world().read_change_tick());
    assert!(changed(moving));
    assert!(!changed(map));
}

#[cfg(test)]
#[test]
fn only_materials_whose_cutoff_can_discard_keep_the_alpha_test() {
    let v = Vec4::ZERO;
    let key = |cutoff: f32| {
        let material = RetailWorldMaterial {
            params: WorldParams { mode: Vec4::new(0., 0., cutoff, 0.), foliage_debug: v, surface: v, family: v,
                fog_ramp: v, fog_color: v, shadow_color: v, sun_direction: v, decal: v, water: [v; 4] },
            diffuse: None, lightmap: None, normal: None, detail: None, macro_map: None, decal: None, specular: None,
            environment: None, diffuse2: None, shadow_state: Handle::default(), alpha: AlphaMode::Opaque, two_sided: false,
        };
        RetailKey::from(&material).alpha_test
    };
    // Opaque retail materials carry -1; ALPHAREF 30 masks and Skate 2's opaque 16/255 alpha test must keep it.
    assert!(!key(-1.));
    assert!(key(30. / 255.) && key(16. / 255.) && key(0.));
}

#[cfg(test)]
#[test]
fn depth_passes_skip_the_material_exactly_when_the_depth_shader_cannot_discard() {
    let v = Vec4::ZERO;
    let material = |cutoff: f32| RetailWorldMaterial {
        params: WorldParams { mode: Vec4::new(0., 0., cutoff, 0.), foliage_debug: v, surface: v, family: v,
            fog_ramp: v, fog_color: v, shadow_color: v, sun_direction: v, decal: v, water: [v; 4] },
        diffuse: None, lightmap: None, normal: None, detail: None, macro_map: None, decal: None, specular: None,
        environment: None, diffuse2: None, shadow_state: Handle::default(), alpha: AlphaMode::Opaque, two_sided: false,
    };
    for cutoff in [-1., 0., 16. / 255., 30. / 255.] {
        let m = material(cutoff);
        // RETAIL_NO_ALPHA_TEST strips every material read from retail_depth.wgsl; with the test kept it samples the slab.
        let skip = !RetailKey::from(&m).alpha_test;
        assert_eq!(Material::depth_passes_skip_material(&m), skip, "cutoff {cutoff}");
        assert_eq!(Material::depth_passes_skip_material(&RetailBlendMaterial(m)), skip, "blend cutoff {cutoff}");
    }
}

/// perf 10-18 (vendor/README.md): binned batch sets sort by material slab before pipeline, so a pass binds each slab
/// once; equal keys still compare equal.
#[cfg(test)]
#[test]
fn batch_sets_sort_by_material_slab_before_pipeline() {
    use bevy::{
        core_pipeline::core_3d::{Opaque3d, Opaque3dBatchSetKey},
        render::{
            mesh::allocator::SlabId,
            render_phase::{Draw, DrawError, DrawFunctions, TrackedRenderPass},
            render_resource::CachedRenderPipelineId,
        },
    };
    struct Nothing;
    impl Draw<Opaque3d> for Nothing {
        fn draw<'w>(&mut self, _: &'w World, _: &mut TrackedRenderPass<'w>, _: Entity, _: &Opaque3d) -> Result<(), DrawError> {
            Ok(())
        }
    }
    let functions = DrawFunctions::<Opaque3d>::default();
    // One guard: a second write() in the same statement would wait on the first forever.
    let (a, b) = {
        let mut functions = functions.write();
        (functions.add(Nothing), functions.add(Nothing))
    };
    let key = |draw_function, slab| Opaque3dBatchSetKey {
        pipeline: CachedRenderPipelineId::INVALID,
        draw_function,
        material_bind_group_index: Some(slab),
        vertex_slab: SlabId::default(),
        index_slab: None,
        lightmap_slab: None,
    };
    let mut keys = vec![key(b, 1), key(a, 1), key(b, 0), key(a, 0)];
    keys.sort();
    let order: Vec<_> = keys.iter().map(|k| (k.draw_function == a, k.material_bind_group_index)).collect();
    assert_eq!(order, [(true, Some(0)), (false, Some(0)), (true, Some(1)), (false, Some(1))]);
    assert_eq!(key(a, 3).cmp(&key(a, 3)), std::cmp::Ordering::Equal);
}
