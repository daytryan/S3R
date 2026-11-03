#import bevy_pbr::{forward_io::Vertex, mesh_view_bindings::{view, fog}}

// x: 0 draws tracks, 1 spray.
struct Mode { mode: vec4<f32> }
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> p: Mode;
// retail_wheel_fx.rs layout: clock; per back wheel 128 positions (xyz, time;
// time < 0 is empty); per wheel 128 normals (xyz, 1 when joined to the point
// before); per particle (position, spawn time) and (velocity, 1 for snow).
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var<storage, read> fx: array<vec4<f32>>;
const TRACK: u32 = 128u;

struct FxOut {
    @builtin(position) position: vec4<f32>,
    // x across the track or particle (-1..1), y along it (0..1).
    @location(0) corner: vec2<f32>,
    @location(1) alpha: f32,
    @location(2) snow: f32,
    // Faded into the weather fog per pixel: Bevy binds fog for fragments only.
    @location(3) world: vec3<f32>,
    @location(4) random: f32,
}

// 0..1 from a slot number (PCG's output step).
fn hash(n: u32) -> f32 {
    var x = n * 747796405u + 2891336453u;
    x = ((x >> ((x >> 28u) + 4u)) ^ x) * 277803737u;
    return f32((x >> 22u) ^ x) / 4294967295.0;
}

fn hidden() -> FxOut {
    var out: FxOut;
    out.position = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    out.corner = vec2<f32>(0.0);
    out.alpha = 0.0;
    out.snow = 0.0;
    out.world = vec3<f32>(0.0);
    out.random = 0.0;
    return out;
}

// Weather fog as on the retail world: overlays fade into it with distance.
fn fogged(world: vec3<f32>) -> f32 {
    let d = length(world - view.world_position);
    return fog.base_color.a * (1.0 - exp(-d * fog.be.x * (d + 600.0) / 700.0));
}

@vertex
fn vertex(v: Vertex) -> FxOut {
    // Each quad: corner in xy, its track segment or particle index in z.
    let id = u32(v.position.z);
    let now = fx[0].x;
    var out = hidden();
    out.corner = v.position.xy;
    if p.mode.x < 0.5 {
        // Segment from the previous point to point j of one back wheel's track.
        let wheel = id / TRACK;
        let j = id % TRACK;
        let b = fx[1u + wheel * TRACK + j];
        let a = fx[1u + wheel * TRACK + (j + TRACK - 1u) % TRACK];
        let n = fx[1u + (2u + wheel) * TRACK + j];
        // Unjoined, overwritten (the ring wrapped) or already faded.
        if a.w < 0.0 || b.w < a.w || n.w < 0.5 || now - a.w > 3.0 { return hidden(); }
        let side = normalize(cross(b.xyz - a.xyz, n.xyz));
        let world = mix(a.xyz, b.xyz, v.position.y) + side * v.position.x * 0.025 + n.xyz * 0.01;
        out.position = view.clip_from_world * vec4<f32>(world, 1.0);
        out.alpha = 1.0 - smoothstep(1.8, 3.0, now - mix(a.w, b.w, v.position.y));
        out.world = world;
        return out;
    }
    let spawn = fx[1u + 4u * TRACK + 2u * id];
    let kick = fx[2u + 4u * TRACK + 2u * id];
    let age = now - spawn.w;
    let snow = kick.w;
    // Each particle its own size and life (a fixed hash of its slot and spawn), so the spray isn't a stream of twins.
    let r = hash(id * 747796405u + u32(spawn.w * 997.0));
    let life = mix(0.5, 0.9, snow) * (0.7 + 0.6 * r);
    out.random = r;
    if spawn.w < 0.0 || age < 0.0 || age > life { return hidden(); }
    var at: vec3<f32>;
    if snow > 0.5 {
        // Loose snow slows quickly in the air and settles gently.
        at = spawn.xyz + kick.xyz * (1.0 - exp(-3.0 * age)) / 3.0 - vec3<f32>(0.0, 1.2 * age * age, 0.0);
    } else {
        at = spawn.xyz + kick.xyz * age - vec3<f32>(0.0, 4.9 * age * age, 0.0);
    }
    // Droplets stay small; clumps of snow spread as they drift, each its own size.
    let size = mix(0.009 + 0.008 * r, (0.025 + 0.05 * r) + 0.07 * age, snow);
    let pixel = length(at - view.world_position) * 2.0 / (view.viewport.w * view.clip_from_view[1][1]);
    let radius = max(size, pixel);
    let right = view.world_from_view[0].xyz;
    let up = view.world_from_view[1].xyz;
    var corner = right * v.position.x + up * (v.position.y * 2.0 - 1.0);
    if snow < 0.5 {
        // A drop streaks along its flight in the frame (as a camera sees fast water), thin across it.
        let vel = kick.xyz - vec3<f32>(0.0, 9.8 * age, 0.0);
        let on_screen = right * dot(vel, right) + up * dot(vel, up);
        let along = select(up, normalize(on_screen), length(on_screen) > 0.01);
        let across = normalize(cross(along, view.world_from_view[2].xyz));
        let streak = 1.0 + min(length(on_screen) * 0.9, 5.0);
        corner = across * v.position.x * 0.7 + along * (v.position.y * 2.0 - 1.0) * streak;
    }
    out.position = view.clip_from_world * vec4<f32>(at + corner * radius, 1.0);
    out.alpha = mix(0.32, 0.5, snow) * (1.0 - age / life) * (size / radius);
    out.snow = snow;
    out.world = at;
    return out;
}

@fragment
fn fragment(i: FxOut) -> @location(0) vec4<f32> {
    let alpha = i.alpha * (1.0 - fogged(i.world));
    if p.mode.x < 0.5 {
        // A groove pressed into the snow: shadowed in the middle, soft at the edges.
        return vec4<f32>(0.05, 0.06, 0.08, alpha * 0.3 * (1.0 - smoothstep(0.35, 1.0, abs(i.corner.x))));
    }
    let q = vec2<f32>(i.corner.x, i.corner.y * 2.0 - 1.0);
    if i.snow > 0.5 {
        // A lumpy clump, not a disc: its edge wobbles with the angle, each clump its own way; near white, lit by the sky.
        let wobble = 0.75 + 0.25 * sin(atan2(q.y, q.x) * 5.0 + i.random * 6.283);
        let shape = saturate(1.0 - dot(q, q) / (wobble * wobble));
        let colour = mix(fog.base_color.rgb * 1.2, vec3<f32>(0.9, 0.92, 0.95), 0.6);
        return vec4<f32>(colour, alpha * shape);
    }
    // Water: clear, catching a little light, densest along the streak's middle.
    let colour = fog.base_color.rgb * 1.05 + vec3<f32>(0.05);
    return vec4<f32>(colour, alpha * saturate(1.0 - q.x * q.x) * saturate(1.0 - q.y * q.y));
}
