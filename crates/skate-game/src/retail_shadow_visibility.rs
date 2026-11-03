//! Conservative spatial acceleration of Bevy directional shadow visibility.
//! Leaves use Bevy's original OBB test; only static map meshes enter the index.
use bevy::{
    camera::{
        primitives::{Aabb, CascadesFrusta, Frustum},
        visibility::{
            CascadesVisibleEntities, NoFrustumCulling, RenderLayers, SetViewVisibility,
            VisibilityRange, VisibilitySystems, VisibleEntityRanges,
        },
    },
    light::{NotShadowCaster, SimulationLightSystems},
    math::Affine3A,
    prelude::*,
    transform::TransformSystems,
};

/// Static map batches. perf 10-18: also culled by each camera's far plane in Bevy's parallel visibility pass
/// (vendor/README.md); [`cull_past_far`] trims the rest by distance.
#[derive(Component)]
#[require(bevy::camera::visibility::FarPlaneCulled)]
pub(crate) struct StaticShadowCaster;
/// FPS pass 10-23: a retail batch whose main-pass alpha test retail_depth.wgsl can't repeat (scrolled UVs, alpha squared:
/// [`super::RetailWorldMaterial::camera_prepass_unsafe`]). [`cull_past_far`] keeps it out of the depth-only prepass,
/// where its depth would hide what the main pass shows through its discarded pixels.
#[derive(Component)]
pub(crate) struct NoCameraPrepass;
#[derive(Clone)]
struct Entry {
    entity: Entity,
    aabb: Aabb,
    transform: GlobalTransform,
    visible: bool,
    layers: RenderLayers,
}
impl Entry {
    fn bounds(&self) -> Aabb {
        let affine = self.transform.affine();
        Aabb {
            center: affine.transform_point3a(self.aabb.center),
            half_extents: affine.matrix3.abs() * self.aabb.half_extents
                + bevy::math::Vec3A::splat(0.01),
        }
    }
}
struct Node {
    bounds: Aabb,
    layers: RenderLayers,
    range: std::ops::Range<usize>,
    children: Option<(usize, usize)>,
}
#[derive(Resource, Default)]
struct Index {
    entries: Vec<Entry>,
    nodes: Vec<Node>,
    initialized: bool,
    /// The entries' entities, so a removal elsewhere doesn't rebuild the index.
    members: bevy::ecs::entity::EntityHashSet,
}
impl Index {
    fn rebuild(&mut self, entries: Vec<Entry>) {
        self.members = entries.iter().map(|e| e.entity).collect();
        self.entries = entries;
        self.nodes.clear();
        if !self.entries.is_empty() {
            self.build(0..self.entries.len());
        }
        self.initialized = true;
    }
    fn build(&mut self, range: std::ops::Range<usize>) -> usize {
        let mut min = bevy::math::Vec3A::splat(f32::INFINITY);
        let mut max = -min;
        let mut layers = RenderLayers::none();
        for entry in &self.entries[range.clone()] {
            layers = layers.union(&entry.layers);
            let bounds = entry.bounds();
            min = min.min(bounds.min());
            max = max.max(bounds.max());
        }
        let bounds = Aabb {
            center: (min + max) * 0.5,
            half_extents: (max - min) * 0.5 + bevy::math::Vec3A::splat(0.01),
        };
        let node = self.nodes.len();
        self.nodes.push(Node {
            bounds,
            layers,
            range: range.clone(),
            children: None,
        });
        if range.len() > 16 {
            let axis = if bounds.half_extents.x >= bounds.half_extents.y
                && bounds.half_extents.x >= bounds.half_extents.z
            {
                0
            } else if bounds.half_extents.y >= bounds.half_extents.z {
                1
            } else {
                2
            };
            self.entries[range.clone()].sort_unstable_by(|a, b| {
                a.bounds().center[axis].total_cmp(&b.bounds().center[axis])
            });
            let middle = range.start + range.len() / 2;
            let left = self.build(range.start..middle);
            let right = self.build(middle..range.end);
            self.nodes[node].children = Some((left, right));
        }
        node
    }
    /// `near`: also test the frustum's sun-side half space (a cascade's [`sun_side`] reach).
    fn visit(
        &self,
        node: usize,
        frustum: &Frustum,
        near: bool,
        layers: &RenderLayers,
        output: &mut Vec<Entity>,
    ) {
        let node = &self.nodes[node];
        if !layers.intersects(&node.layers) {
            return;
        }
        if node.bounds.center.is_finite()
            && node.bounds.half_extents.is_finite()
            && !frustum.intersects_obb(&node.bounds, &Affine3A::IDENTITY, near, true)
        {
            return;
        }
        if let Some((left, right)) = node.children {
            self.visit(left, frustum, near, layers, output);
            self.visit(right, frustum, near, layers, output);
        } else {
            for entry in &self.entries[node.range.clone()] {
                if entry.visible
                    && layers.intersects(&entry.layers)
                    && frustum.intersects_obb(&entry.aabb, &entry.transform.affine(), near, true)
                {
                    output.push(entry.entity);
                }
            }
        }
    }
}

type StaticFilter = (
    With<StaticShadowCaster>,
    With<Mesh3d>,
    Without<NotShadowCaster>,
    Without<DirectionalLight>,
    Without<VisibilityRange>,
    Without<NoFrustumCulling>,
);
fn refresh(
    mut index: ResMut<Index>,
    entries: Query<
        (
            Entity,
            &Aabb,
            &GlobalTransform,
            &InheritedVisibility,
            Option<&RenderLayers>,
        ),
        StaticFilter,
    >,
    changed: Query<
        (),
        (
            With<StaticShadowCaster>,
            Or<(
                Changed<Aabb>,
                Changed<GlobalTransform>,
                Changed<InheritedVisibility>,
                Changed<RenderLayers>,
                Added<StaticShadowCaster>,
            )>,
        ),
    >,
    mut removed_layers: RemovedComponents<RenderLayers>,
    mut removed_excluded: RemovedComponents<NotShadowCaster>,
    mut removed_range: RemovedComponents<VisibilityRange>,
    mut removed_no_cull: RemovedComponents<NoFrustumCulling>,
) {
    // Consume every event reader even when an earlier condition is true. perf 11-01: only removals on map casters
    // (indexed, or joining the index) count. Traffic cars' meshes (NotShadowCaster) despawn every second or so, and
    // each time rebuilt the whole index: 6-14 ms on Skate 1 and NSV.
    let removed = removed_layers
        .read()
        .chain(removed_excluded.read())
        .chain(removed_range.read())
        .chain(removed_no_cull.read())
        .filter(|&e| index.members.contains(&e) || entries.contains(e))
        .count();
    if index.initialized
        && index.entries.len() == entries.iter().len()
        && changed.is_empty()
        && removed == 0
    {
        return;
    }
    index.rebuild(
        entries
            .iter()
            .map(|(entity, aabb, transform, visibility, layers)| Entry {
                entity,
                aabb: *aabb,
                transform: *transform,
                visible: visibility.get(),
                layers: layers.cloned().unwrap_or_default(),
            })
            .collect(),
    );
}
fn check_indexed(
    mut commands: Commands,
    index: Res<Index>,
    mut lights: Query<
        (
            &DirectionalLight,
            &CascadesFrusta,
            &mut CascadesVisibleEntities,
            Option<&RenderLayers>,
            &ViewVisibility,
            Option<&bevy::light::cascade::Cascades>,
        ),
        Without<SpotLight>,
    >,
    dynamic: Query<
        (
            Entity,
            &InheritedVisibility,
            Option<&RenderLayers>,
            Option<&Aabb>,
            Option<&GlobalTransform>,
            Has<VisibilityRange>,
            Has<NoFrustumCulling>,
        ),
        (
            With<Mesh3d>,
            Without<NotShadowCaster>,
            Without<DirectionalLight>,
            Or<(
                Without<StaticShadowCaster>,
                With<VisibilityRange>,
                With<NoFrustumCulling>,
                Without<Aabb>,
                Without<GlobalTransform>,
            )>,
        ),
    >,
    ranges: Option<Res<VisibleEntityRanges>>,
) {
    let mut marked = Vec::new();
    // The static map's height: no static caster stands taller above a receiver on the map.
    let height = index.nodes.first().map_or(0., |root| 2. * root.bounds.half_extents.y);
    for (light, frusta, mut visible, layers, light_visibility, cascades) in &mut lights {
        visible
            .entities
            .retain(|view, _| frusta.frusta.contains_key(view));
        for (view, frusta) in &frusta.frusta {
            let lists = visible.entities.entry(*view).or_default();
            lists.resize(frusta.len(), Default::default());
            for list in lists.iter_mut() {
                list.clear();
            }
            if !light.shadows_enabled || !light_visibility.get() {
                continue;
            }
            let layers = layers.cloned().unwrap_or_default();
            let cascades = cascades.and_then(|c| c.cascades.get(view));
            for (i, (frustum, list)) in frusta.iter().zip(lists.iter_mut()).enumerate() {
                if index.nodes.is_empty() {
                    continue;
                }
                match cascades.and_then(|c| c.get(i)).and_then(|c| sun_side(c.world_from_cascade, height)) {
                    Some(plane) => {
                        let mut frustum = *frustum;
                        frustum.half_spaces[Frustum::NEAR_PLANE_IDX] = plane;
                        index.visit(0, &frustum, true, &layers, &mut list.entities);
                    }
                    None => index.visit(0, frustum, false, &layers, &mut list.entities),
                }
            }
            for (entity, inherited, mask, aabb, transform, range, no_cull) in &dynamic {
                if !inherited.get()
                    || !layers.intersects(mask.unwrap_or_default())
                    || (range
                        && ranges
                            .as_ref()
                            .is_some_and(|r| !r.entity_is_in_range_of_view(entity, *view)))
                {
                    continue;
                }
                for (frustum, list) in frusta.iter().zip(lists.iter_mut()) {
                    if no_cull
                        || aabb.zip(transform).is_none_or(|(a, t)| {
                            frustum.intersects_obb(a, &t.affine(), false, true)
                        })
                    {
                        list.push(entity);
                    }
                }
            }
            for list in lists.iter() {
                marked.extend(list.iter().copied());
            }
        }
    }
    // Match upstream's deferred writes so point-light checks may run in parallel.
    commands.queue(move |world: &mut World| {
        for entity in marked {
            if let Some(mut visible) = world.get_mut::<ViewVisibility>(entity) {
                visible.set_visible();
            }
        }
    });
}

/// perf 10-18: a cascade's sun-side face is never tested (`intersects_obb(.., false, true)`), so every static
/// batch up the light's column joined the walk (the character light's cascade 0 p95 spiked 1.3 ms facing the sun).
/// A caster shadows a receiver no farther up the light ray than its height above it over sin(elevation); receivers sit
/// on the map, so nothing past `height / sin(elevation)` beyond the cascade's sun-side face can shadow anything in it.
/// ponytail: a receiver below the map's lowest batch could lose a caster; none draws there. SKATE_CHAR_SHADOW_REACH=<m>
/// fixes the reach, 0 turns the face off. None: no plane (a light at the horizon reaches across the map).
fn sun_side(world_from_cascade: Mat4, height: f32) -> Option<bevy::camera::primitives::HalfSpace> {
    static FIXED: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
    let fixed = *FIXED.get_or_init(|| std::env::var("SKATE_CHAR_SHADOW_REACH").ok().and_then(|v| v.parse().ok()));
    // The cascade looks down its -z: +z points at the sun, and its origin is the sun-side face's centre.
    let toward_sun = world_from_cascade.z_axis.truncate().normalize_or_zero();
    let reach = match fixed {
        Some(reach) => reach,
        None if toward_sun.y > 0.01 => height / toward_sun.y,
        None => 0.,
    };
    (reach > 0. && reach.is_finite())
        .then(|| bevy::camera::primitives::HalfSpace::new((-toward_sun).extend(toward_sun.dot(world_from_cascade.w_axis.truncate()) + reach)))
}

pub(super) fn install(app: &mut App) {
    // perf 10-18: SKATE_VIS_ONEPASS=0 / SKATE_FAR_CULL=0 restore Bevy's per-camera visibility walks / its far-plane-free
    // culling of static map batches (vendor/README.md).
    for (var, flag) in [
        ("SKATE_VIS_ONEPASS", &bevy::camera::visibility::VISIBILITY_ONE_PASS),
        ("SKATE_FAR_CULL", &bevy::camera::visibility::FAR_PLANE_CULLING),
    ] {
        if std::env::var(var).is_ok_and(|v| v == "0") {
            flag.store(false, std::sync::atomic::Ordering::Relaxed);
        }
    }
    app.register_required_components::<Camera3d, bevy::pbr::PrepassExcluded>();
    app.add_plugins(bevy::render::extract_component::ExtractComponentPlugin::<bevy::pbr::PrepassExcluded>::default());
    app.init_resource::<Index>().add_systems(
        PostUpdate,
        (
            refresh
                .after(VisibilitySystems::CalculateBounds)
                .after(TransformSystems::Propagate)
                .after(VisibilitySystems::VisibilityPropagate)
                .before(check_indexed),
            check_indexed
                .in_set(SimulationLightSystems::CheckLightVisibility)
                .after(VisibilitySystems::CalculateBounds)
                .after(TransformSystems::Propagate)
                .after(SimulationLightSystems::UpdateLightFrusta)
                .after(VisibilitySystems::CheckVisibility)
                .before(VisibilitySystems::MarkNewlyHiddenEntitiesInvisible),
            cull_past_far.after(VisibilitySystems::CheckVisibility),
            cascades_for_3d_views
                .after(SimulationLightSystems::UpdateDirectionalLightCascades)
                .before(SimulationLightSystems::UpdateLightFrusta),
        ),
    );
}

/// Bevy builds shadow cascades for every active camera, 2D included: the presentation and HUD Camera2ds each got a
/// ~2 km orthographic column through the origin, which `check_indexed` walked for every light, marking the map batches
/// inside it visible (perf 10-10 peer report B1, ~3 ms a frame on NSV). Only Camera3d views draw shadows
/// (`prepare_lights`).
pub(crate) fn cascades_for_3d_views(
    mut lights: Query<&mut bevy::light::cascade::Cascades>,
    cameras: Query<(), With<Camera3d>>,
) {
    for mut cascades in &mut lights {
        cascades.cascades.retain(|view, _| cameras.contains(*view));
    }
}
pub(super) fn finish(app: &mut App) {
    app.world_mut()
        .schedule_scope(PostUpdate, |world, schedule| {
            let removed = schedule
                .remove_systems_in_set(
                    bevy::light::check_dir_light_mesh_visibility,
                    world,
                    bevy::ecs::schedule::ScheduleCleanupPolicy::RemoveSystemsOnly,
                )
                .expect("replace directional visibility system");
            assert_eq!(
                removed, 1,
                "exactly one upstream directional visibility system"
            );
        });
}

#[cfg(test)]
#[path = "retail_shadow_visibility_tests.rs"]
mod tests;

#[cfg(test)]
pub(crate) fn benchmark_bounds(bounds: Vec<Aabb>) -> serde_json::Value {
    tests::benchmark_bounds(bounds)
}

/// Far static map batches leave the camera's depth/normal prepass (perf 10-10). Its only reader is SSAO (post passes
/// read the main depth), which shows nothing this far out. Facing the city from NSV or Skate 1, ~60 % of the prepass
/// triangles were past 400 m. Retail world batches only: the PBR fallback loads its normals from the prepass.
pub(crate) const PREPASS_FAR: f32 = 250.;
/// perf 11-01: SKATE_PREPASS_FAR=<m> moves that limit (benchmarks); read once.
fn prepass_far() -> f32 {
    static FAR: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *FAR.get_or_init(|| std::env::var("SKATE_PREPASS_FAR").ok().and_then(|v| v.parse().ok()).filter(|m: &f32| *m >= 0.).unwrap_or(PREPASS_FAR))
}

/// Bevy's view culling never tests the far plane (`intersects_obb(.., true, false)`), so Settings > Draw Distance
/// never cut the map: facing the city from Skate 1's Slappys drew 4.1 M triangles, 2.8 M of them past 1000 m
/// (perf 10-10, ~45 FPS). Static map batches wholly past a camera's far plane leave that camera's visible list
/// (prepass and main pass). Props, skaters and the shadow cascades' own lists are untouched. Retail ones past
/// `PREPASS_FAR` also go on the camera's `PrepassExcluded` list, unless occlusion culling builds its depth pyramid
/// from that prepass or DLSS reads its depth and motion vectors (dlss.rs: a missing batch would read as sky).
/// perf 10-18: batches wholly past the far *plane* are already culled in Bevy's parallel pass ([`StaticShadowCaster`]
/// requires `FarPlaneCulled`); this trims the rest by distance, which also drops those in the far corners.
pub(crate) fn cull_past_far(
    mut cameras: Query<(
        &Projection,
        &GlobalTransform,
        &mut bevy::camera::visibility::VisibleEntities,
        Option<&mut bevy::pbr::PrepassExcluded>,
        Has<bevy::render::experimental::occlusion_culling::OcclusionCulling>,
        Has<bevy::core_pipeline::prepass::MotionVectorPrepass>,
        Has<bevy::core_pipeline::prepass::DepthPrepass>,
        Has<bevy::core_pipeline::prepass::NormalPrepass>,
    )>,
    statics: Query<(&Aabb, &GlobalTransform, Has<MeshMaterial3d<super::RetailWorldMaterial>>), With<StaticShadowCaster>>,
    marked: Query<(), With<NoCameraPrepass>>,
) {
    // perf 10-18: without a depth or normal prepass nothing reads the set (SKATE_PREPASS_EXCLUDED=always fills it anyway).
    static ALWAYS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let always = *ALWAYS.get_or_init(|| std::env::var("SKATE_PREPASS_EXCLUDED").is_ok_and(|v| v == "always"));
    for (projection, view, mut visible, mut excluded, occlusion, motion_vectors, depth, normal) in &mut cameras {
        let Projection::Perspective(p) = projection else { continue };
        let eye = view.translation_vec3a();
        if let Some(excluded) = excluded.as_mut() {
            excluded.0.clear();
        }
        let prepass = depth || normal || always;
        let prepass_far = if occlusion || motion_vectors || !prepass { f32::INFINITY } else { prepass_far() };
        // Only the prepass the FPS pass added (depth only, for early Z); with occlusion, SSAO or DLSS they stay, as shipped.
        let early_z_only = depth && !normal && !occlusion && !motion_vectors;
        let Some(list) = visible.entities.get_mut(&std::any::TypeId::of::<Mesh3d>()) else { continue };
        list.retain(|&entity| {
            if early_z_only && marked.contains(entity) {
                if let Some(excluded) = excluded.as_mut() {
                    excluded.0.insert(entity.into());
                }
            }
            let Ok((aabb, transform, retail)) = statics.get(entity) else { return true };
            let affine = transform.affine();
            let center = affine.transform_point3a(aabb.center);
            let half = affine.matrix3.abs() * aabb.half_extents;
            let distance = ((eye - center).abs() - half).max(bevy::math::Vec3A::ZERO).length();
            if retail && distance > prepass_far {
                if let Some(excluded) = excluded.as_mut() {
                    excluded.0.insert(entity.into());
                }
            }
            distance <= p.far
        });
    }
}
