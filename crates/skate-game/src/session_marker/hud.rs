//! Original APT/GEO triangles, Xbox prompts and bitmap glyphs; no system fonts.
use bevy::{
    asset::RenderAssetUsages,
    camera::visibility::RenderLayers,
    image::ImageSampler,
    prelude::*,
    render::render_resource::{Extent3d, PrimitiveTopology, TextureDimension, TextureFormat},
};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
struct Hud {
    version: u32,
    canvas: [f32; 2],
    /// The exporter's vertex scale (old_hud.py PHONE_SCALE); absent: 1, or 0.6 for an older Skate 2 export.
    #[serde(default)]
    authored_scale: Option<f32>,
    textures: Vec<Texture>,
    meshes: Vec<Primitive>,
    /// Not retail (user 10-25): the B row as Player List (role "players"), apart from `meshes` so an older build never
    /// draws it. Absent from an install exported before it: the B row stays Object Dropper.
    #[serde(default)]
    player_list: Vec<Primitive>,
}
#[derive(Deserialize)]
struct Texture {
    file: String,
    width: u32,
    height: u32,
}
#[derive(Deserialize)]
struct Primitive {
    texture: Option<usize>,
    color: [f32; 4],
    vertices: Vec<Vertex>,
    /// The exporters' row ("return", "place"; "dropper", "players", "art") and the clip it was flattened from.
    #[serde(default)]
    role: String,
    #[serde(default)]
    source: String,
}
#[derive(Deserialize)]
struct Vertex {
    position: [f32; 2],
    uv: [f32; 2],
}
#[derive(Component)]
struct MarkerHudRoot {
    /// Whose panel: 0 Skate 3, 1 Skate 1, 2 Skate 2.
    game: u8,
    /// Shown while the panel is closed instead (Skate 1's tab in the top-left corner).
    tab: bool,
}

/// A marker row's mesh, drawn disabled while its action can't be taken. hudphonelist's UpdateMainMenu: Skate 3's
/// (do_init_action @4716) sets the row's text and button `_alpha = 40` while Game.CanUseSessionMarker (return) /
/// CanPlaceSessionMarker (place) is false, else 100; Skate 2's plays its text's `disabled` label, the same text in
/// #cccccc instead of #003399 (characters 46 / 45), and leaves the button as it is.
#[derive(Component)]
struct MarkerRow {
    place: bool,
    /// The exported (enabled) colour.
    color: [f32; 4],
    skate2: bool,
    disabled: bool,
}

/// Not retail (user 10-25): a B row mesh, the greyed Object Dropper (`false`) or the same row as Player List (`true`,
/// exporter role "players"); one of the two shows, by SessionMarker::player_list.
#[derive(Component)]
struct PlayerListRow(bool);

/// The row's colour, enabled or not.
fn row_color(row: &MarkerRow, enabled: bool) -> [f32; 4] {
    let [r, g, b, a] = row.color;
    match (enabled, row.skate2) {
        (true, _) => row.color,
        (false, false) => [r, g, b, a * 0.4],
        (false, true) => [0.8, 0.8, 0.8, a],
    }
}

/// The factor back to the authored 1:1 size.
fn unscale(authored: Option<f32>, game: u8) -> f32 {
    1. / authored.unwrap_or(if game == 2 { 0.6 } else { 1. })
}

/// The marker panel to show: the active game's, or Skate 3's when that game's panel was not prepared.
fn pick(active: u8, has: impl Fn(u8) -> bool) -> u8 {
    if has(active) { active } else { 0 }
}

pub(super) fn install(app: &mut App) {
    app.add_systems(Startup, load)
        .add_systems(Update, (present, b_row))
        // perf 10-18: after every Update system, so it sees this frame's panel and noise visibility.
        .add_systems(PostUpdate, camera_active);
}

#[derive(Component)]
struct MarkerCamera;

/// SKATE_MARKER_CAM=always: the marker camera draws every frame, as before perf 10-18.
fn camera_always() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("SKATE_MARKER_CAM").is_ok_and(|v| v == "always"))
}

/// perf 10-18: the marker camera draws only while a panel, the tab or the noise screen shows. Hidden, it drew nothing
/// but still cost a 2D pass and a second full-window blit (ClearColorConfig::None: the presentation camera's image).
fn camera_active(
    shown: Query<&Visibility, Or<(With<MarkerHudRoot>, With<super::effect::Screen>)>>,
    mut cameras: Query<&mut Camera, With<MarkerCamera>>,
) {
    let active = camera_always() || shown.iter().any(|v| *v != Visibility::Hidden);
    for mut camera in &mut cameras {
        if camera.is_active != active {
            camera.is_active = active;
        }
    }
}

pub(super) fn overlay_path(root: &Path) -> PathBuf {
    std::env::var_os("SKATE3_SESSION_MARKER_OVERLAY")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("private/session-marker"))
}

fn load(
    mut commands: Commands,
    config: Res<crate::config::Config>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let layer = RenderLayers::layer(29);
    commands.spawn((
        MarkerCamera,
        Camera2d,
        Camera {
            order: 1,
            is_active: camera_always(),
            clear_color: ClearColorConfig::None,
            ..default()
        },
        Msaa::Off,
        layer.clone(),
    ));
    let private = config.asset_root.join("private");
    for (folder, game, tab) in [
        (overlay_path(&config.asset_root), 0, false),
        // Skate 2's phone list (skate2port/old_hud.py), shown instead while its HUD is up.
        (private.join("hud-skate2/session-marker"), 2, false),
        // Skate 1's D-pad panel and the tab it leaves in the corner while closed (skate2port/old_hud1.py).
        (private.join("hud-skate1/session-marker"), 1, false),
        (private.join("hud-skate1/session-marker-tab"), 1, true),
    ] {
        if let Err(e) = spawn_overlay(
            &mut commands,
            &mut meshes,
            &mut materials,
            &mut images,
            &layer,
            &folder,
            MarkerHudRoot { game, tab },
        ) {
            warn!(
                "Original session-marker HUD unavailable at {}: {e}. See docs/hud-installation.md",
                folder.display()
            );
        }
    }
}

fn spawn_overlay(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<ColorMaterial>,
    images: &mut Assets<Image>,
    layer: &RenderLayers,
    folder: &Path,
    root: MarkerHudRoot,
) -> Result<(), String> {
    let read = || -> Result<Hud, String> {
        let bytes = std::fs::read(folder.join("hud.json")).map_err(|e| e.to_string())?;
        let hud: Hud = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if hud.version != 1 || hud.canvas != [1280., 720.] {
            return Err("Unsupported original HUD manifest".into());
        }
        Ok(hud)
    };
    let hud = read()?;
    // Retail draws hudphonelist 1:1 on its 1280x720 stage; the old Skate 2 export shrank it to 0.6 by eye
    // (user 10-14, bug #3: its size looked wrong). Undone here so an existing install is right without a re-export.
    let unscale = unscale(hud.authored_scale, root.game);
    let mut textures = Vec::new();
    for t in hud.textures {
        let path = Path::new(&t.file);
        if path.components().count() != 1 || t.width == 0 || t.height == 0 {
            return Err("Invalid marker texture descriptor".into());
        }
        let Ok(bytes) = std::fs::read(folder.join(path)) else {
            return Err(format!("Missing marker texture {}", t.file));
        };
        if bytes.len() != t.width as usize * t.height as usize * 4 {
            return Err("Invalid marker texture length".into());
        }
        let mut image = Image::new(
            Extent3d {
                width: t.width,
                height: t.height,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            bytes,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        );
        image.sampler = ImageSampler::linear();
        textures.push(images.add(image));
    }
    if hud.meshes.iter().chain(&hud.player_list).any(|m| {
        m.vertices.len() % 3 != 0
            || m.texture.is_some_and(|t| t >= textures.len())
            || !m
                .vertices
                .iter()
                .all(|v| v.position.iter().chain(&v.uv).all(|x| x.is_finite()))
    }) {
        return Err("Invalid marker HUD geometry".into());
    }
    info!(
        "Original session-marker HUD loaded from {}",
        folder.display()
    );
    let game = root.game;
    let player_list = !hud.player_list.is_empty();
    if game == 0 && !player_list {
        warn!(
            "Session-marker HUD at {} has no Player List row (exported before 10-25): online, B keeps the Object Dropper label until the HUD assets refresh",
            folder.display()
        );
    }
    commands
        .spawn((
            root,
            Transform::default(),
            Visibility::Hidden,
        ))
        .with_children(|parent| {
            // The Player List row last: drawn over the panel art, as the Object Dropper row it replaces.
            for (order, primitive) in hud.meshes.into_iter().chain(hud.player_list).enumerate() {
                let mut mesh = Mesh::new(
                    PrimitiveTopology::TriangleList,
                    RenderAssetUsages::default(),
                );
                mesh.insert_attribute(
                    Mesh::ATTRIBUTE_POSITION,
                    primitive
                        .vertices
                        .iter()
                        .map(|v| [v.position[0] * unscale, -v.position[1] * unscale, 0.])
                        .collect::<Vec<_>>(),
                );
                mesh.insert_attribute(
                    Mesh::ATTRIBUTE_UV_0,
                    primitive.vertices.iter().map(|v| v.uv).collect::<Vec<_>>(),
                );
                // ponytail: Skate 1's D-pad panel has no disabled look mapped.
                let row = match primitive.role.as_str() {
                    "return" | "place" if game == 0 || game == 2 && primitive.source.contains("/mText") => Some(MarkerRow {
                        place: primitive.role == "place",
                        color: primitive.color,
                        skate2: game == 2,
                        disabled: false,
                    }),
                    _ => None,
                };
                let b_row = match primitive.role.as_str() {
                    "players" => Some(true),
                    "dropper" if player_list => Some(false),
                    _ => None,
                };
                let [r, g, b, a] = primitive.color;
                let material = ColorMaterial {
                    color: Color::srgba(r, g, b, a),
                    texture: primitive.texture.map(|i| textures[i].clone()),
                    ..default()
                };
                let mut child = parent.spawn((
                    Mesh2d(meshes.add(mesh)),
                    MeshMaterial2d(materials.add(material)),
                    layer.clone(),
                    Transform::from_xyz(0., 0., order as f32 * 0.01),
                ));
                if let Some(row) = row {
                    child.insert(row);
                }
                if let Some(players) = b_row {
                    child.insert((PlayerListRow(players), if players { Visibility::Hidden } else { Visibility::Inherited }));
                }
            }
        });
    Ok(())
}

/// The B row as Player List or as Object Dropper (PlayerListRow).
fn b_row(session: Res<super::SessionMarker>, mut rows: Query<(&PlayerListRow, &mut Visibility)>) {
    for (row, mut visibility) in &mut rows {
        visibility.set_if_neq(if row.0 == session.player_list { Visibility::Inherited } else { Visibility::Hidden });
    }
}

fn present(
    session: Res<super::SessionMarker>,
    map: Res<crate::map_transition::CurrentMap>,
    menu: Option<Res<crate::graphics_menu::Menu>>,
    customiser: Option<Res<crate::customiser::Customiser>>,
    window: Single<&Window>,
    mut roots: Query<(&MarkerHudRoot, &mut Transform, &mut Visibility)>,
    mut rows: Query<(&mut MarkerRow, &MeshMaterial2d<ColorMaterial>)>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    for (mut row, material) in &mut rows {
        let disabled = !if row.place { session.can_place } else { session.can_return };
        if row.disabled != disabled
            && let Some(mut m) = materials.get_mut(&material.0)
        {
            row.disabled = disabled;
            let [r, g, b, a] = row_color(&row, !disabled);
            m.color = Color::srgba(r, g, b, a);
        }
    }
    // The map's game's panel (Settings > HUD Style); Skate 3's is game 0 here.
    let active = crate::skate2_hud::hud_game(menu.as_deref(), &map) % 3;
    let game = pick(active, |g| roots.iter().any(|(r, _, _)| r.game == g && !r.tab));
    // The tab is part of the HUD: it hides with it (Settings > HUD, the Edit Skater screen).
    let hud_shown = menu.is_none_or(|m| m.hud_visible()) && customiser.is_none_or(|c| !c.open);
    let scale = (window.width() / 1280.).min(window.height() / 720.);
    // Unconditional writes would re-propagate the whole HUD tree every frame.
    for (root, mut transform, mut visibility) in &mut roots {
        let shown = root.game == game && if root.tab { !session.visible && hud_shown } else { session.visible };
        visibility.set_if_neq(if shown {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
        let placed = transform
            .with_translation(Vec3::new(-window.width() / 2., window.height() / 2., 0.))
            .with_scale(Vec3::splat(scale));
        transform.set_if_neq(placed);
    }
}

#[cfg(test)]
mod tests {
    use super::{MarkerRow, pick, row_color, unscale};

    /// hudphonelist's UpdateMainMenu: Skate 3 dims a disabled row to _alpha 40, Skate 2 greys its text (#cccccc).
    #[test]
    fn a_disabled_marker_row_looks_as_retails() {
        let row = |skate2| MarkerRow { place: true, color: [0., 0.2, 0.6, 0.75], skate2, disabled: false };
        assert_eq!(row_color(&row(false), true), [0., 0.2, 0.6, 0.75]);
        assert_eq!(row_color(&row(false), false), [0., 0.2, 0.6, 0.75 * 0.4]);
        assert_eq!(row_color(&row(true), true), [0., 0.2, 0.6, 0.75]);
        assert_eq!(row_color(&row(true), false), [0.8, 0.8, 0.8, 0.75]);
    }

    /// User 10-14, bug #3: Skate 2's phone list at retail's 1:1, from an old (0.6) or new (1.0) export.
    #[test]
    fn skate2s_phone_list_draws_at_its_authored_size() {
        assert!((unscale(None, 2) - 1. / 0.6).abs() < 1e-6);
        assert_eq!(unscale(Some(1.), 2), 1.);
        assert_eq!(unscale(None, 0), 1.);
    }

    /// perf 10-18: the marker camera draws only while something on its layer shows.
    #[test]
    fn the_marker_camera_runs_only_while_a_panel_or_the_noise_shows() {
        use bevy::{ecs::system::RunSystemOnce, prelude::*};
        let mut world = World::new();
        let camera = world.spawn((super::MarkerCamera, Camera::default())).id();
        let root = world.spawn((super::MarkerHudRoot { game: 0, tab: false }, Visibility::Hidden)).id();
        let screen = world.spawn((super::super::effect::Screen, Visibility::Hidden)).id();
        let active = |world: &mut World| {
            world.run_system_once(super::camera_active).unwrap();
            world.get::<Camera>(camera).unwrap().is_active
        };
        assert!(!active(&mut world));
        world.entity_mut(root).insert(Visibility::Inherited);
        assert!(active(&mut world));
        world.entity_mut(root).insert(Visibility::Hidden);
        world.entity_mut(screen).insert(Visibility::Inherited);
        assert!(active(&mut world));
        world.entity_mut(screen).insert(Visibility::Hidden);
        assert!(!active(&mut world));
    }

    /// User 10-25: the B row reads Player List where retail's reads Swap Camera, else the greyed Object Dropper.
    #[test]
    fn the_b_row_reads_player_list_or_object_dropper() {
        use bevy::{ecs::system::RunSystemOnce, prelude::*};
        let mut world = World::new();
        world.insert_resource(super::super::SessionMarker::default());
        let dropper = world.spawn((super::PlayerListRow(false), Visibility::Inherited)).id();
        let players = world.spawn((super::PlayerListRow(true), Visibility::Hidden)).id();
        let shown = |world: &mut World, player_list| {
            world.resource_mut::<super::super::SessionMarker>().player_list = player_list;
            world.run_system_once(super::b_row).unwrap();
            [dropper, players].map(|e| *world.get::<Visibility>(e).unwrap() != Visibility::Hidden)
        };
        assert_eq!(shown(&mut world, true), [false, true]);
        assert_eq!(shown(&mut world, false), [true, false]);
    }

    #[test]
    fn the_active_games_panel_shows_when_prepared_else_skate3s() {
        assert_eq!(pick(1, |g| g == 0 || g == 1), 1);
        assert_eq!(pick(1, |g| g == 0), 0);
        assert_eq!(pick(2, |_| true), 2);
        assert_eq!(pick(0, |_| true), 0);
    }
}
