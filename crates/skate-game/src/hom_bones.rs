//! Skate 3's Hall of Meat bones (user 10-16, feature #8). Retail draws the marquee dem_bones_hom model on the skater
//! (HOMSkaterPresEntity, create 0x827637D0) and, during a bail, shows only the damaged bones (update 0x82763A40):
//! sprained / dislocated (level 2..3) in one colour, broken (4..5) in another, both from the vault's
//! hall_of_meat_skeleton row. The model is `private/hom-bones/hom_bones.glb` (tools/asset_pipeline/hom_bones.py).
//! INFERRED: drawn on top of the skater, since retail's depth state isn't decoded (its defaulthom shader is a tinted,
//! rim-lit bone, hom_bones.wgsl). Shown while the bail lasts, since retail's own gate reads a damage state that
//! hasn't been traced. ponytail: retail's 40-frame PauseMan hit-stop on a new bone (mode 0x1C) and the xray.Texture
//! post-FX aren't ported.
use crate::{animation::AnimationStatus, physics::SkaterRuntime};
use bevy::{
    asset::{AssetPath, embedded_asset, embedded_path},
    camera::visibility::NoFrustumCulling,
    gltf::GltfMaterialName,
    light::NotShadowCaster,
    mesh::skinning::SkinnedMesh,
    prelude::*,
    render::render_resource::{AsBindGroup, CompareFunction},
    shader::ShaderRef,
};

const MODEL: &str = "private/hom-bones/hom_bones.glb";

/// Per vault bone, the hall_of_meat Record order and retail's slot order (names 0x821B59BC): its Bones_* mesh.
const MESHES: [&str; 25] = [
    "Skull", "Neck", "Hand_Left", "Forearm_Left", "Bicep_Left", "Bicep_Left", "Hand_Right", "Forearm_Right",
    "Bicep_Right", "Bicep_Right", "Rib_Cage", "Rib_Cage", "Lower_Spine", "Lower_Spine", "Toes_Left", "Ankle_Left",
    "Calf_Left", "Thigh_Left", "Toes_Right", "Ankle_Right", "Calf_Right", "Thigh_Right", "Hips", "Skull", "Hips",
];

/// The tier a Bones_* mesh shows at: the worst of its vault bones (0 hidden, 1 sprained, 2 broken).
fn tier(shown: &[u8; 25], mesh: &str) -> u8 {
    MESHES.iter().zip(shown).filter(|(m, _)| **m == mesh).map(|(_, &t)| t).max().unwrap_or(0)
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
struct BoneMaterial {
    /// rgb: the tier's vault colour.
    #[uniform(0)]
    colour: Vec4,
}
impl Material for BoneMaterial {
    fn fragment_shader() -> ShaderRef {
        AssetPath::from(embedded_path!("hom_bones.wgsl")).with_source("embedded").into()
    }
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
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
        // Through the body, after the opaque pass.
        if let Some(depth) = &mut descriptor.depth_stencil {
            depth.depth_write_enabled = false;
            depth.depth_compare = CompareFunction::Always;
        }
        Ok(())
    }
}

#[derive(Resource, Default)]
struct Bones {
    root: Option<Entity>,
    /// Each skin and its joints' names: posed by the skater's own joints of those names.
    skins: Vec<(Entity, Vec<String>)>,
    /// Each Bones_* primitive and its mesh name.
    parts: Vec<(Entity, String)>,
    materials: Option<[Handle<BoneMaterial>; 2]>,
}

pub(crate) struct HomBonesPlugin;
impl Plugin for HomBonesPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "hom_bones.wgsl");
        app.add_plugins(MaterialPlugin::<BoneMaterial>::default())
            .init_resource::<Bones>()
            .add_systems(Startup, spawn)
            .add_systems(Update, (prepare, show).chain().after(crate::animation::present));
    }
}

fn spawn(mut commands: Commands, server: Res<AssetServer>, config: Option<Res<crate::config::Config>>, mut bones: ResMut<Bones>) {
    // An install made before the exporter has no model: no bones.
    if config.is_some_and(|c| c.asset_root.join(MODEL).is_file()) {
        let scene = server.load(GltfAssetLabel::Scene(0).from_asset(MODEL));
        bones.root = Some(commands.spawn((SceneRoot(scene), Visibility::Hidden)).id());
    }
}

/// Once the model is in: its primitives take the bone material, and its skins are noted for posing.
fn prepare(
    mut commands: Commands,
    mut bones: ResMut<Bones>,
    children: Query<&Children>,
    skins: Query<&SkinnedMesh>,
    names: Query<&Name>,
    primitives: Query<&GltfMaterialName>,
) {
    let Some(root) = bones.root else { return };
    if !bones.skins.is_empty() {
        return;
    }
    for entity in children.iter_descendants(root) {
        if let Ok(skin) = skins.get(entity) {
            let joints = skin.joints.iter().map(|&j| names.get(j).map_or(String::new(), |n| n.to_ascii_uppercase())).collect();
            bones.skins.push((entity, joints));
        }
        if let Some(mesh) = primitives.get(entity).ok().and_then(|n| n.0.strip_prefix("Retail_Bones_")) {
            bones.parts.push((entity, mesh.to_owned()));
            commands.entity(entity).remove::<MeshMaterial3d<StandardMaterial>>().insert((NoFrustumCulling, NotShadowCaster));
        }
    }
}

fn show(
    mut bones: ResMut<Bones>,
    skater: Option<Res<SkaterRuntime>>,
    animation: Res<AnimationStatus>,
    mut assets: ResMut<Assets<BoneMaterial>>,
    mut skins: Query<&mut SkinnedMesh>,
    mut visibility: Query<&mut Visibility>,
    mut commands: Commands,
) {
    let (Some(root), Some(skater)) = (bones.root, skater) else { return };
    let shown = skater.hall_of_meat.shown_bones().filter(|_| animation.ready && !bones.parts.is_empty());
    let on = shown.is_some_and(|s| s.iter().any(|&t| t > 0));
    if let Ok(mut v) = visibility.get_mut(root) {
        v.set_if_neq(if on { Visibility::Inherited } else { Visibility::Hidden });
    }
    let Some(shown) = shown.filter(|_| on) else { return };
    // The skater's joints by bone name: the bones ride the skater's pose (and any rebind after an outfit change).
    let names = &skater.animation.evaluator.frames.bone_names;
    let mut joints = std::collections::HashMap::new();
    for (entity, bone, _) in animation.online_bindings() {
        if let Some(name) = names.get(bone) {
            joints.entry(name.to_ascii_uppercase()).or_insert(entity);
        }
    }
    for (entity, wanted) in &bones.skins {
        if let Ok(mut skin) = skins.get_mut(*entity) {
            let mapped: Vec<Entity> = wanted.iter().zip(&skin.joints).map(|(n, &own)| joints.get(n).copied().unwrap_or(own)).collect();
            if skin.joints != mapped {
                skin.joints = mapped;
            }
        }
    }
    let materials = bones.materials.get_or_insert_with(|| {
        skater.hall_of_meat.bone_colours().map(|c| assets.add(BoneMaterial { colour: c.extend(1.) }))
    });
    let materials = materials.clone();
    for (entity, mesh) in &bones.parts {
        let t = tier(&shown, mesh);
        if let Ok(mut v) = visibility.get_mut(*entity) {
            v.set_if_neq(if t > 0 { Visibility::Inherited } else { Visibility::Hidden });
        }
        if t > 0 {
            commands.entity(*entity).insert(MeshMaterial3d(materials[usize::from(t) - 1].clone()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mesh_shows_its_worst_bone() {
        let mut shown = [0; 25];
        assert_eq!(tier(&shown, "Rib_Cage"), 0);
        // Chest upper sprained, chest mid broken: one rib cage, broken.
        (shown[10], shown[11]) = (1, 2);
        assert_eq!(tier(&shown, "Rib_Cage"), 2);
        // The face (23) is the skull's mesh, the groin (24) the hips'.
        shown[23] = 1;
        assert_eq!((tier(&shown, "Skull"), tier(&shown, "Hips")), (1, 0));
        // Every exported mesh is some bone's.
        for mesh in ["Ankle_Left", "Bicep_Right", "Calf_Left", "Forearm_Right", "Hand_Left", "Lower_Spine", "Neck", "Thigh_Right", "Toes_Left"] {
            assert!(MESHES.contains(&mesh), "{mesh}");
        }
    }
}
