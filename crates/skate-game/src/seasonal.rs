//! Not retail (user 10-19): the Halloween and Christmas weather modes (Settings > World > Weather). Halloween is clear
//! weather, Christmas light snow; both force night and dress only the Super Ultra Mega Park on University, scene by
//! scene from the authored table in seasonal/layout.rs: place.rs lands it on the park's collision, the season's
//! modules build the pieces, strung.rs the light strands and giant webs, and this module spawns it all, a few merged
//! meshes per zone, with a capped set of point lights. Halloween also tints its sky a little orange.
//! retail_weather.wgsl draws the tree effects and the sky from the frame state (snow.z the season, snow.w how near
//! the park the camera is). On a real Halloween or Christmas the mode turns itself on (graphics_menu's `seasonal`
//! offline, online_moves' host start online). Not tested in play.

use crate::retail_render::{PropMaterial, ShadowState};
use bevy::{light::NotShadowCaster, prelude::*};
use std::f32::consts::{FRAC_PI_2, PI, TAU};

mod christmas;
mod halloween;
mod layout;
mod model;
mod place;
mod strung;

use layout::{Kind, ZONES, Zone};
use model::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Season {
    Halloween,
    Christmas,
}

impl Season {
    /// From a graphics_menu WEATHER index.
    pub(crate) fn from_weather(weather: u32) -> Option<Self> {
        match weather {
            5 => Some(Self::Halloween),
            6 => Some(Self::Christmas),
            _ => None,
        }
    }
    /// The graphics_menu WEATHER index.
    pub(crate) fn weather(self) -> u32 {
        match self {
            Self::Halloween => 5,
            Self::Christmas => 6,
        }
    }
}

/// The real days (month, day) each mode turns itself on: Halloween, Christmas Eve and Christmas Day.
const DAYS: [(i64, i64, Season); 3] =
    [(10, 31, Season::Halloween), (12, 24, Season::Christmas), (12, 25, Season::Christmas)];

fn on(month: i64, day: i64) -> Option<Season> {
    DAYS.iter().find(|d| (d.0, d.1) == (month, day)).map(|d| d.2)
}

/// Today's season by the local calendar, if today is one of [`DAYS`].
pub(crate) fn today() -> Option<Season> {
    let [_, month, day, ..] = crate::replay::file::local(crate::replay::file::unix_now());
    on(month, day)
}

/// The Super Ultra Mega Park's outline on University, [x, z] (its online area). retail_weather.wgsl's in_mega_park
/// is its bounding box less the cut-out corner.
const PARK: [[f32; 2]; 6] = [[114., -792.], [491., -794.], [496., -509.], [205., -511.], [205., -606.], [106., -608.]];
/// The park deck's height (its teleport locator stands at 140, the deck spawn at about 133).

pub(crate) struct SeasonalPlugin;

impl Plugin for SeasonalPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (frame_state.after(crate::retail_render::WeatherSet), decorate, animate, share_lights, halloween::animate).chain())
            .add_systems(Update, follow_map.before(crate::retail_render::WeatherSet));
    }
}

/// The Super Ultra Mega Park's map.
fn university(map: Option<&crate::map_transition::CurrentMap>) -> bool {
    map.is_some_and(|m| crate::map_library::same_name(&m.name, "University"))
}

/// Not retail (user 10-25): the modes are offered and in effect only on the park's map (graphics_menu `seasons`).
fn follow_map(map: Option<Res<crate::map_transition::CurrentMap>>, menu: Option<ResMut<crate::graphics_menu::Menu>>) {
    let Some(mut menu) = menu else { return };
    let here = university(map.as_deref());
    if menu.seasons != here {
        (menu.seasons, menu.revision) = (here, menu.revision + 1);
    }
}

/// The season dressing the world now: the weather's, on retail University outside the Edit Skater room.
pub(crate) fn active(
    menu: Option<&crate::graphics_menu::Menu>,
    map: Option<&crate::map_transition::CurrentMap>,
    retail: Option<&crate::retail_render::RetailScene>,
    customiser: Option<&crate::customiser::Customiser>,
) -> Option<Season> {
    let shown = university(map) && retail.is_some_and(|r| r.0) && !customiser.is_some_and(|c| c.open);
    menu?.season().filter(|_| shown)
}

fn frame_state(
    menu: Option<Res<crate::graphics_menu::Menu>>,
    map: Option<Res<crate::map_transition::CurrentMap>>,
    retail: Option<Res<crate::retail_render::RetailScene>>,
    customiser: Option<Res<crate::customiser::Customiser>>,
    cameras: Query<&GlobalTransform, With<Camera3d>>,
    mut state: ResMut<ShadowState>,
) {
    let season = active(menu.as_deref(), map.as_deref(), retail.as_deref(), customiser.as_deref());
    let code = match season {
        Some(Season::Halloween) => 1.,
        Some(Season::Christmas) => 2.,
        None => 0.,
    };
    let near = if season.is_some() { cameras.iter().next().map_or(1., |c| nearness(c.translation())) } else { 0. };
    (state.4.z, state.4.w) = (code, near);
}

/// 1 in the park, easing to 0 by 200 m outside it: how much of the Halloween sky shows.
fn nearness(at: Vec3) -> f32 {
    let x = 1. - (outside(at.x, at.z) / 200.).min(1.);
    x * x * (3. - 2. * x)
}

/// How far (x, z) is outside the park's outline (0 inside).
fn outside(x: f32, z: f32) -> f32 {
    let p = Vec2::new(x, z);
    if crate::online_boundary::inside(&PARK, p.into()) {
        return 0.;
    }
    (0..PARK.len())
        .map(|i| {
            let (a, b) = (Vec2::from(PARK[i]), Vec2::from(PARK[(i + 1) % PARK.len()]));
            let t = ((p - a).dot(b - a) / (b - a).length_squared()).clamp(0., 1.);
            p.distance(a + (b - a) * t)
        })
        .fold(f32::MAX, f32::min)
}

/// The buildings just outside the outline face the park too: they are dressed up to this far out (m).
const MARGIN: f32 = 20.;

/// Every spawned decoration's root (also a MapEntity, so a map change clears it).
#[derive(Component)]
struct Decor;

/// One flicker the jack-o'-lanterns share: their carved faces' glow and the glow they throw on the ground.
struct Flame {
    face: Handle<StandardMaterial>,
    pool: Handle<StandardMaterial>,
}

#[derive(Resource)]
struct Flames(Vec<Flame>);

/// The big tree's star, turning slowly.
#[derive(Component)]
struct Spin;

/// The Christmas bulb colours: every multicoloured bulb uses one of these five, which step through [`PALETTE`].
#[derive(Resource)]
struct Bulbs([Handle<StandardMaterial>; 5]);

/// Christmas light colours (linear), shared with retail_weather.wgsl's bulb_colour.
const PALETTE: [[f32; 3]; 5] = [[1.0, 0.06, 0.04], [0.08, 0.9, 0.12], [0.1, 0.25, 1.0], [1.0, 0.55, 0.05], [1.0, 0.85, 0.55]];
/// A jack-o'-lantern candle (lumens, at the engine's exposure; map lights are authored in the same units).
const CANDLE_LUMENS: f32 = 120_000.;

#[allow(clippy::too_many_arguments)]
fn decorate(
    mut commands: Commands,
    menu: Option<Res<crate::graphics_menu::Menu>>,
    map: Option<Res<crate::map_transition::CurrentMap>>,
    retail: Option<Res<crate::retail_render::RetailScene>>,
    customiser: Option<Res<crate::customiser::Customiser>>,
    physics: Option<Res<crate::physics::GamePhysics>>,
    decor: Query<Entity, With<Decor>>,
    mut done: Local<Option<(Option<Season>, u64)>>,
    mut assets: (ResMut<Assets<Mesh>>, ResMut<Assets<StandardMaterial>>, ResMut<Assets<PropMaterial>>, ResMut<Assets<Image>>),
) {
    let (Some(current), Some(physics)) = (map.as_deref(), physics) else { return };
    let season = active(menu.as_deref(), Some(current), retail.as_deref(), customiser.as_deref());
    let key = (season, current.generation);
    if *done == Some(key) {
        return;
    }
    *done = Some(key);
    for entity in &decor {
        commands.entity(entity).despawn();
    }
    commands.remove_resource::<Bulbs>();
    commands.remove_resource::<Flames>();
    let Some(season) = season else { return };
    let started = std::time::Instant::now();
    let mut plan = place::resolve(physics.world(), layout::layout(season));
    plan.glows = place::glows(physics.world(), &plan, season);
    info!(
        "SEASONAL {season:?} resolved in {:?}: {} pieces, {} pumpkins, {} strands, {} webs, {} flocks, {} roosting bats, {} dropped",
        started.elapsed(),
        plan.pieces.len(),
        plan.pumpkins.len(),
        plan.strands.len(),
        plan.webs.len(),
        plan.flocks.len(),
        plan.roosts.len(),
        plan.dropped.len()
    );
    for zone in layout::ZONES {
        info!(
            "SEASONAL {zone:?}: {} pieces, {} strands, {} webs",
            plan.pieces.iter().filter(|p| p.zone == zone).count(),
            plan.strands.iter().filter(|s| s.zone == zone).count(),
            plan.webs.iter().filter(|w| w.zone == zone).count()
        );
    }
    for line in &plan.dropped {
        info!("SEASONAL dropped {line}");
    }
    let (meshes, unlit, lit, images) = (&mut *assets.0, &mut *assets.1, &mut *assets.2, &mut *assets.3);
    spawn_plan(&mut commands, meshes, unlit, lit, images, &plan, season);
    info!("SEASONAL {season:?} spawned in {:?} ({} ground glows)", started.elapsed(), plan.glows.len());
}

#[allow(clippy::too_many_arguments)]
fn animate(
    time: Res<Time>,
    state: Res<ShadowState>,
    bulbs: Option<Res<Bulbs>>,
    flames: Option<Res<Flames>>,
    mut stars: Query<&mut Transform, With<Spin>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if flames.is_none() && bulbs.is_none() {
        return;
    }
    let t = time.elapsed_secs();
    // Glow on the ground shows most at night (retail_daynight: day_sky.w is the night blend).
    let dark = 0.35 + 0.65 * state.7[1].w;
    let flicker_of = |group: usize| flicker(t, group as f32 * 2.39);
    for (group, flame) in flames.iter().flat_map(|f| f.0.iter().enumerate()) {
        let f = flicker_of(group);
        if let Some(m) = materials.get_mut(&flame.face) {
            m.base_color = Color::linear_rgb(5. * f, 2.1 * f, 0.3 * f);
        }
        if let Some(m) = materials.get_mut(&flame.pool) {
            m.base_color = Color::linear_rgb(1.4 * f * dark, 0.55 * f * dark, 0.08 * f * dark);
        }
    }
    // A new colour on every bulb about every 1.6 s; each of the five sets twinkles on its own.
    let step = (t / 1.6) as usize;
    if let Some(bulbs) = bulbs {
        for (g, handle) in bulbs.0.iter().enumerate() {
            let c = PALETTE[(g + step) % PALETTE.len()];
            let twinkle = 2.5 + 1.5 * (t * (2.1 + 0.7 * g as f32) + g as f32 * 1.7).sin();
            if let Some(m) = materials.get_mut(handle) {
                m.base_color = Color::linear_rgb(c[0] * twinkle, c[1] * twinkle, c[2] * twinkle);
            }
        }
    }
    for mut star in &mut stars {
        star.rotation = Quat::from_rotation_y(t * 0.6);
    }
}

/// Lights the lamps nearest the camera by their [`shares`] and hides the rest; a candle flickers with its flame.
fn share_lights(time: Res<Time>, cameras: Query<&GlobalTransform, With<Camera3d>>, mut lamps: Query<(&Lamp, &mut PointLight, &mut Visibility)>) {
    let Some(eye) = cameras.iter().next().map(GlobalTransform::translation) else { return };
    if lamps.is_empty() {
        return;
    }
    let at: Vec<Vec3> = lamps.iter().map(|(lamp, ..)| lamp.at).collect();
    let t = time.elapsed_secs();
    for ((lamp, mut light, mut visibility), share) in lamps.iter_mut().zip(shares(&at, eye)) {
        light.intensity = lamp.lumens * share * lamp.candle.map_or(1., |group| flicker(t, group as f32 * 2.39));
        visibility.set_if_neq(if share > 0. { Visibility::Inherited } else { Visibility::Hidden });
    }
}

/// A candle's brightness, 0.7 to 1: a few unrelated waves so no two lanterns pulse together.
fn flicker(t: f32, seed: f32) -> f32 {
    0.86 + 0.07 * (t * 11.3 + seed).sin() + 0.04 * (t * 23.7 + seed * 2.1).sin() + 0.03 * (t * 5.3 + seed * 0.7).sin()
}

// ---------------------------------------------------------------------------------------------------------------
// Spawning the resolved park.

/// The most point lights the season shines at once (each one costs every lit pixel near it).
const POINT_LIGHTS: usize = 48;

/// One of the season's point lights: a piece's floodlight, or the candle of a lit jack-o'-lantern (its flicker group).
#[derive(Component, Clone, Copy, Debug)]
struct Lamp {
    at: Vec3,
    colour: [f32; 3],
    lumens: f32,
    range: f32,
    candle: Option<usize>,
}

/// A piece's floodlights by its season's table: (spot in front of it, linear colour, lumens, range, glow diameter).
fn floods(kind: Kind, season: Season) -> Vec<(Vec3, [f32; 3], f32, f32, f32)> {
    match season {
        Season::Halloween => halloween::floods(kind),
        Season::Christmas => christmas::floods(kind),
    }
}

/// Where a flood of `piece` at `spot` (model space) stands in the world.
fn flood_at(piece: &place::Placed, spot: Vec3) -> Vec3 {
    piece.at + Quat::from_rotation_y(piece.yaw) * (spot * piece.scale)
}

/// The point lights to spawn: every piece's floods, then the lit jack-o'-lanterns' candles, biggest first. Only the
/// nearest [`POINT_LIGHTS`] shine at once ([`shares`]).
fn lamps(plan: &place::Plan, season: Season) -> Vec<Lamp> {
    let mut out: Vec<Lamp> = plan
        .pieces
        .iter()
        .flat_map(|p| {
            floods(p.kind, season).into_iter().map(move |(spot, colour, lumens, range, _)| Lamp {
                at: flood_at(p, spot),
                colour,
                lumens,
                range: range * p.scale,
                candle: None,
            })
        })
        .collect();
    let mut lit: Vec<(usize, &Pumpkin)> = plan.pumpkins.iter().enumerate().filter(|(_, p)| p.light).collect();
    lit.sort_by(|a, b| b.1.size.total_cmp(&a.1.size));
    out.extend(lit.into_iter().map(|(k, p)| Lamp {
        at: p.at + Vec3::Y * (0.71 * p.size),
        colour: [1., 0.5, 0.12],
        lumens: CANDLE_LUMENS,
        range: 7.,
        candle: Some(k % FLAMES),
    }));
    out
}

/// How much of its light each lamp at `at` gives with the camera at `eye`. Not retail (user 10-20): the
/// [`POINT_LIGHTS`] nearest shine, each fading out over the last quarter of the way to the nearest one left dark, so a
/// lamp comes and goes at nothing as the camera moves; a fixed pick left whole zones dark.
fn shares(at: &[Vec3], eye: Vec3) -> Vec<f32> {
    if at.len() <= POINT_LIGHTS {
        return vec![1.; at.len()];
    }
    let far: Vec<f32> = at.iter().map(|p| p.distance(eye)).collect();
    let mut sorted = far.clone();
    let cut = *sorted.select_nth_unstable_by(POINT_LIGHTS, f32::total_cmp).1;
    let fade = (0.25 * cut).max(0.01);
    far.iter().map(|&d| ((cut - d) / fade).clamp(0., 1.)).collect()
}

/// How an inflatable moves in the breeze (lean, bob, lift off its foot), or None for a piece that stands still.
fn breeze(kind: Kind) -> Option<(f32, f32, f32)> {
    match kind {
        Kind::House => Some((0.01, 0., 0.)),
        Kind::Witch => Some((0.035, 0., 0.)),
        Kind::Spider => Some((0.015, 0., 0.)),
        Kind::Ghost => Some((0.03, 0.12, 0.3)),
        Kind::SnowGlobe => Some((0.012, 0., 0.)),
        _ => None,
    }
}

/// The glows a piece throws on the ground as authored: a pool under each floodlight, and a mist's three patches.
fn ground_glows(piece: &place::Placed, season: Season) -> Vec<(Transform, [f32; 3])> {
    let mut out: Vec<(Transform, [f32; 3])> = floods(piece.kind, season)
        .into_iter()
        .map(|(spot, colour, _, _, size)| {
            let p = flood_at(piece, spot);
            let size = size * piece.scale;
            (Transform::from_xyz(p.x, piece.at.y + 0.03, p.z).with_scale(Vec3::new(size, 1., size)), colour.map(|c| c * 0.6))
        })
        .collect();
    if piece.kind == Kind::Mist {
        for k in 0..3u32 {
            let s = piece.seed ^ (k * 71 + 1);
            let size = (6. + 4. * random(s)) * piece.scale;
            let at = piece.at + ring(k as usize, 3) * (1.5 * piece.scale) + Vec3::Y * 0.3;
            out.push((Transform::from_translation(at).with_rotation(Quat::from_rotation_y(random(s ^ 1) * TAU)).with_scale(Vec3::new(size, 1., size * 0.7)), [0.18, 0.32, 0.2]));
        }
    }
    out
}

/// A piece's model, by whichever season builds it.
fn piece_model(kind: Kind, seed: u32) -> Option<Model> {
    halloween::model(kind, seed).or_else(|| christmas::model(kind, seed))
}

/// Spawns the season's resolved park: one model per zone (its still pieces, strands and webs), the inflatables, the
/// big tree's star, bats and spiders on threads, pumpkins, mist, uplights, the glows on the ground and [`lamps`].
#[allow(clippy::too_many_arguments)]
fn spawn_plan(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    unlit: &mut Assets<StandardMaterial>,
    lit: &mut Assets<PropMaterial>,
    images: &mut Assets<Image>,
    plan: &place::Plan,
    season: Season,
) {
    let mut materials = materials(unlit, lit, images);
    if season == Season::Christmas {
        let bulbs: [Handle<StandardMaterial>; 5] = std::array::from_fn(|i| unlit.add(glow(PALETTE[i], 4.)));
        for (i, b) in bulbs.iter().enumerate() {
            materials[MULTI + i] = Finish::Glow(b.clone());
        }
        commands.insert_resource(Bulbs(bulbs));
    }
    let pool_texture = images.add(pool_texture());
    let ground = pool(meshes, 1.);
    let mut pools: Vec<([f32; 3], Handle<StandardMaterial>)> = Vec::new();
    let mut glow_of = |unlit: &mut Assets<StandardMaterial>, colour: [f32; 3]| match pools.iter().find(|(c, _)| *c == colour) {
        Some((_, m)) => m.clone(),
        None => {
            let m = pool_material(unlit, pool_texture.clone());
            if let Some(material) = unlit.get_mut(&m) {
                material.base_color = Color::linear_rgb(colour[0], colour[1], colour[2]);
            }
            pools.push((colour, m.clone()));
            m
        }
    };
    let mut glows: Vec<(Transform, [f32; 3])> = plan.glows.clone();
    let mut zones: Vec<(Zone, Model)> = ZONES.iter().map(|&z| (z, Model::new())).collect();
    let mut danglers: Vec<(Vec3, f32)> = Vec::new();
    let mut shared: Vec<(Kind, Vec<(usize, Handle<Mesh>)>)> = Vec::new();
    for piece in &plan.pieces {
        let rotation = Quat::from_rotation_y(piece.yaw);
        match piece.kind {
            Kind::Mist => continue,
            Kind::Uplight => {
                let colours: &[[f32; 3]] = match season {
                    Season::Halloween => &[[0.5, 0.1, 0.9], [0.2, 0.9, 0.12]],
                    Season::Christmas => &[[1., 0.62, 0.3], [0.95, 0.08, 0.06], [0.1, 0.85, 0.2]],
                };
                let colour = colours[(random(piece.seed) * colours.len() as f32) as usize % colours.len()];
                // The wash up the wall behind it: the round glow stood upright, its lower half under the ground.
                let up = Quat::from_rotation_y(piece.yaw) * Quat::from_rotation_x(FRAC_PI_2);
                let at = piece.at + rotation * Vec3::Z * 0.05;
                glows.push((Transform::from_translation(at).with_rotation(up).with_scale(Vec3::new(1.5, 1., 16.) * piece.scale), colour));
                let fixture = &mut zones.iter_mut().find(|(z, _)| *z == piece.zone).expect("every zone has a model").1;
                surface(fixture.part(BLACK), 12, 6, pillow(piece.at + rotation * Vec3::new(0., 0.08, 0.25), Vec3::new(0.12, 0.08, 0.1), 0.3, 0.5));
                continue;
            }
            _ => {}
        }
        let Some(model) = piece_model(piece.kind, piece.seed) else { continue };
        let at = Transform::from_translation(piece.at).with_rotation(rotation).with_scale(Vec3::splat(piece.scale));
        if let Some((lean, bob, lift)) = breeze(piece.kind) {
            let parts = match shared.iter().find(|(k, _)| *k == piece.kind) {
                Some((_, parts)) => parts.clone(),
                None => {
                    let parts = upload(meshes, model);
                    shared.push((piece.kind, parts.clone()));
                    parts
                }
            };
            let root = piece.at + Vec3::Y * (lift * piece.scale);
            commands
                .spawn((
                    Name::new(format!("{season:?} {:?}", piece.kind)),
                    Decor,
                    crate::map_render::MapEntity,
                    Transform::from_translation(root).with_rotation(rotation).with_scale(Vec3::splat(piece.scale)),
                    Visibility::default(),
                    halloween::Sway { at: root, base: rotation, seed: random(piece.seed ^ 21) * TAU, lean, bob },
                ))
                .with_children(|c| spawn_parts(c, &parts, &materials));
            continue;
        }
        if piece.kind == Kind::BigTree {
            let Finish::Glow(gold) = &materials[STAR] else { unreachable!("the star's look glows") };
            commands.spawn((
                Name::new("Christmas star"),
                Decor,
                crate::map_render::MapEntity,
                Spin,
                Mesh3d(meshes.add(christmas::star(0.9, 0.22))),
                MeshMaterial3d(gold.clone()),
                Transform::from_translation(piece.at + Vec3::Y * ((14. + 0.6) * piece.scale)).with_scale(Vec3::splat(piece.scale)),
                NotShadowCaster,
            ));
        }
        zones.iter_mut().find(|(z, _)| *z == piece.zone).expect("every zone has a model").1.append(model, at);
    }
    for (k, strand) in plan.strands.iter().enumerate() {
        let m = &mut zones.iter_mut().find(|(z, _)| *z == strand.zone).expect("every zone has a model").1;
        strung::lights(m, &strand.path, strand.style, strand.palette, strand.garland, k);
    }
    for web in &plan.webs {
        let m = &mut zones.iter_mut().find(|(z, _)| *z == web.zone).expect("every zone has a model").1;
        let hub = strung::web(m, &web.frame, web.seed);
        if !web.spider || web.frame.len() < 3 {
            continue;
        }
        // The giant spider stands on the web at its hub, head up, its back to the web's front.
        let f = &web.frame;
        let n = (f[1] - f[0]).cross(f[2] - f[0]).normalize_or(Vec3::Y);
        let up = (Vec3::Y - n * n.y).normalize_or(Vec3::Z);
        let on = Quat::from_mat3(&Mat3::from_cols(n.cross(up), n, up));
        m.append(halloween::spider(0., 0.), Transform::from_translation(hub).with_rotation(on).with_scale(Vec3::splat(1.6)));
        danglers.extend(web.danglers.iter().copied());
    }
    for (zone, model) in zones {
        let parts = upload(meshes, model);
        if parts.is_empty() {
            continue;
        }
        commands
            .spawn((Name::new(format!("{season:?} {zone:?}")), Decor, crate::map_render::MapEntity, Transform::default(), Visibility::default()))
            .with_children(|c| spawn_parts(c, &parts, &materials));
    }
    halloween::spawn_danglers(commands, meshes, &materials, &danglers);
    halloween::spawn_bats(commands, meshes, &materials, &plan.flocks, &plan.roosts);
    for (at, colour) in glows {
        commands.spawn((Decor, crate::map_render::MapEntity, Mesh3d(ground.clone()), MeshMaterial3d(glow_of(unlit, colour)), at, NotShadowCaster));
    }
    if !plan.pumpkins.is_empty() {
        spawn_pumpkins(commands, meshes, unlit, lit, pool_texture, &plan.pumpkins);
    }
    // Dark until share_lights finds each among the nearest.
    for lamp in lamps(plan, season) {
        let [r, g, b] = lamp.colour;
        commands.spawn((
            Name::new(if lamp.candle.is_some() { "Jack-o'-lantern candle" } else { "Seasonal floodlight" }),
            Decor,
            crate::map_render::MapEntity,
            Transform::from_translation(lamp.at),
            PointLight { color: Color::linear_rgb(r, g, b), intensity: 0., range: lamp.range, radius: 0.1, shadows_enabled: false, ..default() },
            Visibility::Hidden,
            lamp,
        ));
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Jack-o'-lanterns, and the seeds every choice is made from.

#[derive(Clone, Copy, Debug)]
pub(crate) struct Pumpkin {
    pub at: Vec3,
    /// Its face looks along this yaw (radians from +Z toward +X).
    pub yaw: f32,
    /// Radius (m).
    pub size: f32,
    /// Carries the cluster's candle light.
    pub light: bool,
    /// Its face, or None for a plain pumpkin.
    pub carving: Option<Carving>,
    /// Its candle's glow pool lies on a surface. Not retail (user 10-20): up a tower, on a bale or over a beam's edge
    /// the pool hung in the air as a bright streak.
    pub spill: bool,
}

impl Pumpkin {
    /// Where its candle's glow pool lies (the unit [`model::pool`] scaled): ahead of it, 7 radii across.
    pub(crate) fn pool(&self) -> Transform {
        let spill = self.size * 7.;
        let rotation = Quat::from_rotation_y(self.yaw);
        Transform::from_translation(self.at + rotation * Vec3::Z * (spill * 0.3) + Vec3::Y * 0.02)
            .with_rotation(rotation)
            .with_scale(Vec3::new(spill, 1., spill * 1.2))
    }
}

/// A jack-o'-lantern's face.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Carving {
    /// Triangle eyes and nose, a toothy grin.
    Grin,
    /// Slanted eyes, a jagged mouth.
    Scowl,
    /// Round eyes and an O of a mouth.
    Shock,
}
const CARVINGS: [Carving; 3] = [Carving::Grin, Carving::Scowl, Carving::Shock];

fn carving(seed: u32) -> Carving {
    CARVINGS[(random(seed) * 3.) as usize % 3]
}

/// The k-th of n horizontal directions.
fn ring(k: usize, n: usize) -> Vec3 {
    let a = TAU * k as f32 / n as f32;
    Vec3::new(a.cos(), 0., a.sin())
}

/// A stable 0..1 number from a seed.
fn random(seed: u32) -> f32 {
    let mut h = seed.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    h = ((h >> ((h >> 28) + 4)) ^ h).wrapping_mul(277_803_737);
    ((h >> 22) ^ h) as f32 / u32::MAX as f32
}

/// A seed from a place (cm), so the same spot always gets the same choices.
fn seed_at(p: Vec3, salt: u32) -> u32 {
    let q = (p * 100.).as_ivec3();
    (q.x as u32).wrapping_mul(73_856_093) ^ (q.z as u32).wrapping_mul(19_349_663) ^ (q.y as u32).wrapping_mul(83_492_791) ^ salt
}

/// A point on a unit jack-o'-lantern: ten lobes, squat, dimpled top and bottom. `theta` from the top, `phi` from +X
/// toward +Z; its face looks along +Z.
fn pumpkin_point(theta: f32, phi: f32) -> Vec3 {
    let (s, c) = theta.sin_cos();
    let rib = 0.9 + 0.1 * (5. * phi).cos().abs().sqrt();
    let y = 0.8 * c - 0.2 * (1. - s).powi(3) * c.signum();
    Vec3::new(s * rib * phi.cos(), y, s * rib * phi.sin())
}

fn pumpkin_normal(theta: f32, phi: f32) -> Vec3 {
    const E: f32 = 1e-3;
    if theta < E || theta > PI - E {
        return if theta < 1. { Vec3::Y } else { Vec3::NEG_Y };
    }
    let n = (pumpkin_point(theta + E, phi) - pumpkin_point(theta - E, phi))
        .cross(pumpkin_point(theta, phi + E) - pumpkin_point(theta, phi - E))
        .normalize_or_zero();
    if n.dot(pumpkin_point(theta, phi)) < 0. { -n } else { n }
}

fn pumpkin_body() -> Mesh {
    const RINGS: usize = 20;
    const SEGMENTS: usize = 60;
    let mut b = Builder::default();
    for j in 0..=RINGS {
        for i in 0..=SEGMENTS {
            let (theta, phi) = (PI * j as f32 / RINGS as f32, TAU * i as f32 / SEGMENTS as f32);
            b.vertex(pumpkin_point(theta, phi), pumpkin_normal(theta, phi), Vec2::new(i as f32 / SEGMENTS as f32, j as f32 / RINGS as f32));
        }
    }
    let at = |j: usize, i: usize| (j * (SEGMENTS + 1) + i) as u32;
    for j in 0..RINGS {
        for i in 0..SEGMENTS {
            b.quad(at(j, i), at(j + 1, i), at(j + 1, i + 1), at(j, i + 1));
        }
    }
    b.mesh()
}

/// A carved shape on the pumpkin's face: (u, v) are across and up the face in radii, mapped onto the lobed skin and
/// raised `lift` off it.
fn face_vertex(b: &mut Builder, uv: Vec2, lift: f32) -> u32 {
    let phi = PI / 2. - uv.x;
    let theta = (uv.y / 0.8).clamp(-1., 1.).acos();
    let n = pumpkin_normal(theta, phi);
    b.vertex(pumpkin_point(theta, phi) + n * lift, n, uv)
}

/// A triangle of the face, grown by `grow` about its middle and split into 16 so it hugs the skin.
fn face_triangle(b: &mut Builder, corners: [Vec2; 3], grow: f32, lift: f32) {
    let middle = (corners[0] + corners[1] + corners[2]) / 3.;
    let [p, q, r] = corners.map(|c| middle + (c - middle) * grow);
    const N: usize = 4;
    let mut rows: Vec<Vec<u32>> = Vec::new();
    for i in 0..=N {
        rows.push(
            (0..=N - i)
                .map(|j| face_vertex(b, p + (q - p) * (j as f32 / N as f32) + (r - p) * (i as f32 / N as f32), lift))
                .collect(),
        );
    }
    for i in 0..N {
        for j in 0..N - i {
            b.triangle(rows[i][j], rows[i][j + 1], rows[i + 1][j]);
            if j + 1 < N - i {
                b.triangle(rows[i][j + 1], rows[i + 1][j + 1], rows[i + 1][j]);
            }
        }
    }
}

/// A carved band across the face: columns at each of `us` (sorted) from `top(u)` down to `bottom(u)`.
fn face_band(b: &mut Builder, us: &[f32], top: impl Fn(f32) -> f32, bottom: impl Fn(f32) -> f32, lift: f32) {
    let columns: Vec<(u32, u32)> = us
        .iter()
        .map(|&u| {
            let (t, d) = (top(u), bottom(u));
            (face_vertex(b, Vec2::new(u, t), lift), face_vertex(b, Vec2::new(u, d.min(t - 1e-3)), lift))
        })
        .collect();
    for w in columns.windows(2) {
        b.quad(w[0].0, w[0].1, w[1].1, w[1].0);
    }
}

/// A convex carved shape, grown `grow` out from its middle for the rim.
fn face_polygon(b: &mut Builder, points: &[Vec2], grow: f32, lift: f32) {
    let middle = points.iter().copied().sum::<Vec2>() / points.len() as f32;
    let inner = (0..points.len())
        .map(|i| {
            let (p, q) = (points[i], points[(i + 1) % points.len()]);
            let e = (q - p).normalize();
            (middle - p).perp_dot(e).abs()
        })
        .fold(f32::MAX, f32::min);
    let grown: Vec<Vec2> = points.iter().map(|&p| middle + (p - middle) * (1. + 0.6 * grow / inner)).collect();
    for i in 0..grown.len() {
        face_triangle(b, [middle, grown[i], grown[(i + 1) % grown.len()]], 1., lift);
    }
}

/// The grin: a crescent with a tooth hanging from the top and one rising from the bottom. `grow` widens it (and
/// narrows the teeth) for the carved rim around the hole.
fn face_mouth(b: &mut Builder, grow: f32, lift: f32) {
    let (half, upper, lower) = (0.56 + grow, (-0.21, -0.07), (0.07, 0.21));
    let tooth = 0.085 - grow;
    let mut us: Vec<f32> = (0..=28).map(|k| -half + 2. * half * k as f32 / 28.).collect();
    for (a, z) in [upper, lower] {
        us.extend([a + grow - 1e-3, a + grow + 1e-3, z - grow - 1e-3, z - grow + 1e-3]);
    }
    us.sort_by(f32::total_cmp);
    // The ends close to a point.
    let close = |u: f32| 1. - (1. - (u / half).clamp(-1., 1.).powi(2)).sqrt();
    let curve = |u: f32, middle: f32, ends: f32| {
        let t = (u / half).clamp(-1., 1.);
        let y = middle + ends * t * t;
        y + (-0.12 - y) * close(u)
    };
    let top = |u: f32| curve(u, -0.2 + grow, 0.1) - if u > upper.0 + grow && u < upper.1 - grow { tooth } else { 0. };
    let bottom = |u: f32| curve(u, -0.42 - grow, 0.29) + if u > lower.0 + grow && u < lower.1 - grow { tooth } else { 0. };
    face_band(b, &us, top, bottom, lift);
}

/// The scowl's mouth: a wide leer with corners turned up and a row of jagged teeth from above and below.
fn face_leer(b: &mut Builder, grow: f32, lift: f32) {
    let half = 0.54 + grow;
    let us: Vec<f32> = (0..=72).map(|k| -half + 2. * half * k as f32 / 72.).collect();
    let close = |u: f32| 1. - (1. - (u / half).clamp(-1., 1.).powi(2)).sqrt();
    // Teeth 0.15 apart, the lower row between the upper.
    let zig = |u: f32| 1. - ((u / 0.15).rem_euclid(1.) - 0.5).abs() * 2.;
    let curve = |u: f32, middle: f32, ends: f32| {
        let t = (u / half).clamp(-1., 1.);
        let y = middle + ends * t * t;
        y + (-0.05 - y) * close(u)
    };
    let top = |u: f32| curve(u, -0.16 + grow, 0.18) - 0.075 * zig(u) * (1. - close(u));
    let bottom = |u: f32| curve(u, -0.43 - grow, 0.34) + 0.075 * zig(u + 0.075) * (1. - close(u));
    face_band(b, &us, top, bottom, lift);
}

/// The carved face. `grow` 0 is the glowing hole, more is the cut flesh around it.
fn pumpkin_face(carving: Carving, grow: f32, lift: f32) -> Mesh {
    let mut b = Builder::default();
    let mirror = |points: &[Vec2]| points.iter().map(|p| Vec2::new(-p.x, p.y)).rev().collect::<Vec<_>>();
    let circle = |at: Vec2, r: Vec2| (0..14).map(|k| at + Vec2::from_angle(TAU * k as f32 / 14.) * r).collect::<Vec<_>>();
    match carving {
        Carving::Grin => {
            let eye = [Vec2::new(-0.46, 0.12), Vec2::new(-0.12, 0.12), Vec2::new(-0.3, 0.42)];
            face_triangle(&mut b, eye, 1. + grow * 4., lift);
            face_triangle(&mut b, eye.map(|p| Vec2::new(-p.x, p.y)), 1. + grow * 4., lift);
            face_triangle(&mut b, [Vec2::new(-0.08, -0.07), Vec2::new(0.08, -0.07), Vec2::new(0., 0.07)], 1. + grow * 6., lift);
            face_mouth(&mut b, grow, lift);
        }
        Carving::Scowl => {
            // Slanted down to the nose, under a heavy brow.
            let eye = [Vec2::new(-0.5, 0.38), Vec2::new(-0.44, 0.1), Vec2::new(-0.15, 0.05), Vec2::new(-0.1, 0.17)];
            face_polygon(&mut b, &eye, grow, lift);
            face_polygon(&mut b, &mirror(&eye), grow, lift);
            face_triangle(&mut b, [Vec2::new(-0.07, -0.06), Vec2::new(0.07, -0.06), Vec2::new(0., 0.04)], 1. + grow * 6., lift);
            face_leer(&mut b, grow, lift);
        }
        Carving::Shock => {
            face_polygon(&mut b, &circle(Vec2::new(-0.27, 0.2), Vec2::new(0.12, 0.14)), grow, lift);
            face_polygon(&mut b, &circle(Vec2::new(0.25, 0.22), Vec2::new(0.1, 0.12)), grow, lift);
            let (rx, ry, cy) = (0.15 + grow, 0.18 + grow, -0.27);
            let us: Vec<f32> = (0..=24).map(|k| -rx * (PI * k as f32 / 24.).cos()).collect();
            let edge = |u: f32| ry * (1. - (u / rx).powi(2)).max(0.).sqrt();
            face_band(&mut b, &us, |u| cy + edge(u), |u| cy - edge(u), lift);
        }
    }
    b.mesh()
}

/// A short, leaning, tapered stem standing in the top dimple.
fn pumpkin_stem() -> Mesh {
    let mut b = Builder::default();
    const SIDES: usize = 7;
    let rings = [(0.6, 0.085, 0.), (0.74, 0.07, 0.02), (0.86, 0.055, 0.06)];
    for &(y, r, lean) in &rings {
        for k in 0..=SIDES {
            let d = ring(k, SIDES);
            b.vertex(Vec3::new(lean, y, 0.) + d * r, d, Vec2::new(k as f32 / SIDES as f32, y));
        }
    }
    for j in 0..rings.len() - 1 {
        for k in 0..SIDES {
            let at = |j: usize, k: usize| (j * (SIDES + 1) + k) as u32;
            b.quad(at(j, k), at(j, k + 1), at(j + 1, k + 1), at(j + 1, k));
        }
    }
    let (y, r, lean) = rings[2];
    let top = b.vertex(Vec3::new(lean, y + 0.01, 0.), Vec3::Y, Vec2::ZERO);
    let cap: Vec<u32> = (0..=SIDES).map(|k| b.vertex(Vec3::new(lean, y, 0.) + ring(k, SIDES) * r, Vec3::Y, Vec2::ZERO)).collect();
    for k in 0..SIDES {
        b.triangle(top, cap[k], cap[k + 1]);
    }
    b.mesh()
}

/// How many flickers the candles share (each lantern's face and ground glow use one group's materials).
const FLAMES: usize = 6;

fn spawn_pumpkins(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    unlit: &mut Assets<StandardMaterial>,
    lit: &mut Assets<PropMaterial>,
    pool_texture: Handle<Image>,
    pumpkins: &[Pumpkin],
) {
    let (body, stem) = (meshes.add(pumpkin_body()), meshes.add(pumpkin_stem()));
    let faces = CARVINGS.map(|c| (meshes.add(pumpkin_face(c, 0., 0.008)), meshes.add(pumpkin_face(c, 0.035, 0.004))));
    // Two skins, so a row of them is not all one orange.
    let skins = [lit_material(lit, [0.93, 0.42, 0.04], 0.55, 0.), lit_material(lit, [0.85, 0.33, 0.03], 0.6, 0.)];
    let stalk = lit_material(lit, [0.28, 0.22, 0.09], 0.9, 0.);
    // The cut flesh inside the holes, lit by the candle.
    let flesh = unlit.add(glow([0.45, 0.16, 0.03], 1.));
    let flames: Vec<Flame> =
        (0..FLAMES).map(|_| Flame { face: unlit.add(glow([5., 2.1, 0.3], 1.)), pool: pool_material(unlit, pool_texture.clone()) }).collect();
    let ground = pool(meshes, 1.);
    for (k, p) in pumpkins.iter().enumerate() {
        let group = k % FLAMES;
        let r = p.size;
        let rotation = Quat::from_rotation_y(p.yaw);
        // The body rests on the ring round its bottom dimple, about 0.68 radii below its middle.
        let at = p.at + Vec3::Y * (0.66 * r);
        let mut root = commands.spawn((
            Name::new("Jack-o'-lantern"),
            Decor,
            crate::map_render::MapEntity,
            Transform::from_translation(at).with_rotation(rotation).with_scale(Vec3::splat(r)),
            Visibility::default(),
        ));
        root.with_children(|c| {
            c.spawn((Mesh3d(body.clone()), MeshMaterial3d(skins[k % 2].clone())));
            c.spawn((Mesh3d(stem.clone()), MeshMaterial3d(stalk.clone())));
            if let Some(carving) = p.carving {
                let (hole, rim) = &faces[CARVINGS.iter().position(|&c| c == carving).unwrap_or(0)];
                c.spawn((Mesh3d(rim.clone()), MeshMaterial3d(flesh.clone()), NotShadowCaster));
                c.spawn((Mesh3d(hole.clone()), MeshMaterial3d(flames[group].face.clone()), NotShadowCaster));
            }
        });
        if !p.spill {
            continue;
        }
        // The candle's glow spills forward onto the ground (the world shader takes no point lights).
        commands.spawn((
            Name::new("Jack-o'-lantern glow"),
            Decor,
            crate::map_render::MapEntity,
            Mesh3d(ground.clone()),
            MeshMaterial3d(flames[group].pool.clone()),
            p.pool(),
            NotShadowCaster,
        ));
    }
    commands.insert_resource(Flames(flames));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_real_days_turn_the_modes_on() {
        assert_eq!(on(10, 31), Some(Season::Halloween));
        assert_eq!(on(12, 24), Some(Season::Christmas));
        assert_eq!(on(12, 25), Some(Season::Christmas));
        assert_eq!(on(10, 30), None);
        assert_eq!(on(12, 26), None);
        assert_eq!(on(1, 1), None);
    }

    #[test]
    fn seasons_round_trip_their_weather_rows() {
        for season in [Season::Halloween, Season::Christmas] {
            assert_eq!(Season::from_weather(season.weather()), Some(season));
        }
        assert!((0..5).all(|w| Season::from_weather(w).is_none()));
    }

    #[test]
    fn the_sky_tint_fades_with_distance_from_the_park() {
        assert_eq!(nearness(Vec3::new(330., 133., -710.)), 1.);
        let edge = nearness(Vec3::new(330., 133., -894.));
        assert!(edge > 0. && edge < 1., "{edge}");
        assert_eq!(nearness(Vec3::new(330., 133., -1100.)), 0.);
        // The cut-out corner is outside.
        assert!(nearness(Vec3::new(150., 133., -560.)) < 1.);
    }

    #[test]
    fn a_pumpkin_face_hugs_the_skin_and_faces_forward() {
        let mut b = Builder::default();
        let i = face_vertex(&mut b, Vec2::ZERO, 0.);
        let p = Vec3::from(b.positions[i as usize]);
        assert!(p.z > 0.85 && p.x.abs() < 1e-4 && p.y.abs() < 1e-4, "{p}");
        let mesh = pumpkin_face(Carving::Grin, 0., 0.008);
        assert!(mesh.indices().is_some_and(|i| i.len() > 100));
    }

    fn house(k: usize) -> place::Placed {
        place::Placed { zone: layout::Zone::MainFloor, kind: Kind::House, at: Vec3::new(10. * k as f32, 0., 0.), yaw: 0., scale: 1., seed: k as u32 }
    }

    #[test]
    fn the_point_lights_stay_within_budget() {
        // Pumpkin k stands at z = k.
        let pumpkins: Vec<Pumpkin> = (0..200)
            .map(|k| Pumpkin { at: Vec3::new(0., 0., k as f32), yaw: 0., size: 0.2 + 0.001 * k as f32, light: true, carving: Some(Carving::Grin), spill: true })
            .collect();
        // 20 houses along x (3 floods each, 60 in all) and 200 lit pumpkins: every one is a lamp.
        let plan = place::Plan { pieces: (0..20).map(house).collect(), pumpkins, ..default() };
        let l = lamps(&plan, Season::Halloween);
        assert_eq!((l.len(), l.iter().filter(|x| x.candle.is_some()).count()), (260, 200));
        // Not retail (user 10-20): wherever the camera is, the nearest POINT_LIGHTS shine (the nearest fully) and no
        // farther one, so no part of the park is left dark by lamps elsewhere taking the budget.
        let at: Vec<Vec3> = l.iter().map(|x| x.at).collect();
        for eye in [Vec3::ZERO, Vec3::new(190., 0., 0.), Vec3::new(0., 0., 199.)] {
            let s = shares(&at, eye);
            let on: Vec<usize> = (0..at.len()).filter(|&k| s[k] > 0.).collect();
            assert!(!on.is_empty() && on.len() <= POINT_LIGHTS, "{} lamps on at {eye}", on.len());
            let nearest = (0..at.len()).min_by(|&a, &b| at[a].distance(eye).total_cmp(&at[b].distance(eye))).unwrap();
            assert_eq!(s[nearest], 1., "at {eye}");
            let reach = on.iter().map(|&k| at[k].distance(eye)).fold(0., f32::max);
            assert!((0..at.len()).filter(|&k| s[k] == 0.).all(|k| at[k].distance(eye) >= reach), "a farther lamp shines at {eye}");
        }
        // 10 cm on, no lamp turns up or down by more than a little: the farthest fade out, they do not pop.
        let (a, b) = (shares(&at, Vec3::new(50., 0., 20.)), shares(&at, Vec3::new(50., 0., 20.1)));
        assert!(a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 0.05));
        // A few lamps all shine.
        assert_eq!(shares(&at[..10], Vec3::ZERO), vec![1.; 10]);
    }

    /// One placed piece of every kind, a strand of every style, a web with its spider, a flock and a roost.
    fn every_kind() -> place::Plan {
        use layout::{Palette, Style};
        let kinds = [
            Kind::House,
            Kind::Witch,
            Kind::Spider,
            Kind::DrapedSpider,
            Kind::Ghost,
            Kind::HangingGhost,
            Kind::Graveyard,
            Kind::Hay,
            Kind::Cauldron,
            Kind::Broom,
            Kind::Skeleton,
            Kind::EggSac,
            Kind::LightPole,
            Kind::PumpkinTower,
            Kind::BatBanner,
            Kind::CryptLantern,
            Kind::Fence,
            Kind::Pumpkin,
            Kind::Mist,
            Kind::Uplight,
            Kind::BigTree,
            Kind::Snowman,
            Kind::CandyCane,
            Kind::Nutcracker,
            Kind::Wreath,
            Kind::Presents,
            Kind::Gingerbread,
            Kind::Sleigh,
            Kind::Reindeer,
            Kind::SnowGlobe,
            Kind::Sled,
            Kind::Lantern,
            Kind::CarSpider,
        ];
        let pieces = kinds
            .iter()
            .enumerate()
            .map(|(i, &kind)| place::Placed { zone: ZONES[i % ZONES.len()], kind, at: Vec3::new(300. + 8. * i as f32, 100., -700.), yaw: 0.3 * i as f32, scale: 1., seed: i as u32 })
            .collect();
        let styles = [
            (Style::Swag, Palette::Halloween),
            (Style::Taut, Palette::Warm),
            (Style::Icicles, Palette::Ice),
            (Style::Spiral { radius: 2., top: 0.5, turns: 5. }, Palette::Multi),
        ];
        let strands = styles
            .into_iter()
            .enumerate()
            .map(|(i, (style, palette))| {
                let foot = Vec3::new(300., 104., -690. - 5. * i as f32);
                let path = if matches!(style, Style::Spiral { .. }) { vec![foot, foot + Vec3::Y * 8.] } else { vec![foot, foot + Vec3::X * 10.] };
                place::Strand { zone: ZONES[i], path, style, palette, garland: i == 1 }
            })
            .collect();
        let frame = vec![Vec3::new(300., 100., -720.), Vec3::new(310., 100., -720.), Vec3::new(305., 108., -720.)];
        place::Plan {
            pieces,
            pumpkins: vec![
                Pumpkin { at: Vec3::new(305., 100., -705.), yaw: 0., size: 0.3, light: true, carving: Some(Carving::Scowl), spill: true },
                Pumpkin { at: Vec3::new(306., 100., -705.), yaw: 1., size: 0.25, light: false, carving: None, spill: false },
            ],
            strands,
            webs: vec![place::Web { zone: layout::Zone::DeepPit, frame, spider: true, seed: 3, danglers: vec![(Vec3::new(305., 103., -720.), 1.2)] }],
            glows: vec![(Transform::from_xyz(300., 100.03, -700.).with_scale(Vec3::new(4., 1., 4.)), [0.3, 1., 0.12])],
            flocks: vec![Vec3::new(300., 112., -700.)],
            roosts: vec![Vec3::new(305., 106., -700.)],
            dropped: Vec::new(),
        }
    }

    #[test]
    fn everything_spawned_is_cleared_with_its_decor() {
        use bevy::ecs::system::RunSystemOnce;
        for season in [Season::Halloween, Season::Christmas] {
            let mut world = World::new();
            world.init_resource::<Assets<Mesh>>();
            world.init_resource::<Assets<StandardMaterial>>();
            world.init_resource::<Assets<PropMaterial>>();
            world.init_resource::<Assets<Image>>();
            world
                .run_system_once(
                    move |mut commands: Commands,
                          mut meshes: ResMut<Assets<Mesh>>,
                          mut unlit: ResMut<Assets<StandardMaterial>>,
                          mut lit: ResMut<Assets<PropMaterial>>,
                          mut images: ResMut<Assets<Image>>| {
                        spawn_plan(&mut commands, &mut meshes, &mut unlit, &mut lit, &mut images, &every_kind(), season);
                    },
                )
                .unwrap();
            let spawned: Vec<Entity> = world.query_filtered::<Entity, Or<(With<Mesh3d>, With<PointLight>)>>().iter(&world).collect();
            let lights = world.query::<&PointLight>().iter(&world).count();
            assert!(spawned.len() > 50 && lights > 0, "{season:?}: {} meshes and lights, {lights} lights", spawned.len());
            let decor = |mut e: Entity| loop {
                if world.get::<Decor>(e).is_some() {
                    return true;
                }
                match world.get::<ChildOf>(e) {
                    Some(c) => e = c.parent(),
                    None => return false,
                }
            };
            assert!(spawned.into_iter().all(decor), "{season:?}");
        }
    }

    /// Every scene of the season's layout lands on the installed park whole, fast, and as full as "way more" asks.
    #[test]
    #[ignore = "requires SKATE_UNIVERSITY_MAP (an installed University.skate)"]
    fn the_installed_park_gets_every_scene() {
        use skate_core::physics::contact::RetailContactMaterial;
        use std::time::{Duration, Instant};
        let path = std::env::var("SKATE_UNIVERSITY_MAP").expect("SKATE_UNIVERSITY_MAP");
        let map = skate_data::skate_map::SkateMap::load(std::path::Path::new(&path)).unwrap();
        let material = RetailContactMaterial { static_friction: 0., dynamic_friction: 0., restitution: 0. };
        let world = crate::skate_world::collision_world(&map, material).unwrap();
        let lights = |plan: &place::Plan| -> f32 {
            plan.strands.iter().map(|s| strung::wire(&s.path, s.style).windows(2).map(|w| w[0].distance(w[1])).sum::<f32>()).sum()
        };
        let resolved = |season: Season| {
            let started = Instant::now();
            let plan = place::resolve(&world, layout::layout(season));
            let took = started.elapsed();
            for line in &plan.dropped {
                println!("{season:?} dropped {line}");
            }
            assert!(took < Duration::from_secs(1), "{season:?} took {took:?}");
            assert!(plan.dropped.is_empty(), "{season:?}: {} dropped", plan.dropped.len());
            for zone in ZONES {
                let pieces = plan.pieces.iter().filter(|p| p.zone == zone).count();
                assert!(pieces >= 12, "{season:?} {zone:?}: {pieces} pieces");
                assert!(plan.strands.iter().any(|s| s.zone == zone), "{season:?} {zone:?}: no lights");
            }
            let lit = lights(&plan);
            // The first pass had 24 strings of 8 m.
            assert!(lit >= 600., "{season:?}: {lit:.0} m of lights");
            plan
        };
        // Not retail (user 10-20): every zone has pieces that light it.
        let dark = |plan: &place::Plan, season: Season| -> Vec<String> {
            ZONES
                .iter()
                .filter(|&&zone| !plan.pieces.iter().any(|p| p.zone == zone && !floods(p.kind, season).is_empty()))
                .map(|zone| format!("{season:?} {zone:?}: no lamps"))
                .collect()
        };
        let plan = resolved(Season::Halloween);
        let mut missing = dark(&plan, Season::Halloween);
        let (webs, flocks, roosts, pumpkins) = (plan.webs.len(), plan.flocks.len(), plan.roosts.len(), plan.pumpkins.len());
        assert!(webs >= 14 && flocks >= 14 && roosts >= 30, "{webs} webs, {flocks} flocks, {roosts} roosting bats");
        assert!(pumpkins >= 150, "{pumpkins} pumpkins");
        // Not retail (user 10-20): "a lot more things", off the lines: every zone at least 20 pieces, and the park's
        // totals well over the first layout's (123 pieces, 199 pumpkins, 743 m of lights, 16 flocks, 33 roosts).
        let lit = lights(&plan);
        for zone in ZONES {
            let pieces = plan.pieces.iter().filter(|p| p.zone == zone).count();
            assert!(pieces >= 20, "Halloween {zone:?}: {pieces} pieces");
        }
        assert!(plan.pieces.len() >= 200 && pumpkins >= 300, "{} pieces, {pumpkins} pumpkins", plan.pieces.len());
        assert!(lit >= 1100. && flocks >= 20 && roosts >= 40, "{lit:.0} m of lights, {flocks} flocks, {roosts} roosting bats");
        assert!(plan.pieces.iter().any(|p| p.kind == Kind::CarSpider), "the spider on the red SUV");

        // Christmas (Task 9): the same scene checks, the spec's set pieces, and the Halloween park's density.
        let plan = resolved(Season::Christmas);
        use layout::Zone::{DeepPit, SouthGate, SouthRim, WestGate};
        let count = |kind: Kind, zone: Option<layout::Zone>| {
            plan.pieces.iter().filter(|p| p.kind == kind && zone.is_none_or(|z| p.zone == z)).count()
        };
        let snowmen = (count(Kind::Snowman, None), count(Kind::Snowman, Some(SouthRim)), count(Kind::Snowman, Some(DeepPit)));
        assert!(snowmen.0 >= 20 && snowmen.1 >= 12 && snowmen.2 >= 4, "snowmen (all, south rim, pit): {snowmen:?}");
        assert!(count(Kind::CandyCane, None) >= 30, "{} candy canes", count(Kind::CandyCane, None));
        assert_eq!(count(Kind::BigTree, None), 1, "the big tree");
        assert_eq!(count(Kind::Nutcracker, Some(WestGate)), 2, "the west gate's nutcrackers");
        for gate in [WestGate, SouthGate] {
            assert!(count(Kind::Wreath, Some(gate)) >= 1, "{gate:?}: no wreath");
        }
        let lit = lights(&plan);
        for zone in ZONES {
            let pieces = plan.pieces.iter().filter(|p| p.zone == zone).count();
            assert!(pieces >= 20, "Christmas {zone:?}: {pieces} pieces");
        }
        assert!(plan.pieces.len() >= 200 && lit >= 1100., "{} pieces, {lit:.0} m of lights", plan.pieces.len());
        // Not retail (user 10-20): nothing stands in the big tree's boughs; the spec's sleds are out.
        let tree = plan.pieces.iter().find(|p| p.kind == Kind::BigTree).expect("the big tree");
        let (reach, height) = Kind::BigTree.room();
        for p in plan.pieces.iter().filter(|p| p.kind != Kind::BigTree) {
            let (d, up) = (Vec2::new(p.at.x - tree.at.x, p.at.z - tree.at.z).length(), p.at.y - tree.at.y);
            if d < reach * tree.scale + p.kind.room().0 * p.scale && up > -1. && up < height * tree.scale {
                missing.push(format!("{:?} at {} stands in the big tree's boughs", p.kind, p.at));
            }
        }
        if count(Kind::Sled, None) < 2 {
            missing.push(format!("{} sleds", count(Kind::Sled, None)));
        }
        missing.extend(dark(&plan, Season::Christmas));
        assert!(missing.is_empty(), "{missing:#?}");
    }
}
