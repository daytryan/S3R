// Paused-world backdrop behind the original Start menu. Kernel from the retail
// blur_hBlurPS/blur_vBlurPS (shaders_final.big): 11 taps, spacing
// i_BlurRadius/3200 UV on each axis (no aspect correction, as retail).
// Both passes are folded into one 11x11 product here.
#import bevy_ui::ui_vertex_output::UiVertexOutput
@group(1) @binding(0) var scene: texture_2d<f32>;
@group(1) @binding(1) var scene_sampler: sampler;
// ponytail: i_BlurRadius is set by default.xex at runtime and not yet recovered;
// 5 gives the 1/640 UV step baked into the retail shader. Tune against a capture.
const BLUR_RADIUS: f32 = 5.0;
@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    let weights = array<f32, 6>(0.20056541, 0.17699836, 0.12164907, 0.06511406, 0.027143577, 0.008812229);
    let step = BLUR_RADIUS / 3200.0;
    var color = vec3(0.0);
    for (var y = -5; y <= 5; y++) {
        for (var x = -5; x <= 5; x++) {
            color += textureSample(scene, scene_sampler, in.uv + vec2(f32(x), f32(y)) * step).rgb
                * weights[abs(x)] * weights[abs(y)];
        }
    }
    return vec4(color, 1.0);
}
