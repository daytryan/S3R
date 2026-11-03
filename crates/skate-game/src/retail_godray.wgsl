// Skate 2's godray.spotlight beams (retail_godray.rs): the effect godrayspotlight, render.big
// shaders/final/godrayspotlight_defaultVS/PS (.updb HLSL, constant tables c0-c7 g_matVP + i_matWorld, c0 g_vViewPos + s0).
// VS: world position (its cylindrical billboard matrix is dead code: matRW is never used). PS: the diffuse squared
// (GetDiffuseTextureValue = MoveToLinearSpaceRGB, x*x) times |dot(vNormal, normalize(g_vViewPos - vPos))|, alpha 0,
// added ONE/ONE to the frame; the tone pass adds this target to its sqrt-encoded value.
struct View {
    clip_from_world: mat4x4<f32>,
    eye: vec4<f32>,
}
@group(0) @binding(0) var<uniform> view: View;
@group(1) @binding(0) var diffuse: texture_2d<f32>;
@group(1) @binding(1) var diffuse_sampler: sampler;

struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) world: vec3<f32>,
}

@vertex
fn vertex(@location(0) position: vec3<f32>, @location(1) normal: vec3<f32>, @location(2) uv: vec2<f32>) -> Out {
    var o: Out;
    o.clip = view.clip_from_world*vec4<f32>(position,1.0);
    o.uv = uv;
    o.normal = normal;
    o.world = position;
    return o;
}

@fragment
fn fragment(i: Out) -> @location(0) vec4<f32> {
    let c = textureSample(diffuse,diffuse_sampler,i.uv).rgb;
    // The interpolated normal is not renormalised, as retail's.
    let viewangle = abs(dot(i.normal,normalize(view.eye.xyz-i.world)));
    return vec4<f32>(viewangle*c*c,0.0);
}
