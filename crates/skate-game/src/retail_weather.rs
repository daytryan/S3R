//! Visual-only rain and snow on retail maps: grey sky, fog, wet ground and puddles
//! or a snow layer, falling rain or snow, drops on the lens, and
//! heavy-rain lightning. Gameplay never reads it. Shading lives in retail_weather.wgsl.
use super::{RetailScene, ShadowState};
use bevy::{
    asset::{AssetPath, embedded_asset, embedded_path},
    camera::visibility::NoFrustumCulling,
    light::{CascadeShadowConfigBuilder, NotShadowCaster, NotShadowReceiver},
    pbr::{ExtendedMaterial, MaterialExtension},
    prelude::*,
    render::render_resource::AsBindGroup,
    shader::ShaderRef,
};

/// Streak quads animated entirely on the GPU; light rain draws a subset.
const DROPS: u32 = 12_000;
/// Overcast horizon: fog, cloud and puddle-reflection colour (scene units).
const OVERCAST: Vec3 = Vec3::new(0.36, 0.38, 0.41);
/// Snow haze: paler than rain cloud.
const SNOW_HAZE: Vec3 = Vec3::new(0.5, 0.53, 0.57);

/// Camera fog colour, opacity and density for the weather; transparent when clear,
/// thickest in heavy snow. Opacity follows `cover` in retail_weather.wgsl.
/// Not retail (user 10-19): the Christmas mode's snow (`christmas`) hazes lighter, so the far side of the Super Ultra
/// Mega Park (about 380 m) stays in sight.
fn fog(rain: f32, snow: f32, christmas: bool) -> (Vec3, f32, f32) {
    if snow > 0. && christmas {
        (SNOW_HAZE, 0.55, 0.0018)
    } else if snow > 0. {
        (SNOW_HAZE, 0.85 * (snow * 2.).min(1.), 0.001 + 0.006 * snow)
    } else {
        (OVERCAST, 0.85 * (rain * 2.).min(1.), 0.0032 + 0.0033 * rain)
    }
}

/// How much snow falls: heavy snow falls in full; any other lays its layer without falling, except the Christmas
/// mode's (not retail, user 10-19), which falls gently (retail_rain.wgsl draws 0.8 × this of its flakes).
fn snowfall(snow: f32, christmas: bool) -> f32 {
    if snow >= 1. {
        1.
    } else if christmas && snow > 0. {
        0.5
    } else {
        0.
    }
}

/// Not retail (user 10-14, feature #5): the Fog setting's (opacity, metres to half that) in place of the weather's
/// own thickness. Every fog shader thickens as `a·(1 − exp(−d·k·(d + 600)/700))`, so k puts the half-way at `half`.
fn chosen_fog(weather: (Vec3, f32, f32), choice: Option<(f32, f32)>) -> (Vec3, f32, f32) {
    match choice {
        Some((opacity, half)) => (weather.0, opacity, 700. * std::f32::consts::LN_2 / (half * (half + 600.))),
        None => weather,
    }
}

/// Not retail (user 10-19): the Halloween mode's night mist (seasonal.rs): colour (darkened with the sky like the
/// rest), opacity and metres to half of it.
const HALLOWEEN_FOG: (Vec3, f32, f32) = (Vec3::new(0.5, 0.45, 0.66), 0.92, 55.);

/// The weather's fog thickened to the Halloween mist as the camera nears the Super Ultra Mega Park (`near` 1 in it).
fn halloween_fog(weather: (Vec3, f32, f32), near: f32) -> (Vec3, f32, f32) {
    let (_, opacity, density) = chosen_fog(weather, Some((HALLOWEEN_FOG.1, HALLOWEEN_FOG.2)));
    (weather.0.lerp(HALLOWEEN_FOG.0, near), weather.1 + (opacity - weather.1) * near, weather.2 + (density - weather.2) * near)
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub(crate) struct RainMaterial {
    /// x: rain strength, y: snowfall.
    #[uniform(0)]
    settings: Vec4,
}
impl Material for RainMaterial {
    fn vertex_shader() -> ShaderRef {
        AssetPath::from(embedded_path!("retail_rain.wgsl")).with_source("embedded").into()
    }
    fn fragment_shader() -> ShaderRef {
        Self::vertex_shader()
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
        overlay(descriptor);
        Ok(())
    }
}

/// Transparent, double-sided overlays (rain, snow, wheel effects) that never hide
/// what is behind them.
pub(super) fn overlay(descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor) {
    descriptor.primitive.cull_mode = None;
    if let Some(depth) = &mut descriptor.depth_stencil {
        depth.depth_write_enabled = false;
    }
}

/// `count` quads for GPU-placed particles: corner in xy (x -1..1, y 0..1), index in z.
pub(super) fn quads(count: u32) -> Mesh {
    let positions: Vec<[f32; 3]> = (0..count)
        .flat_map(|d| [[-1., 0.], [1., 0.], [1., 1.], [-1., 1.]].map(|[x, y]| [x, y, d as f32]))
        .collect();
    let indices: Vec<u32> = (0..count).flat_map(|d| [0, 1, 2, 0, 2, 3].map(|i| d * 4 + i)).collect();
    Mesh::new(bevy::mesh::PrimitiveTopology::TriangleList, bevy::asset::RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_indices(bevy::mesh::Indices::U32(indices))
}

/// PBR props (benches, rails, tables): StandardMaterial lighting plus the weather
/// snow layer read from the shared frame state. Bindless like StandardMaterial.
pub(crate) type PropMaterial = ExtendedMaterial<StandardMaterial, PropWeather>;

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
#[bindless(index_table(range(50..51), binding(100)))]
pub(crate) struct PropWeather {
    #[storage(50, read_only, binding_array(101))]
    frame_state: Handle<bevy::render::storage::ShaderStorageBuffer>,
}
impl Default for PropWeather {
    fn default() -> Self {
        Self { frame_state: super::shadow::BUFFER }
    }
}
impl MaterialExtension for PropWeather {
    fn fragment_shader() -> ShaderRef {
        AssetPath::from(embedded_path!("retail_prop.wgsl")).with_source("embedded").into()
    }
}

#[derive(Component)]
struct Rain;

/// Straight-down, shadow-only light that exists while rain or snow falls;
/// retail_rain.wgsl hides drops with a caster above them (roofs, bridges, trees).
#[derive(Component)]
struct Shelter;

fn shelter_light() -> impl Bundle {
    (
        Shelter,
        Name::new("Rain shelter"),
        DirectionalLight {
            illuminance: 0.,
            shadows_enabled: true,
            affects_lightmapped_mesh_diffuse: false,
            // Shaders find this light by normal bias 0 among non-lightmapped shadow
            // lights (the character visibility light keeps the default 1.8).
            shadow_normal_bias: 0.,
            ..default()
        },
        Transform::default().looking_to(Vec3::NEG_Y, Vec3::X),
        // ponytail: one 20 m cascade (~54 m wide map, ~5 cm texels) covers the visible
        // drops; farther drops always fall. Raise maximum_distance if that shows.
        CascadeShadowConfigBuilder {
            num_cascades: 1,
            maximum_distance: 20.,
            ..default()
        }
        .build(),
    )
}

/// Seconds a strike lasts.
const STRIKE: f32 = 0.7;
/// Brightness `age` seconds into a strike: three quick flickers, the last fading out.
fn flash(age: f32) -> f32 {
    if !(0. ..STRIKE).contains(&age) {
        return 0.;
    }
    [(0., 1.), (0.13, 0.6), (0.27, 0.9)]
        .into_iter()
        .filter(|&(start, _)| age >= start)
        .map(|(start, peak)| peak * (-(age - start) / 0.06).exp())
        .fold(0., f32::max)
}

/// Reduce Flashing (online safety Q6): a strike's flicker is held under this brightness. Knob.
// ponytail: a cap on the same three flickers, not a smooth glow; small enough not to read as flashing.
const CALM_FLASH: f32 = 0.1;
fn calm(flash: f32, reduce: bool) -> f32 {
    if reduce { flash.min(CALM_FLASH) } else { flash }
}

/// Heavy-rain lightning: a strike every 10-30 s somewhere in the sky ahead.
#[derive(Resource)]
struct Storm {
    rng: u64,
    wait: f32,
    age: Option<f32>,
    /// Radians from the camera's heading when the strike starts; ahead, so it is seen.
    bearing: f32,
    seed: f32,
    /// How far away the strike is (1-4 km): its thunder comes `distance_m / 343` s after the flash.
    distance_m: f32,
}
impl Storm {
    fn new(seed: u64) -> Self {
        let mut storm = Self { rng: seed | 1, wait: 0., age: None, bearing: 0., seed: 0., distance_m: 1000. };
        // The first strike comes soon after the storm starts.
        storm.wait = 4. + 8. * storm.random();
        storm
    }
    /// xorshift64*, uniform in 0..1.
    fn random(&mut self) -> f32 {
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        (self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32 / (1u64 << 24) as f32
    }
    /// Advance by `dt` seconds and return this frame's flash (0..1).
    fn step(&mut self, dt: f32, stormy: bool) -> f32 {
        if !stormy {
            self.age = None;
            return 0.;
        }
        self.wait -= dt;
        if self.wait <= 0. {
            self.wait += 10. + 20. * self.random();
            self.age = Some(0.);
            self.bearing = (self.random() - 0.5) * 2.1;
            self.seed = (self.random() * 100.).floor();
            self.distance_m = 1000. + 3000. * self.random();
        } else if let Some(age) = &mut self.age {
            *age += dt;
        }
        let brightness = self.age.map_or(0., flash);
        if self.age.is_some_and(|age| age >= STRIKE) {
            self.age = None;
        }
        brightness
    }
}

pub(super) fn install(app: &mut App) {
    embedded_asset!(app, "retail_rain.wgsl");
    embedded_asset!(app, "retail_prop.wgsl");
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |d| d.as_nanos() as u64);
    app.add_plugins((MaterialPlugin::<RainMaterial>::default(), MaterialPlugin::<PropMaterial>::default()))
        .insert_resource(Storm::new(seed))
        .add_systems(Startup, spawn_rain)
        .add_message::<LightningStrike>()
        .add_systems(Update, apply.in_set(WeatherSet));
}

/// A lightning strike as it flashes: world azimuth (`atan2(z, x)`, as the bolt), distance and seed. Weather audio
/// turns it into thunder after the speed-of-sound delay.
#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub(crate) struct LightningStrike {
    pub azimuth: f32,
    pub distance_m: f32,
    pub seed: u32,
}

/// The rain level the screen shows (0, 0.5 light, 1 heavy), gated like the shaders: shared with weather audio.
pub(crate) fn rain_level(
    menu: Option<&crate::graphics_menu::Menu>,
    retail: Option<&RetailScene>,
    customiser: Option<&crate::customiser::Customiser>,
) -> f32 {
    menu.map_or(0., |m| strength(m.rain(), retail.is_some_and(|r| r.0), customiser.is_some_and(|c| c.open)))
}

/// Sets this frame's weather; effects that follow it run after.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct WeatherSet;

fn spawn_rain(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<RainMaterial>>) {
    // retail_rain.wgsl places every drop.
    let mesh = quads(DROPS);
    // No Aabb: like the sky dome, never frustum or occlusion culled.
    commands.spawn((
        Name::new("Rain"),
        Rain,
        Mesh3d(meshes.add(mesh)),
        MeshMaterial3d(materials.add(RainMaterial { settings: Vec4::ZERO })),
        Transform::default(),
        Visibility::Hidden,
        NoFrustumCulling,
        NotShadowCaster,
        NotShadowReceiver,
    ));
}

/// In a replay: the recorded rain at the playhead, the flash of a recorded strike under way, and that strike's
/// azimuth and seed. None outside a replay.
fn recorded(replay: &crate::replay::Replay) -> Option<(f32, f32, Option<(f32, f32)>)> {
    let (index, now) = replay.playhead()?;
    let rain = replay.weather_at(index).map_or(0., |w| w.1);
    let strike = replay.frames().strikes().filter(|&(t, _)| (0. ..f64::from(STRIKE)).contains(&(now - t))).last();
    Some(match strike {
        Some((t, s)) => (rain, flash((now - t) as f32), Some((s.azimuth, s.seed as f32))),
        None => (rain, 0., None),
    })
}

/// Seconds the drops have fallen past the game clock: a Skate 2 tutorial scene holds play (virtual time stands still)
/// and runs on real time at most 0.1 s a frame (cutscene.rs); the rain falls on with it. Not retail (the weather is the
/// port's; user 10-13: in cutscenes the rain "freezes in the air").
fn fall_offset(offset: f32, game_dt: f32, real_dt: f32, cutscene: bool) -> f32 {
    if cutscene && game_dt == 0. { offset + real_dt.min(0.1) } else { offset }
}

/// Rain strength the shaders should show: none off retail maps or in the indoor
/// Edit Skater room, which shares the retail world shader.
fn strength(setting: f32, retail: bool, editing_skater: bool) -> f32 {
    if retail && !editing_skater { setting } else { 0. }
}

#[allow(clippy::too_many_arguments)]
fn apply(
    mut commands: Commands,
    time: Res<Time>,
    menu: Option<Res<crate::graphics_menu::Menu>>,
    retail: Option<Res<RetailScene>>,
    customiser: Option<Res<crate::customiser::Customiser>>,
    mut storm: ResMut<Storm>,
    mut bolt: Local<f32>,
    mut state: ResMut<ShadowState>,
    mut tone: ResMut<super::exposure::Settings>,
    mut cameras: Query<(Entity, &GlobalTransform, Option<&mut DistanceFog>), With<Camera3d>>,
    mut rain_materials: ResMut<Assets<RainMaterial>>,
    mut drops: Query<(&MeshMaterial3d<RainMaterial>, &mut Visibility), With<Rain>>,
    shelters: Query<Entity, With<Shelter>>,
    look: Res<super::DayLook>,
    mut strikes: MessageWriter<LightningStrike>,
    (real, cutscene, mut fallen, replay): (
        Res<Time<Real>>,
        Option<Res<crate::challenge::cutscene::Cutscene>>,
        Local<f32>,
        Option<Res<crate::replay::Replay>>,
    ),
) {
    *fallen = fall_offset(*fallen, time.delta_secs(), real.delta_secs(), cutscene.is_some_and(|c| c.active()));
    let (on, editing) = (retail.is_some_and(|r| r.0), customiser.is_some_and(|c| c.open));
    let (rain, snow) = menu.as_ref().map_or((0., 0.), |m| (strength(m.rain(), on, editing), strength(m.snow(), on, editing)));
    let fog_choice = menu.as_ref().and_then(|m| m.fog()).filter(|_| on && !editing);
    // Not retail (user 10-19): the Christmas weather on every map (spec lead call); it only falls where it lies.
    let christmas = menu.as_ref().is_some_and(|m| m.season() == Some(crate::seasonal::Season::Christmas));
    let snowfall = snowfall(snow, christmas);
    let mut seed = storm.seed;
    let (rain, flash) = match replay.as_deref().and_then(recorded) {
        // A replay shows the recording's rain and lightning, as weather audio plays them; the live storm holds.
        Some((recorded_rain, flash, strike)) => {
            storm.step(time.delta_secs(), false);
            if let Some((azimuth, strike_seed)) = strike {
                (*bolt, seed) = (azimuth, strike_seed);
            }
            (strength(recorded_rain, on, editing), flash)
        }
        None => (rain, storm.step(time.delta_secs(), rain >= 1.)),
    };
    let flash = calm(flash, menu.as_ref().is_some_and(|m| m.reduce_flashing()));
    if storm.age == Some(0.) {
        // Fix the bolt in the world where it starts, relative to the view.
        if let Some((_, view, _)) = cameras.iter().next() {
            let ahead = view.forward();
            *bolt = ahead.z.atan2(ahead.x) + storm.bearing;
        }
        strikes.write(LightningStrike { azimuth: *bolt, distance_m: storm.distance_m, seed: storm.seed as u32 });
    }
    // World, sky and prop shaders read the shared frame state; the final screen
    // pass draws the flash and what lands on the lens.
    state.3 = Vec4::new(rain, flash, *bolt, seed);
    // z and w are seasonal.rs's (the Halloween and Christmas modes).
    (state.4.x, state.4.y) = (snow, snowfall);
    tone.weather = Vec4::new(rain, flash, time.elapsed_secs() + *fallen, snowfall);
    // One fog for every shader: retail world, sky horizon, PBR props and
    // characters. The world and characters thicken it with distance, matching
    // props at 100 m.
    // Halloween's mist (seasonal.rs sets snow.z and .w after this, so a frame behind).
    let own = fog(rain, snow, christmas);
    let weather = if state.4.z == 1. { halloween_fog(own, state.4.w) } else { own };
    let (colour, opacity, density) = chosen_fog(weather, fog_choice);
    // Cloud, rain and spray darken with the sky at night (retail_daynight.rs).
    let colour = colour * look.sky;
    let fog = DistanceFog {
        color: Color::linear_rgba(colour.x, colour.y, colour.z, opacity),
        falloff: FogFalloff::Exponential { density },
        ..default()
    };
    for (camera, _, current) in &mut cameras {
        match current {
            // Only this system writes the fog (colour and density): rewriting it unchanged fired change detection on
            // every camera every frame (perf 10-10 #7).
            Some(mut current) => {
                if current.color != fog.color || !matches!(current.falloff, FogFalloff::Exponential { density: d } if d == density) {
                    *current = fog.clone();
                }
            }
            None => {
                commands.entity(camera).insert(fog.clone());
            }
        }
    }
    let falling = Vec4::new(rain, snowfall, 0., 0.);
    // The shelter light's shadow pass only costs while something falls.
    if falling == Vec4::ZERO {
        for shelter in &shelters {
            commands.entity(shelter).despawn();
        }
    } else if shelters.is_empty() {
        commands.spawn(shelter_light());
    }
    // z: the held-scene offset on the shader's clock (changes only while a scene holds play).
    let settings = falling.with_z(*fallen);
    for (material, mut visibility) in &mut drops {
        visibility.set_if_neq(if falling != Vec4::ZERO { Visibility::Visible } else { Visibility::Hidden });
        if rain_materials.get(material).is_some_and(|m| m.settings != settings) {
            rain_materials.get_mut(material).unwrap().settings = settings;
        }
    }
}

/// The storm for weather audio's tests.
#[cfg(test)]
pub(crate) mod tests_api {
    pub(crate) struct TestStorm(super::Storm);
    pub(crate) fn storm(seed: u64) -> TestStorm {
        TestStorm(super::Storm::new(seed))
    }
    pub(crate) fn recorded(replay: &crate::replay::Replay) -> Option<(f32, f32, Option<(f32, f32)>)> {
        super::recorded(replay)
    }
    impl TestStorm {
        pub(crate) fn step(&mut self, dt: f32, stormy: bool) -> f32 {
            self.0.step(dt, stormy)
        }
        pub(crate) fn age(&self) -> Option<f32> {
            self.0.age
        }
        pub(crate) fn distance_m(&self) -> f32 {
            self.0.distance_m
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Storm, calm, chosen_fog, fall_offset, flash, fog, shelter_light, snowfall, strength};
    use bevy::prelude::*;
    /// Q6: with Reduce Flashing a strike never flickers above CALM_FLASH; off it is retail's.
    #[test]
    fn reduce_flashing_holds_lightning_down() {
        let peak = (0..70).map(|i| flash(i as f32 / 100.)).fold(0., f32::max);
        assert!(peak > 0.9 && calm(peak, false) == peak);
        assert!((0..70).all(|i| calm(flash(i as f32 / 100.), true) <= super::CALM_FLASH));
    }
    #[test]
    fn the_fog_setting_sets_thickness_and_distance() {
        let shaded = |a: f32, k: f32, d: f32| a * (1. - (-d * k * (d + 600.) / 700.).exp());
        let (_, a, k) = chosen_fog(fog(0., 0., false), Some((0.7, 150.)));
        assert_eq!(a, 0.7);
        assert!((shaded(a, k, 150.) - 0.35).abs() < 1e-4, "half as thick at its distance");
        assert_eq!(chosen_fog(fog(1., 0., false), None), fog(1., 0., false), "Weather: the weather's own fog");
        assert_eq!(chosen_fog(fog(1., 0., false), Some((0., 300.))).1, 0., "Off");
    }
    #[test]
    fn the_halloween_mist_thickens_toward_the_park() {
        let clear = fog(0., 0., false);
        assert_eq!(super::halloween_fog(clear, 0.), clear, "far from the park, the weather's");
        let (colour, a, k) = super::halloween_fog(clear, 1.);
        assert!(colour.distance(super::HALLOWEEN_FOG.0) < 1e-6 && (a - super::HALLOWEEN_FOG.1).abs() < 1e-6);
        let mist = chosen_fog(clear, Some((super::HALLOWEEN_FOG.1, super::HALLOWEEN_FOG.2))).2;
        assert!((k - mist).abs() < 1e-7, "{k} {mist}");
    }
    #[test]
    fn drops_keep_falling_through_a_cutscene_that_holds_the_game() {
        // A Skate 2 tutorial scene pauses play (virtual time stands still) but plays on real time: the rain does too.
        assert_eq!(fall_offset(2., 0., 0.016, true), 2.016);
        // The scene's own step cap (cutscene.rs: 0.1 s).
        assert_eq!(fall_offset(0., 0., 0.5, true), 0.1);
        // Play running: the game clock already moves the drops; a pause menu holds them with the game.
        assert_eq!(fall_offset(2., 0.016, 0.016, true), 2.);
        assert_eq!(fall_offset(2., 0., 0.016, false), 2.);
    }
    #[test]
    fn shelter_light_points_straight_down_and_emits_nothing() {
        let mut world = World::new();
        let id = world.spawn(shelter_light()).id();
        let entity = world.entity(id);
        let light = entity.get::<DirectionalLight>().unwrap();
        assert!(light.shadows_enabled && !light.affects_lightmapped_mesh_diffuse);
        assert_eq!(light.illuminance, 0.);
        // Bevy's direction_to_light is the transform's back; the shader wants +Y.
        assert!(entity.get::<Transform>().unwrap().back().y > 0.9999);
        assert_eq!(light.shadow_normal_bias, 0.);
        assert_eq!(entity.get::<bevy::light::CascadeShadowConfig>().unwrap().bounds, vec![20.]);
    }
    #[test]
    fn snow_haze_is_paler_than_rain_and_thickest_when_heavy() {
        let (rain, _, heavy_rain) = fog(1., 0., false);
        let (snow, light_opacity, light_snow) = fog(0., 0.5, false);
        let (_, heavy_opacity, heavy_snow) = fog(0., 1., false);
        assert!(snow.min_element() > rain.max_element());
        assert!(heavy_snow > heavy_rain && heavy_snow > light_snow);
        assert!(light_opacity > 0. && heavy_opacity >= light_opacity);
        assert_eq!(fog(0., 0., false).1, 0.);
    }
    #[test]
    fn christmas_snows_gently_under_a_light_haze() {
        assert_eq!(snowfall(1., false), 1.);
        assert_eq!(snowfall(0.5, false), 0.);
        assert_eq!(snowfall(0.5, true), 0.5);
        assert_eq!(snowfall(0., true), 0., "no snow (Edit Skater room, non-retail) never falls");
        let (_, light_opacity, light_density) = fog(0., 0.5, false);
        let (_, opacity, density) = fog(0., 0.5, true);
        assert!(opacity < light_opacity && density < light_density);
    }
    #[test]
    fn heavy_rain_strikes_every_ten_to_thirty_seconds() {
        let mut storm = Storm::new(7);
        let (dt, mut starts, mut previous) = (1. / 60., Vec::new(), 0.);
        for frame in 0..60 * 600 {
            let now = storm.step(dt, true);
            if now > 0. && previous == 0. {
                starts.push(frame as f32 * dt);
            }
            previous = now;
        }
        assert!(starts.len() >= 20, "{} strikes in 10 minutes", starts.len());
        for gap in starts.windows(2).map(|w| w[1] - w[0]) {
            assert!((9.9..=30.1).contains(&gap), "gap {gap}");
        }
        // Light rain and clear skies never flash, and an ongoing strike stops.
        for _ in 0..60 * 120 {
            assert_eq!(storm.step(dt, false), 0.);
        }
    }
    #[test]
    fn a_strike_flickers_then_goes_dark() {
        let samples: Vec<f32> = (0..60).map(|i| flash(i as f32 / 100.)).collect();
        assert!(samples[0] > 0.8);
        let peaks = samples.windows(3).filter(|w| w[1] > w[0] && w[1] >= w[2]).count();
        assert!(peaks >= 2, "{peaks} flickers");
        assert!(samples.iter().all(|f| (0. ..=1.).contains(f)));
        assert_eq!(flash(1.), 0.);
    }
    #[test]
    fn rain_only_shows_on_retail_maps_outside_the_skater_room() {
        assert_eq!(strength(1., true, false), 1.);
        assert_eq!(strength(0.5, true, false), 0.5);
        assert_eq!(strength(1., false, false), 0.);
        assert_eq!(strength(1., true, true), 0.);
    }
}
