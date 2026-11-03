//! Character SH/key/rim shader, independent of the world's additive lights.
use crate::retail_irradiance::Irradiance;
use bevy::{
    asset::embedded_asset,
    camera::visibility::RenderLayers,
    gltf::GltfMaterialName,
    prelude::*,
    render::{
        Render, RenderApp, RenderSystems,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_asset::RenderAssets,
        render_resource::{AsBindGroup, BufferUsages, ShaderType},
        renderer::RenderQueue,
        storage::{GpuShaderStorageBuffer, ShaderStorageBuffer},
    },
    shader::ShaderRef,
};
use serde::Deserialize;
use std::collections::HashMap;

pub(crate) struct CharacterLightingPlugin;
impl Plugin for CharacterLightingPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "retail_character.wgsl");
        embedded_asset!(app, "retail_character_depth.wgsl");
        embedded_asset!(app, "retail_character_common.wgsl");
        let common: Handle<Shader> = bevy::asset::load_embedded_asset!(
            app.world().resource::<AssetServer>(),
            "retail_character_common.wgsl"
        );
        app.insert_resource(CharacterShader(common));
        // Shadow pass is fill-bound: 2048 -> 1024 cut it 6.4 -> 2.4 ms on an RTX 2050.
        // Shadow Quality adds player cascades instead (apply_shadow_quality).
        app.insert_resource(bevy::light::DirectionalLightShadowMap { size: 1024 });
        app.add_plugins((MaterialPlugin::<CharacterMaterial>::default(), ExtractResourcePlugin::<SharedLighting>::default()))
            .init_resource::<SharedLighting>()
            .add_systems(Startup, (insert_lighting_buffer, load))
            .add_systems(
                PreUpdate,
                load.after(crate::map_transition::MapTransitionSet)
                    .run_if(crate::retail_render::world_changed),
            )
            .add_systems(Update, ((bind, shadow_views, update).chain(), apply_shadow_quality));
        if let Some(render) = app.get_sub_app_mut(RenderApp) {
            render.add_systems(Render, upload_lighting.in_set(RenderSystems::PrepareResources));
        }
    }
}

/// The world's character lighting: the map's light, the probe SH where the player stands and the time of day's key.
/// Every character material, and every skater material lit by the world (tint.w != 0, key.w >= 0; team photos and
/// tiles keep their own), reads it from one storage buffer (perf 10-12, C4). Writing it into each material instead
/// re-prepared all of them, remote skaters' too, each frame the smoothed SH moved.
#[derive(Resource, Clone, Copy, ExtractResource)]
struct SharedLighting {
    light: Vec4,
    sh: [Vec4; 9],
    key: Vec4,
}
impl Default for SharedLighting {
    /// What a skater material starts with (customiser_parts): light.w 0 leaves it to Bevy's lighting.
    fn default() -> Self {
        Self { light: Vec4::ZERO, sh: [Vec4::ZERO; 9], key: Vec4::ONE }
    }
}
impl SharedLighting {
    fn bytes(&self) -> impl Iterator<Item = u8> + '_ {
        std::iter::once(self.light).chain(self.sh).chain([self.key]).flat_map(|v| v.to_array()).flat_map(f32::to_le_bytes)
    }
}
pub(crate) const LIGHTING: Handle<ShaderStorageBuffer> = bevy::asset::uuid_handle!("8c3f1b52-6d0e-4b7a-9e21-5a4c7d2f9b13");

fn insert_lighting_buffer(shared: Res<SharedLighting>, mut buffers: ResMut<Assets<ShaderStorageBuffer>>) {
    let mut buffer = ShaderStorageBuffer::new(&shared.bytes().collect::<Vec<u8>>(), bevy::asset::RenderAssetUsages::default());
    buffer.buffer_description.usage |= BufferUsages::COPY_DST;
    buffers.insert(LIGHTING.id(), buffer).expect("reserved character lighting asset");
}

/// For a test that draws skaters without the game's plugins (team photos, Custom Graphic tiles): [`LIGHTING`], which
/// every skater material binds. Without it no skater material is ever prepared and Bevy skips the meshes unlogged.
#[cfg(test)]
pub(crate) fn lighting_for_tests(app: &mut App) {
    app.init_resource::<SharedLighting>().add_systems(Startup, insert_lighting_buffer);
}

fn upload_lighting(
    shared: Res<SharedLighting>,
    buffers: Res<RenderAssets<GpuShaderStorageBuffer>>,
    queue: Res<RenderQueue>,
    mut bytes: Local<Vec<u8>>,
) {
    // Extracted only when it changed; a (re)created GPU buffer needs the data too.
    if !shared.is_changed() && !buffers.is_changed() {
        return;
    }
    if let Some(buffer) = buffers.get(LIGHTING.id()) {
        bytes.clear();
        bytes.extend(shared.bytes());
        queue.write_buffer(&buffer.buffer, 0, &bytes);
    }
}
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct MaterialData {
    pub shader: String,
    pub params: Vec<[f32; 4]>,
    pub specular: Option<String>,
    /// Coverage map (R) on the second UV set: retail hair/alpha shaders (defaulthair_defaultPS tf5,
    /// cac_alpha_defaultPS) cut by their own alpha map on TEXCOORD1, not by the diffuse alpha.
    #[serde(default)]
    pub opacity: Option<String>,
}
impl MaterialData {
    pub(crate) fn is_hair(&self) -> bool {
        matches!(
            self.shader.as_str(),
            "character.hair"
                | "character.hair_ropa"
                | "character.default_hair"
                | "character.default_hair_ropa"
        )
    }
}

#[derive(Resource)]
struct CharacterShader(#[allow(dead_code)] Handle<Shader>);
#[derive(Deserialize)]
struct LightingData {
    materials: HashMap<String, MaterialData>,
    default_sh: [[f32; 3]; 9],
    #[serde(default)]
    native: HashMap<String, HashMap<String, MaterialData>>,
}
#[derive(Resource)]
struct Lighting {
    data: LightingData,
    probes: Irradiance,
    light: Vec4,
    /// The map's sun; `light` swaps in a cutscene shot's ([`crate::retail_render::ShotPost`]).
    sun: Vec3,
    display_sh: Option<[Vec4; 9]>,
}
#[derive(Clone, Debug, ShaderType)]
pub(crate) struct CharacterParams {
    pub light: Vec4,
    pub tint: Vec4,
    // normal map present, dedicated specular mask, alpha cutoff, hair family
    pub options: Vec4,
    pub rows: [Vec4; 9],
    pub sh: [Vec4; 9],
    /// Time of day's key light colour (retail_daynight.rs): scales the key, rim and
    /// specular rows. One at noon.
    pub key: Vec4,
}
impl Default for CharacterParams {
    fn default() -> Self {
        Self { light: Vec4::ZERO, tint: Vec4::ZERO, options: Vec4::ZERO, rows: [Vec4::ZERO; 9], sh: [Vec4::ZERO; 9], key: Vec4::ONE }
    }
}
#[derive(Asset, TypePath, AsBindGroup, Clone)]
#[bind_group_data(CharacterKey)]
struct CharacterMaterial {
    #[uniform(0)]
    params: CharacterParams,
    #[texture(1)]
    #[sampler(2)]
    diffuse: Option<Handle<Image>>,
    #[texture(3)]
    #[sampler(4)]
    normal: Option<Handle<Image>>,
    #[texture(5)]
    #[sampler(6)]
    mask: Option<Handle<Image>>,
    /// Sampled on uv_b; only native pieces with a coverage map carry a second UV set.
    #[texture(7)]
    #[sampler(8)]
    coverage: Option<Handle<Image>>,
    /// Always [`LIGHTING`]: its light, SH and key stand in for `params`' (retail_character.wgsl).
    #[storage(9, read_only)]
    lighting: Handle<ShaderStorageBuffer>,
    alpha: AlphaMode,
}
/// A hair-card material (a coverage map) draws both faces, as customiser_parts draws the player's opacity pieces: the
/// native cards have no back-face twins (user 10-29: the cast's hair showed white where the sky shone through).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct CharacterKey {
    two_sided: bool,
}
impl From<&CharacterMaterial> for CharacterKey {
    fn from(m: &CharacterMaterial) -> Self {
        Self { two_sided: m.coverage.is_some() }
    }
}
impl Material for CharacterMaterial {
    fn specialize(
        _: &bevy::pbr::MaterialPipeline,
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        _: &bevy::mesh::MeshVertexBufferLayoutRef,
        key: bevy::pbr::MaterialPipelineKey<Self>,
    ) -> Result<(), bevy::render::render_resource::SpecializedMeshPipelineError> {
        if key.bind_group_data.two_sided {
            descriptor.primitive.cull_mode = None;
        }
        Ok(())
    }
    fn fragment_shader() -> ShaderRef {
        bevy::asset::AssetPath::from(bevy::asset::embedded_path!("retail_character.wgsl"))
            .with_source("embedded")
            .into()
    }
    fn prepass_fragment_shader() -> ShaderRef {
        bevy::asset::AssetPath::from(bevy::asset::embedded_path!("retail_character_depth.wgsl"))
            .with_source("embedded")
            .into()
    }
    fn alpha_mode(&self) -> AlphaMode {
        self.alpha
    }
}
#[derive(Component)]
struct ShadowSource;
/// The player-only shadow light that Shadow Quality tunes.
#[derive(Component)]
struct PlayerShadow;

#[derive(Component)]
struct OriginalCharacterMaterial {
    material: Handle<StandardMaterial>,
    layers: Option<RenderLayers>,
}
fn load(
    mut commands: Commands,
    config: Res<crate::config::Config>,
    sources: Query<Entity, With<ShadowSource>>,
    mut shadow: ResMut<crate::retail_render::ShadowState>,
    retail: Res<crate::retail_render::RetailScene>,
    originals: Query<(Entity, &OriginalCharacterMaterial)>,
    cameras: Query<(Entity, &RenderLayers), With<crate::camera::GameplayCamera>>,
    mut customiser: ResMut<Assets<crate::customiser_material::SkaterMaterial>>,
    mut shared: ResMut<SharedLighting>,
) {
    for (_, material) in customiser.iter_mut().filter(|(_, m)| m.extension.retail.key.w >= 0.) {
        material.extension.retail.light = Vec4::ZERO;
    }
    // The skater materials lit by the world read this light: off until a retail map's lighting loads below.
    shared.light = Vec4::ZERO;
    commands.remove_resource::<Lighting>();
    for e in &sources {
        commands.entity(e).despawn();
    }
    *shadow = Default::default();
    for (entity, original) in &originals {
        let mut entity = commands.entity(entity);
        entity
            .remove::<(MeshMaterial3d<CharacterMaterial>, OriginalCharacterMaterial)>()
            .insert(MeshMaterial3d(original.material.clone()));
        if let Some(layers) = &original.layers {
            entity.insert(layers.clone());
        } else {
            entity.remove::<RenderLayers>();
        }
    }
    for (entity, layers) in &cameras {
        commands.entity(entity).insert(layers.clone().without(28));
    }
    if !retail.0 {
        return;
    }
    let Some(map_path) = &config.map_path else {
        return;
    };
    let path = config.asset_root.join("private/character-lighting.json");
    let mut data = match std::fs::read(&path)
        .map_err(|e| e.to_string())
        .and_then(|b| serde_json::from_slice::<LightingData>(&b).map_err(|e| e.to_string()))
    {
        Ok(data) => data,
        Err(e) => {
            warn!("SKATE_CHARACTER_LIGHTING: {e}");
            return;
        }
    };
    data.native = std::fs::read(
        crate::customiser_parts::asset_directory(&config.asset_root).join("native-lighting.json"),
    )
    .ok()
    .and_then(|b| serde_json::from_slice(&b).ok())
    .unwrap_or_default();
    // The Skate 2 tutorial's cast: Skate 2's own models, so their own rows (skate2port/old_tutorial_cast.py).
    let cast: HashMap<String, HashMap<String, MaterialData>> =
        std::fs::read(config.asset_root.join("private/skate2-tutorial/cast/lighting.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
    data.native.extend(cast.into_iter().map(|(model, rows)| (format!("skate2-tutorial/{model}"), rows)));
    let name = map_path.file_stem().unwrap_or_default().to_string_lossy();
    let overlay = config
        .asset_root
        .join("private/native-lighting")
        .join(format!("{name}.irradiance"));
    let probe_path = if overlay.is_file() {
        overlay
    } else {
        map_path.with_extension("irradiance")
    };
    let probes = match std::fs::read(probe_path)
        .map_err(|e| e.to_string())
        .and_then(|b| Irradiance::parse(&b))
    {
        Ok(probes) => probes,
        Err(e) => {
            warn!("SKATE_CHARACTER_LIGHTING: spatial data unavailable: {e}");
            return;
        }
    };
    let sky_path = config
        .asset_root
        .join("private/native-skies")
        .join(format!("{name}.json"));
    let sky: serde_json::Value = match std::fs::read(sky_path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
    {
        Some(sky) => sky,
        None => {
            warn!("SKATE_CHARACTER_LIGHTING: missing authored light direction");
            return;
        }
    };
    let Some(sun) = sky["environment"]["sun_direction"].as_array() else {
        return;
    };
    if sun.len() != 3 {
        return;
    }
    let mut light = Vec4::new(0., 0., 0., 2.5); // Existing retail scene exposure.
    for i in 0..3 {
        let Some(v) = sun[i].as_f64() else {
            return;
        };
        light[i] = v as f32;
    }
    info!("SKATE_CHARACTER_LIGHTING: loaded authored {name} irradiance and character parameters");
    spawn_shadow_sources(&mut commands, light.truncate());
    commands.insert_resource(Lighting {
        data,
        probes,
        light,
        sun: light.truncate(),
        display_sh: None,
    });
}
fn spawn_shadow_sources(commands: &mut Commands, light: Vec3) {
    // World + skater casters, sampled only by the character shader.
    commands.spawn((
        ShadowSource,
        Name::new("Character shadow visibility"),
        DirectionalLight {
            illuminance: 0.,
            shadows_enabled: true,
            affects_lightmapped_mesh_diffuse: false,
            ..default()
        },
        Transform::default().looking_to(-light, Vec3::Y),
        // ponytail: only skater pixels sample this map and the skater stays near the
        // camera, so one 24 m cascade replaces four out to 100 m (was ~2/3 of GPU
        // frame time on University). Ceiling: a camera >24 m from the skater loses
        // world shadowing on the character; raise maximum_distance if that matters.
        bevy::light::CascadeShadowConfigBuilder {
            num_cascades: 1,
            maximum_distance: 24.,
            ..default()
        }
        .build(),
    ));
    // Only skinned player pieces inhabit layer 28. The world receiver samples
    // this separate map so baked building/terrain shadows are not re-applied.
    commands.spawn((
        ShadowSource,
        PlayerShadow,
        Name::new("Player shadow onto baked world"),
        DirectionalLight {
            illuminance: 0.,
            shadows_enabled: true,
            affects_lightmapped_mesh_diffuse: true,
            // Receiver-only map: no self-shadow acne to hide with large bias.
            shadow_depth_bias: 0.002,
            shadow_normal_bias: 0.0,
            ..default()
        },
        RenderLayers::layer(28),
        Transform::default().looking_to(-light, Vec3::Y),
        shadow_quality(crate::graphics_menu::SHADOW_QUALITY_DEFAULT),
    ));
}

/// Shadow Quality: the player-shadow cascades; every map stays 1024. One 24 m
/// cascade spans ~65 m at the retail FOV, ~6.4 cm a texel (Low, the old look).
/// Medium splits at 8 m, ~2.2 cm where the camera (2-3 m from the skater) looks;
/// High splits at 5 and ~11 m, ~1.4 cm. Layer-28 casters are only the player, so
/// extra cascades are nearly free (+4 MB each).
pub(crate) fn shadow_quality(quality: u32) -> bevy::light::CascadeShadowConfig {
    let (num_cascades, first_cascade_far_bound) = match quality {
        0 => (1, 8.),
        1 => (2, 8.),
        _ => (3, 5.),
    };
    bevy::light::CascadeShadowConfigBuilder {
        num_cascades,
        first_cascade_far_bound,
        maximum_distance: 24.,
        ..default()
    }
    .build()
}

/// Follows the Shadow Quality setting; writes only on change.
fn apply_shadow_quality(
    menu: Option<Res<crate::graphics_menu::Menu>>,
    mut lights: Query<&mut bevy::light::CascadeShadowConfig, With<PlayerShadow>>,
) {
    let quality = menu.map_or(crate::graphics_menu::SHADOW_QUALITY_DEFAULT, |m| m.shadow_quality());
    let cascades = shadow_quality(quality);
    for mut config in &mut lights {
        if config.bounds != cascades.bounds {
            *config = cascades.clone();
        }
    }
}

fn bind(
    mut commands: Commands,
    lighting: Option<Res<Lighting>>,
    source: Res<Assets<StandardMaterial>>,
    server: Res<AssetServer>,
    mut materials: ResMut<Assets<CharacterMaterial>>,
    entities: Query<(
        Entity,
        &GltfMaterialName,
        &MeshMaterial3d<StandardMaterial>,
        Option<&RenderLayers>,
    )>,
    parents: Query<&ChildOf>,
    players: Query<
        (),
        Or<(
            With<crate::world::PlayerRoot>,
            With<crate::multiplayer::appearance::RemoteCharacter>,
            With<crate::multiplayer::appearance::PuppetCharacter>,
            // A native model standing on its own: the Skate 2 tutorial's cast (skate2_cast::place, user 10-29 bug 1).
            With<crate::custom_models::NativeModelRoot>,
        )>,
    >,
    parts: Query<(), With<crate::customiser_parts::PartRoot>>,
    native: Query<&crate::custom_models::NativeModelRoot>,
    imports: Query<(), With<crate::custom_models::CustomModelRoot>>,
    (changed, moved, children, mut pending): (
        Query<
            Entity,
            (
                With<GltfMaterialName>,
                Or<(Changed<GltfMaterialName>, Changed<MeshMaterial3d<StandardMaterial>>)>,
            ),
        >,
        Query<
            Entity,
            Or<(
                Changed<ChildOf>,
                Added<crate::world::PlayerRoot>,
                Added<crate::multiplayer::appearance::RemoteCharacter>,
                Added<crate::multiplayer::appearance::PuppetCharacter>,
            )>,
        >,
        Query<&Children>,
        Local<Vec<Entity>>,
    ),
) {
    let Some(lighting) = lighting else {
        return;
    };
    // perf 10-19: SKATE_BIND_ADDED: this walked every glTF mesh in the world (the map's, the traffic's) up its
    // ancestors every frame. Whether one binds changes only with its material or name, its ancestry (an entity
    // whose parent changed, or that became a skater root: it and everything below it), the lighting tables (a new
    // Lighting: everything), or its material loading (retried each frame, `pending`). PartRoot, NativeModelRoot and
    // CustomModelRoot are only ever inserted with their scene's root and never removed.
    let todo: Vec<Entity> = if !crate::config::perf_on("SKATE_BIND_ADDED") || lighting.is_added() {
        pending.clear();
        entities.iter().map(|(entity, ..)| entity).collect()
    } else {
        let mut todo = std::mem::take(&mut *pending);
        todo.extend(&changed);
        for e in &moved {
            if players.contains(e) || parents.iter_ancestors(e).any(|a| players.contains(a)) {
                todo.push(e);
                todo.extend(children.iter_descendants(e));
            }
        }
        todo.sort_unstable();
        todo.dedup();
        todo
    };
    for (entity, name, handle, layers) in todo.into_iter().filter_map(|e| entities.get(e).ok()) {
        if !parents.iter_ancestors(entity).any(|e| players.contains(e)) {
            continue;
        }
        // Modular CAC pieces own an extended material with tattoos/hair coverage.
        // Giving them a second material races publication and can render twice.
        if parents.iter_ancestors(entity).any(|e| parts.contains(e)) {
            continue;
        }
        let native_key = parents
            .iter_ancestors(entity)
            .find_map(|e| native.get(e).ok().map(|n| &n.0));
        if native_key.is_none() && parents.iter_ancestors(entity).any(|e| imports.contains(e)) {
            continue;
        }
        let table = if let Some(key) = native_key {
            lighting.data.native.get(key)
        } else {
            Some(&lighting.data.materials)
        };
        let Some(data) = table.and_then(|m| m.get(&name.0)) else {
            continue;
        };
        if data.params.len() != 9 {
            continue;
        }
        let Some(m) = source.get(&handle.0) else {
            pending.push(entity); // perf 10-19: retried once it loads
            continue;
        };
        let alpha_cutoff = if let AlphaMode::Mask(cutoff) = m.alpha_mode {
            cutoff
        } else {
            -1.
        };
        let material = materials.add(CharacterMaterial {
            params: CharacterParams {
                light: lighting.light,
                tint: Vec4::from_array(m.base_color.to_linear().to_f32_array()),
                options: Vec4::new(
                    f32::from(m.normal_map_texture.is_some()),
                    f32::from(data.specular.is_some()),
                    alpha_cutoff,
                    f32::from(data.is_hair()),
                ),
                rows: std::array::from_fn(|i| Vec4::from_array(data.params[i])),
                sh: lighting
                    .data
                    .default_sh
                    .map(|v| Vec3::from_array(v).extend(0.)),
                key: Vec4::ONE,
            },
            diffuse: m.base_color_texture.clone(),
            normal: m.normal_map_texture.clone(),
            mask: data.specular.as_ref().map(|path| linear(&server, path)),
            coverage: data.opacity.as_ref().map(|path| linear(&server, path)),
            lighting: LIGHTING,
            alpha: m.alpha_mode,
        });
        // Audit 10-24: try_, as a remote skater's mesh can be despawned this frame (it left, or a newer look replaced it:
        // multiplayer::render::spawn, not ordered against this); a plain insert there panics the game.
        commands
            .entity(entity)
            .remove::<MeshMaterial3d<StandardMaterial>>()
            .try_insert((
                OriginalCharacterMaterial {
                    material: handle.0.clone(),
                    layers: layers.cloned(),
                },
                MeshMaterial3d(material),
                RenderLayers::from_layers(&[0, 28]),
            ));
    }
}
fn linear(server: &AssetServer, path: &str) -> Handle<Image> {
    server.load_with_settings(path.to_owned(), |settings: &mut bevy::image::ImageLoaderSettings| {
        settings.is_srgb = false;
    })
}
fn shadow_views(
    mut commands: Commands,
    lighting: Option<Res<Lighting>>,
    cameras: Query<(Entity, Option<&RenderLayers>), With<crate::camera::GameplayCamera>>,
) {
    if lighting.is_none() {
        return;
    }
    for (entity, layers) in &cameras {
        let layers = layers.cloned().unwrap_or_default();
        if !layers.intersects(&RenderLayers::layer(28)) {
            commands.entity(entity).insert(layers.with(28));
        }
    }
}
#[allow(clippy::too_many_arguments)]
fn update(
    lighting: Option<ResMut<Lighting>>,
    root: Query<&Transform, With<crate::world::PlayerRoot>>,
    rooms: Query<(&Transform, &Visibility), (With<crate::cas_room::Room>, Without<ShadowSource>)>,
    mut shared: ResMut<SharedLighting>,
    mut shadow: ResMut<crate::retail_render::ShadowState>,
    time: Res<Time>,
    post: Option<Res<crate::retail_render::ShotPost>>,
    mut sources: Query<&mut Transform, (With<ShadowSource>, Without<crate::world::PlayerRoot>)>,
    look: Res<crate::retail_render::DayLook>,
) {
    let (Some(mut lighting), Ok(root)) = (lighting, root.single()) else {
        return;
    };
    // Edit Skater's room (cas_room) turns with the player wherever they open it; retail's stands in place, past every
    // probe and under the world's sun. So in it the skater has the default SH (the native sample's fallback) and the sun
    // as the room faces it, the same every time (user 10-30: "sometimes not properly lit").
    let room = rooms.iter().find(|(_, shown)| **shown == Visibility::Visible).map(|(t, _)| t.rotation);
    // Time of day: the probe's ambient and the key light, as the world's light changes; Edit Skater's room is lit by
    // day whatever the world's time, so the skater and its shadow look the same every time it opens.
    let look = if room.is_some() { &crate::retail_render::DAY_LOOK } else { &*look };
    let tint = |sh: [Vec4; 9]| sh.map(|v| (v.truncate() * look.ambient).extend(v.w));
    let key = look.key.extend(1.);
    let sun = match room {
        Some(faces) => faces * crate::cas_room::SUN,
        // A cutscene shot's SUN_DIRECTION stands in for the map's sun (Skate 2 0x825a3784 -> its light state +0xa0).
        None => post.map(|p| p.sun).filter(|s| *s != Vec3::ZERO).unwrap_or(lighting.sun),
    };
    if lighting.light.truncate() != sun {
        lighting.light = sun.extend(lighting.light.w);
        for mut t in &mut sources {
            *t = Transform::default().looking_to(-sun, Vec3::Y);
        }
    }
    let fallback = lighting
        .data
        .default_sh
        .map(|v| Vec3::from_array(v).extend(0.));
    let sh = if room.is_some() { fallback } else { lighting.probes.sample(root.translation, fallback) };
    // The room's light cuts in and out with the room; in the world it eases from probe to probe.
    let displayed = match lighting.display_sh {
        Some(old) if room.is_none() => {
            let weight = 1. - (-time.delta_secs().clamp(0., 0.05) / 0.35).exp();
            std::array::from_fn(|i| old[i].lerp(sh[i], weight))
        }
        _ => sh,
    };
    lighting.display_sh = room.is_none().then_some(displayed);
    publish(&mut shared, SharedLighting { light: lighting.light, sh: tint(displayed), key });
    // Adapter floor: the local probe's direction-independent ambient term.
    // The native per-frame c8 shadow-colour controller remains unrecovered.
    // Untinted: the world applies the time of day after this floor.
    shadow.approach(sh[0].truncate(), time.delta_secs());
}

/// Written only when a bit differs, so a settled SH uploads nothing.
fn publish(shared: &mut impl std::ops::DerefMut<Target = SharedLighting>, new: SharedLighting) {
    let old = &**shared;
    if !(same_bits(&old.sh, &new.sh) && same_bits(&[old.light, old.key], &[new.light, new.key])) {
        **shared = new;
    }
}
/// No tolerance: signed zeros and NaN payloads count as changes.
fn same_bits(a: &[Vec4], b: &[Vec4]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(a, b)| a.to_array().map(f32::to_bits) == b.to_array().map(f32::to_bits))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn customiser_hair_families_include_pro_skater_defaults() {
        for shader in [
            "character.hair",
            "character.hair_ropa",
            "character.default_hair",
            "character.default_hair_ropa",
        ] {
            assert!(
                MaterialData {
                    shader: shader.into(),
                    params: vec![],
                    specular: None,
                    opacity: None,
                }
                .is_hair(),
                "{shader}"
            );
        }
        for shader in [
            "character.default_skin",
            "character.default_cloth",
            "character.default_cloth_ropa",
        ] {
            assert!(
                !MaterialData {
                    shader: shader.into(),
                    params: vec![],
                    specular: None,
                    opacity: None,
                }
                .is_hair(),
                "{shader}"
            );
        }
    }
    #[test]
    fn customiser_material_binding_uses_native_rows_and_leaves_cac_material_owned() {
        use bevy::ecs::system::RunSystemOnce;
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()));
        app.init_resource::<Assets<StandardMaterial>>()
            .init_resource::<Assets<CharacterMaterial>>()
            .init_asset::<Image>();
        let world = app.world_mut();
        let material = world
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let authored = MaterialData {
            shader: "character.cloth".into(),
            params: vec![[2., 3., 4., 5.]; 9],
            specular: None,
            opacity: None,
        };
        world.insert_resource(Lighting {
            data: LightingData {
                materials: HashMap::new(),
                default_sh: [[0.; 3]; 9],
                native: HashMap::from([(
                    "pro".into(),
                    HashMap::from([("Retail_Torso".into(), authored)]),
                )]),
            },
            probes: default(),
            light: Vec4::ONE,
            sun: Vec3::ONE,
            display_sh: None,
        });
        let player = world.spawn(crate::world::PlayerRoot).id();
        let pro = world
            .spawn((
                crate::custom_models::NativeModelRoot("pro".into()),
                ChildOf(player),
            ))
            .id();
        let torso = world
            .spawn((
                ChildOf(pro),
                GltfMaterialName("Retail_Torso".into()),
                MeshMaterial3d(material.clone()),
            ))
            .id();
        let part = world
            .spawn((
                crate::customiser_parts::PartRoot("shirt".into()),
                ChildOf(player),
            ))
            .id();
        let cloth = world
            .spawn((
                ChildOf(part),
                GltfMaterialName("Retail_Torso".into()),
                MeshMaterial3d(material),
            ))
            .id();
        let hair_data = MaterialData {
            shader: "character.default_hair".into(),
            params: vec![[1.; 4]; 9],
            specular: None,
            opacity: Some("hair_alpha.png".into()),
        };
        world
            .resource_mut::<Lighting>()
            .data
            .native
            .get_mut("pro")
            .unwrap()
            .insert("Retail_Hair".into(), hair_data);
        let hair_material = world
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let hair = world
            .spawn((
                ChildOf(pro),
                GltfMaterialName("Retail_Hair".into()),
                MeshMaterial3d(hair_material),
            ))
            .id();
        world.run_system_once(bind).unwrap();
        let hair_handle = &world
            .get::<MeshMaterial3d<CharacterMaterial>>(hair)
            .unwrap()
            .0;
        assert_eq!(
            world
                .resource::<Assets<CharacterMaterial>>()
                .get(hair_handle)
                .unwrap()
                .params
                .options
                .w,
            1.
        );
        let coverage = |handle: &Handle<CharacterMaterial>| {
            world.resource::<Assets<CharacterMaterial>>().get(handle).unwrap().coverage.is_some()
        };
        assert!(coverage(hair_handle), "the hair's own coverage map is bound");

        let h = &world
            .get::<MeshMaterial3d<CharacterMaterial>>(torso)
            .unwrap()
            .0;
        assert_eq!(
            world
                .resource::<Assets<CharacterMaterial>>()
                .get(h)
                .unwrap()
                .params
                .rows[0],
            Vec4::new(2., 3., 4., 5.)
        );
        assert!(!world.resource::<Assets<CharacterMaterial>>().get(h).unwrap().coverage.is_some());
        assert!(
            world
                .get::<MeshMaterial3d<StandardMaterial>>(torso)
                .is_none()
        );
        assert!(
            world
                .get::<MeshMaterial3d<CharacterMaterial>>(cloth)
                .is_none()
        );
        assert!(
            world
                .get::<MeshMaterial3d<StandardMaterial>>(cloth)
                .is_some()
        );
    }

    #[test]
    fn custom_scene_restores_character_and_removes_only_gameplay_shadow_layer() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.insert_resource(crate::config::Config {
            asset_root: "unused".into(),
            verification_capture: None,
            hero_clip: None,
            skate2_physics_dir: None,
            physics_override: None,
            map: None,
            map_path: None,
            difficulty: crate::difficulty::Difficulty::Hardcore,
            check_assets: false,
            start_paused: false,
            teleport: None,
            multiplayer: Default::default(),
            map_fingerprint: 0,
        });
        world.insert_resource(crate::retail_render::RetailScene(false));
        world.init_resource::<Assets<crate::customiser_material::SkaterMaterial>>();
        world.insert_resource(SharedLighting { light: Vec4::ONE, ..default() });
        world.insert_resource(crate::retail_render::ShadowState(
            Vec4::ONE,
            Vec4::ONE,
            [Vec4::ONE; 7],
            Vec4::ZERO,
            Vec4::ZERO,
            Vec4::ZERO,
            Vec4::ZERO,
            [Vec4::ONE; 4],
        ));
        let material = Handle::<StandardMaterial>::default();
        let player = world
            .spawn((
                OriginalCharacterMaterial {
                    material: material.clone(),
                    layers: None,
                },
                MeshMaterial3d(Handle::<CharacterMaterial>::default()),
                RenderLayers::from_layers(&[0, 28]),
            ))
            .id();
        let light = world.spawn(ShadowSource).id();
        let camera = world
            .spawn((
                crate::camera::GameplayCamera,
                RenderLayers::from_layers(&[0, 28]),
            ))
            .id();
        let overlay = world.spawn(RenderLayers::layer(31)).id();
        world.run_system_once(load).unwrap();
        assert!(world.get_entity(light).is_err());
        // Skater materials lit by the world go back to Bevy's lighting off retail maps.
        assert_eq!(world.resource::<SharedLighting>().light, Vec4::ZERO);
        assert_eq!(
            world
                .get::<MeshMaterial3d<StandardMaterial>>(player)
                .unwrap()
                .0,
            material
        );
        assert!(
            world
                .get::<MeshMaterial3d<CharacterMaterial>>(player)
                .is_none()
        );
        assert!(world.get::<RenderLayers>(player).is_none());
        assert_eq!(
            world.get::<RenderLayers>(camera).unwrap(),
            &RenderLayers::default()
        );
        assert_eq!(
            world.get::<RenderLayers>(overlay).unwrap(),
            &RenderLayers::layer(31)
        );
        assert_eq!(
            world.resource::<crate::retail_render::ShadowState>().0,
            Vec4::ZERO
        );
    }

    /// perf 10-19: bind looks only at what changed, yet a scene parented to a skater late, or whose material loads
    /// late, still binds; one never under a skater does not.
    #[test]
    fn late_parented_and_late_loaded_meshes_still_bind() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()));
        app.init_resource::<Assets<StandardMaterial>>()
            .init_resource::<Assets<CharacterMaterial>>()
            .init_asset::<Image>()
            .add_systems(Update, bind);
        let cloth = MaterialData { shader: "character.cloth".into(), params: vec![[1.; 4]; 9], specular: None, opacity: None };
        app.world_mut().insert_resource(Lighting {
            data: LightingData { materials: HashMap::from([("Cloth".into(), cloth)]), default_sh: [[0.; 3]; 9], native: HashMap::new() },
            probes: default(),
            light: Vec4::ONE,
            sun: Vec3::ONE,
            display_sh: None,
        });
        let world = app.world_mut();
        let loaded = world.resource_mut::<Assets<StandardMaterial>>().add(StandardMaterial::default());
        let late = world.resource::<Assets<StandardMaterial>>().reserve_handle();
        let player = world.spawn(crate::world::PlayerRoot).id();
        let scene = world.spawn_empty().id();
        let mesh = |world: &mut World, parent, material: &Handle<StandardMaterial>| {
            world.spawn((ChildOf(parent), GltfMaterialName("Cloth".into()), MeshMaterial3d(material.clone()))).id()
        };
        let (moved, loading) = (mesh(world, scene, &loaded), mesh(world, player, &late));
        let bound = |app: &App, e: Entity| app.world().get::<MeshMaterial3d<CharacterMaterial>>(e).is_some();
        app.update();
        app.update();
        assert!(!bound(&app, moved) && !bound(&app, loading));
        app.world_mut().entity_mut(scene).insert(ChildOf(player));
        app.update();
        assert!(bound(&app, moved), "its scene parented to the skater late");
        assert!(!bound(&app, loading));
        app.world_mut().resource_mut::<Assets<StandardMaterial>>().insert(&late, StandardMaterial::default()).unwrap();
        app.update();
        assert!(bound(&app, loading), "its material loaded late");
    }

    #[test]
    fn shared_lighting_changes_only_when_a_bit_differs() {
        let mut world = World::new();
        world.init_resource::<SharedLighting>();
        let lit = |sh: Vec4| SharedLighting { light: Vec4::new(0., 1., 0., 2.5), sh: [sh; 9], key: Vec4::ONE };
        publish(&mut world.resource_mut::<SharedLighting>(), lit(Vec4::ONE));
        world.clear_trackers();
        // A settled SH uploads nothing.
        publish(&mut world.resource_mut::<SharedLighting>(), lit(Vec4::ONE));
        assert!(!world.is_resource_changed::<SharedLighting>());
        // Signed zero and NaN payloads must retain their bits, without epsilon tests.
        let odd = Vec4::new(-0.0, f32::from_bits(0x7fc01234), 1.0, 0.0);
        publish(&mut world.resource_mut::<SharedLighting>(), lit(odd));
        assert!(world.is_resource_changed::<SharedLighting>());
        let shared = *world.resource::<SharedLighting>();
        assert_eq!(shared.sh[8].to_array().map(f32::to_bits), odd.to_array().map(f32::to_bits));
        world.clear_trackers();
        publish(&mut world.resource_mut::<SharedLighting>(), lit(odd));
        assert!(!world.is_resource_changed::<SharedLighting>());
        // The light alone (a cutscene shot's sun) and the key alone are changes too.
        publish(&mut world.resource_mut::<SharedLighting>(), SharedLighting { light: Vec4::new(1., 0., 0., 2.5), ..lit(odd) });
        assert!(world.is_resource_changed::<SharedLighting>());
        world.clear_trackers();
        publish(&mut world.resource_mut::<SharedLighting>(), SharedLighting { light: Vec4::new(1., 0., 0., 2.5), key: Vec4::new(0.5, 0.5, 0.5, 1.), ..lit(odd) });
        assert!(world.is_resource_changed::<SharedLighting>());
        // The buffer layout retail_character_common.wgsl reads: light, nine SH rows, key.
        let bytes: Vec<u8> = world.resource::<SharedLighting>().bytes().collect();
        assert_eq!(bytes.len(), 11 * 16);
        assert_eq!(bytes[..16], Vec4::new(1., 0., 0., 2.5).to_array().map(f32::to_le_bytes).concat()[..]);
        assert_eq!(bytes[160..], Vec4::new(0.5, 0.5, 0.5, 1.).to_array().map(f32::to_le_bytes).concat()[..]);
    }

    /// User 10-30 ("sometimes your skater isn't properly lit" in Edit Skater): the room (cas_room) turns with the player
    /// wherever they open it, but retail's stands in place, past every probe and under the world's sun. So while it is
    /// shown the skater is lit by the default SH (the native sample's fallback) and the sun as the room faces it, not by
    /// the probe under the player's feet and the map's sun, whichever way they faced; the light cuts with the room.
    #[test]
    fn the_edit_skater_room_lights_the_skater_the_same_wherever_it_opens() {
        let spot = Vec3::new(5., 2., -3.);
        // One dark probe where the player stands (under a bridge, say).
        let mut bytes = b"SHP1".to_vec();
        bytes.extend([1u32, 1].iter().flat_map(|w| w.to_le_bytes()));
        let mut probe = [0f32; 40];
        probe[..3].copy_from_slice(&[0.02; 3]);
        probe[36..39].copy_from_slice(&spot.to_array());
        bytes.extend(probe.iter().flat_map(|f| f.to_le_bytes()));
        let map_sun = Vec3::new(-0.4167, 0.4065, 0.8131);
        let mut default_sh = [[0.; 3]; 9];
        default_sh[0] = [0.3; 3];
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<SharedLighting>()
            .init_resource::<crate::retail_render::ShadowState>()
            .init_resource::<crate::retail_render::DayLook>()
            .insert_resource(Lighting {
                data: LightingData { materials: HashMap::new(), default_sh, native: HashMap::new() },
                probes: Irradiance::parse(&bytes).unwrap(),
                light: map_sun.extend(2.5),
                sun: map_sun,
                display_sh: None,
            })
            .add_systems(Update, update);
        let world = app.world_mut();
        let source = world.spawn((ShadowSource, Transform::default().looking_to(-map_sun, Vec3::Y))).id();
        let player = world.spawn((crate::world::PlayerRoot, Transform::from_translation(spot))).id();
        let room = world.spawn((crate::cas_room::Room, Transform::default(), Visibility::Hidden)).id();
        let lit = |app: &App| (*app.world().resource::<SharedLighting>(), app.world().get::<Transform>(source).unwrap().forward().as_vec3());
        app.update();
        let (shared, _) = lit(&app);
        assert_eq!(shared.sh[0].x, 0.02, "in the world: the probe under the player");
        assert!(shared.light.truncate().distance(map_sun) < 1e-6, "and the map's sun");
        for yaw in [0., 1.3, std::f32::consts::PI, -2.2] {
            let turn = Quat::from_rotation_y(yaw);
            app.world_mut().entity_mut(player).insert(Transform::from_translation(spot).with_rotation(turn));
            let faces = turn * Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
            app.world_mut().entity_mut(room).insert((Transform::from_rotation(faces), Visibility::Visible));
            app.update();
            let (shared, forward) = lit(&app);
            assert_eq!(shared.sh[0].truncate(), Vec3::splat(0.3), "yaw {yaw}: the room's light at once, not the probe");
            assert!((faces.inverse() * shared.light.truncate()).distance(crate::cas_room::SUN) < 1e-5, "yaw {yaw}: the sun as the room faces it");
            assert!(forward.distance(-(faces * crate::cas_room::SUN).normalize()) < 1e-4, "yaw {yaw}: its shadows too");
        }
        // At night the room is still lit by day: the same light and shadow every time Edit Skater opens.
        let day = crate::retail_render::DAY_LOOK;
        app.world_mut().insert_resource(crate::retail_render::DayLook { key: Vec3::splat(0.1), ambient: Vec3::splat(0.2), ..day });
        app.update();
        assert_eq!(lit(&app).0.key.truncate(), day.key, "the room's key light is the day's at night");
        // Back to day, so the world probe below reads unscaled.
        app.world_mut().insert_resource(day);
        app.world_mut().entity_mut(room).insert(Visibility::Hidden);
        app.update();
        let (shared, forward) = lit(&app);
        assert_eq!(shared.sh[0].x, 0.02, "closed: the probe under the player again, at once");
        assert!(shared.light.truncate().distance(map_sun) < 1e-6 && forward.distance(-map_sun.normalize()) < 1e-4, "and the map's sun");
    }

    #[test]
    fn shadow_quality_sharpens_the_player_shadow_near_the_camera() {
        assert_eq!(shadow_quality(0).bounds, vec![24.]);
        assert_eq!(shadow_quality(1).bounds, vec![8., 24.]);
        let high = shadow_quality(2).bounds;
        assert_eq!(high.len(), 3);
        assert!(high[0] == 5. && (high[1] - 10.95).abs() < 0.01 && (high[2] - 24.).abs() < 1e-3, "{high:?}");
        // The setting reaches a spawned player light.
        let mut app = App::new();
        app.insert_resource(crate::graphics_menu::test_menu())
            .add_systems(Update, apply_shadow_quality);
        let mut queue = bevy::ecs::world::CommandQueue::default();
        spawn_shadow_sources(&mut Commands::new(&mut queue, app.world()), Vec3::Y);
        queue.apply(app.world_mut());
        app.world_mut()
            .resource_mut::<crate::graphics_menu::Menu>()
            .choose(crate::graphics_menu::Setting::ShadowQuality, 0);
        app.update();
        let bounds = |app: &mut App| {
            app.world_mut()
                .query::<(&DirectionalLight, &bevy::light::CascadeShadowConfig)>()
                .iter(app.world())
                .map(|(l, c)| (l.affects_lightmapped_mesh_diffuse, c.bounds.clone()))
                .collect::<Vec<_>>()
        };
        assert!(bounds(&mut app).contains(&(true, vec![24.])));
        app.world_mut()
            .resource_mut::<crate::graphics_menu::Menu>()
            .choose(crate::graphics_menu::Setting::ShadowQuality, 2);
        app.update();
        assert!(bounds(&mut app).contains(&(true, high)));
        // The character visibility light keeps its single cascade.
        assert!(bounds(&mut app).contains(&(false, vec![24.])));
    }

    #[test]
    fn world_receiver_source_excludes_world_casters_and_emits_no_light() {
        let mut world = World::new();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        spawn_shadow_sources(&mut Commands::new(&mut queue, &world), Vec3::Y);
        queue.apply(&mut world);
        let player = RenderLayers::from_layers(&[0, 28]);
        let terrain = RenderLayers::default();
        let mut receivers = 0;
        let mut character_sources = 0;
        for (light, layers, cascades) in world
            .query::<(
                &DirectionalLight,
                Option<&RenderLayers>,
                &bevy::light::CascadeShadowConfig,
            )>()
            .iter(&world)
        {
            let layers = layers.cloned().unwrap_or_default();
            assert_eq!(light.illuminance, 0.);
            assert!(light.shadows_enabled);
            assert!(layers.intersects(&player));
            if light.affects_lightmapped_mesh_diffuse {
                assert!(!layers.intersects(&terrain));
                assert_eq!(cascades.bounds, vec![8., 24.]);
                assert_eq!(light.shadow_normal_bias, 0.);
                assert!(light.shadow_depth_bias < 0.005);
                receivers += 1;
            } else {
                assert!(layers.intersects(&terrain));
                // Shaders tell it from the rain shelter light (bias 0) by this.
                assert!(light.shadow_normal_bias > 0.);
                character_sources += 1;
            }
        }
        assert_eq!((receivers, character_sources), (1, 1));
    }

    /// User 10-29 (bug 1): the Skate 2 tutorial's cast stands on its own (no player root) and kept the glTF's
    /// StandardMaterial, which a retail map lights with the flat ambient only (every directional light has
    /// illuminance 0): 2.6-18x darker than the player and unshaded. Its root's native key binds its own rows.
    #[test]
    fn a_skate2_cast_member_is_lit_like_the_player() {
        use bevy::ecs::system::RunSystemOnce;
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()));
        app.init_resource::<Assets<StandardMaterial>>().init_resource::<Assets<CharacterMaterial>>().init_asset::<Image>();
        let world = app.world_mut();
        let material = world.resource_mut::<Assets<StandardMaterial>>().add(StandardMaterial::default());
        let rows = MaterialData { shader: "character.default_cloth".into(), params: vec![[0.5, 1.2, 1., 3.]; 9], specular: None, opacity: None };
        world.insert_resource(Lighting {
            data: LightingData {
                materials: HashMap::new(),
                default_sh: [[0.; 3]; 9],
                native: HashMap::from([("skate2-tutorial/slappy".into(), HashMap::from([("Retail_Torso".into(), rows)]))]),
            },
            probes: default(),
            light: Vec4::ONE,
            sun: Vec3::ONE,
            display_sh: None,
        });
        let root = world.spawn((Name::new("Skate 2 cast: slappy"), crate::custom_models::NativeModelRoot("skate2-tutorial/slappy".into()))).id();
        let torso = world.spawn((ChildOf(root), GltfMaterialName("Retail_Torso".into()), MeshMaterial3d(material))).id();
        world.run_system_once(bind).unwrap();
        let handle = &world.get::<MeshMaterial3d<CharacterMaterial>>(torso).expect("the cast takes the character shader").0;
        let bound = world.resource::<Assets<CharacterMaterial>>().get(handle).unwrap();
        assert_eq!(bound.params.rows[0], Vec4::new(0.5, 1.2, 1., 3.));
    }

    /// User 10-29 (white parts in the cast's hair): coverage (hair-card) materials draw both faces like the
    /// customiser's opacity pieces; the rest keep the back-face cull.
    #[test]
    fn hair_cards_draw_both_faces() {
        let mut m = CharacterMaterial {
            params: default(),
            diffuse: None,
            normal: None,
            mask: None,
            coverage: None,
            lighting: LIGHTING,
            alpha: AlphaMode::Opaque,
        };
        assert!(!CharacterKey::from(&m).two_sided);
        m.coverage = Some(Handle::default());
        assert!(CharacterKey::from(&m).two_sided);
    }
}
