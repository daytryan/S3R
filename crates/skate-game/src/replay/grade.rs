//! The Replay Editor's Brightness / Contrast / Saturation / Tint channels as one fullscreen pass after the main
//! post-processing, active only while the editor (or its export) shows the replay; and the Replay Settings
//! visibility (green screen, the player, the other skaters).
//!
//! Stage 0 §6 [disc] scales: brightness ×0.01, contrast ×0.1 about mid 0.5, saturation ×0.01, tint RGBA / 255.
//! RULING (accepted): `c = contrast*(c - 0.5) + 0.5 + brightness`, `c *= lum(c)*s + 1`, `c = lerp(c, rgb, a)`.
//! The saturate term is retail's own grade form: Skate 1's decoded grade shader does
//! `saturate(f*(lum(f)*intensity + colour))` (retail_tone.wgsl:79-84), with the grade-set fields packed as
//! retail_grading.rs:525-533 shows (saturate colour + intensity). With colour = 1 it is `f*(lum(f)*s + 1)`: it
//! brightens saturated highlights rather than shifting chroma (review M3).
use super::editor::{Channel, Editor, Look};
use bevy::{
    asset::embedded_asset,
    core_pipeline::{
        FullscreenShader,
        core_3d::graph::{Core3d, Node3d},
    },
    ecs::query::QueryItem,
    prelude::*,
    render::{
        RenderApp, RenderStartup,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_graph::{NodeRunError, RenderGraphContext, RenderGraphExt, RenderLabel, ViewNode, ViewNodeRunner},
        render_resource::{binding_types::*, *},
        renderer::{RenderContext, RenderDevice, RenderQueue},
        view::ViewTarget,
    },
};

/// The grade the pass draws; `active` false skips the pass.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, ExtractResource)]
pub(crate) struct Grade {
    pub active: bool,
    /// brightness, contrast, saturation; w the letterbox's picture aspect (item 16 extension; 0 = off).
    pub bcs: Vec4,
    pub tint: Vec4,
}
impl Grade {
    /// The editor's channels at playhead `t` (held as retail, or eased by an extension key) and the letterbox.
    pub fn at(editor: &Editor, t: f32) -> Self {
        let tint = editor.tint_value_at(t).map(|v| v / 255.);
        Self {
            active: true,
            bcs: Vec4::new(
                editor.value_at(Channel::Brightness, t) * 0.01,
                editor.value_at(Channel::Contrast, t) * 0.1,
                editor.value_at(Channel::Saturation, t) * 0.01,
                editor.look.aspect(),
            ),
            tint: Vec4::from_array(tint),
        }
    }
}

/// The CPU twin of grade.wgsl (tests and the export's checks).
pub(crate) fn apply(grade: &Grade, rgb: Vec3) -> Vec3 {
    if !grade.active {
        return rgb;
    }
    let c = grade.bcs.y * (rgb - Vec3::splat(0.5)) + Vec3::splat(0.5 + grade.bcs.x);
    let lum = c.dot(Vec3::new(0.299, 0.587, 0.114));
    let c = c * (lum * grade.bcs.z + 1.);
    c.lerp(grade.tint.truncate(), grade.tint.w)
}

/// Item 16 extension: the letterbox bars for a `width` x `height` frame at picture aspect `aspect` (0 = none), as
/// (rows top and bottom, columns left and right); grade.wgsl draws the same bars.
pub(crate) fn letterbox_bars(width: u32, height: u32, aspect: f32) -> (u32, u32) {
    if aspect <= 0. || width == 0 || height == 0 {
        return (0, 0);
    }
    let screen = width as f32 / height as f32;
    if aspect > screen {
        (((height as f32 - width as f32 / aspect) * 0.5).round().max(0.) as u32, 0)
    } else {
        (0, ((width as f32 - height as f32 * aspect) * 0.5).round().max(0.) as u32)
    }
}

/// What Replay Settings shows: (world, local skater, other skaters, green background). Retail's three toggles are
/// independent bools (0x82615728); green screen swaps the world for the green background.
pub(crate) fn visibility(look: &Look) -> (bool, bool, bool, bool) {
    (!look.green_screen, !look.hide_player, !look.hide_skaters, look.green_screen)
}

pub(crate) fn install(app: &mut App) {
    embedded_asset!(app, "grade.wgsl");
    app.init_resource::<Grade>().add_plugins(ExtractResourcePlugin::<Grade>::default());
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render
            .add_systems(RenderStartup, initialize)
            .add_render_graph_node::<ViewNodeRunner<GradeNode>>(Core3d, GradeLabel)
            .add_render_graph_edges(Core3d, (Node3d::EndMainPassPostProcessing, GradeLabel, Node3d::Upscaling));
    }
}

#[derive(Resource)]
struct Pipeline {
    layout: BindGroupLayoutDescriptor,
    /// HDR and LDR main textures.
    pipelines: [CachedRenderPipelineId; 2],
    uniform: Buffer,
    sampler: Sampler,
}

fn initialize(
    mut commands: Commands,
    device: Res<RenderDevice>,
    cache: Res<PipelineCache>,
    assets: Res<AssetServer>,
    fullscreen: Res<FullscreenShader>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "replay grade",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                uniform_buffer::<[Vec4; 2]>(false),
            ),
        ),
    );
    let shader =
        assets.load(bevy::asset::AssetPath::from(bevy::asset::embedded_path!("grade.wgsl")).with_source("embedded"));
    let pipelines = [ViewTarget::TEXTURE_FORMAT_HDR, TextureFormat::bevy_default()].map(|format| {
        cache.queue_render_pipeline(RenderPipelineDescriptor {
            label: Some("replay grade".into()),
            layout: vec![layout.clone()],
            vertex: fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: shader.clone(),
                entry_point: Some("fragment".into()),
                targets: vec![Some(ColorTargetState { format, blend: None, write_mask: ColorWrites::ALL })],
                ..default()
            }),
            ..default()
        })
    });
    let uniform = device.create_buffer(&BufferDescriptor {
        label: Some("replay grade"),
        size: 32,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let sampler = device.create_sampler(&SamplerDescriptor::default());
    commands.insert_resource(Pipeline { layout, pipelines, uniform, sampler });
}

#[derive(Debug, Hash, PartialEq, Eq, Clone, RenderLabel)]
struct GradeLabel;
#[derive(Default)]
struct GradeNode;
impl ViewNode for GradeNode {
    type ViewQuery = &'static ViewTarget;
    fn run<'w>(
        &self,
        _: &mut RenderGraphContext,
        context: &mut RenderContext,
        view: QueryItem<Self::ViewQuery>,
        world: &'w World,
    ) -> Result<(), NodeRunError> {
        let Some(grade) = world.get_resource::<Grade>().filter(|g| g.active) else {
            return Ok(());
        };
        let (Some(p), Some(cache)) = (world.get_resource::<Pipeline>(), world.get_resource::<PipelineCache>()) else {
            return Ok(());
        };
        let Some(pipeline) = cache.get_render_pipeline(p.pipelines[usize::from(!view.is_hdr())]) else {
            return Ok(());
        };
        let data: Vec<u8> = [grade.bcs, grade.tint].iter().flat_map(|v| v.to_array()).flat_map(f32::to_le_bytes).collect();
        world.resource::<RenderQueue>().write_buffer(&p.uniform, 0, &data);
        let post = view.post_process_write();
        let group = context.render_device().create_bind_group(
            "replay grade",
            &cache.get_bind_group_layout(&p.layout),
            &BindGroupEntries::sequential((post.source, &p.sampler, p.uniform.as_entire_binding())),
        );
        let mut pass = context.begin_tracked_render_pass(RenderPassDescriptor {
            label: Some("replay grade"),
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
        pass.set_render_pipeline(pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grade_identity_at_defaults() {
        let grade = Grade::at(&Editor::new(false), 0.5);
        assert!(grade.active);
        assert_eq!(grade.bcs, Vec4::new(0., 1., 0., 0.));
        for rgb in [Vec3::ZERO, Vec3::ONE, Vec3::new(0.2, 0.5, 0.9)] {
            assert!((apply(&grade, rgb) - rgb).length() < 1e-6);
        }
    }

    #[test]
    fn grade_channels_move_the_image() {
        let mut editor = Editor::new(false);
        editor.adjust(Channel::Brightness, true);
        let brighter = apply(&Grade::at(&editor, 0.5), Vec3::splat(0.5));
        assert!((brighter - Vec3::splat(0.7)).length() < 1e-6, "+20 brightness = +0.2");
        let mut editor = Editor::new(false);
        editor.adjust(Channel::Contrast, true);
        let contrast = apply(&Grade::at(&editor, 0.5), Vec3::splat(0.75));
        assert!((contrast.x - 0.8).abs() < 1e-6, "contrast 1.2 about 0.5");
    }

    /// grade.wgsl (with the letterbox) compiles and validates.
    #[test]
    fn grade_shader_validates() {
        use naga_oil::compose::{ComposableModuleDescriptor, Composer, NagaModuleDescriptor};
        let mut composer = Composer::default();
        composer
            .add_composable_module(ComposableModuleDescriptor {
                source: "#define_import_path bevy_core_pipeline::fullscreen_vertex_shader\nstruct FullscreenVertexOutput {@builtin(position) position:vec4<f32>,@location(0) uv:vec2<f32>}",
                file_path: "fullscreen",
                ..Default::default()
            })
            .unwrap();
        let module = composer
            .make_naga_module(NagaModuleDescriptor { source: include_str!("grade.wgsl"), file_path: "grade", ..Default::default() })
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&composer)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all()).validate(&module).unwrap();
    }

    /// Item 16: a ramped Brightness key grades between its values; the letterbox rides in bcs.w.
    #[test]
    fn eased_grade_and_letterbox() {
        use crate::replay::editor::{Ease, Key, Value};
        let mut editor = Editor::new(false);
        editor.keys[Channel::Brightness.index()] = vec![
            Key { t: 0., soft: false, cut: false, value: Value::Int(0), ease: Ease::Linear },
            Key { t: 0.5, soft: true, cut: false, value: Value::Int(100), ease: Ease::Hold },
        ];
        assert!((Grade::at(&editor, 0.25).bcs.x - 0.5).abs() < 1e-6);
        assert_eq!(Grade::at(&editor, 0.75).bcs.x, 1.);
        assert_eq!(Grade::at(&editor, 0.25).bcs.w, 0., "letterbox off by default");
        editor.look.letterbox = 1;
        assert_eq!(Grade::at(&editor, 0.25).bcs.w, 2.39);
        assert_eq!(letterbox_bars(1920, 1080, 2.39), (138, 0));
        assert_eq!(letterbox_bars(1920, 1080, 4. / 3.), (0, 240));
        assert_eq!(letterbox_bars(1920, 1080, 0.), (0, 0));
    }

    #[test]
    fn green_screen_hides_world() {
        let mut look = Look::default();
        assert_eq!(visibility(&look), (true, true, true, false));
        look.green_screen = true;
        assert_eq!(visibility(&look), (false, true, true, true));
        look = Look { hide_skaters: true, ..default() };
        assert_eq!(visibility(&look), (true, true, false, false));
        look = Look { hide_player: true, ..default() };
        assert_eq!(visibility(&look), (true, false, true, false));
    }
}
