//! GPU-only exposure adaptation. Native evaluator, portable luminance sampling.
use bevy::{
    asset::embedded_asset,
    core_pipeline::{
        FullscreenShader,
        core_3d::graph::{Core3d, Node3d},
    },
    ecs::query::QueryItem,
    prelude::*,
    render::{
        Render, RenderApp, RenderStartup, RenderSystems,
        extract_component::ExtractComponentPlugin,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_graph::{
            NodeRunError, RenderGraphContext, RenderGraphExt, RenderLabel, ViewNode, ViewNodeRunner,
        },
        render_resource::{binding_types::*, *},
        renderer::{RenderContext, RenderDevice, RenderQueue},
        view::{ExtractedView, Msaa, ViewDepthTexture, ViewTarget},
    },
};
use std::{collections::VecDeque, sync::Mutex};

#[derive(Resource, Clone, ExtractResource)]
pub(super) struct Settings {
    /// Target luminance, min, max, damping; Skate 2 maps' zones override it (retail_grading.rs).
    pub(super) tuning: Vec4,
    /// A Skate 2 cutscene shot's TONEMAP_VALUE (retail_grading.rs ShotPost), 0 without: while > 0 the meter is held
    /// at it, as Skate 2's 0x8232c53c replaces its exposure, and adaptation carries on from it after.
    pub(super) hold: f32,
    timing: Vec4,
    /// Rain strength, lightning flash and clock for the final screen pass.
    pub(super) weather: Vec4,
    /// Time of day (retail_daynight.rs) scales the tuning: a lower target so night
    /// reads darker than day, raised bounds so dark corners stay readable. One at noon.
    pub(super) day: Vec4,
    /// Skate 2 maps meter as Skate 2 does (0x82325a48, retail_exposure.wgsl); set by retail_grading.rs.
    pub(super) skate2: bool,
    /// Skate 1's map meters as Skate 1 does (0x8273ef80, retail_exposure.wgsl); set by retail_grading.rs.
    pub(super) skate1: bool,
    /// #33: a videoscreen is in view, so the tone pass also refreshes [`FEED`].
    feed: bool,
}
impl Default for Settings {
    fn default() -> Self {
        // Missing metadata retains the existing fixed exposure.
        Self {
            tuning: Vec4::new(0., 2.5, 2.5, 0.),
            hold: 0.,
            timing: Vec4::ZERO,
            weather: Vec4::ZERO,
            day: Vec4::ONE,
            skate2: false,
            skate1: false,
            feed: false,
        }
    }
}
/// #33: retail's videoscreen material samples the main scene target instead of its own texture (TU3 827E596C:
/// G(0x83083C5C)+0x1E8 = PostFX->rt_e8; Skate 2 8268FFE0: G(0x82DF2494)+0x768 = PostFX->rt_88): the player's own view
/// as the last frame resolved it, at the 1152x640 the game renders. Ours copies it here after the tone pass.
pub(crate) const FEED: Handle<Image> = bevy::asset::uuid_handle!("5c0b1e2a-9d7f-4e61-b3a8-2f4c6d8e0a17");
const FEED_SIZE: UVec2 = UVec2::new(1152, 640);
fn feed_image(mut images: ResMut<Assets<Image>>) {
    let mut image = Image::new_target_texture(FEED_SIZE.x, FEED_SIZE.y, TextureFormat::Rgba8Unorm, None);
    image.data = Some([0, 0, 0, 255].repeat((FEED_SIZE.x * FEED_SIZE.y) as usize));
    image.sampler = bevy::image::ImageSampler::linear();
    image.asset_usage = bevy::asset::RenderAssetUsages::RENDER_WORLD;
    images.insert(FEED.id(), image).expect("reserved jumbotron feed asset");
}
#[derive(serde::Deserialize)]
struct Authored {
    target_luminance: f32,
    min: f32,
    max: f32,
    damping: f32,
}

pub(super) fn install(app: &mut App) {
    embedded_asset!(app, "retail_exposure.wgsl");
    app.init_resource::<Settings>()
        .add_plugins((
            ExtractResourcePlugin::<Settings>::default(),
            ExtractComponentPlugin::<super::RetailTone>::default(),
        ))
        .add_systems(Startup, (load, feed_image))
        .add_systems(
            PreUpdate,
            load.after(crate::map_transition::MapTransitionSet)
                .run_if(super::world_changed),
        )
        .add_systems(Update, advance);
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render
            .add_systems(RenderStartup, initialize)
            .add_systems(Render, upload.in_set(RenderSystems::PrepareResources))
            .add_systems(
                Render,
                prune_bindings.in_set(RenderSystems::PrepareBindGroups),
            )
            .add_render_graph_node::<ViewNodeRunner<ExposureNode>>(Core3d, ExposureLabel)
            .add_render_graph_edges(
                Core3d,
                (
                    Node3d::Tonemapping,
                    ExposureLabel,
                    Node3d::EndMainPassPostProcessing,
                ),
            );
    }
}

fn load(
    config: Res<crate::config::Config>,
    retail: Res<super::RetailScene>,
    mut settings: ResMut<Settings>,
) {
    let generation = settings.timing.y + 1.;
    *settings = Settings::default();
    settings.timing.y = generation;
    if !retail.0 {
        return;
    }
    if std::env::var_os("SKATE_FIXED_EXPOSURE").is_some_and(|v| v == "1") {
        info!("RETAIL_EXPOSURE: fixed 2.5 comparison mode");
        return;
    }
    let profile = config
        .map_path
        .as_ref()
        .and_then(|p| p.file_stem())
        .and_then(|name| {
            let bytes =
                std::fs::read(config.asset_root.join("private/exposure-profiles.json")).ok()?;
            let mut profiles: std::collections::BTreeMap<String, Authored> =
                serde_json::from_slice(&bytes).ok()?;
            profiles.remove(&name.to_string_lossy().to_lowercase())
        });
    let fallback = || {
        std::fs::read(config.asset_root.join("private/exposure.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Authored>(&b).ok())
    };
    if let Some(a) = profile.or_else(fallback) {
        if [a.target_luminance, a.min, a.max, a.damping]
            .iter()
            .all(|x| x.is_finite())
            && a.min > 0.
            && a.max >= a.min
            && a.target_luminance > 0.
            && a.damping >= 0.
        {
            settings.tuning = Vec4::new(a.target_luminance, a.min, a.max, a.damping);
            info!(
                "RETAIL_EXPOSURE: authored target={} range={}..{} damping={}; portable GPU meter",
                a.target_luminance, a.min, a.max, a.damping
            );
        }
    }
}
fn advance(time: Res<Time>, screens: Query<&ViewVisibility, With<super::JumbotronScreen>>, mut settings: ResMut<Settings>) {
    settings.timing.x = time.delta_secs().clamp(0., 0.05);
    // Last frame's visibility, so a screen coming into view shows one stale frame first.
    settings.feed = screens.iter().any(|v| v.get());
}

#[derive(Resource)]
struct Pipeline {
    compute_layout: BindGroupLayoutDescriptor,
    tone_layout: BindGroupLayoutDescriptor,
    /// The meter without and with MSAA: its depth binding (group 1, Skate 1's sky alpha) is multisampled with it.
    computes: [CachedComputePipelineId; 2],
    /// The tone pass per MSAA sample count (1, 2, 4, 8): its depth binding (group 2) is multisampled above 1.
    tones: [CachedRenderPipelineId; 4],
    /// Skate 1's film passes (skate1_film, _down, _blur_h, _blur_v) per MSAA sample count.
    films: [[CachedRenderPipelineId; 4]; 4],
    /// #33: the jumbotron copy into [`FEED`] (retail_tone.wgsl `jumbotron`, group 0 only).
    feed: CachedRenderPipelineId,
    depth_layouts: [BindGroupLayoutDescriptor; 2],
    /// 1x1 depth bound instead of the scene's off Skate 1 (single-sampled, multisampled).
    depth_stand_ins: [TextureView; 2],
    /// #36: 1x1 (zero) bound instead of the godray target without godrays (retail_godray.rs).
    godray_stand_in: TextureView,
    /// The camera's near plane (reverse-Z depth -> distance), rewritten each frame.
    near: Buffer,
    settings: Buffer,
    state: Buffer,
    sampler: Sampler,
    // The buffers, sampler and layouts are immutable for this Pipeline's life.
    // Source views can alternate, resize or belong to different cameras. Keep a
    // bounded cache so retired targets cannot accumulate across map/size changes.
    bindings: Mutex<VecDeque<(TextureViewId, BindGroup, BindGroup)>>,
    /// perf 10-19: the depth group by depth view (a stand-in or a camera's scene depth), as `bindings`; it was built
    /// every frame. SKATE_POST_GROUP_CACHE=0 builds it every frame again.
    depth_groups: Mutex<VecDeque<((TextureViewId, TextureViewId), BindGroup)>>,
}

fn initialize(
    mut commands: Commands,
    device: Res<RenderDevice>,
    cache: Res<PipelineCache>,
    assets: Res<AssetServer>,
    fullscreen: Res<FullscreenShader>,
) {
    let compute_layout = BindGroupLayoutDescriptor::new(
        "retail exposure meter",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                uniform_buffer::<[Vec4; 2]>(false),
                storage_buffer::<Vec4>(false),
            ),
        ),
    );
    let tone_layout = BindGroupLayoutDescriptor::new(
        "retail exposed tone",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                storage_buffer_read_only::<Vec4>(false),
                uniform_buffer::<[Vec4; 3]>(false),
            ),
        ),
    );
    let meter_shader = assets.load(
        bevy::asset::AssetPath::from(bevy::asset::embedded_path!("retail_exposure.wgsl")).with_source("embedded"),
    );
    let computes = [false, true].map(|multisampled| {
        cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some("retail exposure".into()),
            layout: vec![compute_layout.clone(), depth_layout(multisampled)],
            shader: meter_shader.clone(),
            shader_defs: if multisampled { vec!["DEPTH_MSAA".into()] } else { vec![] },
            entry_point: Some("meter".into()),
            ..default()
        })
    });
    let depth_layouts = [depth_layout(false), depth_layout(true)];
    let tone_shader = assets.load(
        bevy::asset::AssetPath::from(bevy::asset::embedded_path!("retail_tone.wgsl")).with_source("embedded"),
    );
    let queue = |samples: u32, entry: &'static str, format: TextureFormat| {
        let multisampled = samples > 1;
        cache.queue_render_pipeline(RenderPipelineDescriptor {
            label: Some("retail exposed tone".into()),
            layout: vec![tone_layout.clone(), super::grading::tone_layout(), depth_layouts[multisampled as usize].clone()],
            vertex: fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: tone_shader.clone(),
                shader_defs: if multisampled { vec!["DEPTH_MSAA".into()] } else { vec![] },
                entry_point: Some(entry.into()),
                targets: vec![Some(ColorTargetState {
                    format,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
            }),
            ..default()
        })
    };
    let tones = [1u32, 2, 4, 8].map(|samples| queue(samples, "fragment", ViewTarget::TEXTURE_FORMAT_HDR));
    // Skate 1's film passes (retail_grading.rs Gpu::skate1): 8-bit targets, as its own.
    let films = [1u32, 2, 4, 8].map(|samples| {
        ["skate1_film", "skate1_down", "skate1_blur_h", "skate1_blur_v"].map(|entry| queue(samples, entry, TextureFormat::Rgba8Unorm))
    });
    let feed = cache.queue_render_pipeline(RenderPipelineDescriptor {
        label: Some("retail jumbotron feed".into()),
        layout: vec![tone_layout.clone()],
        vertex: fullscreen.to_vertex_state(),
        fragment: Some(FragmentState {
            shader: tone_shader.clone(),
            entry_point: Some("jumbotron".into()),
            targets: vec![Some(ColorTargetState { format: TextureFormat::Rgba8Unorm, blend: None, write_mask: ColorWrites::ALL })],
            ..default()
        }),
        ..default()
    });
    let settings = device.create_buffer(&BufferDescriptor {
        label: Some("retail exposure settings"),
        size: 48,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let state = device.create_buffer_with_data(&BufferInitDescriptor {
        label: Some("retail exposure state"),
        contents: &bytes([Vec4::new(2.5, 0., 0., 0.)]),
        usage: BufferUsages::STORAGE,
    });
    let near = device.create_buffer(&BufferDescriptor {
        label: Some("retail tone near plane"),
        size: 16,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let depth_stand_ins = [1, 4].map(|sample_count| {
        device
            .create_texture(&TextureDescriptor {
                label: Some("retail tone depth stand-in"),
                size: Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count,
                dimension: TextureDimension::D2,
                format: TextureFormat::Depth32Float,
                usage: TextureUsages::TEXTURE_BINDING | TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&TextureViewDescriptor::default())
    });
    let godray_stand_in = device
        .create_texture(&TextureDescriptor {
            label: Some("retail godray stand-in"),
            size: Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba16Float,
            usage: TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&TextureViewDescriptor::default());
    commands.insert_resource(Pipeline {
        compute_layout,
        tone_layout,
        computes,
        tones,
        films,
        feed,
        depth_layouts,
        depth_stand_ins,
        godray_stand_in,
        near,
        settings,
        state,
        sampler: device.create_sampler(&SamplerDescriptor {
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..default()
        }),
        bindings: default(),
        depth_groups: default(),
    });
}
/// The scene depth and the camera's near plane: the tone pass's group 2 (Skate 1's depth of field), and group 1 of
/// the bright pass (retail_grading.wgsl) and the meter, which read Skate 1's sky from it. #36: then the godray target
/// (retail_godray.rs), which the tone and bright passes add to the encoded scene.
pub(super) fn depth_layout(multisampled: bool) -> BindGroupLayoutDescriptor {
    BindGroupLayoutDescriptor::new(
        "retail tone depth",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT | ShaderStages::COMPUTE,
            (
                if multisampled { texture_depth_2d_multisampled() } else { texture_depth_2d() },
                uniform_buffer::<Vec4>(false),
                texture_2d(TextureSampleType::Float { filterable: false }),
            ),
        ),
    )
}
fn bytes<const N: usize>(values: [Vec4; N]) -> Vec<u8> {
    values
        .into_iter()
        .flat_map(|v| v.to_array())
        .flat_map(f32::to_le_bytes)
        .collect()
}
fn upload(settings: Res<Settings>, pipeline: Res<Pipeline>, queue: Res<RenderQueue>) {
    let mut data = [0u8; 48];
    // The meter clamps to min..max: equal bounds hold it.
    let tuning = if settings.hold > 0. {
        Vec4::new(0., settings.hold, settings.hold, 0.)
    } else {
        settings.tuning * settings.day
    };
    // timing: frame seconds, map generation, Skate 2 meter, Skate 1 meter.
    let timing = settings
        .timing
        .with_z(f32::from(u8::from(settings.skate2)))
        .with_w(f32::from(u8::from(settings.skate1)));
    for (chunk, value) in data.chunks_exact_mut(4).zip(
        [tuning, timing, settings.weather]
            .into_iter()
            .flat_map(|v| v.to_array()),
    ) {
        chunk.copy_from_slice(&value.to_le_bytes());
    }
    queue.write_buffer(&pipeline.settings, 0, &data);
}

fn prune_bindings(pipeline: Res<Pipeline>, views: Query<(&ViewTarget, Option<&ViewDepthTexture>), With<super::RetailTone>>) {
    // Bind groups retain their textures. Release retired camera/resize targets
    // before rendering, including when there are no exposure views left.
    pipeline.bindings.lock().unwrap().retain(|(id, _, _)| {
        views.iter().any(|(view, _)| {
            *id == view.main_texture_view().id() || *id == view.main_texture_other_view().id()
        })
    });
    // perf 10-19: and retired scene depths (the stand-ins stay).
    pipeline.depth_groups.lock().unwrap().retain(|((id, _), _)| {
        pipeline.depth_stand_ins.iter().any(|s| s.id() == *id)
            || views.iter().any(|(_, depth)| depth.is_some_and(|d| d.view().id() == *id))
    });
}

#[derive(Debug, Hash, PartialEq, Eq, Clone, RenderLabel)]
struct ExposureLabel;
#[derive(Default)]
struct ExposureNode;
impl ViewNode for ExposureNode {
    type ViewQuery = (
        &'static ViewTarget,
        &'static super::RetailTone,
        &'static ViewDepthTexture,
        &'static Msaa,
        &'static ExtractedView,
        Option<&'static bevy::camera::MainPassResolutionOverride>,
    );
    fn run<'w>(
        &self,
        _: &mut RenderGraphContext,
        context: &mut RenderContext,
        (view, _, depth, msaa, extracted, resolution): QueryItem<Self::ViewQuery>,
        world: &'w World,
    ) -> Result<(), NodeRunError> {
        let p = world.resource::<Pipeline>();
        let cache = world.resource::<PipelineCache>();
        let variant = [1, 2, 4, 8].iter().position(|&n| n == msaa.samples()).unwrap_or(0);
        let (Some(compute), Some(tone)) = (
            cache.get_compute_pipeline(p.computes[(variant > 0) as usize]),
            cache.get_render_pipeline(p.tones[variant]),
        ) else {
            return Ok(());
        };
        // Reverse-Z perspective: depth = near / distance; the projection keeps near in [3][2].
        let near = extracted.clip_from_view.w_axis.z;
        // DLSS draws the main pass (and its depth) into the top-left render-resolution corner and upscales the colour
        // to the full view before these passes: depth lookups scale by that share (exactly 1 without it).
        let share = resolution.map_or(Vec2::ONE, |r| r.0.as_vec2() / extracted.viewport.zw().as_vec2().max(Vec2::ONE));
        world.resource::<RenderQueue>().write_buffer(&p.near, 0, &bytes([Vec4::new(near, share.x, share.y, 0.)]));
        // perf 10-18: only Skate 1's passes read the depth (its sky alpha, depth of field, motion blur); elsewhere a
        // 1x1 stand-in spares readying the (multisampled) scene depth for sampling. SKATE_POST_DEPTH=always: the scene's.
        static ALWAYS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let always = *ALWAYS.get_or_init(|| std::env::var("SKATE_POST_DEPTH").is_ok_and(|v| v == "always"));
        let skate1 = world.resource::<Settings>().skate1 || world.resource::<super::grading::Gpu>().film();
        let depth_view = if skate1 || always { depth.view() } else { &p.depth_stand_ins[(variant > 0) as usize] };
        // perf 11-01: these passes record into their own encoder, finished below.
        let mut encoder =
            context.render_device().create_command_encoder(&CommandEncoderDescriptor { label: Some("retail post") });
        // #36: the view's godrays, before the bright and tone passes read them.
        let godray =
            super::godray::draw(context, &mut encoder, world, depth, msaa.samples(), extracted, resolution.map(|r| r.0));
        let godray_view = godray.as_ref().unwrap_or(&p.godray_stand_in);
        let key = (depth_view.id(), godray_view.id());
        let create = || {
            context.render_device().create_bind_group(
                "retail tone depth",
                &cache.get_bind_group_layout(&p.depth_layouts[(variant > 0) as usize]),
                &BindGroupEntries::sequential((depth_view, p.near.as_entire_binding(), godray_view)),
            )
        };
        // perf 10-19: built once per depth view (near is rewritten in place), SKATE_POST_GROUP_CACHE=0 every frame.
        let depth_group = if crate::config::perf_on("SKATE_POST_GROUP_CACHE") {
            let mut groups = p.depth_groups.lock().unwrap();
            match groups.iter().find(|(id, _)| *id == key) {
                Some((_, group)) => group.clone(),
                None => {
                    if groups.len() == 4 {
                        groups.pop_front();
                    }
                    let group = create();
                    groups.push_back((key, group.clone()));
                    group
                }
            }
        } else {
            create()
        };
        let post = view.post_process_write();
        use bevy::render::diagnostic::RecordDiagnostics;
        let diagnostics = context.diagnostic_recorder();
        let mut bindings = p.bindings.lock().unwrap();
        let index = if let Some(index) = bindings
            .iter()
            .position(|(id, _, _)| *id == post.source.id())
        {
            index
        } else {
            let meter = context.render_device().create_bind_group(
                "retail meter",
                &cache.get_bind_group_layout(&p.compute_layout),
                &BindGroupEntries::sequential((
                    post.source,
                    &p.sampler,
                    p.settings.as_entire_binding(),
                    p.state.as_entire_binding(),
                )),
            );
            let output = context.render_device().create_bind_group(
                "retail exposed tone",
                &cache.get_bind_group_layout(&p.tone_layout),
                &BindGroupEntries::sequential((
                    post.source,
                    &p.sampler,
                    p.state.as_entire_binding(),
                    p.settings.as_entire_binding(),
                )),
            );
            if bindings.len() == 8 {
                bindings.pop_front();
            }
            bindings.push_back((post.source.id(), meter, output));
            bindings.len() - 1
        };
        let (_, meter, output) = &bindings[index];
        let size = post.source_texture.size();
        let size = UVec2::new(size.width, size.height);
        let gpu = world.resource::<super::grading::Gpu>();
        let bloom =
            gpu.frame(context, &mut encoder, cache, post.source, size, &p.state, &p.settings, (&depth_group, variant > 0));
        {
            let mut pass = encoder
                .begin_compute_pass(&ComputePassDescriptor {
                    label: Some("retail exposure meter"),
                    timestamp_writes: None,
                });
            pass.set_pipeline(compute);
            let span = diagnostics.pass_span(&mut pass, "retail_exposure_meter");
            pass.set_bind_group(0, meter, &[]);
            pass.set_bind_group(1, &depth_group, &[]);
            pass.dispatch_workgroups(1, 1, 1);
            span.end(&mut pass);
        }
        // Skate 1's map: its E8DC/66ED/3D17/9B58 film passes (after the meter, as its exposure feeds them).
        let clip_from_world = extracted
            .clip_from_world
            .unwrap_or_else(|| extracted.clip_from_view * extracted.world_from_view.to_matrix().inverse());
        let passes = p.films[variant].map(|id| cache.get_render_pipeline(id));
        let film = match passes {
            [Some(a), Some(b), Some(c), Some(d)] => gpu.skate1(
                context,
                &mut encoder,
                cache,
                world.resource::<RenderQueue>(),
                &bloom,
                size,
                [a, b, c, d],
                (output, &depth_group),
                clip_from_world,
            ),
            _ => None,
        };
        let grading = film.unwrap_or_else(|| gpu.group(context.render_device(), cache, &bloom, None));
        let mut pass = self::pass(context, &mut encoder, RenderPassDescriptor {
            label: Some("retail exposed tone"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: post.destination,
                depth_slice: None,
                resolve_target: None,
                ops: Operations::default(),
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_render_pipeline(tone);
        let span = diagnostics.pass_span(&mut pass, "retail_exposed_tone");
        pass.set_bind_group(0, output, &[]);
        pass.set_bind_group(1, &grading, &[]);
        pass.set_bind_group(2, &depth_group, &[]);
        pass.draw(0..3, 0..1);
        span.end(&mut pass);
        drop(pass);
        // #33: one 1152x640 fullscreen pass, only while a screen is in view; the screens sample it next frame.
        let target = world.resource::<bevy::render::render_asset::RenderAssets<bevy::render::texture::GpuImage>>().get(FEED.id());
        if world.resource::<Settings>().feed
            && let (Some(feed), Some(target)) = (cache.get_render_pipeline(p.feed), target)
        {
            let mut pass = self::pass(context, &mut encoder, RenderPassDescriptor {
                label: Some("retail jumbotron feed"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &target.texture_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations::default(),
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_render_pipeline(feed);
            let span = diagnostics.pass_span(&mut pass, "retail_jumbotron_feed");
            pass.set_bind_group(0, output, &[]);
            pass.draw(0..3, 0..1);
            span.end(&mut pass);
        }
        // perf 11-01: finishing the encoder (wgpu replays and checks every pass: ~0.5 ms for Skate 1's 14) runs with
        // the frame's other command buffer tasks instead of on the render graph thread; the command buffers keep their
        // order. SKATE_POST_TASK=0: finished here.
        if crate::config::perf_on("SKATE_POST_TASK") {
            context.add_command_buffer_generation_task(move |_| encoder.finish());
        } else {
            context.add_command_buffer(encoder.finish());
        }
        Ok(())
    }
}

/// A tracked render pass on `encoder`, as [`RenderContext::begin_tracked_render_pass`] begins one on its own.
pub(super) fn pass<'a>(
    context: &RenderContext,
    encoder: &'a mut CommandEncoder,
    descriptor: RenderPassDescriptor<'_>,
) -> bevy::render::render_phase::TrackedRenderPass<'a> {
    bevy::render::render_phase::TrackedRenderPass::new(context.render_device(), encoder.begin_render_pass(&descriptor))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// perf 10-19 (SKATE_HDR_RG11B10, vendor bevy_render): a pipeline built for the HDR constant, as the tone pass
    /// is, targets the chosen HDR format; its other targets keep theirs.
    #[test]
    fn hdr_targets_follow_the_chosen_format() {
        let target = |format| Some(ColorTargetState { format, blend: None, write_mask: ColorWrites::ALL });
        let mut descriptor = RenderPipelineDescriptor {
            fragment: Some(FragmentState {
                targets: vec![target(ViewTarget::TEXTURE_FORMAT_HDR), target(TextureFormat::Rgba8Unorm), None],
                ..default()
            }),
            ..default()
        };
        retarget_hdr(&mut descriptor, TextureFormat::Rg11b10Ufloat);
        let formats: Vec<_> = descriptor.fragment.unwrap().targets.iter().map(|t| t.as_ref().map(|t| t.format)).collect();
        assert_eq!(formats, [Some(TextureFormat::Rg11b10Ufloat), Some(TextureFormat::Rgba8Unorm), None]);
    }
}
