//! Bug 10-25 #36: Skate 2's godray.spotlight beams (New San Vanelona's Big Event stadium, 46 quads) drew on the PBR
//! fallback as opaque dark slabs. Retail's effect godrayspotlight (render.big shaders/final/godrayspotlight.xml) adds
//! them to the frame: SRCBLEND ONE, DESTBLEND ONE (alpha too), ALPHABLEND TRUE, ZENABLE TRUE, ZWRITEENABLE FALSE,
//! CULLMODE FRONT (baseenvironment's too, so ours is the world's Face::Back). The frame it adds to holds the world's
//! sqrt-encoded output (GRADING_AUDIT_S2.md row 1), which the engine only forms in its tone pass: the beams accumulate
//! here, depth-tested against the scene, and the tone and bright passes add this target to that encoded value
//! (retail_tone.wgsl, retail_grading.wgsl `godray`).
use bevy::{
    asset::{AssetPath, embedded_asset, embedded_path},
    core_pipeline::core_3d::CORE_3D_DEPTH_FORMAT,
    mesh::VertexBufferLayout,
    ecs::query::QueryItem,
    prelude::*,
    render::{
        Render, RenderApp, RenderStartup, RenderSystems,
        extract_component::{ExtractComponent, ExtractComponentPlugin},
        render_asset::RenderAssets,
        render_resource::{binding_types::*, *},
        renderer::{RenderContext, RenderDevice, RenderQueue},
        texture::GpuImage,
        view::{ExtractedView, ViewDepthTexture},
    },
    shader::Shader,
};
use std::{collections::HashMap, sync::{Arc, Mutex}};

/// The package's material shader name.
pub(crate) const SHADER: &str = "godray.spotlight";

/// One godray.spotlight batch: world-space triangle corners (position, normal, uv) and its diffuse.
#[derive(Component, Clone)]
pub(crate) struct Godray {
    pub(crate) corners: Arc<[[f32; 8]]>,
    pub(crate) diffuse: Handle<Image>,
}
impl ExtractComponent for Godray {
    type QueryData = (&'static Godray, &'static InheritedVisibility);
    type QueryFilter = ();
    type Out = Godray;
    fn extract_component((godray, visible): QueryItem<'_, '_, Self::QueryData>) -> Option<Godray> {
        visible.get().then(|| godray.clone())
    }
}

pub(super) fn install(app: &mut App) {
    embedded_asset!(app, "retail_godray.wgsl");
    app.add_plugins(ExtractComponentPlugin::<Godray>::default());
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render
            .add_systems(RenderStartup, initialize)
            .add_systems(Render, prepare.in_set(RenderSystems::PrepareBindGroups));
    }
}

const SAMPLES: [u32; 4] = [1, 2, 4, 8];

/// The traced pass state (module docs); `samples` is the scene depth's MSAA count.
fn descriptor(shader: Handle<Shader>, layout: Vec<BindGroupLayoutDescriptor>, samples: u32) -> RenderPipelineDescriptor {
    let add = BlendComponent { src_factor: BlendFactor::One, dst_factor: BlendFactor::One, operation: BlendOperation::Add };
    RenderPipelineDescriptor {
        label: Some("retail godray".into()),
        layout,
        vertex: VertexState {
            shader: shader.clone(),
            entry_point: Some("vertex".into()),
            buffers: vec![VertexBufferLayout::from_vertex_formats(
                VertexStepMode::Vertex,
                [VertexFormat::Float32x3, VertexFormat::Float32x3, VertexFormat::Float32x2],
            )],
            ..default()
        },
        primitive: PrimitiveState { cull_mode: Some(Face::Back), ..default() },
        // Reverse-Z: retail's LESSEQUAL.
        depth_stencil: Some(DepthStencilState {
            format: CORE_3D_DEPTH_FORMAT,
            depth_write_enabled: false,
            depth_compare: CompareFunction::GreaterEqual,
            stencil: default(),
            bias: default(),
        }),
        multisample: MultisampleState { count: samples, ..default() },
        fragment: Some(FragmentState {
            shader,
            entry_point: Some("fragment".into()),
            targets: vec![Some(ColorTargetState {
                format: TextureFormat::Rgba16Float,
                blend: Some(BlendState { color: add, alpha: add }),
                write_mask: ColorWrites::ALL,
            })],
            ..default()
        }),
        ..default()
    }
}

#[derive(Resource)]
struct Pipeline {
    view_layout: BindGroupLayoutDescriptor,
    texture_layout: BindGroupLayoutDescriptor,
    /// Per MSAA sample count (SAMPLES).
    pipelines: [CachedRenderPipelineId; 4],
    /// clip_from_world, eye: rewritten per view.
    view: Buffer,
}

fn initialize(mut commands: Commands, device: Res<RenderDevice>, cache: Res<PipelineCache>, assets: Res<AssetServer>) {
    let view_layout = BindGroupLayoutDescriptor::new(
        "retail godray view",
        &BindGroupLayoutEntries::single(ShaderStages::VERTEX_FRAGMENT, uniform_buffer::<[Vec4; 5]>(false)),
    );
    let texture_layout = BindGroupLayoutDescriptor::new(
        "retail godray diffuse",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (texture_2d(TextureSampleType::Float { filterable: true }), sampler(SamplerBindingType::Filtering)),
        ),
    );
    let shader = assets.load(AssetPath::from(embedded_path!("retail_godray.wgsl")).with_source("embedded"));
    let pipelines = SAMPLES.map(|samples| {
        cache.queue_render_pipeline(descriptor(shader.clone(), vec![view_layout.clone(), texture_layout.clone()], samples))
    });
    let view = device.create_buffer(&BufferDescriptor {
        label: Some("retail godray view"),
        size: 80,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    commands.insert_resource(Pipeline { view_layout, texture_layout, pipelines, view });
    commands.insert_resource(Batches::default());
}

/// The frame's godray draws, rebuilt in prepare.
#[derive(Resource, Default)]
struct Batches {
    vertices: Option<Buffer>,
    /// The extracted batches' identity (corner arrays), so an unchanged set skips the upload.
    uploaded: Vec<usize>,
    view_group: Option<BindGroup>,
    groups: HashMap<TextureViewId, BindGroup>,
    draws: Vec<(TextureViewId, std::ops::Range<u32>)>,
    /// The accumulation target (multisampled with the scene depth) and its resolved view.
    targets: Mutex<Option<(Extent3d, u32, Option<TextureView>, TextureView)>>,
}

fn prepare(
    godrays: Query<&Godray>,
    images: Res<RenderAssets<GpuImage>>,
    device: Res<RenderDevice>,
    cache: Res<PipelineCache>,
    pipeline: Res<Pipeline>,
    mut batches: ResMut<Batches>,
) {
    let batches = &mut *batches;
    batches.draws.clear();
    if godrays.is_empty() {
        // Another map: release the buffers, groups and targets.
        if batches.vertices.is_some() {
            *batches = Batches { view_group: batches.view_group.take(), ..default() };
        }
        return;
    }
    let mut ready = Vec::new();
    for godray in &godrays {
        // Drawn once its texture is resident.
        if let Some(image) = images.get(godray.diffuse.id()) {
            ready.push((godray, image));
        }
    }
    let identity: Vec<usize> = ready.iter().map(|(g, _)| Arc::as_ptr(&g.corners) as *const u8 as usize).collect();
    if identity != batches.uploaded || batches.vertices.is_none() {
        let bytes: Vec<u8> = ready
            .iter()
            .flat_map(|(g, _)| g.corners.iter().flatten().flat_map(|f| f.to_le_bytes()))
            .collect();
        if bytes.is_empty() {
            return;
        }
        batches.vertices = Some(device.create_buffer_with_data(&BufferInitDescriptor {
            label: Some("retail godray corners"),
            contents: &bytes,
            usage: BufferUsages::VERTEX,
        }));
        batches.uploaded = identity;
    }
    let mut first = 0;
    for (godray, image) in ready {
        let id = image.texture_view.id();
        batches.groups.entry(id).or_insert_with(|| {
            device.create_bind_group(
                "retail godray diffuse",
                &cache.get_bind_group_layout(&pipeline.texture_layout),
                &BindGroupEntries::sequential((&image.texture_view, &image.sampler)),
            )
        });
        let count = godray.corners.len() as u32;
        batches.draws.push((id, first..first + count));
        first += count;
    }
    batches.view_group.get_or_insert_with(|| {
        device.create_bind_group(
            "retail godray view",
            &cache.get_bind_group_layout(&pipeline.view_layout),
            &BindGroupEntries::single(pipeline.view.as_entire_binding()),
        )
    });
}

/// Draws the view's godrays into the accumulation target, depth-tested against the scene (`depth`, `samples` per
/// pixel; under DLSS the main pass drew the top-left `resolution` corner). None without godrays: nothing to add.
pub(super) fn draw(
    context: &RenderContext,
    encoder: &mut CommandEncoder,
    world: &World,
    depth: &ViewDepthTexture,
    samples: u32,
    view: &ExtractedView,
    resolution: Option<UVec2>,
) -> Option<TextureView> {
    let batches = world.resource::<Batches>();
    let (Some(vertices), Some(view_group)) = (&batches.vertices, &batches.view_group) else {
        return None;
    };
    if batches.draws.is_empty() {
        return None;
    }
    let p = world.resource::<Pipeline>();
    let index = SAMPLES.iter().position(|&n| n == samples)?;
    let pipeline = world.resource::<PipelineCache>().get_render_pipeline(p.pipelines[index])?;
    let clip_from_world = view
        .clip_from_world
        .unwrap_or_else(|| view.clip_from_view * view.world_from_view.to_matrix().inverse());
    let data: Vec<u8> = clip_from_world
        .to_cols_array()
        .into_iter()
        .chain(view.world_from_view.translation().extend(1.).to_array())
        .flat_map(f32::to_le_bytes)
        .collect();
    world.resource::<RenderQueue>().write_buffer(&p.view, 0, &data);
    let size = depth.texture.size();
    let mut targets = batches.targets.lock().unwrap();
    if !targets.as_ref().is_some_and(|t| t.0 == size && t.1 == samples) {
        let target = |sample_count: u32, label: &'static str| {
            context
                .render_device()
                .create_texture(&TextureDescriptor {
                    label: Some(label),
                    size,
                    mip_level_count: 1,
                    sample_count,
                    dimension: TextureDimension::D2,
                    format: TextureFormat::Rgba16Float,
                    usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&TextureViewDescriptor::default())
        };
        let multisampled = (samples > 1).then(|| target(samples, "retail godray msaa"));
        *targets = Some((size, samples, multisampled, target(1, "retail godray")));
    }
    let (_, _, multisampled, resolved) = targets.as_ref()?;
    // perf 11-01: timed in --trace-gpu captures.
    use bevy::render::diagnostic::RecordDiagnostics;
    let diagnostics = context.diagnostic_recorder();
    let mut pass = super::exposure::pass(context, encoder, RenderPassDescriptor {
        label: Some("retail godray"),
        color_attachments: &[Some(RenderPassColorAttachment {
            view: multisampled.as_ref().unwrap_or(resolved),
            depth_slice: None,
            resolve_target: multisampled.as_ref().map(|_| &**resolved),
            ops: Operations { load: LoadOp::Clear(default()), store: StoreOp::Store },
        })],
        depth_stencil_attachment: Some(depth.get_attachment(StoreOp::Store)),
        timestamp_writes: None,
        occlusion_query_set: None,
    });
    let span = diagnostics.pass_span(&mut pass, "retail_godray");
    if let Some(r) = resolution {
        pass.set_viewport(0., 0., r.x as f32, r.y as f32, 0., 1.);
    }
    pass.set_render_pipeline(pipeline);
    pass.set_bind_group(0, view_group, &[]);
    pass.set_vertex_buffer(0, vertices.slice(..));
    for (id, range) in &batches.draws {
        if let Some(group) = batches.groups.get(id) {
            pass.set_bind_group(1, group, &[]);
            pass.draw(range.clone(), 0..1);
        }
    }
    span.end(&mut pass);
    drop(pass);
    Some(resolved.clone())
}

/// Triangle corners of a batch's index list, as the vertex buffer lays them out.
pub(crate) fn corners<'a>(vertices: impl Iterator<Item = &'a skate_data::skate_map::Vertex>) -> Arc<[[f32; 8]]> {
    vertices
        .map(|v| {
            let [x, y, z] = v.position;
            let [nx, ny, nz] = v.normal;
            let [u, w] = v.uv;
            [x, y, z, nx, ny, nz, u, w]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    /// The pass state traced from render.big shaders/final/godrayspotlight.xml (module docs).
    #[test]
    fn godray_pass_matches_the_retail_effect_states() {
        let d = descriptor(Handle::default(), Vec::new(), 4);
        let target = d.fragment.as_ref().unwrap().targets[0].clone().unwrap();
        let one = BlendComponent { src_factor: BlendFactor::One, dst_factor: BlendFactor::One, operation: BlendOperation::Add };
        assert_eq!(target.blend, Some(BlendState { color: one, alpha: one }));
        let depth = d.depth_stencil.unwrap();
        assert!(!depth.depth_write_enabled);
        assert_eq!(depth.depth_compare, CompareFunction::GreaterEqual);
        assert_eq!(d.primitive.cull_mode, Some(Face::Back));
        assert_eq!(d.multisample.count, 4);
        // Position, normal, uv (corners).
        assert_eq!(d.vertex.buffers[0].array_stride, 32);
    }
}
