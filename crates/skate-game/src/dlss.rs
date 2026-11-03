//! NVIDIA DLSS (item 28): Settings > Graphics > DLSS. Compiled only with the `dlss` feature (NVIDIA's SDK at build
//! time, nvngx_dlss.dll beside the exe: docs/dlss.md) and offered only where Bevy found `DlssSuperResolutionSupported`.
//! Bevy's DlssPlugin upscales between the main pass and the post passes (retail tone, grading, sharpening, the replay
//! grade), so those run at the full size and the HUD and menus never pass through it. This module adds the camera
//! parts, motion vectors for the sky (drawn outside the prepass) and history resets on camera cuts.
use bevy::{
    anti_alias::dlss::{Dlss, DlssPerfQualityMode, DlssSuperResolutionSupported},
    asset::{AssetPath, embedded_asset, embedded_path, uuid::Uuid},
    core_pipeline::{
        FullscreenShader, Skybox,
        core_3d::CORE_3D_DEPTH_FORMAT,
        prepass::{MotionVectorPrepass, NormalPrepass, PreviousViewData, PreviousViewUniforms, prepass_target_descriptors},
        skybox::prepass::{RenderSkyboxPrepassPipeline, SkyboxPrepassBindGroup},
    },
    prelude::*,
    render::{
        Extract, ExtractSchedule, Render, RenderApp, RenderStartup, RenderSystems,
        camera::{MipBias, TemporalJitter},
        render_resource::{
            BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, CompareFunction, DepthStencilState,
            FragmentState, MultisampleState, PipelineCache, RenderPipelineDescriptor, ShaderStages,
            SpecializedRenderPipeline, SpecializedRenderPipelines, binding_types::uniform_buffer,
        },
        renderer::RenderDevice,
        sync_world::RenderEntity,
        view::{Hdr, ViewUniform, ViewUniforms},
    },
};

/// The game's NGX application id (DlssProjectId): one fixed UUID for Skate 3 Remaster.
pub(crate) const PROJECT_ID: Uuid = Uuid::from_u128(0x5c3a_9e1f_7b24_4d0a_9f6e_2b8d_41c7_e3a5);

/// A [`crate::graphics_menu::DLSS_MODES`] index as Bevy's mode; None is off.
pub(crate) fn mode(index: u32) -> Option<DlssPerfQualityMode> {
    Some(match index {
        1 => DlssPerfQualityMode::Dlaa,
        2 => DlssPerfQualityMode::Quality,
        3 => DlssPerfQualityMode::Balanced,
        4 => DlssPerfQualityMode::Performance,
        5 => DlssPerfQualityMode::UltraPerformance,
        6 => DlssPerfQualityMode::Auto,
        _ => return None,
    })
}

pub(crate) struct DlssSettingsPlugin;
impl Plugin for DlssSettingsPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "dlss_sky_motion.wgsl");
        app.add_systems(PostStartup, detect.after(crate::graphics_menu::PresentationSetup))
            .add_systems(Update, camera.after(crate::graphics_menu::apply))
            .add_systems(PostUpdate, reset_on_cut.after(bevy::transform::TransformSystems::Propagate));
        if let Some(render) = app.get_sub_app_mut(RenderApp) {
            render
                .init_resource::<SpecializedRenderPipelines<SkyMotion>>()
                .add_systems(RenderStartup, init_sky_motion)
                .add_systems(ExtractSchedule, extract_cleanup)
                .add_systems(Render, prepare_sky_motion.in_set(RenderSystems::PrepareBindGroups));
        }
    }
}

/// NVSDK_NGX_DLSS_Hint_Render_Preset F: the CNN model, cheap on any RTX GPU.
const PRESET_CNN: u32 = 6;
/// Preset K: the transformer model, DLSS 310's default for every mode but Ultra Performance. Sharper, but on an
/// RTX 2050 it cost more than it saved (Quality 136 FPS vs 140-146 off and 174-176 with F, 10-11).
const PRESET_TRANSFORMER: u32 = 11;

/// The transformer on RTX 40/50 and later (Ada, Blackwell: the FP8 tensor cores NVIDIA built it for), the CNN on
/// RTX 20/30 and anything unrecognised. ponytail: reads the marketing name; a GPU named outside these patterns gets
/// the CNN, which is the safe (faster) pick.
fn render_preset(gpu: &str) -> u32 {
    let gpu = gpu.to_ascii_uppercase();
    let series = gpu.split("RTX ").nth(1).and_then(|r| r.chars().next()).and_then(|c| c.to_digit(10));
    let newer = gpu.contains(" ADA") || gpu.contains("BLACKWELL") || (!gpu.contains("QUADRO") && series.is_some_and(|s| s >= 4));
    if newer { PRESET_TRANSFORMER } else { PRESET_CNN }
}

/// The row shows, and the setting applies, only where DlssPlugin found the GPU, driver and DLL able to run it.
/// Runs before any camera has Dlss, so the first DLSS context already uses this GPU's model.
fn detect(
    menu: Option<ResMut<crate::graphics_menu::Menu>>,
    supported: Option<Res<DlssSuperResolutionSupported>>,
    adapter: Option<Res<bevy::render::renderer::RenderAdapterInfo>>,
) {
    let gpu = adapter.map(|a| a.name.clone()).unwrap_or_default();
    let preset = render_preset(&gpu);
    dlss_wgpu::super_resolution::RENDER_PRESET.store(preset, std::sync::atomic::Ordering::Relaxed);
    info!(
        "DLSS: {} ({gpu}: {} model)",
        if supported.is_some() { "supported" } else { "not supported on this system" },
        if preset == PRESET_CNN { "CNN" } else { "transformer" }
    );
    if let Some(mut menu) = menu {
        menu.dlss_supported = supported.is_some();
    }
}

/// DLSS on: `Dlss` (its required TemporalJitter, MipBias, DepthPrepass, MotionVectorPrepass and Hdr come with it;
/// graphics_menu::apply has already turned MSAA, FXAA/SMAA and SSAO off and keeps DepthPrepass). Off: exactly the
/// camera from before. Retail scenes only: they already have Hdr (camera.rs), which Bevy's DLSS needs; procedural
/// scenes have none, and adding it would change their tonemapping.
pub(crate) fn camera(
    mut commands: Commands,
    menu: Res<crate::graphics_menu::Menu>,
    cameras: Query<(Entity, Option<&Dlss>, Has<crate::retail_render::RetailTone>), With<Camera3d>>,
) {
    for (entity, dlss, retail) in &cameras {
        match (mode(menu.dlss()).filter(|_| retail), dlss) {
            (Some(mode), Some(dlss)) if dlss.perf_quality_mode == mode => {}
            (Some(mode), _) => {
                commands.entity(entity).insert(Dlss { perf_quality_mode: mode, reset: true, ..default() });
            }
            (None, Some(_)) => {
                commands.entity(entity).remove::<(Dlss, TemporalJitter, MipBias, MotionVectorPrepass)>();
            }
            (None, None) => {}
        }
    }
}

/// Bevy cleans a camera's render-world DLSS state only while it has Hdr; camera.rs drops Hdr on a switch to a
/// procedural scene a frame before `camera` drops Dlss, which would leave DLSS upscaling a non-HDR view.
fn extract_cleanup(mut commands: Commands, cameras: Extract<Query<(RenderEntity, Has<Hdr>), With<Camera3d>>>) {
    for (entity, hdr) in &cameras {
        if !hdr {
            if let Ok(mut view) = commands.get_entity(entity) {
                view.remove::<(Dlss, bevy::camera::MainPassResolutionOverride)>();
            }
        }
    }
}

/// A cut (map load, teleport, replay camera, the customiser) leaves no useful history: DLSS starts the image over
/// instead of ghosting the old view into the new one. ponytail: a jump threshold, not every cut's own event; skating
/// and vehicles move far less than 4 m or 40 degrees in a frame.
fn reset_on_cut(mut cameras: Query<(&GlobalTransform, &mut Dlss)>, mut last: Local<Option<(Vec3, Quat)>>) {
    for (transform, mut dlss) in &mut cameras {
        let (_, rotation, position) = transform.to_scale_rotation_translation();
        if let Some((from, turned)) = *last {
            if from.distance(position) > 4. || turned.angle_between(rotation) > 40_f32.to_radians() {
                dlss.reset = true;
            }
        }
        *last = Some((position, rotation));
    }
}

/// Motion vectors where the prepass drew nothing: the sky (RetailSkyMaterial has no prepass) and any cleared pixel,
/// moving only with the camera. Bevy's skybox prepass pass does this for `Skybox` cameras; the prepass node draws it
/// for any view carrying its pipeline and bind group components, which this gives DLSS views.
#[derive(Resource)]
pub(crate) struct SkyMotion {
    layout: BindGroupLayoutDescriptor,
    fullscreen: FullscreenShader,
    shader: Handle<Shader>,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SkyMotionKey {
    samples: u32,
    normal_prepass: bool,
}
impl SpecializedRenderPipeline for SkyMotion {
    type Key = SkyMotionKey;
    /// Bevy 0.18.1's SkyboxPrepassPipeline: a fullscreen triangle at depth 0 (reverse Z: only where nothing drew).
    fn specialize(&self, key: Self::Key) -> RenderPipelineDescriptor {
        RenderPipelineDescriptor {
            label: Some("dlss sky motion vectors".into()),
            layout: vec![self.layout.clone()],
            vertex: self.fullscreen.to_vertex_state(),
            depth_stencil: Some(DepthStencilState {
                format: CORE_3D_DEPTH_FORMAT,
                depth_write_enabled: false,
                depth_compare: CompareFunction::GreaterEqual,
                stencil: default(),
                bias: default(),
            }),
            multisample: MultisampleState { count: key.samples, mask: !0, alpha_to_coverage_enabled: false },
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                targets: prepass_target_descriptors(key.normal_prepass, true, false),
                ..default()
            }),
            ..default()
        }
    }
}
fn init_sky_motion(mut commands: Commands, fullscreen: Res<FullscreenShader>, assets: Res<AssetServer>) {
    commands.insert_resource(SkyMotion {
        layout: BindGroupLayoutDescriptor::new(
            "dlss sky motion vectors",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::FRAGMENT,
                (uniform_buffer::<ViewUniform>(true), uniform_buffer::<PreviousViewData>(true)),
            ),
        ),
        fullscreen: fullscreen.clone(),
        shader: assets.load(AssetPath::from(embedded_path!("dlss_sky_motion.wgsl")).with_source("embedded")),
    });
}
fn prepare_sky_motion(
    mut commands: Commands,
    cache: Res<PipelineCache>,
    mut pipelines: ResMut<SpecializedRenderPipelines<SkyMotion>>,
    pipeline: Res<SkyMotion>,
    (view_uniforms, previous): (Res<ViewUniforms>, Option<Res<PreviousViewUniforms>>),
    device: Res<RenderDevice>,
    views: Query<(Entity, Has<NormalPrepass>, &Msaa), (With<Dlss>, With<MotionVectorPrepass>, Without<Skybox>)>,
    stale: Query<Entity, (With<RenderSkyboxPrepassPipeline>, Without<Skybox>, Or<(Without<Dlss>, Without<MotionVectorPrepass>)>)>,
) {
    // DLSS off: the prepass has no motion vector target left for this pipeline to write.
    for entity in &stale {
        commands.entity(entity).remove::<(RenderSkyboxPrepassPipeline, SkyboxPrepassBindGroup)>();
    }
    let (Some(view), Some(previous)) = (view_uniforms.uniforms.binding(), previous.as_ref().and_then(|p| p.uniforms.binding()))
    else {
        return;
    };
    for (entity, normal_prepass, msaa) in &views {
        let id = pipelines.specialize(&cache, &pipeline, SkyMotionKey { samples: msaa.samples(), normal_prepass });
        let group = device.create_bind_group(
            "dlss sky motion vectors",
            &cache.get_bind_group_layout(&pipeline.layout),
            &BindGroupEntries::sequential((view.clone(), previous.clone())),
        );
        commands.entity(entity).insert((RenderSkyboxPrepassPipeline(id), SkyboxPrepassBindGroup(group)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphics_menu::{DLSS_MODES, test_menu};

    #[test]
    fn every_dlss_row_choice_has_a_mode() {
        assert_eq!(mode(0), None);
        assert_eq!(
            (1..DLSS_MODES.len() as u32).map(mode).collect::<Vec<_>>(),
            [
                DlssPerfQualityMode::Dlaa,
                DlssPerfQualityMode::Quality,
                DlssPerfQualityMode::Balanced,
                DlssPerfQualityMode::Performance,
                DlssPerfQualityMode::UltraPerformance,
                DlssPerfQualityMode::Auto,
            ]
            .map(Some)
        );
        assert_eq!(mode(DLSS_MODES.len() as u32), None, "an unknown index is off");
    }

    /// The transformer model only where its FP8 tensor cores are (RTX 40/50 and later); the CNN everywhere else.
    #[test]
    fn the_model_follows_the_gpu_generation() {
        for gpu in [
            "NVIDIA GeForce RTX 2050",
            "NVIDIA GeForce RTX 3090 Ti",
            "NVIDIA GeForce RTX 3060 Laptop GPU",
            "NVIDIA RTX A6000",
            "Quadro RTX 5000",
            "NVIDIA TITAN RTX",
            "",
        ] {
            assert_eq!(render_preset(gpu), PRESET_CNN, "{gpu}");
        }
        for gpu in [
            "NVIDIA GeForce RTX 4060 Laptop GPU",
            "NVIDIA GeForce RTX 5090",
            "NVIDIA RTX 2000 Ada Generation",
            "NVIDIA RTX PRO 6000 Blackwell Workstation Edition",
        ] {
            assert_eq!(render_preset(gpu), PRESET_TRANSFORMER, "{gpu}");
        }
    }

    /// Each mode adds DLSS's camera parts (and graphics_menu::apply drops MSAA, FXAA/SMAA and SSAO); Off restores the
    /// camera exactly, on a retail (Hdr) camera and a procedural one alike.
    #[test]
    fn dlss_modes_add_their_camera_parts_and_off_restores_the_camera() {
        use crate::graphics_menu::{SceneTarget, Setting};
        use bevy::{
            anti_alias::{fxaa::Fxaa, smaa::Smaa},
            core_pipeline::prepass::DepthPrepass,
            pbr::ScreenSpaceAmbientOcclusion,
            render::render_resource::TextureFormat,
            window::PrimaryWindow,
        };
        for retail in [false, true] {
            for post_aa in [0, 1, 2, 4] {
                let mut app = App::new();
                let mut images = Assets::<Image>::default();
                let target = images.add(Image::new_target_texture(1280, 720, TextureFormat::Rgba8UnormSrgb, None));
                let mut menu = test_menu();
                menu.dlss_supported = true;
                menu.choose(Setting::AmbientOcclusion, 1);
                menu.choose(Setting::Msaa, post_aa);
                app.insert_resource(SceneTarget(target))
                    .insert_resource(images)
                    .insert_resource(menu)
                    .add_systems(Update, (crate::graphics_menu::apply, camera).chain());
                app.world_mut().spawn((Window::default(), PrimaryWindow));
                let id = app.world_mut().spawn((Camera3d::default(), Msaa::Off)).id();
                if retail {
                    app.world_mut().entity_mut(id).insert((Hdr, crate::retail_render::RetailTone::default()));
                }
                let components = |app: &App| {
                    let mut names: Vec<String> =
                        app.world().inspect_entity(id).unwrap().map(|c| c.name().to_string()).collect();
                    names.sort();
                    (names, *app.world().get::<Msaa>(id).unwrap())
                };
                app.update();
                let before = components(&app);
                for index in 1..DLSS_MODES.len() {
                    app.world_mut().resource_mut::<crate::graphics_menu::Menu>().choose(Setting::Dlss, index);
                    app.update();
                    let entity = app.world().entity(id);
                    assert!(!entity.contains::<Fxaa>() && !entity.contains::<Smaa>());
                    if !retail {
                        // Procedural scenes (no Hdr): no DLSS, though the menu still hands AA and render scale to it.
                        assert!(!entity.contains::<Dlss>() && !entity.contains::<Hdr>() && !entity.contains::<TemporalJitter>());
                        continue;
                    }
                    assert_eq!(entity.get::<Dlss>().unwrap().perf_quality_mode, mode(index as u32).unwrap());
                    assert!(entity.contains::<TemporalJitter>() && entity.contains::<MipBias>());
                    assert!(entity.contains::<DepthPrepass>() && entity.contains::<MotionVectorPrepass>());
                    assert!(!entity.contains::<ScreenSpaceAmbientOcclusion>() && !entity.contains::<NormalPrepass>());
                    assert_eq!(*entity.get::<Msaa>().unwrap(), Msaa::Off);
                }
                app.world_mut().resource_mut::<crate::graphics_menu::Menu>().choose(Setting::Dlss, 0);
                app.update();
                assert_eq!(components(&app), before, "retail {retail}, anti-aliasing {post_aa}");
            }
        }
    }
}
