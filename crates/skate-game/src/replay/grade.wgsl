// The Replay Editor's grade (replay/grade.rs; Stage 0 §6 ruling) and the item 16 letterbox.
#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;
// [0] brightness, contrast, saturation, letterbox aspect (0 = off); [1] tint rgb, alpha.
@group(0) @binding(2) var<uniform> grade: array<vec4<f32>, 2>;

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let colour = textureSample(source, source_sampler, in.uv);
    // The letterbox (grade.rs letterbox_bars): black bars outside the picture's aspect.
    let aspect = grade[0].w;
    if (aspect > 0.0) {
        let size = vec2<f32>(textureDimensions(source));
        let screen = size.x / size.y;
        let inside = select(abs(in.uv.x - 0.5) * 2.0 <= aspect / screen, abs(in.uv.y - 0.5) * 2.0 <= screen / aspect, aspect > screen);
        if (!inside) {
            return vec4(0.0, 0.0, 0.0, colour.a);
        }
    }
    var c = grade[0].y * (colour.rgb - vec3(0.5)) + vec3(0.5 + grade[0].x);
    let lum = dot(c, vec3(0.299, 0.587, 0.114));
    c = c * (lum * grade[0].z + 1.0);
    c = mix(c, grade[1].rgb, grade[1].a);
    return vec4(c, colour.a);
}
