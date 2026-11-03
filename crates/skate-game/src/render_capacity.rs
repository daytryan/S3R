//! Keep instance-buffer identities stable as views discover more geometry, and let cameras that stopped rendering
//! give back their view textures.
use bevy::{
    pbr::{MeshInputUniform, MeshPipeline, MeshUniform},
    prelude::*,
    render::{
        Render, RenderApp, RenderSystems,
        batching::gpu_preprocessing::{BatchedInstanceBuffers, write_batched_instance_buffers},
        renderer::RenderDevice,
    },
};

pub(crate) struct RenderCapacityPlugin;
impl Plugin for RenderCapacityPlugin {
    fn build(&self, app: &mut App) {
        if let Some(render) = app.get_sub_app_mut(RenderApp) {
            render.add_systems(
                Render,
                reserve_instances
                    .in_set(RenderSystems::PrepareResourcesFlush)
                    .before(write_batched_instance_buffers::<MeshPipeline>),
            );
            render.add_systems(Render, release_idle_view_textures::<bevy::render::view::ExtractedView, IdleViewTextures, HoldsViewTextures>.in_set(RenderSystems::Cleanup));
        }
    }
}

/// What a camera's render entity holds onto between frames: its main textures (a and b), depth, prepass, shadow
/// and SSAO textures.
type IdleViewTextures = (
    bevy::render::view::ViewTarget,
    bevy::render::view::ViewDepthTexture,
    bevy::core_pipeline::prepass::ViewPrepassTextures,
    bevy::pbr::ViewShadowBindings,
    bevy::pbr::ScreenSpaceAmbientOcclusionResources,
);
type HoldsViewTextures = Or<(
    With<bevy::render::view::ViewTarget>,
    With<bevy::render::view::ViewDepthTexture>,
    With<bevy::core_pipeline::prepass::ViewPrepassTextures>,
    With<bevy::pbr::ViewShadowBindings>,
    With<bevy::pbr::ScreenSpaceAmbientOcclusionResources>,
)>;

/// Item 29 (leak audit): Bevy strips a camera that goes inactive of its extracted view, but not of the textures its
/// last frame used, so every camera that was ever on and is now off kept them: three 1920x1080 sets (main a + b +
/// depth, 23.7 MiB each, 71 MiB of VRAM) all session (SKATE_PERF_VIEWS). A camera turned back on gets fresh ones the
/// same frame from Bevy's texture cache, which had already dropped the old ones. `V` marks a view rendering this
/// frame (ExtractedView: an active camera or a shadow cascade).
fn release_idle_view_textures<V: Component, B: Bundle, F: bevy::ecs::query::QueryFilter>(
    mut commands: Commands,
    idle: Query<Entity, (Without<V>, F)>,
) {
    for entity in &idle {
        commands.entity(entity).remove::<B>();
    }
}

fn capacity(required: usize, limit: usize) -> usize {
    if required == 0 {
        return 0;
    }
    required
        .checked_next_power_of_two()
        .unwrap_or(required)
        .min(limit)
        .max(required)
}

fn reserve_instances(
    device: Res<RenderDevice>,
    buffers: Option<ResMut<BatchedInstanceBuffers<MeshUniform, MeshInputUniform>>>,
) {
    let Some(mut buffers) = buffers else {
        return;
    };
    let limits = device.limits();
    let maximum = limits
        .max_buffer_size
        .min(u64::from(limits.max_storage_buffer_binding_size))
        / std::mem::size_of::<MeshUniform>() as u64;
    for phase in buffers.phase_instance_buffers.values_mut() {
        let required = phase.data_buffer.len();
        if required != 0 {
            // Bevy's subsequent write_buffer preserves the logical count and
            // initializes only the actual instances through GPU preprocessing.
            phase
                .data_buffer
                .reserve(capacity(required, maximum as usize), &device);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Component)]
    struct Main;
    #[derive(Component)]
    struct Depth;
    #[derive(Component)]
    struct Prepass;
    #[derive(Component)]
    struct Shadows;
    #[derive(Component)]
    struct Ssao;
    #[derive(Component)]
    struct Rendering;

    /// An off camera loses every view texture; a rendering view (a camera or a shadow cascade) keeps them; other
    /// components stay.
    #[test]
    fn cameras_that_stopped_rendering_give_back_their_view_textures() {
        let mut world = World::new();
        let off = world.spawn((Main, Depth, Prepass, Shadows, Ssao, Name::new("off camera"))).id();
        let half = world.spawn((Depth, Name::new("off, depth only"))).id();
        let on = world.spawn((Main, Depth, Rendering)).id();
        let mut system =
            IntoSystem::into_system(release_idle_view_textures::<
                Rendering,
                (Main, Depth, Prepass, Shadows, Ssao),
                Or<(With<Main>, With<Depth>, With<Prepass>, With<Shadows>, With<Ssao>)>,
            >);
        system.initialize(&mut world);
        system.run((), &mut world).unwrap();
        system.apply_deferred(&mut world);
        let e = world.entity(off);
        assert!(!e.contains::<Main>() && !e.contains::<Depth>() && !e.contains::<Prepass>() && !e.contains::<Shadows>());
        assert!(!e.contains::<Ssao>() && e.contains::<Name>());
        assert!(!world.entity(half).contains::<Depth>());
        assert!(world.entity(on).contains::<Main>() && world.entity(on).contains::<Depth>());
    }
    #[test]
    fn growth_is_bounded_and_never_smaller_than_required() {
        assert_eq!(capacity(0, 100), 0);
        assert_eq!(capacity(33, 100), 64);
        assert_eq!(capacity(64, 100), 64);
        assert_eq!(capacity(65, 100), 100);
        // Leave unsupported requests to Bevy's validation; never truncate data.
        assert_eq!(capacity(101, 100), 101);
        for n in 1..10000 {
            assert!(capacity(n, 10000) >= n);
            assert!(capacity(n, 10000) <= 10000);
        }
    }
}
