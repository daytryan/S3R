// Native multiplicative evaluator (827F0D00) and centre weighting (827F0B78).
// Portable meter: 16x16 bilinear HDR samples, not the console resolve surface.
// Skate 2 maps (timing.z): Skate 2's evaluator 0x82325a48's weights and tick: (x*y)^2 with x and y
// running 0..2 from the top-left corner, divided by the pixel count, state *= 1 + damping*(target -
// average) once a frame, clamped to min..max. Its input is not the scene: byte 0 (alpha) of the 32x16
// bloom slot from two frames ago, a constant of its down/blur passes. retail_grading.rs pins min = max
// at the zone's max, as every capture shows (GRADING_AUDIT_S2.md 6.7); this average is then unused.
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;
struct Settings { tuning: vec4<f32>, timing: vec4<f32> }
@group(0) @binding(2) var<uniform> settings: Settings;
@group(0) @binding(3) var<storage, read_write> state: vec4<f32>;
// The scene depth (retail_exposure.rs depth_layout), multisampled with MSAA: Skate 1's sky alpha.
#ifdef DEPTH_MSAA
@group(1) @binding(0) var depth: texture_depth_multisampled_2d;
#else
@group(1) @binding(0) var depth: texture_depth_2d;
#endif
// x: near plane; yz: the main pass's share of the depth texture (retail_tone.wgsl view_near).
@group(1) @binding(1) var<uniform> view_near: vec4<f32>;
var<workgroup> sums: array<f32, 256>;
@compute @workgroup_size(256)
fn meter(@builtin(local_invocation_index) id: u32) {
    if id == 0u && state.z != settings.timing.y {
        state.x = clamp(2.5, settings.tuning.y, settings.tuning.z);
        state.z = settings.timing.y;
    }
    storageBarrier();
    let skate2=settings.timing.z>0.5;
    let uv=(vec2<f32>(f32(id%16u),f32(id/16u))+0.5)/16.0;
    let xy=uv*2.0-1.0;
    let weight=select(pow(1.0-abs(xy.x*xy.y),2.0),pow(4.0*uv.x*uv.y,2.0),skate2);
    // Material output currently includes baseline exposure 2.5. Meter the
    // current adapted image, with a portable Rec.709 luminance conversion.
    let c=max(textureSampleLevel(source,source_sampler,uv,0.0).rgb,vec3<f32>(0.0))*state.x/2.5;
    var lum=min(dot(c,vec3<f32>(0.2126,0.7152,0.0722)),16.0);
    if skate2 {
        let t=saturate(1.0-c);
        let display=saturate((max(c*0.25+0.75,vec3<f32>(1.0))-t*t)*0.5);
        lum=saturate(dot(display,vec3<f32>(0.3,0.4,0.3)));
    }
    // Skate 1 (timing.w; bloom tail 0x8273ef80): the mean of its bright pass's alpha (C3FA8514),
    // saturate(dot(rgb, (.15, .25, .1))*(1 + 2a)) of its 8-bit scene: rgb = saturate(d), a = saturate(lum(d)/8),
    // d = its world shaders' display (unclamped) = the engine's display at 2.5 (old_skies.py E_REF) x exposure/2.5.
    // Every pixel counts the same (its 32x16 read-back, row skip 0).
    let skate1=settings.timing.w>0.5;
    if skate1 {
        let r=c*2.5/max(state.x,1e-4);
        let t=saturate(1.0-r);
        let d=sqrt(max((max(r*0.25+0.75,vec3<f32>(1.0))-t*t)*0.5,vec3<f32>(0.0)))*1.41*state.x/2.5;
        // The sky's alpha is sky.psh's dot(colour before exposure, sky_bloom_intensity) (retail_grading.wgsl bright).
        let px=min(vec2<i32>(uv*vec2<f32>(textureDimensions(depth))*view_near.yz),vec2<i32>(textureDimensions(depth))-1);
        let a=select(dot(d,vec3<f32>(0.3,0.6,0.1))/8.0,dot(d*2.5/max(state.x,1e-4),vec3<f32>(0.09603,0.11569,0.12179)),textureLoad(depth,px,0)<1e-6);
        lum=saturate(dot(saturate(d),vec3<f32>(0.15,0.25,0.1))*(1.0+2.0*saturate(a)));
    }
    sums[id]=lum*select(weight,1.0,skate1);
    workgroupBarrier();
    for(var stride=128u;stride>0u;stride/=2u) {
        if id<stride { sums[id]+=sums[id+stride]; }
        workgroupBarrier();
    }
    if id==0u {
        let average=sums[0]/256.0*select(2.515,1.0,skate2||skate1);
        let factor=max(0.001,1.0+settings.tuning.w*(settings.tuning.x-average));
        // Native evaluator ticks per frame; adapt to a 30 Hz time basis here.
        state.x=clamp(state.x*pow(factor,settings.timing.x*30.0),settings.tuning.y,settings.tuning.z);
        state.y=average;
    }
}
