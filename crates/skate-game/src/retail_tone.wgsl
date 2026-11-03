#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput
#import skate_retail::weather
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;
@group(0) @binding(2) var<storage, read> exposure: vec4<f32>;
// Exposure tuning and timing, then weather: rain strength, lightning flash, clock,
// snowfall.
@group(0) @binding(3) var<uniform> settings: array<vec4<f32>, 3>;
// Skate 1/2 maps' grading (retail_grading.rs): bloom, the colour cubes fading from/to, lens vignette (Skate 1:
// grunge texture), parameters [0] enabled, bloom, blend, lookup scale; [1] lookup offset, lens, fisheye centre
// scale, fisheye r^4 scale; [2] bloom; [3] bloom saturation, Skate 1 (film); Skate 1 (retail_grading.rs
// skate1_params): [4] adjust_hv_ratio, warp and vignette powers; [5] hv_upscaler, hv_warp_multiplier; [6] far depth
// of field from, to (metres), bDOFLerpScale; [7..12) far grade set A, [12..17) near set B (FxMatrix rows, saturate
// colour + intensity, post modulate + brightness); [17] NIS near, far, grunge radius, 1/feather; [18] grunge
// centre, vignette multiplier; [19] maxCoC ramp; [20..24) world_from_clip, [24..28) deck_from_world, [28..32) last
// frame's clip_from_world; [32] deck AO XLen, ZLen, Radius, HeightFallOff, [33] its Colour, on; [34] motion blur
// intensity, enable; [35..47) skater stencil capsules (end a, radius), (end b, 0); [47] Skate 3's shot bloom (the bloom
// passes'; here only [0].x and [3].w: Skate 3 adds its bloom like Skate 2, with no cube). Then Skate 1's composite (E8DC)
// and half-size depth-of-field targets (black elsewhere).
@group(1) @binding(0) var bloom: texture_2d<f32>;
@group(1) @binding(1) var cube_from: texture_3d<f32>;
@group(1) @binding(2) var cube_to: texture_3d<f32>;
@group(1) @binding(3) var vignette: texture_2d<f32>;
@group(1) @binding(4) var grade_sampler: sampler;
@group(1) @binding(5) var<uniform> grade: array<vec4<f32>, 48>;
@group(1) @binding(6) var film: texture_2d<f32>;
@group(1) @binding(7) var dof: texture_2d<f32>;
// The scene depth (multisampled with MSAA) and the camera's near plane.
#ifdef DEPTH_MSAA
@group(2) @binding(0) var depth: texture_depth_multisampled_2d;
#else
@group(2) @binding(0) var depth: texture_depth_2d;
#endif
// x: the camera's near plane; yz: the main pass's share of the depth texture (1 but under DLSS, whose main pass draws
// the top-left render-resolution corner and upscales after; retail_exposure.rs).
@group(2) @binding(1) var<uniform> view_near: vec4<f32>;
// #36: Skate 2's godray.spotlight beams (retail_godray.rs) at the main pass's resolution, 1x1 zero without any.
@group(2) @binding(2) var godray: texture_2d<f32>;

// The beams added (ONE/ONE) to the frame at uv, which holds the world's encoded sqrt(tm/2) (godrayspotlight.xml).
fn godray_at(uv: vec2<f32>) -> vec3<f32> {
    let size = vec2<i32>(textureDimensions(godray));
    return textureLoad(godray,clamp(vec2<i32>(uv*vec2<f32>(size)*view_near.yz),vec2<i32>(0),size-1),0).rgb;
}

// Skate 1's depth at full-size pixel `px` (reverse-Z: near / distance), its world position (uv of that depth's
// pixel), and its skater stencil s = saturate(2 stencil - 1): 1 inside the skater's capsules.
fn skate1_depth(px: vec2<i32>) -> f32 {
    let q = vec2<i32>(vec2<f32>(px)*view_near.yz);
    return max(textureLoad(depth,clamp(q,vec2<i32>(0),vec2<i32>(textureDimensions(depth))-1),0),1e-7);
}
fn skate1_world(uv: vec2<f32>, d: f32) -> vec3<f32> {
    let h = mat4x4<f32>(grade[20],grade[21],grade[22],grade[23])*vec4<f32>(uv.x*2.0-1.0,1.0-uv.y*2.0,d,1.0);
    return h.xyz/h.w;
}
fn skate1_stencil(p: vec3<f32>) -> f32 {
    for (var i = 35u; i < 47u; i += 2u) {
        let a = grade[i].xyz;
        let ab = grade[i+1u].xyz-a;
        if distance(p,a+ab*saturate(dot(p-a,ab)/max(dot(ab,ab),1e-8))) < grade[i].w { return 1.0; }
    }
    return 0.0;
}

// E8DC/3D17's motion vector: (-x, y, 1)*z through g_MotionBlurMatrix to last frame, w' = max(w, z/2), then
// (-x, y) - last; in uv, 2(uv' - uv).
fn skate1_motion(uv: vec2<f32>, d: f32) -> vec2<f32> {
    let c = mat4x4<f32>(grade[28],grade[29],grade[30],grade[31])*vec4<f32>(skate1_world(uv,d),1.0);
    let last = c.xy/max(c.w,0.5*view_near.x/d);
    return vec2<f32>(last.x-(uv.x*2.0-1.0),(1.0-uv.y*2.0)-last.y);
}

// 0x82737bf0's 9 taps for both blur passes: at (k - 4)*2/9*maxCoC texels, weights (1 - ((k - 4)*2/9)^2)/maxCoC + .05
// normalised (their sum: (9 - 240/81)/maxCoC + .45). maxCoC = 1 + 4*clamp((exposure - .48)/3.52) (grade[19]).
fn skate1_tap(k: u32) -> vec2<f32> {
    let coc = grade[19].x+(grade[19].y-grade[19].x)*saturate((exposure.x/2.5-grade[19].z)/(grade[19].w-grade[19].z));
    let u = (f32(k)-4.0)*2.0/9.0;
    return vec2<f32>(u*coc,((1.0-u*u)/coc+0.05)/((9.0-240.0/81.0)/coc+0.45));
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

// One grade set of E8DC16147EF280C5 (grade[i..i+5)): f = FxMatrix (c0, 1), saturate(f*(lum(f)*intensity + colour)),
// then *PostModulate + PostBrightness (both over PostQuantise; its quantise vignette never reaches the screen).
fn skate1_set(c0: vec3<f32>, i: u32) -> vec3<f32> {
    let f = vec3<f32>(dot(grade[i].xyz,c0)+grade[i].w,dot(grade[i+1u].xyz,c0)+grade[i+1u].w,dot(grade[i+2u].xyz,c0)+grade[i+2u].w);
    let x = saturate(f*(dot(f,vec3<f32>(0.3,0.6,0.1))*grade[i+3u].w+grade[i+3u].xyz));
    return x*grade[i+4u].x+grade[i+4u].y;
}

// E8DC: the 8-bit scene screen-blended with the bloom, graded by the near set (B) fading into the far set (A) from
// NIS near to far (metres). Its stencil lerp is 0.
fn skate1_composite(c: vec3<f32>, glow: vec3<f32>, z: f32) -> vec3<f32> {
    let scr = saturate(skate1_display(c));
    let c0 = scr+(1.0-scr)*glow;
    // Settings > World > Colour Grading off: no grade sets.
    if grade[3].z <= 0.0 { return saturate(c0); }
    let near = skate1_set(c0,12u);
    return saturate(mix(near,skate1_set(c0,7u),saturate((z-grade[17].x)/(grade[17].y-grade[17].x))));
}

// E8DC16147EF280C5 into its 8-bit target: the composite times the deck AO, P = deck_from_world*position:
// lerp(Colour, 1, saturate(1 - ao)), ao = 0 above P.y > .04, else (Radius > dist ? 1 - saturate(dist/Radius) : 0)*
// (1 - saturate((.08 - P.y)*HeightFallOff)), dist = |saturate(|P.xz| - (XLen, ZLen))|. Alpha: the larger of the far
// depth of field saturate(saturate((z - 60)/(300 - 60))*bDOFLerpScale) and the motion blur
// saturate(enable*saturate(45*intensity*|mv|)*(1 - m)), m = saturate(.2*(10 - z)*s) keeping the near skater sharp.
@fragment
fn skate1_film(i: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let px = vec2<i32>(i.position.xy);
    let d = skate1_depth(px);
    let z = view_near.x/d;
    var g = skate1_composite(textureLoad(source,px,0).rgb,textureSampleLevel(bloom,grade_sampler,i.uv,0.0).rgb*grade[3].w,z);
    let p = skate1_world(i.uv,d);
    if grade[33].w > 0.0 {
        let q = (mat4x4<f32>(grade[24],grade[25],grade[26],grade[27])*vec4<f32>(p,1.0)).xyz;
        let dist = length(saturate(abs(q.xz)-grade[32].xy));
        let ao = select(0.0,1.0-saturate(dist/grade[32].z),grade[32].z > dist)*(1.0-saturate((0.08-q.y)*grade[32].w));
        g *= mix(grade[33].xyz,vec3<f32>(1.0),saturate(1.0-select(ao,0.0,q.y > 0.04)));
    }
    let m = saturate(0.2*(10.0-z)*skate1_stencil(p));
    let far = saturate(saturate((z-grade[6].x)/(grade[6].y-grade[6].x))*grade[6].z);
    let blur = saturate(grade[34].y*saturate(45.0*grade[34].x*length(skate1_motion(i.uv,d)))*(1.0-m));
    return vec4<f32>(g,max(far,blur));
}

// 66EDF2B4AF9B8974: the composite's 2x2 full-size texels into one half-size texel, each weighted
// saturate(f + .01)*(1 - m) + .001 by its far focus f = saturate((z - 60)/240) (g_focalConstantsFar; the near set,
// (.002, -500), is never chosen); alpha .25 x the weights' sum. ponytail: its texcoord offsets (+.5 and +1.5
// texels of 1280x720) are read as the texels' centres.
@fragment
fn skate1_down(i: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let base = vec2<i32>(i.position.xy)*2;
    let size = vec2<f32>(textureDimensions(film));
    var sum = vec3<f32>(0.0);
    var total = 0.0;
    for (var k = 0; k < 4; k++) {
        let px = base+vec2<i32>(k&1,k>>1);
        let d = skate1_depth(px);
        let z = view_near.x/d;
        let m = saturate(0.2*(10.0-z)*skate1_stencil(skate1_world((vec2<f32>(px)+0.5)/size,d)));
        let w = saturate(saturate((z-grade[6].x)/(grade[6].y-grade[6].x))+0.01)*(1.0-m)+0.001;
        sum += textureLoad(film,min(px,vec2<i32>(size)-1),0).rgb*w;
        total += w;
    }
    return vec4<f32>(sum/total,0.25*total);
}

// 3D177FB281E92888, the horizontal pass with the motion blur: r = max(alpha, .3), M = 45*intensity*mv at the texel's
// depth, L = saturate(|M|), 9 taps at uv + (lerp(r, M.x, L), M.y*L)*t/width (both axes over the width), each weighed
// (saturate(z + L)*alpha + .0001); taps 4 and 5 take taps 3's and 4's weights, as its code does. Alpha
// max(alpha, 1/255).
@fragment
fn skate1_blur_h(i: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(dof));
    let centre = textureSampleLevel(dof,grade_sampler,i.uv,0.0).a;
    let px = vec2<i32>(i.position.xy)*2;
    let mv = 45.0*grade[34].x*skate1_motion((vec2<f32>(px)+0.5)/vec2<f32>(textureDimensions(depth)),skate1_depth(px));
    let l = saturate(length(mv));
    let s = vec2<f32>(mix(max(centre,0.3),mv.x,l),mv.y*l)/size.x;
    var sum = vec3<f32>(0.0);
    var total = 0.0;
    for (var k = 0u; k < 9u; k++) {
        let c = textureSampleLevel(dof,grade_sampler,i.uv+s*skate1_tap(k).x,0.0);
        let w = saturate(skate1_tap(select(k,k-1u,k == 4u || k == 5u)).y+l)*c.a+0.0001;
        sum += c.rgb*w;
        total += w;
    }
    return vec4<f32>(sum/total,max(centre,1.0/255.0));
}

// 9B5860EB8BBA8E08, the vertical pass: 9 taps at uv + (0, t*max(alpha, .3)/height), weighed z*alpha; alpha kept.
@fragment
fn skate1_blur_v(i: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(dof));
    let centre = textureSampleLevel(dof,grade_sampler,i.uv,0.0).a;
    var sum = vec3<f32>(0.0);
    var total = 0.0;
    for (var k = 0u; k < 9u; k++) {
        let t = skate1_tap(k);
        let c = textureSampleLevel(dof,grade_sampler,i.uv+vec2<f32>(0.0,t.x*max(centre,0.3)/size.y),0.0);
        sum += c.rgb*t.y*c.a;
        total += t.y*c.a;
    }
    return vec4<f32>(sum/total,centre);
}

// Skate 1's final pass B699240D: p = (uv - .5)*adjust, d2 = |p|^2, the composite read at .5 + p*(1 + warp*d2^power.x)*
// upscale, mixed toward the depth-of-field blur by the composite's alpha, times the vignette
// 1 - multiplier*d2^power.y, through the colour cube (g_bTint) and darkened by the grunge texture as
// saturate((|2(uv - centre)| - radius)*feather) fades it in. Its noise term is 1 and final matrix identity in play.
fn skate1(uv: vec2<f32>) -> vec3<f32> {
    let p = (uv-0.5)*grade[4].xy;
    let d2 = max(dot(p,p),1e-12);
    let at = 0.5+p*(1.0+grade[5].zw*pow(d2,grade[4].z))*grade[5].xy;
    let f = textureSampleLevel(film,grade_sampler,at,0.0);
    var g = mix(f.rgb,textureSampleLevel(dof,grade_sampler,at,0.0).rgb,f.a);
    let size = vec2<f32>(textureDimensions(film));
    let w = settings[2];
    if w.x > 0.0 || w.w > 0.0 {
        let lens = weather::screen_drop(uv,size.x/size.y,w.z,w.x);
        if lens.z > 0.0 { g = mix(g,textureSampleLevel(film,grade_sampler,uv+lens.xy,0.0).rgb*1.04,lens.z)*(1.0-0.3*lens.z*lens.w); }
    }
    g = saturate(g*(1.0-grade[18].z*pow(d2,grade[4].w)));
    let look = g*grade[0].w+grade[1].x;
    if grade[3].z > 0.0 { g = mix(textureSampleLevel(cube_from,grade_sampler,look,0.0).rgb,textureSampleLevel(cube_to,grade_sampler,look,0.0).rgb,grade[0].z); }
    let grunge = saturate((length((uv-grade[18].xy)*2.0)-grade[17].z)*grade[17].w);
    return g*(1.0-saturate(grunge*textureSampleLevel(vignette,grade_sampler,at,0.0).rgb));
}

// Bevy's final output attachment performs sRGB encoding. The retail curve already includes gamma; invert sRGB here
// to avoid encoding twice. The reference's final tone pass is opaque. Material coverage has already been resolved;
// carrying it into the presentation blit would composite foliage a second time against the window's clear colour.
fn output(gamma: vec3<f32>) -> vec4<f32> {
    return vec4<f32>(select(gamma/12.92,pow((gamma+0.055)/1.055,vec3<f32>(2.4)),gamma>vec3<f32>(0.04045)),1.0);
}

@fragment
fn fragment(i: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    if grade[3].y > 0.0 { return output(skate1(i.uv)); }
    // Skate 2's last pass draws the frame through a fisheye: uv' = .5 + (uv - .5)*(k0 + k1*r^4).
    let o = i.uv-0.5;
    let r2 = dot(o,o);
    let uv = select(i.uv,0.5+o*(grade[1].z+grade[1].w*r2*r2),grade[1].y > 0.0);
    var c = textureSample(source,source_sampler,uv).rgb;
    let w = settings[2];
    let size = vec2<f32>(textureDimensions(source));
    if w.x > 0.0 || w.w > 0.0 {
        // Rain drops, or snow melted on the lens (less often: no rain amount).
        let lens = weather::screen_drop(i.uv,size.x/size.y,w.z,w.x);
        if lens.z > 0.0 { c = mix(c,textureSampleLevel(source,source_sampler,i.uv+lens.xy,0.0).rgb*1.04,lens.z)*(1.0-0.3*lens.z*lens.w); }
    }
    // Lightning lights the whole view at once, after the exposure meter sampled it.
    let xe = max(c*exposure.x/2.5,vec3<f32>(0.0))*weather::gloom(w.x)*weather::flash_gain(w.y)+w.y*0.02*vec3<f32>(0.85,0.9,1.0);
    let t = saturate(1.0-xe);
    let tm = max(xe*0.25+0.75,vec3<f32>(1.0))-t*t;
    // #36: the frame's 8-bit sqrt(tm/2) plus the beams (saturate((e + g)*1.41) equals the 8-bit clamp's).
    var gamma = saturate((sqrt(max(tm*0.5,vec3<f32>(0.0)))+godray_at(uv))*1.41);
    if grade[0].x > 0.0 {
        // The old games' final pass: display value plus bloom, looked up in the zone's colour cube.
        let g = saturate(gamma+textureSampleLevel(bloom,grade_sampler,uv,0.0).rgb*grade[3].w);
        let at = g*grade[0].w+grade[1].x;
        gamma = select(g,mix(textureSampleLevel(cube_from,grade_sampler,at,0.0).rgb,textureSampleLevel(cube_to,grade_sampler,at,0.0).rgb,grade[0].z),grade[3].z > 0.0);
        if grade[1].y > 0.5 {
            // Vignette gradient at 1 - (distance from the centre, x scaled by 9D5F41FC's literal 16/9 whatever
            // the window's aspect); corners go dark. Skate 3's scene lens (flag 0.25) has the warp alone.
            let d = length(vec2<f32>(o.x*16.0/9.0,o.y));
            gamma *= textureSampleLevel(vignette,grade_sampler,vec2<f32>(1.0-d,0.5),0.0).rgb;
        }
    }
    return output(gamma);
}

// #33: the jumbotron feed (retail_exposure.rs FEED). Retail's videoscreen samples the scene target its tone pass
// resolved (TU3 827E596C, Skate 2 8268FFE0), which holds sqrt(tm/2): the curve above before its final 1.41, bloom and
// grade. Flipped in V, as the converted maps flip their V and textures (tools/asset_pipeline/map_writer.py).
@fragment
fn jumbotron(i: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let c = textureSampleLevel(source,source_sampler,vec2<f32>(i.uv.x,1.0-i.uv.y),0.0).rgb;
    let xe = max(c*exposure.x/2.5,vec3<f32>(0.0));
    let t = saturate(1.0-xe);
    let tm = max(xe*0.25+0.75,vec3<f32>(1.0))-t*t;
    return vec4<f32>(sqrt(max(tm*0.5,vec3<f32>(0.0))),1.0);
}
