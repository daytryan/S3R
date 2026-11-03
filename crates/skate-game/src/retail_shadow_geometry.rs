//! Shadow-only batches for static opaque retail geometry (perf 11-01, off unless SKATE_SHADOW_PROXIES=1; the v5
//! experiment's layer, docs/render-batching-experiment.md). The world lights' cascades drew whole 512 m map batches for
//! a ~65 m box (Skate 1 Slappys: 858k vertices for 46k triangles kept, 0.73 ms). Here each light draws 64 m cells of
//! welded positions instead. A depth pass of an opaque material without an alpha test reads positions alone (no
//! fragment stage; retail_depth.wgsl reads no UV under RETAIL_NO_ALPHA_TEST), so the shadow maps are the same.
//! Main meshes, alpha-tested and blended casters, props and collision are untouched.
use crate::map_render::AssetSink;
use bevy::{
    camera::visibility::{RenderLayers, VisibilitySystems},
    prelude::*,
};
use std::collections::HashMap;

pub(crate) const LAYER: usize = 27;
const CELL: f32 = 64.;

/// A proxy entity (tests and counters tell them from the map batches).
#[derive(Component)]
pub(crate) struct ShadowProxy;

/// perf 11-01: opt-in. With them on, the 11-01 look check showed the skater's world shadows changing (a shoe in
/// shadow came out lit), so some casters miss the character map; cause not found yet (.superpowers/sdd/2026-11-01-perf).
pub(crate) fn enabled() -> bool {
    std::env::var_os("SKATE_SHADOW_PROXIES").is_some_and(|v| v == "1")
}

/// An opaque retail material without an alpha test: its depth passes read positions alone.
pub(crate) fn eligible(material: &crate::retail_render::RetailWorldMaterial) -> bool {
    matches!(material.alpha, AlphaMode::Opaque) && material.params.mode.z < 0.
}

pub(crate) fn install(app: &mut App) {
    app.add_systems(
        PostUpdate,
        include_world_layer.before(VisibilitySystems::CheckVisibility),
    );
}

fn include_world_layer(
    mut commands: Commands,
    lights: Query<
        (Entity, Option<&RenderLayers>),
        Or<(With<DirectionalLight>, With<PointLight>, With<SpotLight>)>,
    >,
) {
    for (entity, layers) in &lights {
        let layers = layers.cloned().unwrap_or_default();
        // World lights gain the shadow-only geometry. Player-only layer 28 and
        // other isolated light masks retain their exact existing membership.
        if layers.intersects(&RenderLayers::default())
            && !layers.intersects(&RenderLayers::layer(LAYER))
        {
            commands.entity(entity).insert(layers.with(LAYER));
        }
    }
}

#[derive(Default)]
pub(crate) struct ShadowGeometry {
    lookup: HashMap<([i32; 3], bool), usize>,
    pub(crate) batches: Vec<(bool, Vec<u32>)>,
    /// A material of each side mode (one-sided, two-sided): the proxies' depth pipelines are the originals'.
    materials: [Option<Handle<crate::retail_render::RetailWorldMaterial>>; 2],
}
impl ShadowGeometry {
    /// A batch's triangles, which then leave the world lights to these proxies (its entity gets NotShadowCaster).
    pub(crate) fn add(
        &mut self,
        geometry: &skate_data::skate_map::Geometry,
        indices: &[u32],
        material: &Handle<crate::retail_render::RetailWorldMaterial>,
        two_sided: bool,
    ) {
        self.materials[usize::from(two_sided)].get_or_insert_with(|| material.clone());
        for triangle in indices.chunks_exact(3) {
            let center = triangle
                .iter()
                .map(|&i| Vec3::from_array(geometry.vertices[i as usize].position) / 3.)
                .sum::<Vec3>();
            let cell = center.to_array().map(|v| (v / CELL).floor() as i32);
            let batch = *self.lookup.entry((cell, two_sided)).or_insert_with(|| {
                let index = self.batches.len();
                self.batches.push((two_sided, Vec::new()));
                index
            });
            // Full triangles, including those crossing a cell boundary; the bounds come from all their positions.
            self.batches[batch].1.extend_from_slice(triangle);
        }
    }

    pub(crate) fn spawn(
        self,
        geometry: &skate_data::skate_map::Geometry,
        commands: &mut crate::map_render::SceneCommands,
        meshes: &mut impl AssetSink<Mesh>,
    ) {
        let Self { batches, materials, .. } = self;
        let (count, triangles) = (batches.len(), batches.iter().map(|(_, i)| i.len() / 3).sum::<usize>());
        let mut bytes = 0;
        for (two_sided, indices) in batches {
            let Some(material) = materials[usize::from(two_sided)].clone() else { continue };
            // One vertex per distinct position (bits), in order of first use; triangle order and winding stay.
            let mut remap = HashMap::new();
            let mut positions = Vec::new();
            let local: Vec<u32> = indices
                .iter()
                .map(|&i| {
                    let p = geometry.vertices[i as usize].position;
                    *remap.entry(p.map(f32::to_bits)).or_insert_with(|| {
                        positions.push(p);
                        positions.len() as u32 - 1
                    })
                })
                .collect();
            let bounds = bevy::camera::primitives::Aabb::enclosing(positions.iter().map(|&p| Vec3::from_array(p)))
                .unwrap_or_default();
            let small = positions.len() <= 1 << 16;
            bytes += positions.len() * 12 + local.len() * if small { 2 } else { 4 };
            let indices = if small {
                bevy::mesh::Indices::U16(local.into_iter().map(|i| i as u16).collect())
            } else {
                bevy::mesh::Indices::U32(local)
            };
            let mesh = Mesh::new(
                bevy::mesh::PrimitiveTopology::TriangleList,
                bevy::asset::RenderAssetUsages::RENDER_WORLD,
            )
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
            .with_inserted_indices(indices);
            commands.spawn((
                Name::new("Retail opaque shadow batch"),
                ShadowProxy,
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(material),
                Transform::default(),
                RenderLayers::layer(LAYER),
                crate::retail_render::shadow_visibility::StaticShadowCaster,
                bounds,
                bevy::camera::visibility::NoAutoAabb,
            ));
        }
        eprintln!("SKATE_SHADOW_BATCHES count={count} triangles={triangles} bytes={bytes}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    #[test]
    fn shadow_layer_reaches_world_lights_but_not_cameras_or_player_only_lights() {
        let mut world = World::new();
        let directional = world.spawn(DirectionalLight::default()).id();
        let point = world.spawn(PointLight::default()).id();
        let spot = world.spawn(SpotLight::default()).id();
        let player = world
            .spawn((DirectionalLight::default(), RenderLayers::layer(28)))
            .id();
        let isolated = world
            .spawn((PointLight::default(), RenderLayers::layer(9)))
            .id();
        let camera = world
            .spawn((Camera3d::default(), RenderLayers::from_layers(&[0, 28])))
            .id();
        world.run_system_once(include_world_layer).unwrap();
        for entity in [directional, point, spot] {
            let layers = world.get::<RenderLayers>(entity).unwrap();
            assert!(layers.intersects(&RenderLayers::default()));
            assert!(layers.intersects(&RenderLayers::layer(LAYER)));
        }
        for entity in [player, isolated, camera] {
            assert!(
                !world
                    .get::<RenderLayers>(entity)
                    .unwrap()
                    .intersects(&RenderLayers::layer(LAYER))
            );
        }
    }

    #[test]
    fn spatial_shadow_batches_keep_boundary_crossing_triangles_and_side_modes() {
        let mut map = skate_data::skate_map::SkateMap::parse(include_bytes!(
            "../../../maps/format-demo.skate"
        ))
        .unwrap();
        map.geometry.vertices.truncate(3);
        for (v, p) in
            map.geometry
                .vertices
                .iter_mut()
                .zip([[-130., 0., 0.], [130., 0., 0.], [0., 200., 1.]])
        {
            v.position = p;
        }
        let material = |n: u128| Handle::Uuid(bevy::asset::uuid::Uuid::from_u128(n), default());
        let (one, two) = (material(1), material(2));
        let mut geometry = ShadowGeometry::default();
        geometry.add(&map.geometry, &[0, 1, 2], &one, false);
        geometry.add(&map.geometry, &[2, 1, 0], &two, true);
        geometry.add(&map.geometry, &[1, 2, 0], &two, false);
        // A cell by centroid per side mode, whole triangles in order, and each side mode's first material.
        assert_eq!(geometry.batches, [(false, vec![0, 1, 2, 1, 2, 0]), (true, vec![2, 1, 0])]);
        assert_eq!(geometry.materials, [Some(one), Some(two)]);
    }
}
