//! Authored presentation supplements outside the static district geometry.
use bevy::prelude::*;
use skate_data::skate_map::SkateMap;
use std::path::Path;

pub(crate) fn spawn_backdrop(
    name: &str,
    asset_root: &Path,
    commands: &mut crate::map_render::SceneCommands,
    meshes: &mut impl crate::map_render::AssetSink<Mesh>,
    materials: &mut impl crate::map_render::AssetSink<super::PropMaterial>,
    retail_materials: &mut impl crate::map_render::AssetSink<super::RetailWorldMaterial>,
    blend_materials: &mut impl crate::map_render::AssetSink<super::RetailBlendMaterial>,
    images: &mut impl crate::map_render::AssetSink<Image>,
) {
    spawn_package(name, asset_root, "native-backdrops", commands, meshes, materials, retail_materials, blend_materials, images);
    commands.tagged(OnlineHidden, |commands| {
        spawn_package(name, asset_root, "native-props", commands, meshes, materials, retail_materials, blend_materials, images);
    });
}

/// Not retail (user 10-25, bug #34): retail keeps its props online, solid and owned by whoever grabs one
/// (82586FC0, 8283BC00..BCE8, 82897828); ours draw without collision, so they hide while a session is online.
/// SKATE_PROPS=1's solid pieces (PropPiece, shown by props::present) stay.
#[derive(Component, Clone)]
pub(crate) struct OnlineHidden;

pub(super) fn install(app: &mut App) {
    app.add_systems(Update, hide_online);
}

type Props<'w, 's> =
    Query<'w, 's, &'static mut Visibility, (With<OnlineHidden>, Without<crate::physics::props::PropPiece>)>;
fn hide_online(network: Option<Res<crate::multiplayer::Multiplayer>>, props: Props) {
    show_props(network.is_some_and(|n| n.active()), props);
}
fn show_props(online: bool, mut props: Props) {
    let visibility = if online { Visibility::Hidden } else { Visibility::Inherited };
    for mut v in &mut props {
        v.set_if_neq(visibility);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    /// Bug 10-25 #34: tagged props hide online and come back offline; SKATE_PROPS's solid pieces and untagged map
    /// meshes are left alone.
    #[test]
    fn props_hide_while_online() {
        let mut world = World::new();
        let prop = world.spawn((OnlineHidden, Visibility::Inherited)).id();
        let solid = world.spawn((OnlineHidden, crate::physics::props::PropPiece(0), Visibility::Hidden)).id();
        let wall = world.spawn(Visibility::Inherited).id();
        let shown = |world: &World| [prop, solid, wall].map(|e| *world.get::<Visibility>(e).unwrap());
        world.run_system_once(|props: Props| show_props(true, props)).unwrap();
        assert_eq!(shown(&world), [Visibility::Hidden, Visibility::Hidden, Visibility::Inherited]);
        world.run_system_once(|props: Props| show_props(false, props)).unwrap();
        assert_eq!(shown(&world), [Visibility::Inherited, Visibility::Hidden, Visibility::Inherited]);
    }

    /// Brief 10-29: Industrial's sea is DIST_Water's ocean.default (mesh 7), which only this global model holds. The
    /// backdrop carries it as a material the retail ocean (family 31) draws, over the whole sea collision (the
    /// Unrideable plane at x -3679..1839, z -492..1457); without it the sea draws nothing but the clear colour.
    #[test]
    #[ignore = "requires SKATE3_ASSET_ROOT (install assets, read-only)"]
    fn industrials_backdrop_draws_the_retail_ocean() {
        ocean_covers("Industrial", [-3679., 1839., -492., 1457.]);
    }

    /// Brief 10-29: New San Vanelona's sea is Skate 2's global DIST_Water (miscboot.big, two ocean.default meshes at
    /// y -64.63), outside the mod's district stream: old_sea.py exports it. It covers the Unrideable sea plane (x
    /// -359..1637, z 779..1978); without it the sea is the stream's default_gray collision meshes.
    #[test]
    #[ignore = "requires SKATE3_ASSET_ROOT (install assets, read-only)"]
    fn new_san_vanelonas_backdrop_draws_the_retail_sea() {
        ocean_covers("New_San_Vanelona", [-359., 1637., 779., 1978.]);
    }

    /// `map`'s backdrop draws family-31 (the retail ocean) over at least the box x0..x1, z0..z1.
    fn ocean_covers(name: &str, [x0, x1, z0, z1]: [f32; 4]) {
        let root = std::path::PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap());
        let bytes = std::fs::read(root.join(format!("private/native-backdrops/{name}.skate"))).unwrap();
        let map = SkateMap::parse_geometry_only(&bytes).unwrap();
        let tuning = super::super::MaterialTuning::load(&root);
        let ocean: Vec<u32> = (1..=map.materials.len() as u32)
            .filter(|&i| {
                let d = map.materials[i as usize - 1].retail_definition.as_deref().and_then(super::super::Definition::parse);
                d.is_some_and(|d| d.family == 31 && d.supported(&tuning))
            })
            .collect();
        assert!(!ocean.is_empty(), "no drawable ocean.default in {name}'s backdrop");
        let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
        for v in map.geometry.vertices.iter().filter(|v| ocean.contains(&v.material)) {
            let [x, _, z] = v.position;
            (lo, hi) = ([lo[0].min(x), lo[1].min(z)], [hi[0].max(x), hi[1].max(z)]);
        }
        assert!(lo[0] <= x0 && hi[0] >= x1 && lo[1] <= z0 && hi[1] >= z1, "{name}: ocean spans {lo:?}..{hi:?}");
    }
}

fn spawn_package(
    name: &str,
    asset_root: &Path,
    folder: &str,
    commands: &mut crate::map_render::SceneCommands,
    meshes: &mut impl crate::map_render::AssetSink<Mesh>,
    materials: &mut impl crate::map_render::AssetSink<super::PropMaterial>,
    retail_materials: &mut impl crate::map_render::AssetSink<super::RetailWorldMaterial>,
    blend_materials: &mut impl crate::map_render::AssetSink<super::RetailBlendMaterial>,
    images: &mut impl crate::map_render::AssetSink<Image>,
) {
    let path = asset_root
        .join("private")
        .join(folder)
        .join(format!("{name}.skate"));
    if !path.is_file() {
        return;
    }
    let map = match std::fs::read(&path)
        .map_err(|e| e.to_string())
        .and_then(|data| SkateMap::parse_render_only(&data))
    {
        Ok(map) => map,
        Err(error) => {
            error!("SKATE_BACKDROP: {}: {error}", path.display());
            return;
        }
    };
    // This package contributes presentation only; never route it into physics.
    if map.name != name
        || !map.geometry.collision.is_empty()
        || !map.lights.is_empty()
        || !map.doors.is_empty()
        || !map.rails.is_empty()
    {
        error!(
            "SKATE_BACKDROP: invalid render-only package {}",
            path.display()
        );
        return;
    }
    info!(
        "SKATE_BACKDROP: {name} {folder} triangles={}",
        map.geometry.indices.len() / 3
    );
    // Movable props become one entity group per instance (physics/props.rs).
    let instances = if folder == "native-props" && crate::physics::props::enabled() {
        crate::physics::props::render_instances(asset_root, &map)
    } else {
        Vec::new()
    };
    crate::skate_world::spawn_instances(
        &map,
        &instances,
        commands,
        meshes,
        materials,
        retail_materials,
        blend_materials,
        images,
        &super::MaterialTuning::load(asset_root),
    );
}
