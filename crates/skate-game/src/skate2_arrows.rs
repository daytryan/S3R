//! Skate 2's visual indicators over the tutorial's guide locators: the neon outline icons (guide chevrons, the red
//! "!" triangle, ...) exported by skate2port/old_indicators.py to private/skate2-tutorial/indicator/indicators.json.
//!
//! On the disc each one is a ribbon along a 2D spline outline (visual_indicators_sk82.rx2), drawn by
//! render.big shaders/spline.fx in two passes: "darken" (visual_indicator_background, alpha blend) then "default"
//! (the style's colour texture, additive ONE/ONE). The outline stands in the locator's XY plane (Y up), as authored.
//! The textures are ribbon cross-sections: V runs across the ribbon, U along it.
//!
//! ponytail: the per-type vault profile (class 2F57F64DD2B4F257: floats 0.2, 1, 90, 0.15, 0.4, 0.1, 1, 0.05, 20 and a
//! point graph) is unnamed, so no animation or distance fade is applied; WIDTH borrows its 0.15. Name those fields
//! from the spline indicator update code (exe 0x8267b480 manager) to add them.
use crate::config::Config;
use bevy::{
    asset::{AssetPath, RenderAssetUsages, embedded_asset, embedded_path},
    mesh::Indices,
    prelude::*,
    render::render_resource::{AsBindGroup, PrimitiveTopology},
    shader::ShaderRef,
};
use serde::Deserialize;
use std::collections::HashMap;

const DIR: &str = "private/skate2-tutorial/indicator";
/// Ribbon width, metres.
const WIDTH: f32 = 0.15;
/// Shape and style for a locator the export doesn't list (the minimap waypoint): the tutorial's guide chevron.
const FALLBACK: (&str, &str) = ("0001AE2403E38707", "20BCE4F1E06CEC58");
/// An unlisted `signup:<locator>` (the Skate 3 career's rows): Skate 2's sign-up indicator from signup.json.
/// Not retail (story mode stage 1): Skate 3 draws its SignUpIndicator ribbons.
const SIGNUP: (&str, &str) = ("00019AF403E38707", "D7EDBD362D7D2152");
/// The outlines whose every indicator type stands 1 m over its locator, its vault `Offset` (0, 1, 0): arrow and
/// 03E671FA364B1D9F (0001AE2203E38707), 0D3CA3EC5796AA8B / 3A92BA28DD23FFEE (0001B33903E38707), 02CED9657D9BAF21 /
/// 5D54A7FBA9448686 (0001D0CB03E38707), 74095F976D5F99D9 / 862578E6E653391D (0001D0CC03E38707), teleport
/// (0001DA7E03E38707). The chevrons' types stand none; the sign-ups' 2 m are their callers' (SIGNUP_RISE).
const LIFTED: [&str; 5] = ["0001AE2203E38707", "0001B33903E38707", "0001D0CB03E38707", "0001D0CC03E38707", "0001DA7E03E38707"];

/// Locators (name, row-major matrix with translation in row 3) that should carry an indicator.
#[derive(Resource, Default)]
pub(crate) struct Arrows {
    pub wanted: Vec<(String, [[f32; 4]; 4])>,
    /// More indicators.json files (asset-relative, same format) whose shapes, styles and locators join the tutorial's
    /// (the Hall of Meat rows' indicators). Textures stay the tutorial folder's.
    pub more: Vec<String>,
    /// Each outline turns about world Y to face the gameplay camera. Not retail-traced (the spline indicator
    /// update, 0x827B2720, is not decoded): without it an outline whose plane runs along the view, as on
    /// Skate 3's Industrial S.K.A.T.E. rows, is a sliver from behind the skater.
    pub face_camera: bool,
    /// Ribbons along world points in a style of the loaded files (a kind-1 VisualIndicator: skate3_indicators::Shown::
    /// Ribbon). They lie where authored and never turn.
    pub ribbons: Vec<Ribbon>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Ribbon {
    /// The colour pass's texture and the darkening pass's (asset paths).
    pub glow: String,
    pub darken: String,
    pub points: Vec<[f32; 3]>,
    pub closed: bool,
}

/// A row's kind-1 VisualIndicator as tools/skate3_online.py exports it (`outlines`): its spline's world points.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(crate) struct Outline {
    pub points: Vec<[f32; 3]>,
    pub closed: bool,
}
impl Outline {
    /// Its ribbon in the textures of `dir` (a mode's indicator folder). The spline indicator keeps its glow texture at
    /// +0x1C and its darken texture at +0x20, filling them when unset with visual_indicator_orange and
    /// visual_indicator_background (0x827AD4A8, 0x827AFB1C). Retail S.K.A.T.E. glows blue ("blue boxes drawn on the
    /// ground"), so +0x1C is set there; nothing found sets +0x20, so the darken pass keeps the background, whose
    /// alpha falls off across the ribbon.
    pub fn ribbon(&self, dir: &str) -> Ribbon {
        Ribbon {
            glow: format!("{dir}/visual_indicator_blue.png"),
            darken: format!("{dir}/visual_indicator_background.png"),
            points: self.points.clone(),
            closed: self.closed,
        }
    }
}

/// An indicator root that [`face_camera`] turns.
#[derive(Component)]
struct FacesCamera;

#[derive(Deserialize, Default)]
struct Data {
    shapes: HashMap<String, Vec<[f32; 2]>>,
    styles: HashMap<String, Style>,
    locators: HashMap<String, Look>,
}
#[derive(Deserialize)]
struct Style {
    /// In DIR, or (with a `/`) an asset path: Skate 3 S.K.A.T.E.'s textures are its own.
    texture: String,
    background: Option<String>,
    tint: [f32; 4],
}
#[derive(Deserialize)]
struct Look {
    shape: String,
    style: String,
    /// Metres the outline stands over its locator ([`merge`]).
    #[serde(skip)]
    lift: f32,
}

type Pass = (Handle<StandardMaterial>, Option<Handle<StandardMaterial>>);

#[derive(Default)]
struct Spawned {
    at: Vec<(String, [[f32; 4]; 4])>,
    more: Vec<String>,
    face: bool,
    ribbons: Vec<Ribbon>,
    roots: Vec<Entity>,
    data: Option<Option<Data>>,
    meshes: HashMap<String, Handle<Mesh>>,
    materials: HashMap<String, Pass>,
}

pub(crate) struct Skate2ArrowsPlugin;
impl Plugin for Skate2ArrowsPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "skate2_arrows_spline.wgsl");
        app.add_plugins(MaterialPlugin::<SplineMaterial>::default())
            .init_resource::<Arrows>()
            .add_systems(Update, (sync, face_camera).chain());
    }
}

/// The default spline profile's i_intensity: glow 1, darken 0.7 (+0x44, +0x5C; 0x827B27E4..0x827B2808).
const SPLINE_INTENSITY: Vec4 = Vec4::new(1., 0.7, 0., 0.);
/// Its i_clipvalues (g4, g5 - g4, g6, g7 - g6 of its graph, 0x827B279C..0x827B27D8): spline_defaultVS fades the
/// ribbon in over the first 8.3 m of view depth and out from 94.3 to 100 m.
const SPLINE_CLIP: Vec4 = Vec4::new(0., 8.30619, 94.2997, 5.7003);

/// A kind-1 ribbon pass as retail draws it: spline_darkenPS blended over the scene, or spline_defaultPS added
/// (skate2_arrows_spline.wgsl). Blend states are assumed from Skate 2's spline.fx; retail's are set through its
/// effect pass (0x82CA5D10) and not traced.
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
struct SplineMaterial {
    /// x glow, y darken, z 1 for the glow pass.
    #[uniform(0)]
    intensity: Vec4,
    #[uniform(1)]
    clip: Vec4,
    #[texture(2)]
    #[sampler(3)]
    texture: Handle<Image>,
    glow: bool,
}
impl Material for SplineMaterial {
    fn fragment_shader() -> ShaderRef {
        AssetPath::from(embedded_path!("skate2_arrows_spline.wgsl")).with_source("embedded").into()
    }
    fn alpha_mode(&self) -> AlphaMode {
        if self.glow { AlphaMode::Add } else { AlphaMode::Blend }
    }
    fn depth_bias(&self) -> f32 {
        if self.glow { 1. } else { 0. }
    }
    fn enable_prepass() -> bool {
        false
    }
    fn enable_shadows() -> bool {
        false
    }
    fn specialize(
        _: &bevy::pbr::MaterialPipeline,
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        _: &bevy::mesh::MeshVertexBufferLayoutRef,
        _: bevy::pbr::MaterialPipelineKey<Self>,
    ) -> Result<(), bevy::render::render_resource::SpecializedMeshPipelineError> {
        // Both faces: the crossed strips are seen from either side.
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

/// Half a spline indicator's strip, metres: (+0x3C + +0x40) / 4 of its vault profile (0x827B2834..0x827B283C). A
/// kind-1 entry resolves to profile 2F57F64DD2B4F257 "default", whose fields there are 0.4 and 0.3.
const SPLINE_HALF: f32 = 0.175;

/// A kind-1 outline as retail draws it (0x827B2BA8): the strip three times over the same points, its two edges at
/// -/+SPLINE_HALF along z, then y, then x (i_spline_pos_offset, filled at 0x827B2894..0x827B2950 while the profile's
/// bool 5D92E494915216BD is set, as in "default"), so the line reads from every side.
pub(crate) fn tube(points: &[[f32; 3]], closed: bool) -> Mesh {
    let mut points: Vec<Vec3> = points.iter().map(|&p| Vec3::from(p)).collect();
    points.dedup_by(|a, b| a.distance(*b) < 1e-4);
    if closed && points.len() > 2 && points[0].distance(points[points.len() - 1]) >= 1e-4 {
        points.push(points[0]);
    }
    let (mut positions, mut normals, mut uvs, mut indices) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for axis in [Vec3::Z, Vec3::Y, Vec3::X] {
        let mut length = 0.;
        for (k, &p) in points.iter().enumerate() {
            if k > 0 {
                length += p.distance(points[k - 1]);
                let a = positions.len() as u32 - 2;
                indices.extend([a, a + 2, a + 1, a + 1, a + 2, a + 3]);
            }
            let along = points.get(k + 1).map_or(p - points[k.saturating_sub(1)], |&next| next - p);
            for (edge, v) in [(-1., 0.), (1., 1.)] {
                positions.push((p + axis * SPLINE_HALF * edge).to_array());
                normals.push(axis.cross(along).normalize_or(Vec3::Y).to_array());
                uvs.push([length / (2. * SPLINE_HALF), v]);
            }
        }
    }
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_indices(Indices::U32(indices))
}

/// A flat ribbon WIDTH wide centred on the outline, mitred at corners.
pub(crate) fn ribbon(outline: &[[f32; 2]]) -> Mesh {
    let mut points: Vec<Vec2> = outline.iter().map(|&p| Vec2::from(p)).collect();
    points.dedup_by(|a, b| a.distance(*b) < 1e-4);
    let (mut positions, mut uvs, mut indices, mut length) = (Vec::new(), Vec::new(), Vec::new(), 0.);
    for (i, &p) in points.iter().enumerate() {
        let before = (p - points[i.saturating_sub(1)]).normalize_or_zero();
        let after = (points.get(i + 1).copied().unwrap_or(p) - p).normalize_or_zero();
        let along = (before + after).normalize_or(after.max(before));
        let side = along.perp();
        // Mitre: keep the edges WIDTH apart through the turn (capped so tips don't spike).
        let reach = WIDTH / 2. / side.dot(after.perp().normalize_or(side)).abs().max(0.5);
        if i > 0 {
            length += p.distance(points[i - 1]);
        }
        for (edge, v) in [(-1., 0.), (1., 1.)] {
            let q = p + side * reach * edge;
            positions.push([q.x, q.y, 0.]);
            uvs.push([length / WIDTH, v]);
        }
        if i > 0 {
            let a = 2 * i as u32 - 2;
            indices.extend([a, a + 2, a + 1, a + 1, a + 2, a + 3]);
        }
    }
    let normals = vec![[0., 0., 1.]; positions.len()];
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_indices(Indices::U32(indices))
}

fn load(config: &Config, more: &[String]) -> Option<Data> {
    let read = |path: std::path::PathBuf| -> Option<Data> {
        let text = std::fs::read_to_string(&path).map_err(|e| warn!("Skate 2 indicators: {}: {e}", path.display())).ok()?;
        serde_json::from_str(&text).map_err(|e| warn!("Skate 2 indicators: {}: {e}", path.display())).ok()
    };
    // The tutorial's own file first; without it (Skate 2 not installed) the others still draw.
    let base = read(config.asset_root.join(DIR).join("indicators.json"));
    merge(base, more.iter().filter_map(|m| read(config.asset_root.join(m))).collect())
}

/// The tutorial's file with the others' shapes, styles and looks over it. Retail stands each outline its type's vault
/// `Offset` up ([`LIFTED`]): here the modes' looks (S.K.A.T.E., the contests, Hall of Meat). The tutorial's own stay
/// where they were approved, though retail lifts its five arrows too.
fn merge(base: Option<Data>, extras: Vec<Data>) -> Option<Data> {
    let found = base.is_some();
    let mut data = base.unwrap_or_default();
    if !found && extras.is_empty() {
        return None;
    }
    for extra in extras {
        data.shapes.extend(extra.shapes);
        data.styles.extend(extra.styles);
        let lifted = |look: Look| Look { lift: if LIFTED.contains(&look.shape.as_str()) { 1. } else { 0. }, ..look };
        data.locators.extend(extra.locators.into_iter().map(|(name, look)| (name, lifted(look))));
    }
    Some(data)
}

#[allow(clippy::too_many_arguments)]
fn sync(
    mut commands: Commands,
    arrows: Res<Arrows>,
    config: Res<Config>,
    server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut splines: ResMut<Assets<SplineMaterial>>,
    mut spawned: Local<Spawned>,
) {
    if spawned.at == arrows.wanted && spawned.more == arrows.more && spawned.face == arrows.face_camera && spawned.ribbons == arrows.ribbons
    {
        return;
    }
    spawned.face = arrows.face_camera;
    spawned.ribbons = arrows.ribbons.clone();
    if spawned.more != arrows.more {
        spawned.more = arrows.more.clone();
        spawned.data = None;
    }
    for root in spawned.roots.drain(..) {
        commands.entity(root).despawn();
    }
    spawned.at = arrows.wanted.clone();
    let spawned = &mut *spawned;
    let Some(data) = spawned.data.get_or_insert_with(|| load(&config, &spawned.more)) else { return };
    for (locator, matrix) in &arrows.wanted {
        let unlisted = if locator.starts_with("signup:") { SIGNUP } else { FALLBACK };
        let listed = data.locators.get(locator);
        let (shape, style) = listed.map_or(unlisted, |l| (l.shape.as_str(), l.style.as_str()));
        let (Some(outline), Some(look)) = (data.shapes.get(shape), data.styles.get(style)) else { continue };
        let mesh = spawned.meshes.entry(shape.to_owned()).or_insert_with(|| meshes.add(ribbon(outline))).clone();
        let pass = spawned.materials.entry(style.to_owned()).or_insert_with(|| passes(look, &server, &mut materials)).clone();
        let mut at = Transform::from_matrix(Mat4::from_cols_array_2d(matrix));
        at.translation.y += listed.map_or(0., |l| l.lift);
        let root = spawn(&mut commands, format!("Skate 2 visual indicator {locator}"), at, mesh, pass);
        if spawned.face {
            commands.entity(root).insert(FacesCamera);
        }
        spawned.roots.push(root);
    }
    for r in &arrows.ribbons {
        let mesh = meshes.add(tube(&r.points, r.closed));
        let mut pass = |texture: &str, glow| {
            let intensity = SPLINE_INTENSITY.with_z(if glow { 1. } else { 0. });
            splines.add(SplineMaterial { intensity, clip: SPLINE_CLIP, texture: server.load(texture.to_owned()), glow })
        };
        let (darken, glow) = (pass(&r.darken, false), pass(&r.glow, true));
        let root = commands
            .spawn((Name::new("Skate 3 ribbon indicator"), Transform::IDENTITY, Visibility::Inherited))
            .with_children(|root| {
                root.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(darken)));
                root.spawn((Mesh3d(mesh), MeshMaterial3d(glow)));
            })
            .id();
        spawned.roots.push(root);
    }
}

/// A style's two passes: the background darkening (alpha blend) and the colour glow (additive).
fn passes(look: &Style, server: &AssetServer, materials: &mut Assets<StandardMaterial>) -> Pass {
    let [r, g, b, a] = look.tint;
    let path = |texture: &str| if texture.contains('/') { texture.to_owned() } else { format!("{DIR}/{texture}") };
    let pass = |texture: &str, alpha_mode, color| StandardMaterial {
        base_color: color,
        base_color_texture: Some(server.load(path(texture))),
        unlit: true,
        alpha_mode,
        double_sided: true,
        cull_mode: None,
        ..default()
    };
    // The colour pass adds (ONE, ONE): its texture's alpha is unused there.
    let mut glow = pass(&look.texture, AlphaMode::Add, Color::linear_rgba(r, g, b, a));
    glow.depth_bias = 1.;
    let darken = look.background.as_deref().map(|t| materials.add(pass(t, AlphaMode::Blend, Color::WHITE)));
    (materials.add(glow), darken)
}

fn spawn(commands: &mut Commands, name: String, at: Transform, mesh: Handle<Mesh>, (glow, darken): Pass) -> Entity {
    commands
        .spawn((Name::new(name), at, Visibility::Inherited))
        .with_children(|root| {
            if let Some(darken) = darken {
                root.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(darken)));
            }
            root.spawn((Mesh3d(mesh), MeshMaterial3d(glow)));
        })
        .id()
}

/// The yaw about world Y that turns an outline's plane (normal +Z) toward `camera`.
fn facing(at: Vec3, camera: Vec3) -> Quat {
    let d = camera - at;
    Quat::from_rotation_y(d.x.atan2(d.z))
}

fn face_camera(cameras: Query<&GlobalTransform, With<crate::camera::GameplayCamera>>, mut roots: Query<&mut Transform, With<FacesCamera>>) {
    let Some(camera) = cameras.iter().next().map(GlobalTransform::translation) else { return };
    for mut t in &mut roots {
        t.rotation = facing(t.translation, camera);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_outline_is_drawn_three_times_crossed_on_z_y_and_x() {
        let mesh = tube(&[[0., 0., 0.], [2., 0., 0.]], false);
        let Some(bevy::mesh::VertexAttributeValues::Float32x3(p)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION) else {
            panic!("positions")
        };
        assert_eq!(p.len(), 3 * 2 * 2, "three copies of a two-point strip");
        for (copy, axis) in [Vec3::Z, Vec3::Y, Vec3::X].into_iter().enumerate() {
            let gap = Vec3::from(p[copy * 4 + 1]) - Vec3::from(p[copy * 4]);
            assert!((gap - axis * 2. * SPLINE_HALF).length() < 1e-5, "copy {copy}: {gap}");
        }
        assert_eq!(mesh.indices().map(Indices::len), Some(3 * 6));
        assert_eq!(tube(&[[0., 0., 0.], [1., 0., 0.], [1., 0., 1.]], true).indices().map(Indices::len), Some(3 * 3 * 6));
    }

    /// skate_indu_03 ("Can You Spill SKATE?"): its arrow's locator plane runs along the attempt spawn's heading, so
    /// from a camera behind the skater the unturned outline is edge-on; turned, it faces that camera.
    #[test]
    fn an_arrow_edge_on_to_the_camera_turns_to_face_it() {
        let arrow = Mat4::from_cols_array_2d(&[
            [-4.371_139e-8, 0., -1., 0.],
            [0., 1., 0., 0.],
            [1., 0., -4.371_139e-8, 0.],
            [-29.587_644, 35.82687, -512.6513, 1.],
        ]);
        let (_, rotation, at) = arrow.to_scale_rotation_translation();
        // The spawn faces -Z; the camera sits 3 m behind it and 1.5 m up.
        let camera = Vec3::new(-29.587_644, 34.871_418, -511.769) + Vec3::new(0., 1.5, 3.);
        let view = (camera - at).with_y(0.).normalize();
        assert!((rotation * Vec3::Z).dot(view).abs() < 0.05, "as authored: edge-on");
        assert!((facing(at, camera) * Vec3::Z).dot(view) > 0.99, "turned: facing the camera");
        assert!((facing(at, camera) * Vec3::Y - Vec3::Y).length() < 1e-6, "still upright");
    }

    /// User 11-01 (bug 5): retail stands each indicator its type's vault `Offset` up, the arrows 1 m and the chevrons
    /// none. A mode's file (Arrows::more) gets it; the tutorial's own outlines stay as approved.
    #[test]
    fn a_modes_arrow_stands_a_metre_up_and_its_chevron_does_not() {
        let file = |looks: &[(&str, &str)]| Data {
            locators: looks.iter().map(|&(name, shape)| (name.to_owned(), Look { shape: shape.into(), style: "s".into(), lift: 0. })).collect(),
            ..default()
        };
        let mode = file(&[("arrow", "0001AE2203E38707"), ("chevron", "0001AE2403E38707")]);
        let data = merge(Some(file(&[("guide", "0001AE2203E38707")])), vec![mode]).unwrap();
        let lift = |name: &str| data.locators[name].lift;
        assert_eq!((lift("arrow"), lift("chevron"), lift("guide")), (1., 0., 0.));
    }
}
