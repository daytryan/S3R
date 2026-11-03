#import bevy_pbr::{forward_io::VertexOutput, mesh_view_bindings::view}

// hom_bones.rs: rgb is the tier's colour (vault hall_of_meat_skeleton).
struct Bone { colour: vec4<f32> }
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> bone: Bone;

// After retail's defaulthom_defaultPS (best-effort decode): the bone colour, brightest facing the camera, plus a
// fresnel rim. The 0.5 / 0.3 are hall_of_meat_skeleton's i_colorParams-like pair (INFERRED as rim strength / floor).
@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let v = normalize(view.world_position - in.world_position.xyz);
    let d = saturate(dot(normalize(in.world_normal), v));
    let rim = pow(1.0 - d, 2.0);
    return vec4<f32>(bone.colour.rgb * (0.3 + 0.7 * d) + vec3<f32>(rim * 0.5), 1.0);
}
