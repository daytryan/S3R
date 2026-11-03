//! Authored camera-relative sky and its radial sun gradient.
use super::RetailWorldMaterial;
use bevy::{
    prelude::*,
    render::render_resource::{AsBindGroup, ShaderType},
    shader::ShaderRef,
};

#[derive(Clone, Debug, ShaderType)]
pub(crate) struct SkyParams {
    // anchor height, scene exposure, sky multiplier, sun angular scale (0 disables).
    pub settings: Vec4,
    pub sun_direction: Vec4,
    /// After Dark's night sky for the time of day: moon gradient scale (x, 0
    /// without the night textures).
    pub night: Vec4,
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub(crate) struct RetailSkyMaterial {
    #[uniform(0)]
    pub params: SkyParams,
    #[texture(1)]
    #[sampler(2)]
    pub diffuse: Handle<Image>,
    #[texture(3)]
    #[sampler(4)]
    pub sun: Option<Handle<Image>>,
    /// Shared frame state; the sky reads its weather (rain, lightning).
    #[storage(5, read_only)]
    pub shadow_state: Handle<bevy::render::storage::ShaderStorageBuffer>,
    /// After Dark's night panorama and moon gradient (retail_daynight.rs).
    #[texture(6)]
    #[sampler(7)]
    pub night: Option<Handle<Image>>,
    #[texture(8)]
    #[sampler(9)]
    pub moon: Option<Handle<Image>>,
}

/// A map whose own sky is night (After Dark's Sanatorium, the night park): the time
/// of day leaves it as authored.
#[derive(Component)]
pub(crate) struct AuthoredNight;
/// After Dark's sky (Sanatorium), the night every other retail map gets.
const NIGHT_SKY: &str = "SanitariumDLC";
impl Material for RetailSkyMaterial {
    fn enable_prepass() -> bool {
        false
    }
    fn enable_shadows() -> bool {
        false
    }
    fn vertex_shader() -> ShaderRef {
        bevy::asset::AssetPath::from(bevy::asset::embedded_path!("retail_sky.wgsl"))
            .with_source("embedded")
            .into()
    }
    fn fragment_shader() -> ShaderRef {
        Self::vertex_shader()
    }
    fn specialize(
        _: &bevy::pbr::MaterialPipeline,
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        _: &bevy::mesh::MeshVertexBufferLayoutRef,
        _: bevy::pbr::MaterialPipelineKey<Self>,
    ) -> Result<(), bevy::render::render_resource::SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = None;
        if let Some(depth) = &mut descriptor.depth_stencil {
            depth.depth_write_enabled = false;
        }
        Ok(())
    }
}

#[derive(serde::Deserialize)]
struct FogFrame {
    ramp: [f32; 4],
    colour: [f32; 4],
}

#[derive(serde::Deserialize)]
struct Environment {
    anchor_height: f32,
    sun_direction: [f32; 3],
    sun_scale: f32,
    multiplier: f32,
    location_chain: Vec<String>,
    #[serde(default)]
    fog_frame: Option<FogFrame>,
    #[serde(default)]
    sky_model: String,
}
#[derive(serde::Deserialize)]
struct Sky {
    width: u32,
    height: u32,
    positions: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
    #[serde(default)]
    sun_width: u32,
    #[serde(default)]
    sun_height: u32,
    environment: Option<Environment>,
}
fn load(root: &std::path::Path, name: &str) -> Result<(Sky, Vec<u8>, Option<Vec<u8>>), String> {
    if name.is_empty() || name.contains(['/', '\\', ':']) || matches!(name, "." | "..") {
        return Err("Invalid retail sky name".into());
    }
    let base = root.join("private/native-skies");
    let read = |suffix: &str| {
        std::fs::read(base.join(format!("{name}{suffix}"))).map_err(|e| e.to_string())
    };
    let sky: Sky = serde_json::from_slice(&read(".json")?).map_err(|e| e.to_string())?;
    let rgba = read(".rgba")?;
    if sky.width == 0
        || sky.height == 0
        || u64::from(sky.width) * u64::from(sky.height) * 4 != rgba.len() as u64
        || sky.positions.is_empty()
        || sky.positions.len() != sky.uvs.len()
        || sky.indices.is_empty()
        || sky.indices.len() % 3 != 0
        || sky
            .indices
            .iter()
            .any(|&i| i as usize >= sky.positions.len())
        || sky
            .positions
            .iter()
            .flatten()
            .chain(sky.uvs.iter().flatten())
            .any(|v| !v.is_finite())
    {
        return Err("Invalid retail sky dimensions/geometry".into());
    }
    let sun = if let Some(env) = &sky.environment {
        let direction = Vec3::from_array(env.sun_direction);
        if !env.anchor_height.is_finite()
            || !direction.is_finite()
            || !(0.99..=1.01).contains(&direction.length_squared())
            || !env.sun_scale.is_finite()
            || env.sun_scale <= 0.
            || !env.multiplier.is_finite()
            || env.multiplier < 0.
            || sky.sun_width == 0
            || sky.sun_height != 16
        {
            return Err("Invalid authored sky environment".into());
        }
        if let Some(fog) = &env.fog_frame {
            if !fog
                .ramp
                .iter()
                .chain(fog.colour.iter())
                .all(|v| v.is_finite())
                || fog.ramp[0] < 0.
                || fog.ramp[2] <= 0.
                || !(-1. ..=0.).contains(&fog.colour[3])
            {
                return Err("Invalid authored fog frame".into());
            }
        }
        let bytes = read(".sun.rgba")?;
        if u64::from(sky.sun_width) * u64::from(sky.sun_height) * 4 != bytes.len() as u64 {
            return Err("Invalid sun-gradient payload".into());
        }
        Some(bytes)
    } else {
        None
    };
    Ok((sky, rgba, sun))
}
fn image(rgba: Vec<u8>, width: u32, height: u32, repeat: bool) -> Image {
    use bevy::render::render_resource::*;
    let mut image = Image::new(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba8Unorm,
        bevy::asset::RenderAssetUsages::RENDER_WORLD,
    );
    let mut sampler = bevy::image::ImageSamplerDescriptor::linear();
    if repeat {
        sampler.address_mode_u = bevy::image::ImageAddressMode::Repeat;
    }
    image.sampler = bevy::image::ImageSampler::Descriptor(sampler);
    // Not retail (user 10-19): Performance mode's low-resolution sky, sun and moon.
    crate::graphics_menu::performance_texture(&mut image);
    image
}
pub(crate) fn spawn_sky(
    name: &str,
    root: &std::path::Path,
    commands: &mut crate::map_render::SceneCommands,
    meshes: &mut impl crate::map_render::AssetSink<Mesh>,
    images: &mut impl crate::map_render::AssetSink<Image>,
    materials: &mut impl crate::map_render::AssetSink<RetailSkyMaterial>,
    world_materials: &mut crate::map_render::StagedAssets<RetailWorldMaterial>,
    blend_materials: &mut crate::map_render::StagedAssets<super::RetailBlendMaterial>,
) {
    let (sky, rgba, sun) = match load(root, name) {
        Ok(v) => v,
        Err(e) => {
            warn!("Retail sky unavailable for {name}: {e}");
            return;
        }
    };
    // Legacy sky packages retain their original parameters. The scene exposure
    // remains the adapter's capture default until auto-exposure is recovered.
    let mut params = SkyParams {
        settings: Vec4::new(165., 2.5, 1., 0.),
        sun_direction: Vec4::ZERO,
        night: Vec4::ZERO,
    };
    let authored_night = sky.environment.as_ref().is_some_and(|e| e.sky_model.to_ascii_lowercase().contains("night"));
    // ponytail: reads After Dark's 4 MB panorama per retail map load; cache the
    // images across loads if map switching shows it.
    let night = (!authored_night).then(|| load(root, NIGHT_SKY)).and_then(Result::ok).and_then(|(night, rgba, moon)| {
        let moon = moon?;
        let scale = night.environment.as_ref()?.sun_scale;
        params.night = Vec4::new(scale, 0., 0., 0.);
        Some((
            images.add(image(rgba, night.width, night.height, true)),
            images.add(image(moon, night.sun_width, night.sun_height, false)),
        ))
    });
    if let Some(env) = &sky.environment {
        params.settings = Vec4::new(env.anchor_height, 2.5, env.multiplier, env.sun_scale);
        params.sun_direction = Vec3::from_array(env.sun_direction).extend(0.);
        let blended = blend_materials.iter_mut().map(|(_, m)| &mut m.0);
        for material in world_materials.iter_mut().map(|(_, m)| m).chain(blended) {
            // Only the existing tangent-sign term uses this vector. No added
            // directional light energy or imported-world shadow map.
            material.params.sun_direction = params.sun_direction;
            if let Some(fog) = &env.fog_frame {
                material.params.fog_ramp = Vec4::from_array(fog.ramp);
                material.params.fog_color = Vec4::from_array(fog.colour);
            }
        }
        if let Some(fog) = &env.fog_frame {
            // World fog colour is scaled by the world's exposure baseline (mode.w 2.5);
            // fold it in so PBR fallbacks blend toward the same colour.
            let [r, g, b, a] = fog.colour;
            commands.insert_resource(super::AuthoredFog {
                ramp: Vec4::from_array(fog.ramp),
                colour: Vec4::new(r * 2.5, g * 2.5, b * 2.5, a),
            });
        }
        info!(
            "SKATE_SKY_READY map={name:?} location={:?} anchor={} sun_scale={} multiplier={} authored_direction={:?}",
            env.location_chain, env.anchor_height, env.sun_scale, env.multiplier, env.sun_direction
        );
    } else {
        warn!(
            "Retail sky {name} has legacy metadata; reconvert skies for authored parameters and sun"
        );
    }
    let sun = sun.map(|bytes| images.add(image(bytes, sky.sun_width, sky.sun_height, false)));
    let mesh = Mesh::new(
        bevy::mesh::PrimitiveTopology::TriangleList,
        bevy::asset::RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, sky.positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, sky.uvs)
    .with_inserted_indices(bevy::mesh::Indices::U32(sky.indices));
    let (night, moon) = night.unzip();
    let mut dome = commands.spawn((
        Name::new("Retail sky dome"),
        Mesh3d(meshes.add(mesh)),
        MeshMaterial3d(materials.add(RetailSkyMaterial {
            params,
            diffuse: images.add(image(rgba, sky.width, sky.height, true)),
            sun,
            shadow_state: super::shadow::BUFFER,
            night,
            moon,
        })),
        Transform::default(),
        bevy::camera::visibility::NoFrustumCulling,
        bevy::light::NotShadowCaster,
        bevy::light::NotShadowReceiver,
    ));
    if authored_night {
        dome.insert(AuthoredNight);
    }
}
