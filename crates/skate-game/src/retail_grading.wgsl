// Skate 2's bloom, ported from its own pixel shaders (Xenia shader cache) with the constants its GPU
// command buffer carried (retail_grading.rs). Levels: L0 = 1/4 of the view, L1 = 1/16. Every target is
// 8-bit, as on the 360, so each pass clamps to 0..1.
#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput
#import skate_retail::weather
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var second: texture_2d<f32>;
@group(0) @binding(2) var bilinear: sampler;
@group(0) @binding(3) var<storage, read> exposure: vec4<f32>;
@group(0) @binding(4) var<uniform> settings: array<vec4<f32>, 3>;
// [0] enabled, bloom, blend, lookup scale; [1] lookup offset; [2] threshold, threshold scale,
// level 0 weight, level 1 weight; [3] bloom saturation, Skate 1 film pass (no lens ghosts).
// [47] Skate 3's shot bloom (retail_grading.rs skate3_bloom): on, the bright pass's offset, level 1's horizontal gain.
@group(0) @binding(5) var<uniform> grade: array<vec4<f32>, 48>;
// The bright pass only: the scene depth (retail_exposure.rs depth_layout), multisampled with MSAA.
#ifdef DEPTH_MSAA
@group(1) @binding(0) var depth: texture_depth_multisampled_2d;
#else
@group(1) @binding(0) var depth: texture_depth_2d;
#endif
// x: near plane; yz: the main pass's share of the depth texture (retail_tone.wgsl view_near).
@group(1) @binding(1) var<uniform> view_near: vec4<f32>;
// #36: Skate 2's godray.spotlight beams (retail_godray.rs; retail_tone.wgsl godray_at), part of the frame read here.
@group(1) @binding(2) var godray: texture_2d<f32>;

// Skate 2 stores the scene as sqrt(tm/2) in 8 bits; tm is retail_tone.wgsl's curve, same inputs.
fn encoded(c: vec3<f32>) -> vec3<f32> {
    let w = settings[2];
    let xe = max(c*exposure.x/2.5,vec3<f32>(0.0))*weather::gloom(w.x)*weather::flash_gain(w.y)+w.y*0.02*vec3<f32>(0.85,0.9,1.0);
    let t = saturate(1.0-xe);
    let tm = max(xe*0.25+0.75,vec3<f32>(1.0))-t*t;
    return saturate(sqrt(max(tm*0.5,vec3<f32>(0.0))));
}

// Skate 1's world shaders write lerp(lit, fog, f)*exposure in gamma space, unclamped until the 8-bit target: the
// map's light is converted so the engine's display at exposure 2.5 (old_skies.py E_REF) is its exposure-1 value.
fn skate1_display(c: vec3<f32>) -> vec3<f32> {
    let w = settings[2];
    let xe = max(c,vec3<f32>(0.0))*weather::gloom(w.x)*weather::flash_gain(w.y)+w.y*0.02*vec3<f32>(0.85,0.9,1.0);
    let t = saturate(1.0-xe);
    let tm = max(xe*0.25+0.75,vec3<f32>(1.0))-t*t;
    return sqrt(max(tm*0.5,vec3<f32>(0.0)))*1.41*exposure.x/2.5;
}

// Skate 1 blurs in texels of the level blurred (the table x 288: 1.46, 3.40, 5.35, 7.30), whatever its size; Skate 3
// too (its offsets come from the target's width and height, 0x827e91a0).
fn step_for(t: texture_2d<f32>, skate2: vec2<f32>, axis: vec2<f32>) -> vec2<f32> {
    return select(skate2,axis*288.0/vec2<f32>(textureDimensions(t)),grade[3].y > 0.0 || grade[47].x > 0.0);
}

// Bright pass: four bilinear taps (2x2 averages of the encoded scene) over the 4x4 block, squared
// back to bilinear and averaged, then saturate((x - threshold)*scale) and the saturation matrix
// lum*(1-s) + s*I with lum (0.3, 0.6, 0.1).
@fragment
fn bright(i: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let base = vec2<i32>(floor(i.position.xy))*4;
    let last = vec2<i32>(textureDimensions(source))-1;
    if grade[3].y > 0.0 {
        // Skate 1 (C3FA8514): its 8-bit scene holds rgb = saturate(d) and alpha = saturate(lum(d)/8), d its world
        // shaders' unclamped display (skate1_display). Four bilinear taps average the 4x4 block; it keeps
        // rgb*saturate((a - threshold)/(1.0000001 - threshold)) and writes its meter's input to alpha.
        // The sky (sky.psh) writes a = dot(its colour before exposure, sky_bloom_intensity) instead, the 'sky'
        // material row's (.09603, .11569, .12179); here the pixels the sky drew (depth 1e-7, retail_sky.wgsl) or cleared.
        var avg = vec4<f32>(0.0);
        for (var k = 0; k < 16; k++) {
            let px = min(base+vec2<i32>(k%4,k/4),last);
            let d = skate1_display(textureLoad(source,px,0).rgb);
            let sky = textureLoad(depth,min(vec2<i32>(vec2<f32>(px)*view_near.yz),vec2<i32>(textureDimensions(depth))-1),0) < 1e-6;
            let a = select(dot(d,vec3<f32>(0.3,0.6,0.1))/8.0,dot(d*2.5/max(exposure.x,1e-4),vec3<f32>(0.09603,0.11569,0.12179)),sky);
            avg += vec4<f32>(saturate(d),saturate(a));
        }
        avg /= 16.0;
        let t = grade[2].x;
        return vec4<f32>(saturate(avg.rgb*saturate((avg.a-t)/(1.0000001-t))),saturate(dot(avg.rgb,vec3<f32>(0.15,0.25,0.1))*(1.0+2.0*avg.a)));
    }
    var sum = vec3<f32>(0.0);
    for (var q = 0; q < 4; q++) {
        let o = base+vec2<i32>(q%2,q/2)*2;
        var e = vec3<f32>(0.0);
        for (var k = 0; k < 4; k++) {
            let px = min(o+vec2<i32>(k%2,k/2),last);
            let g = vec2<i32>(textureDimensions(godray));
            e += saturate(encoded(textureLoad(source,px,0).rgb)+textureLoad(godray,min(vec2<i32>(vec2<f32>(px)*view_near.yz),g-1),0).rgb);
        }
        e *= 0.25;
        sum += e*e;
    }
    // Skate 3 adds max(-threshold, 0) after the saturate (f_coefficients.x; 0 elsewhere).
    let bright = saturate((sum*0.25-grade[2].x)*grade[2].y)+grade[47].y;
    let s = grade[3].x;
    return vec4<f32>(saturate(dot(bright,vec3<f32>(0.3,0.6,0.1))*(1.0-s)+s*bright),1.0);
}

// Box downsample: four bilinear taps one source texel either side of the output pixel's centre.
@fragment
fn down(i: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let t = 1.0/vec2<f32>(textureDimensions(source));
    var c = vec3<f32>(0.0);
    for (var k = 0; k < 4; k++) {
        c += textureSample(source,bilinear,i.uv+vec2<f32>(f32(k%2)*2.0-1.0,f32(k/2)*2.0-1.0)*t).rgb;
    }
    return vec4<f32>(c*0.25,1.0);
}

// 9-tap blur, offsets in UV as Skate 2 set them for L0 horizontally (L1 x4; L0 vertical x 1.8). Its L1 vertical pass reuses the horizontal UV offsets, so it is narrower.
const WEIGHT = array<f32,5>(0.132981,0.232277,0.135327,0.051156,0.0125393);
const OFFSET = array<f32,5>(0.0,0.00506399,0.0118194,0.0185827,0.0253574);
fn blur(t: texture_2d<f32>, uv: vec2<f32>, step: vec2<f32>) -> vec3<f32> {
    var c = textureSample(t,bilinear,uv).rgb*WEIGHT[0];
    for (var k = 1; k < 5; k++) {
        let o = step*OFFSET[k];
        c += (textureSample(t,bilinear,uv+o).rgb+textureSample(t,bilinear,uv-o).rgb)*WEIGHT[k];
    }
    return c;
}
@fragment
fn blur_h0(i: FullscreenVertexOutput) -> @location(0) vec4<f32> { return vec4<f32>(blur(source,i.uv,step_for(source,vec2<f32>(1.0,0.0),vec2<f32>(1.0,0.0))),1.0); }
@fragment
fn blur_h1(i: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    // Skate 3 multiplies this one by f_BloomMultiply.y (0x827efd98's b2).
    let gain = select(1.0,grade[47].z,grade[47].x > 0.0);
    return vec4<f32>(blur(source,i.uv,step_for(source,vec2<f32>(4.0,0.0),vec2<f32>(1.0,0.0)))*gain,1.0);
}
@fragment
fn blur_v1(i: FullscreenVertexOutput) -> @location(0) vec4<f32> { return vec4<f32>(blur(source,i.uv,step_for(source,vec2<f32>(0.0,4.0),vec2<f32>(0.0,1.0))),1.0); }

// Combine: level 0 blurred vertically, times bloom_1, plus level 1 times bloom_2. Alpha keeps the lens
// ghosts off the screen edges: 1 - saturate(5*|max(|2uv-1| - 0.6, 0)| - 1).
@fragment
fn combine(i: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    // Skate 2's vertical offsets are its horizontal ones x 1152/640 = 1.8 (its fixed render size), whatever
    // the window's aspect. Skate 1 (B3DF42D9): saturate(blur*g_BloomMultiply.x + level 1*.y).
    let c = saturate(grade[2].z*blur(source,i.uv,step_for(source,vec2<f32>(0.0,1.8),vec2<f32>(0.0,1.0)))+grade[2].w*textureSample(second,bilinear,i.uv).rgb);
    let edge = saturate(abs(i.uv*2.0-1.0)-0.6);
    return vec4<f32>(c,1.0-saturate(length(edge)*5.0-1.0));
}

// Lens ghosts: five mirrored, scaled copies of the bloom's luminance, tinted and masked by the
// combine pass's alpha, added to the bloom.
const GHOST_SCALE = array<f32,5>(-0.8,-1.6,-1.75,-2.0,-6.0);
const GHOST_TINT = array<vec3<f32>,5>(vec3<f32>(0.2,0.0,0.0),vec3<f32>(0.2,0.0,0.0),vec3<f32>(0.0,0.2,0.0),vec3<f32>(0.0,0.0,0.2),vec3<f32>(0.2,0.0,0.0));
@fragment
fn ghosts(i: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    // Skate 1 has none; Skate 3's six (vault Ghost UV scales and tints) aren't ported.
    if grade[3].y > 0.0 || grade[47].x > 0.0 { return vec4<f32>(textureSample(source,bilinear,i.uv).rgb,1.0); }
    var g = vec3<f32>(0.0);
    for (var k = 0; k < 5; k++) {
        let t = textureSample(source,bilinear,(i.uv-0.5)*GHOST_SCALE[k]+0.5);
        g += dot(t.rgb,vec3<f32>(0.33))*t.a*GHOST_TINT[k];
    }
    return vec4<f32>(textureSample(source,bilinear,i.uv).rgb+saturate(g),1.0);
}
