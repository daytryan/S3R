#import bevy_pbr::{forward_io::Vertex, mesh_view_bindings::{view, globals, fog, lights}, shadows::fetch_directional_shadow}
#import skate_retail::weather

// x: rain strength; y: snowfall. A drop whose random pick is above the amount
// stays hidden. z: seconds added to the clock (a held cutscene's, retail_weather.rs).
struct RainParams { settings: vec4<f32> }
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> p: RainParams;
struct RainOut {
    @builtin(position) position: vec4<f32>,
    // x across the streak or flake (-1..1), y from its tail or bottom (0) to its
    // head or top (1).
    @location(0) streak: vec2<f32>,
    @location(1) alpha: f32,
    // The drop's world position, for the shelter test.
    @location(2) @interpolate(flat) head: vec3<f32>,
}

@vertex
fn vertex(v: Vertex) -> RainOut {
    var out: RainOut;
    // Each quad is one drop: xy is the corner, z the drop's index.
    let id = v.position.z;
    out.streak = v.position.xy;
    out.alpha = 0.0;
    out.head = vec3<f32>(0.0);
    let snowing = p.settings.y > 0.0;
    if weather::hash(vec2<f32>(id, 1.0)) >= select(p.settings.x, 0.8 * p.settings.y, snowing) {
        out.position = vec4<f32>(2.0, 2.0, 2.0, 1.0);
        return out;
    }
    let size = vec3<f32>(36.0, 24.0, 36.0);
    let start = vec3<f32>(weather::hash(vec2<f32>(id, 2.0)), weather::hash(vec2<f32>(id, 3.0)), weather::hash(vec2<f32>(id, 4.0)));
    let speed = weather::hash(vec2<f32>(id, 5.0));
    let fall = select(vec3<f32>(1.0, -9.0, 0.5) * (0.85 + 0.3 * speed), vec3<f32>(0.3, -1.1, 0.15) * (0.6 + 0.8 * speed), snowing);
    // Drops stay fixed in the world and wrap within a box that follows the camera.
    let camera = view.world_position;
    let clock = globals.time + p.settings.z;
    let offset = (fract((start * size + fall * clock - camera) / size) - 0.5) * size;
    let distance = max(length(offset), 0.01);
    let edge = abs(offset) / (size * 0.5);
    // Flakes right at the lens would be large soft blobs; they fade in further out.
    let near = select(vec2<f32>(0.4, 1.5), vec2<f32>(1.2, 3.5), snowing);
    let fade = (1.0 - smoothstep(0.6, 1.0, max(edge.x, max(edge.y, edge.z)))) * smoothstep(near.x, near.y, distance);
    // At least a screen pixel wide (the output, not DLSS's smaller internal size, which made Ultra Performance flakes
    // blocks); drops thinner than a pixel fade instead of aliasing.
    let pixel = distance * 2.0 / (view.viewport.w * view.clip_from_view[1][1]);
    var world: vec3<f32>;
    if snowing {
        // Flakes sway as they fall and always face the camera.
        let phase = weather::hash(vec2<f32>(id, 6.0)) * 6.2832;
        let sway = vec3<f32>(sin(clock * 1.3 + phase), 0.0, cos(clock * 1.1 + phase)) * 0.35;
        let flake = max(0.03, pixel * 1.5);
        let corner = view.world_from_view[0].xyz * v.position.x + view.world_from_view[1].xyz * (v.position.y * 2.0 - 1.0);
        out.head = camera + offset + sway;
        world = out.head + corner * flake;
        out.alpha = 0.8 * (0.03 / flake) * fade;
    } else {
        let head = camera + offset;
        out.head = head;
        let dir = normalize(fall);
        let width = max(0.008, pixel);
        let side = cross(dir, offset);
        let across = side * inverseSqrt(max(dot(side, side), 1e-8));
        world = mix(head - dir * 0.6, head, v.position.y) + across * v.position.x * width * 0.5;
        out.alpha = 0.45 * (0.008 / width) * fade;
    }
    out.position = view.clip_from_world * vec4<f32>(world, 1.0);
    return out;
}

// Under a roof, bridge or tree: retail_weather.rs's straight-down, shadow-only
// light (the non-lightmapped one with normal bias 0) has a caster above the drop. Shadow maps are fragment-only bindings, so
// this runs per fragment on the drop's flat position. Past that light's 20 m
// cascade the lookup reports lit and drops stay visible.
fn sheltered(head: vec3<f32>) -> bool {
    for (var id = 0u; id < lights.n_directional_lights; id += 1u) {
        if (lights.directional_lights[id].flags & 5u) == 1u && lights.directional_lights[id].shadow_normal_bias == 0.0 {
            let view_z = (view.view_from_world * vec4<f32>(head, 1.0)).z;
            return fetch_directional_shadow(id, vec4<f32>(head, 1.0), vec3<f32>(0.0, 1.0, 0.0), view_z) < 0.5;
        }
    }
    return false;
}

@fragment
fn fragment(i: RainOut) -> @location(0) vec4<f32> {
    // Faded drops skip the shadow lookup.
    if i.alpha < 1e-3 || sheltered(i.head) {
        discard;
    }
    if p.settings.y > 0.0 {
        // A soft round flake.
        let q = vec2<f32>(i.streak.x, i.streak.y * 2.0 - 1.0);
        return vec4<f32>(fog.base_color.rgb * 2.0, i.alpha * saturate(1.0 - dot(q, q)));
    }
    return vec4<f32>(fog.base_color.rgb * 1.6, i.alpha * (1.0 - i.streak.x * i.streak.x) * i.streak.y);
}
