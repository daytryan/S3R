//! Skin stamps modify albedo before the unchanged StandardMaterial lighting.
use bevy::{
    asset::{AssetPath, embedded_asset, embedded_path},
    pbr::{ExtendedMaterial, MaterialExtension},
    prelude::*,
    render::render_resource::AsBindGroup,
    shader::ShaderRef,
};

pub(crate) type SkaterMaterial = ExtendedMaterial<StandardMaterial, SkinStamp>;

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub(crate) struct SkinStamp {
    #[texture(105)]
    #[sampler(106)]
    pub retail_mask: Option<Handle<Image>>,
    #[uniform(107)]
    #[reflect(ignore)]
    pub retail: crate::retail_character::CharacterParams,
    /// Zone 0's graphic (the tattoo on skin).
    #[texture(100)]
    #[sampler(101)]
    pub texture: Option<Handle<Image>>,
    /// cac_hair_defaultPS samples opacity.r on TEXCOORD1, independently of diffuse alpha.
    #[texture(103)]
    #[sampler(104)]
    pub hair_opacity: Option<Handle<Image>>,
    #[texture(108)]
    #[sampler(109)]
    pub texture_1: Option<Handle<Image>>,
    #[texture(110)]
    #[sampler(111)]
    pub texture_2: Option<Handle<Image>>,
    #[texture(112)]
    #[sampler(113)]
    pub texture_3: Option<Handle<Image>>,
    /// Per zone, the two rows of custom_graphic::stamp.
    #[uniform(102)]
    pub rows: [Vec4; 8],
    /// y: hair opacity map; z: cloth graphics lie over, skin tattoos multiply; w: a Solid grip's flat colour.
    #[uniform(102)]
    pub enabled: Vec4,
    /// Per zone on/off.
    #[uniform(102)]
    pub zones: Vec4,
    /// Always the world's character lighting (retail_character::LIGHTING): with tint.w != 0 and key.w >= 0 its light,
    /// SH and key stand in for `retail`'s (customiser_stamp.wgsl).
    #[storage(114, read_only)]
    #[reflect(ignore)]
    pub lighting: Handle<bevy::render::storage::ShaderStorageBuffer>,
}
impl Default for SkinStamp {
    fn default() -> Self {
        Self {
            retail_mask: None,
            retail: default(),
            texture: None,
            hair_opacity: None,
            texture_1: None,
            texture_2: None,
            texture_3: None,
            rows: default(),
            enabled: Vec4::ZERO,
            zones: Vec4::ZERO,
            lighting: crate::retail_character::LIGHTING,
        }
    }
}
impl SkinStamp {
    /// Show `image` in `zone` (0-3) placed by custom_graphic::stamp.
    pub fn set_zone(&mut self, zone: usize, image: Handle<Image>, stamp: [Vec4; 2]) {
        *[&mut self.texture, &mut self.texture_1, &mut self.texture_2, &mut self.texture_3][zone] = Some(image);
        self.rows[zone * 2] = stamp[0];
        self.rows[zone * 2 + 1] = stamp[1];
        self.zones[zone] = 1.;
    }
}
impl MaterialExtension for SkinStamp {
    fn fragment_shader() -> ShaderRef {
        AssetPath::from(embedded_path!("customiser_stamp.wgsl"))
            .with_source("embedded")
            .into()
    }
    fn prepass_fragment_shader() -> ShaderRef {
        AssetPath::from(embedded_path!("customiser_prepass.wgsl"))
            .with_source("embedded")
            .into()
    }
    fn deferred_fragment_shader() -> ShaderRef {
        Self::fragment_shader()
    }
}
pub(crate) fn register(app: &mut App) {
    embedded_asset!(app, "customiser_stamp.wgsl");
    embedded_asset!(app, "customiser_prepass.wgsl");
    app.add_plugins(MaterialPlugin::<SkaterMaterial>::default());
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn embedded_shader_paths_match_this_binary() {
        for (shader, registered) in [
            (
                SkinStamp::fragment_shader(),
                embedded_path!("customiser_stamp.wgsl"),
            ),
            (
                SkinStamp::prepass_fragment_shader(),
                embedded_path!("customiser_prepass.wgsl"),
            ),
        ] {
            let ShaderRef::Path(path) = shader else {
                panic!("Expected embedded shader path")
            };
            assert_eq!(path, AssetPath::from(registered).with_source("embedded"));
        }
    }
    #[test]
    fn customiser_stamp_maps_authored_borders() {
        // Placement itself: custom_graphic::tests (retail's start table, fit and mips).
        assert_eq!(
            bevy::asset::embedded_path!("customiser_stamp.wgsl"),
            std::path::PathBuf::from(format!(
                "{}/customiser_stamp.wgsl",
                module_path!().split("::").next().unwrap()
            ))
        );
    }
    #[test]
    fn customiser_shader_compiles_offline() {
        use bevy::render::render_resource::{DownlevelFlags, WgpuFeatures};
        use bevy::shader::{ShaderCache, ShaderDefVal};
        let Ok(path) = std::env::var("SKATE_CAC_TEST_BEVY_SOURCES") else {
            return;
        };
        fn walk(path: &std::path::Path, result: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(path).unwrap().flatten() {
                let p = entry.path();
                if p.is_dir() {
                    walk(&p, result);
                } else if p.extension().is_some_and(|e| e == "wgsl") {
                    result.push(p);
                }
            }
        }
        let mut files = vec![];
        for entry in std::fs::read_dir(path).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("bevy_") && name.ends_with("-0.18.1") {
                walk(&entry.path().join("src"), &mut files);
            }
        }
        let mut assets = Assets::<Shader>::default();
        let mut cache =
            ShaderCache::<(), ()>::new(WgpuFeatures::empty(), DownlevelFlags::all(), |_, _, _| {
                Ok(())
            });
        let defs: Vec<_> = [
            ("MAX_DIRECTIONAL_LIGHTS", 10),
            ("MAX_CASCADES_PER_LIGHT", 4),
            ("MAX_VIEW_LIGHT_PROBES", 8),
            ("PER_OBJECT_BUFFER_BATCH_SIZE", 64),
            ("MATERIAL_BIND_GROUP", 3),
            ("AVAILABLE_STORAGE_BUFFER_BINDINGS", 8),
        ]
        .into_iter()
        .map(|(n, v)| ShaderDefVal::UInt(n.into(), v))
        .collect();
        for path in files {
            let source = std::fs::read_to_string(&path).unwrap();
            if !source.contains("#define_import_path") {
                continue;
            }
            let shader = Shader::from_wgsl_with_defs(
                source,
                path.to_string_lossy().to_string(),
                defs.clone(),
            );
            let h = assets.add(shader.clone());
            cache.set_shader(h.id(), shader);
        }
        for (source, path, prepass) in [
            (
                include_str!("retail_character_common.wgsl"),
                "retail_character_common.wgsl",
                false,
            ),
            (
                include_str!("retail_character.wgsl"),
                "retail_character.wgsl",
                false,
            ),
            (
                include_str!("customiser_stamp.wgsl"),
                "customiser_stamp.wgsl",
                false,
            ),
            (
                include_str!("customiser_prepass.wgsl"),
                "customiser_prepass.wgsl",
                true,
            ),
            // Native characters' depth/shadow pass: coverage on uv_b (retail_character.rs).
            (
                include_str!("retail_character_depth.wgsl"),
                "retail_character_depth.wgsl",
                true,
            ),
            // Snow on PBR props (benches, rails) reuses StandardMaterial lighting.
            (include_str!("retail_weather.wgsl"), "retail_weather.wgsl", false),
            (include_str!("retail_prop.wgsl"), "retail_prop.wgsl", false),
            // Rain/snow: real view lights and shadow sampling for the shelter test.
            (include_str!("retail_rain.wgsl"), "retail_rain.wgsl", false),
        ] {
            let shader = Shader::from_wgsl(source, path);
            let h = assets.add(shader.clone());
            cache.set_shader(h.id(), shader);
            for secondary_uv in [false, true] {
                let mut d = defs.clone();
                for name in [
                    "VERTEX_POSITIONS",
                    "VERTEX_UVS",
                    "MAY_DISCARD",
                    "VERTEX_UVS_A",
                    "VERTEX_OUTPUT_INSTANCE_INDEX",
                    "VERTEX_NORMALS",
                    "MESH_PIPELINE",
                    "STANDARD_MATERIAL",
                    "STANDARD_MATERIAL_BASE_COLOR_TEXTURE",
                ] {
                    d.push(ShaderDefVal::Bool(name.into(), true));
                }
                if secondary_uv {
                    d.push(ShaderDefVal::Bool("VERTEX_UVS_B".into(), true));
                    // The Ambient Occlusion setting's variant.
                    d.push(ShaderDefVal::Bool("SCREEN_SPACE_AMBIENT_OCCLUSION".into(), true));
                }
                if prepass {
                    d.push(ShaderDefVal::Bool("PREPASS_PIPELINE".into(), true));
                }
                cache
                    .get(&(), 0, h.id(), &d)
                    .unwrap_or_else(|e| panic!("{path} secondary UV {secondary_uv}: {e:?}"));
                if prepass {
                    for name in [
                        "PREPASS_FRAGMENT",
                        "NORMAL_PREPASS",
                        "NORMAL_PREPASS_OR_DEFERRED_PREPASS",
                    ] {
                        d.push(ShaderDefVal::Bool(name.into(), true));
                    }
                    cache
                        .get(&(), 0, h.id(), &d)
                        .unwrap_or_else(|e| panic!("Normal {path} UV {secondary_uv}: {e:?}"));
                }
            }
        }
    }
}
