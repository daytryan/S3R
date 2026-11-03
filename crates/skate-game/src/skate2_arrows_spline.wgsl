#import bevy_pbr::{forward_io::VertexOutput, mesh_view_bindings::view}

// skate2_arrows.rs SplineMaterial: i_intensity (x glow, y darken), i_clipvalues (near start, near length, far start,
// far length, metres of view depth), z 1 for the glow pass.
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> intensity: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var<uniform> clip: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var diffuse: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var diffuse_sampler: sampler;

// Retail spline_defaultVS's fade on clip depth, then spline_defaultPS (glow) or spline_darkenPS. Retail squares the
// texture, scales it and takes the square root, so in linear terms the colour is the texture times the scale.
@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let t = textureSample(diffuse, diffuse_sampler, in.uv);
    let depth = -(view.view_from_world * vec4<f32>(in.world_position.xyz, 1.0)).z;
    let fade = min(saturate((depth - clip.x) / clip.y), 1.0 - saturate((depth - clip.z) / clip.w));
    if intensity.z > 0.5 {
        // Additive: alpha 0 under Bevy's premultiplied blend adds the colour.
        return vec4<f32>(t.rgb * intensity.x * fade / max(t.a, 1e-4), 0.0);
    }
    return vec4<f32>(t.rgb * intensity.y, sqrt(intensity.y * t.a * fade));
}
