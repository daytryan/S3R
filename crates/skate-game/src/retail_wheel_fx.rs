//! Visual-only wheel effects in weather: spray kicked up by the back wheels
//! (water in rain, snow in snow) and fading wheel tracks in snow. The GPU places
//! both from one storage buffer (retail_wheel_fx.wgsl); gameplay never reads it.
use super::ShadowState;
use bevy::{
    asset::{AssetPath, embedded_asset, embedded_path},
    camera::visibility::NoFrustumCulling,
    light::{NotShadowCaster, NotShadowReceiver},
    prelude::*,
    render::{
        Render, RenderApp, RenderSystems,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_asset::RenderAssets,
        render_resource::{AsBindGroup, BufferUsages},
        renderer::RenderQueue,
        storage::{GpuShaderStorageBuffer, ShaderStorageBuffer},
    },
    shader::ShaderRef,
};
use skate_core::physics::board::BodyId;

/// Track points per back wheel, this far apart (metres): enough for the fade.
const TRACK: usize = 128;
const SPACING: f32 = 0.15;
/// Spray particles alive at once.
const SPRAY: usize = 256;
/// Buffer of vec4s: clock; per wheel TRACK positions (xyz, time; time < 0 is
/// empty); per wheel TRACK normals (xyz, 1 when joined to the point before);
/// per particle (position, spawn time) and (velocity, 1 for snow).
const LEN: usize = 1 + 4 * TRACK + 2 * SPRAY;
fn position(wheel: usize, i: usize) -> usize { 1 + wheel * TRACK + i }
fn normal(wheel: usize, i: usize) -> usize { 1 + (2 + wheel) * TRACK + i }
fn particle(i: usize) -> usize { 1 + 4 * TRACK + 2 * i }

const BUFFER: Handle<ShaderStorageBuffer> = bevy::asset::uuid_handle!("5f0c2a6e-58f1-4d0b-9a57-2c7e3d9b41a8");

/// Where a back wheel touches the ground.
#[derive(Clone, Copy, Debug)]
struct Contact {
    point: Vec3,
    normal: Vec3,
}

/// A wheel's ground probe reads about 0.03 m resting on the ground, more once lifted.
const GROUNDED: f32 = 0.055;

/// Per side (right, left), the wheel marking the ground: the one that is down,
/// or the trailing one when both are (back wheels forward, front wheels fakie).
/// Wheels are right front, left front, right back, left back. The ground probe,
/// unlike the solver's contact reports, stays steady while rolling. A probe that
/// misses keeps its last distance, so `nearest` (reset every tick) must also be down.
fn ground_wheels(centres: [Vec3; 4], distances: [f32; 4], nearest: f32, normals: [Vec3; 4], velocity: Vec3) -> [Option<Contact>; 2] {
    if nearest >= GROUNDED {
        return [None, None];
    }
    let contact = |i: usize| (distances[i] < GROUNDED).then(|| Contact { point: centres[i] - normals[i] * distances[i], normal: normals[i] });
    [(0, 2), (1, 3)].map(|(front, back)| match (contact(front), contact(back)) {
        (Some(f), Some(b)) => Some(if (centres[back] - centres[front]).dot(velocity) <= 0. { b } else { f }),
        (f, b) => b.or(f),
    })
}

#[derive(Resource, Clone, ExtractResource)]
struct WheelFx {
    data: Vec<Vec4>,
    heads: [usize; 2],
    last: [Option<Vec3>; 2],
    rolling: [bool; 2],
    next: usize,
    debt: [f32; 2],
    rng: u64,
}
impl WheelFx {
    fn new(seed: u64) -> Self {
        let mut data = vec![Vec4::new(0., 0., 0., -1.); LEN];
        data[0] = Vec4::ZERO;
        Self { data, heads: [0; 2], last: [None; 2], rolling: [false; 2], next: 0, debt: [0.; 2], rng: seed | 1 }
    }
    /// xorshift64*, uniform in 0..1.
    fn random(&mut self) -> f32 {
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        (self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32 / (1u64 << 24) as f32
    }
    /// Advance to `now`: back wheels in contact (right, left), board velocity and
    /// the weather shown.
    fn step(&mut self, now: f32, dt: f32, wheels: [Option<Contact>; 2], velocity: Vec3, rain: f32, snow: f32) {
        self.data[0].x = now;
        for (wheel, contact) in wheels.into_iter().enumerate() {
            let Some(contact) = contact else {
                self.rolling[wheel] = false;
                self.debt[wheel] = 0.;
                continue;
            };
            if snow > 0. && self.last[wheel].is_none_or(|p| p.distance(contact.point) >= SPACING) {
                // Joined unless the wheel left the ground or jumped (teleport, respawn).
                let joined = self.rolling[wheel] && self.last[wheel].is_some_and(|p| p.distance(contact.point) < 1.);
                let i = self.heads[wheel];
                self.data[position(wheel, i)] = contact.point.extend(now);
                self.data[normal(wheel, i)] = contact.normal.extend(if joined { 1. } else { 0. });
                self.heads[wheel] = (i + 1) % TRACK;
                self.last[wheel] = Some(contact.point);
            }
            self.rolling[wheel] = true;
            // Spray grows with speed above a jog, and with the weather.
            let rate = ((velocity.length() - 1.5) * 12.).clamp(0., 60.) * rain.max(snow);
            self.debt[wheel] += rate * dt;
            while self.debt[wheel] >= 1. {
                self.debt[wheel] -= 1.;
                self.emit(now, contact, velocity, snow > 0.);
            }
        }
    }
    /// Kick one particle up and back off a wheel.
    fn emit(&mut self, now: f32, contact: Contact, velocity: Vec3, snow: bool) {
        let side = velocity.cross(contact.normal).normalize_or_zero();
        let (lift, spread, carry) = if snow { (0.4 + 0.8 * self.random(), 0.6, 0.4) } else { (0.6 + 1.2 * self.random(), 0.8, 0.5) };
        let kick = velocity * carry + contact.normal * lift + side * (self.random() - 0.5) * spread;
        let i = self.next;
        self.next = (i + 1) % SPRAY;
        self.data[particle(i)] = (contact.point + contact.normal * 0.02).extend(now);
        self.data[particle(i) + 1] = kick.extend(if snow { 1. } else { 0. });
    }
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
struct WheelFxMaterial {
    /// x: 0 draws tracks, 1 spray.
    #[uniform(0)]
    mode: Vec4,
    #[storage(1, read_only)]
    data: Handle<ShaderStorageBuffer>,
}
impl Material for WheelFxMaterial {
    fn vertex_shader() -> ShaderRef {
        AssetPath::from(embedded_path!("retail_wheel_fx.wgsl")).with_source("embedded").into()
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
        super::weather::overlay(descriptor);
        Ok(())
    }
}

#[derive(Component)]
struct WheelEffect;

pub(super) fn install(app: &mut App) {
    embedded_asset!(app, "retail_wheel_fx.wgsl");
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |d| d.as_nanos() as u64);
    app.add_plugins((MaterialPlugin::<WheelFxMaterial>::default(), ExtractResourcePlugin::<WheelFx>::default()))
        .insert_resource(WheelFx::new(seed))
        .add_systems(Startup, spawn)
        .add_systems(Update, apply.after(super::weather::WeatherSet));
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render.add_systems(Render, upload.in_set(RenderSystems::PrepareResources));
    }
}

fn spawn(
    mut commands: Commands,
    fx: Res<WheelFx>,
    mut buffers: ResMut<Assets<ShaderStorageBuffer>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<WheelFxMaterial>>,
) {
    let mut buffer = ShaderStorageBuffer::from(fx.data.clone());
    buffer.buffer_description.usage |= BufferUsages::COPY_DST;
    buffers.insert(BUFFER.id(), buffer).expect("reserved wheel effect asset");
    // No Aabb: like the rain, never frustum or occlusion culled.
    for (mode, count) in [(0., 2 * TRACK), (1., SPRAY)] {
        commands.spawn((
            Name::new(if mode == 0. { "Wheel tracks" } else { "Wheel spray" }),
            WheelEffect,
            Mesh3d(meshes.add(super::weather::quads(count as u32))),
            MeshMaterial3d(materials.add(WheelFxMaterial { mode: Vec4::new(mode, 0., 0., 0.), data: BUFFER })),
            Transform::default(),
            Visibility::Hidden,
            NoFrustumCulling,
            NotShadowCaster,
            NotShadowReceiver,
        ));
    }
}

fn apply(
    time: Res<Time>,
    state: Res<ShadowState>,
    physics: Option<Res<crate::physics::GamePhysics>>,
    replay: Option<Res<crate::replay::Replay>>,
    mut fx: ResMut<WheelFx>,
    mut effects: Query<&mut Visibility, With<WheelEffect>>,
) {
    // The weather actually shown (0 off retail maps and in the skater room).
    let (rain, snow) = (state.3.x, state.4.x);
    let on = rain > 0. || snow > 0.;
    for mut visibility in &mut effects {
        visibility.set_if_neq(if on { Visibility::Visible } else { Visibility::Hidden });
    }
    let mut wheels = [None; 2];
    let mut velocity = Vec3::ZERO;
    if let Some(physics) = physics.filter(|_| on && !replay.is_some_and(|r| r.active)) {
        let vec = |v: skate_core::math::Vector3| Vec3::new(v.x, v.y, v.z);
        velocity = vec(physics.board.bodies()[BodyId::Deck.index()].rates.linear_velocity);
        let lines = &physics.riding.wheel_lines;
        wheels = ground_wheels(
            std::array::from_fn(|i| vec(physics.board.bodies()[i].rates.position)),
            lines.distances,
            lines.minimum_distance,
            lines.normals.map(|n| vec(n).normalize_or(Vec3::Y)),
            velocity,
        );
    }
    // Hidden while off: the clock and wheel state stay current without flagging a change, so the 16 KB buffer isn't
    // cloned to the render world and uploaded every frame (perf 10-12). Turning on flags it before anything draws.
    let fx = if on { &mut *fx } else { fx.bypass_change_detection() };
    fx.step(time.elapsed_secs(), time.delta_secs(), wheels, velocity, rain, snow);
}

fn upload(
    fx: Res<WheelFx>,
    buffers: Res<RenderAssets<GpuShaderStorageBuffer>>,
    queue: Res<RenderQueue>,
    mut bytes: Local<Vec<u8>>,
) {
    // Extracted only when it changed; a (re)created GPU buffer needs the data too.
    if !fx.is_changed() && !buffers.is_changed() {
        return;
    }
    if let Some(buffer) = buffers.get(BUFFER.id()) {
        bytes.clear();
        bytes.extend(fx.data.iter().flat_map(|v| v.to_array()).flat_map(f32::to_le_bytes));
        queue.write_buffer(&buffer.buffer, 0, &bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn at(x: f32) -> Option<Contact> {
        Some(Contact { point: Vec3::new(x, 0., 0.), normal: Vec3::Y })
    }
    /// Recorded points of one wheel, oldest first: position and joined flag.
    fn track(fx: &WheelFx, wheel: usize) -> Vec<(Vec3, bool)> {
        let mut points: Vec<_> = (0..TRACK)
            .map(|i| (fx.data[position(wheel, i)], fx.data[normal(wheel, i)]))
            .filter(|(p, _)| p.w >= 0.)
            .collect();
        points.sort_by(|a, b| a.0.w.total_cmp(&b.0.w));
        points.into_iter().map(|(p, n)| (p.truncate(), n.w > 0.5)).collect()
    }
    fn live(fx: &WheelFx) -> Vec<Vec4> {
        (0..SPRAY).filter(|&i| fx.data[particle(i)].w >= 0.).map(|i| fx.data[particle(i) + 1]).collect()
    }
    #[test]
    fn the_trailing_grounded_wheel_on_each_side_marks_the_snow() {
        // Right front, left front, right back, left back; the back is at -x.
        let wheels = [Vec3::new(0.4, 0.03, 0.1), Vec3::new(0.4, 0.03, -0.1), Vec3::new(-0.4, 0.03, 0.1), Vec3::new(-0.4, 0.03, -0.1)];
        let pick = |distances: [f32; 4], velocity: Vec3| {
            ground_wheels(wheels, distances, distances.into_iter().fold(1., f32::min), [Vec3::Y; 4], velocity).map(|c| c.map(|c| c.point.x))
        };
        let down = [0.03; 4];
        // Rolling forward the back wheels trail; fakie the front wheels do.
        assert_eq!(pick(down, Vec3::X), [Some(-0.4), Some(-0.4)]);
        assert_eq!(pick(down, -Vec3::X), [Some(0.4), Some(0.4)]);
        // Nose manual: only the front wheels are down.
        assert_eq!(pick([0.03, 0.03, 0.15, 0.15], Vec3::X), [Some(0.4), Some(0.4)]);
        // Contact points sit on the ground below the wheel centres.
        assert!(ground_wheels(wheels, down, 0.03, [Vec3::Y; 4], Vec3::X)[0].unwrap().point.y.abs() < 1e-6);
        // In the air.
        assert_eq!(pick([0.2; 4], Vec3::X), [None, None]);
        // Airborne probes miss and keep their last distances; only the nearest
        // distance is reset each tick, so stale ground readings must not count.
        assert!(ground_wheels(wheels, down, 0.3, [Vec3::Y; 4], Vec3::X).iter().all(Option::is_none));
    }
    #[test]
    fn snow_tracks_follow_rolling_wheels_and_break_in_the_air() {
        let dt = 1. / 60.;
        let mut fx = WheelFx::new(3);
        // One metre at 5 m/s: a point every 15 cm, each joined to the one before.
        for frame in 0..=12 {
            let x = frame as f32 * 5. * dt;
            fx.step(frame as f32 * dt, dt, [at(x), None], Vec3::X * 5., 0., 1.);
        }
        let points = track(&fx, 0);
        assert_eq!(points.len(), 7);
        assert!(!points[0].1 && points[1..].iter().all(|p| p.1));
        assert!(track(&fx, 1).is_empty());
        // Airborne, then landing further on: a new, unconnected track.
        fx.step(0.3, dt, [None, None], Vec3::X * 5., 0., 1.);
        fx.step(0.4, dt, [at(3.), None], Vec3::X * 5., 0., 1.);
        assert_eq!(track(&fx, 0).last(), Some(&(Vec3::new(3., 0., 0.), false)));
        // Rain leaves no tracks.
        let mut wet = WheelFx::new(3);
        for frame in 0..30 {
            wet.step(frame as f32 * dt, dt, [at(frame as f32 * 0.1), None], Vec3::X * 6., 1., 0.);
        }
        assert!(track(&wet, 0).is_empty());
    }
    #[test]
    fn spray_needs_weather_ground_and_speed() {
        let dt = 1. / 60.;
        let run = |rain: f32, snow: f32, speed: f32, wheels: [Option<Contact>; 2]| {
            let mut fx = WheelFx::new(9);
            for frame in 0..60 {
                fx.step(frame as f32 * dt, dt, wheels, Vec3::X * speed, rain, snow);
            }
            live(&fx)
        };
        let both = [at(0.), at(0.2)];
        assert!(run(0., 0., 6., both).is_empty());
        assert!(run(1., 0., 1., both).is_empty());
        assert!(run(1., 0., 6., [None, None]).is_empty());
        let water = run(1., 0., 6., both);
        let snow = run(0., 1., 6., both);
        assert!(!water.is_empty() && water.iter().all(|v| v.w == 0. && v.y > 0.));
        assert!(!snow.is_empty() && snow.iter().all(|v| v.w == 1.));
        // Light rain throws less than heavy rain.
        assert!(run(0.5, 0., 6., both).len() < water.len());
    }
}
