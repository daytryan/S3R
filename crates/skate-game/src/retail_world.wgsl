#import bevy_pbr::{forward_io::VertexOutput, mesh_view_bindings as frame}
#import bevy_pbr::shadows::fetch_directional_shadow

#import skate_retail::material_bindings as bindings
#import skate_retail::weather
#ifdef BINDLESS
#import bevy_pbr::mesh_bindings::mesh
#endif

// perf 11-01 (SKATE_RETAIL_SPECIALIZE, RetailKey::path): a pipeline per game and family group, so each compiles
// only its own path below; the flags still say the same, so the output is the same.
#ifdef RETAIL_SPECIALIZED
const KNOWN: bool = true;
#else
const KNOWN: bool = false;
#endif
#ifdef RETAIL_GAME_S1
const IS_S1: bool = true;
#else
const IS_S1: bool = false;
#endif
#ifdef RETAIL_GAME_S2
const IS_S2: bool = true;
#else
const IS_S2: bool = false;
#endif
#ifdef RETAIL_GROUP_WATER
const G_WATER: bool = true;
#else
const G_WATER: bool = false;
#endif
#ifdef RETAIL_GROUP_TREE
const G_TREE: bool = true;
#else
const G_TREE: bool = false;
#endif
#ifdef RETAIL_GROUP_LAMP
const G_LAMP: bool = true;
#else
const G_LAMP: bool = false;
#endif
#ifdef RETAIL_GROUP_WORLD
const G_WORLD: bool = true;
#else
const G_WORLD: bool = false;
#endif
const MAY_WATER: bool = !KNOWN || G_WATER;
const MAY_TREE: bool = !KNOWN || G_TREE;
const MAY_LAMP: bool = !KNOWN || G_LAMP;
const MAY_WORLD: bool = !KNOWN || G_WORLD;

// Textures and base UVs both have V flipped by the exporter. Scale in the
// authored coordinate system, then return to the flipped texture rows.
fn scaled_uv(uv: vec2<f32>, scale: f32) -> vec2<f32> {
    return vec2<f32>(uv.x*scale, 1.0-(1.0-uv.y)*scale);
}

// Skate 2 GetLightMap (incandescent): four half-texel taps averaged, then squared.
// Installed pages are page² = 3·Pc²·C, so 4.5·Pc²·C = 1.5·page².
fn s2_lightmap4(slot: u32, uv: vec2<f32>) -> vec3<f32> {
    let t = 0.5/vec2<f32>(bindings::lightmap_dimensions(slot));
    let l = (bindings::sample_lightmap(slot,uv+t,0.0).rgb+bindings::sample_lightmap(slot,uv-t,0.0).rgb
        +bindings::sample_lightmap(slot,uv+t*vec2<f32>(-1.0,1.0),0.0).rgb
        +bindings::sample_lightmap(slot,uv+t*vec2<f32>(1.0,-1.0),0.0).rgb)*0.25;
    return 1.5*l*l;
}

// Skate 1 (GRADING_AUDIT_S1.md part 5). Its world shaders write lerp(lit, fog_colour, Fog)*exposure in gamma space;
// the engine displays xe at g(xe)*exposure/2.5 (retail_grading.wgsl skate1_display), so a Skate 1 colour V is
// written as xe = g^-1(V). g(c) = 1.41*sqrt(tm/2), tm = c(2 - c) up to 1, then c/4 + 3/4.
fn s1_tone(c: vec3<f32>) -> vec3<f32> {
    let t = saturate(1.0-c);
    return sqrt(max((max(c*0.25+0.75,vec3<f32>(1.0))-t*t)*0.5,vec3<f32>(0.0)))*1.41;
}
fn s1_untone(v: vec3<f32>) -> vec3<f32> {
    let y2 = (v/1.41)*(v/1.41);
    return select(8.0*y2-3.0,1.0-sqrt(max(1.0-2.0*y2,vec3<f32>(0.0))),2.0*y2 <= vec3<f32>(1.0));
}
// Skate 1 GetLightMap: four half-texel taps averaged. The installed page already holds LUT(lm_val) (its 32-texel
// lightmapLUTTexture). The LUT is piecewise linear, so averaging after it equals averaging before except where the
// four taps straddle one of its two knees.
fn s1_lightmap4(slot: u32, uv: vec2<f32>) -> vec3<f32> {
    let t = 0.5/vec2<f32>(bindings::lightmap_dimensions(slot));
    return (bindings::sample_lightmap(slot,uv+t,0.0).rgb+bindings::sample_lightmap(slot,uv-t,0.0).rgb
        +bindings::sample_lightmap(slot,uv+t*vec2<f32>(-1.0,1.0),0.0).rgb
        +bindings::sample_lightmap(slot,uv+t*vec2<f32>(1.0,-1.0),0.0).rgb)*0.25;
}

@fragment
fn fragment(i: VertexOutput) -> @location(0) vec4<f32> {
#ifdef BINDLESS
    let slot = mesh[i.instance_index].material_and_lightmap_bind_group_slot & 0xffffu;
    let index = bindings::indices[slot];
    let p = bindings::params[index.params];
    let frame_state = bindings::frames[index.frame_state];
#else
    let slot = 0u;
    let p = bindings::p;
    let frame_state = bindings::frame_state;
#endif
    // Visual-only weather (0 when clear): rain, snow, and the cloud either brings.
    let rain = frame_state.weather.x;
    let snow = frame_state.snow.x;
    let clouds = max(rain,snow);
    let fam = u32(p.mode.x);
    let flags = u32(p.mode.y);
    // Skate 2 lighting (Definition::build, package evidence; GRADING_AUDIT_S2.md part 2).
    let s2 = select((flags & 1024u) != 0u, IS_S2, KNOWN);
    // Skate 1 lighting (Definition::build, package evidence; GRADING_AUDIT_S1.md part 5).
    let s1 = select((flags & 16384u) != 0u, IS_S1, KNOWN);
    var diffuse_uv=i.uv;
    if fam==14u { diffuse_uv+=fract(frame_state.clock.x*p.water[1].xy*vec2<f32>(1.0,-1.0)); }
    let a = bindings::sample_diffuse(slot, diffuse_uv);
    // Skate 1 lightmap.psh / transparentlightmap.psh take four taps; tree.psh one (GetLightMap_SingleTap).
    let four_taps = s1 && fam != 9u && fam != 10u;
    // FPS pass 10-23 (SKATE_FPS_LM1): the single tap only where something reads it; four-tap families never do.
    // An explicit level, so it may sit in a branch.
    var lm = vec3<f32>(0.0);
#ifdef RETAIL_LM_TAP_ALWAYS
    lm = bindings::sample_lightmap(slot, i.uv_b, 0.0).rgb;
#else
    if !four_taps { lm = bindings::sample_lightmap(slot, i.uv_b, 0.0).rgb; }
#endif
    // Sample before alpha rejection: implicit derivatives must be uniform.
    var nm = vec3<f32>(0.5,0.5,1.0);
    var detail = vec2<f32>(0.5);
    var overlay_sample = vec3<f32>(0.5);
    var art = vec4<f32>(0.0);
    var masks = vec3<f32>(0.0);
    if (flags & 1u) != 0u && (fam <= 6u || fam == 13u) { nm = bindings::sample_normal_map(slot,i.uv).rgb; }
    if (flags & 2u) != 0u && fam != 2u {
        detail = bindings::sample_detail_map(slot,scaled_uv(i.uv,p.surface.z)).rg;
    }
    if fam == 5u || fam == 6u || fam == 13u {
        // The reference folds the reflective material's constant detail texel.
        if (flags & 128u) != 0u { detail = bindings::load_detail(slot,vec2<i32>(0),0).rg; }
    }
    if (flags & 4u) != 0u { overlay_sample = bindings::sample_macro_map(slot,scaled_uv(i.uv,p.surface.x)).rgb; }
    // Vertex colours carry decal UVs; only batches that sample them have any.
#ifdef VERTEX_COLORS
    if (flags & 8u) != 0u && (fam == 3u || fam == 4u) { art = bindings::sample_decal_map(slot,i.color.xy); }
#endif
    if (flags & 16u) != 0u { masks = bindings::sample_specular_map(slot,i.uv).rgb; }
    // Skate 2 terrain (bit 32768, GRADING_AUDIT_S2.md §7): the slope maps at uv·diffuse2UVScale (surface.z).
    // terrain.default keeps specular2 in the decal slot; decalterrain reads i_specular at both UVs.
    let terrain = s2 && (flags & 32768u) != 0u;
    var slope = vec3<f32>(0.0);
    var slope_nm = vec3<f32>(0.5,0.5,1.0);
    var slope_masks = vec3<f32>(0.0);
    if terrain {
        let uv2 = scaled_uv(i.uv,p.surface.z);
        slope = bindings::sample_diffuse2(slot,uv2).rgb;
        if (flags & 128u) != 0u { slope_nm = bindings::sample_detail_map(slot,uv2).rgb; }
        if fam == 3u {
            if (flags & 16u) != 0u { slope_masks = bindings::sample_specular_map(slot,uv2).rgb; }
        } else if (flags & 8u) != 0u { slope_masks = bindings::sample_decal_map(slot,uv2).rgb; }
    }
    var wn = normalize(i.world_normal);
    // Before normal mapping: how much the surface faces the sky; level ground
    // is where rain pools.
    let sky_facing = wn.y;
    let level = saturate((sky_facing-0.75)*5.0);
    let dp1 = dpdx(i.world_position.xyz);
    let dp2 = dpdy(i.world_position.xyz);
    // Exporter flips V and texture rows; recover original UV derivatives.
    let du1 = dpdx(i.uv * vec2<f32>(1.0,-1.0));
    let du2 = dpdy(i.uv * vec2<f32>(1.0,-1.0));
    let dp2p = cross(dp2,wn);
    let dp1p = cross(wn,dp1);
    var kt = dp2p * du1.x + dp1p * du2.x;
    var kb = dp2p * du1.y + dp1p * du2.y;
    kt *= -inverseSqrt(max(dot(kt,kt),1e-12));
    kb *= inverseSqrt(max(dot(kb,kb),1e-12));
#ifdef VERTEX_TANGENTS
    if dot(i.world_tangent.xyz,i.world_tangent.xyz) > 0.01 {
        kt = normalize(i.world_tangent.xyz);
        kb = normalize(cross(wn,kt)) * i.world_tangent.w;
    }
#endif
    let rpos = i.world_position.xyz - frame::view.world_position;
    let vd = -normalize(rpos);
    // Authored render-location direction for the tangent-space sign terms.
    // This does not add directional light energy or a shadow source.
    let sun = p.sun_direction.xyz;
    var d = a.rgb*a.rgb;
    var alpha = 1.0;
    // What mode.z rejects: diffuse alpha, or a Skate 2 variant's output alpha.
    var test = a.a;
#ifndef RETAIL_NO_ALPHA_TEST
#ifndef RETAIL_LATE_DISCARD
    // perf 11-01: a cut-out texel of a tree or an alpha-tested simple material leaves before its lighting. Nothing
    // below takes implicit derivatives for these families, and only Mega Park's seasonal webs raise a tree's test.
    if test < p.mode.z && (flags & 2048u) == 0u && (fam == 7u || ((fam == 9u || fam == 10u)
        && !(frame_state.snow.z == 1.0 && weather::in_mega_park(i.world_position.xz)))) { discard; }
#endif
#endif
    var lin = vec3<f32>(0.0);
    var lm_s = lm;
    if four_taps { lm_s = s1_lightmap4(slot,i.uv_b); }
    var baked = lm_s*lm_s;
#ifdef SCREEN_SPACE_AMBIENT_OCCLUSION
    // ponytail: lightmaps already bake AO, so SSAO only half-darkens the world
    // (mostly contact shading around the skater and props). Strength is the 0.5.
    baked *= mix(1.0, textureLoad(frame::screen_space_ambient_occlusion_texture, vec2<i32>(i.position.xy), 0).r, 0.5);
#endif
    if clouds>0.0 { baked=weather::overcast(baked,clouds); }
    // Skate 2: installed pages are page² = 3·Pc²·C, so lm_val = Pc² = mean(page²)
    // and the shaders' 4.5·Pc²·C = 1.5·page².
    let s2_v = dot(baked,vec3<f32>(1.0/3.0));
    if s2 { baked *= 1.5; }
    // Sky light before the time of day and the player's shadow (snow settles by
    // it at night too), and that shadow (1 where lit).
    let sky_light = baked;
    let day = frame_state.day_light;
    var skater = 1.0;
    if frame_state.shadow.w>0.0 && (fam<=8u || fam==13u) {
        let view_z=(frame::view.view_from_world*i.world_position).z;
        for (var light_id=0u; light_id<frame::lights.n_directional_lights; light_id+=1u) {
            // The lightmapped receiver source contains only player/board casters.
            if (frame::lights.directional_lights[light_id].flags & 5u)==5u {
                skater=fetch_directional_shadow(light_id,i.world_position,wn,view_z);
                // Rain cloud softens the skater's shadow; snow keeps it for depth.
                // Skate 2 applies its own shadow colour in its branch below.
                if s1 {
                    // Skate 1 GetShadowedLightMap: lerp(min(lm_val, shadow_color.x*.5), lm_val, inv), LUT after;
                    // LUT(.24593) per channel. CSM_ShadowMap_PS fades the shadow out from 25 to 35 m
                    // (shadow_bias .z, .y), its full 3D distance.
                    let l = sqrt(baked);
                    let lit = mix(skater,1.0,0.6*weather::cover(rain));
                    let inv = 1.0-(1.0-lit)*(1.0-saturate((length(rpos)-25.0)/10.0));
                    let sl = mix(min(l,vec3<f32>(0.2184,0.2680,0.3356)),l,inv);
                    baked = sl*sl;
                } else if !s2 { baked=min(baked,vec3<f32>(mix(skater,1.0,0.6*weather::cover(rain)))+frame_state.shadow.rgb); }
                break;
            }
        }
    }
    // Time of day (retail_daynight.rs): the baked sun at this hour, after the player's
    // shadow so that darkens as baked shade does. Exact at noon.
    let s2_lit = baked;
    baked = weather::daylight(baked, day);
    if MAY_WATER && fam==14u {
        lin=d*p.water[0].y;
        // Skate 2 scrollincandescent: diffuse²·lerp(1, lightmap, m_params.x)·m_params.y.
        if s2 { lin=d*mix(vec3<f32>(1.0),weather::daylight(s2_lightmap4(slot,i.uv_b),day),p.family.x)*p.family.y; }
    } else if MAY_WATER && fam==32u {
        lin=d*p.water[0].x*weather::day_scale(day);
        alpha=saturate((i.world_position.y-p.water[0].z)/max(p.water[0].y-p.water[0].z,1e-4));
    } else if MAY_WATER && fam==31u {
        let raw_uv=vec2<f32>(i.uv.x,1.0-i.uv.y);
        let uv=raw_uv*p.water[1].w;
        let sample_uv=vec2<f32>(uv.x,1.0-uv.y);
        let c0=bindings::sample_normal_map(slot,sample_uv)*2.0-1.0;
        let c1=bindings::sample_detail_map(slot,sample_uv)*2.0-1.0;
        let pca=(vec3<f32>(dot(c0,frame_state.pca[1])+dot(c1,frame_state.pca[2]),
            dot(c0,frame_state.pca[3])+dot(c1,frame_state.pca[4]),
            dot(c0,frame_state.pca[5])+dot(c1,frame_state.pca[6]))+frame_state.pca[0].xyz)*2.0-1.0;
        var overlay=1.0;
        if (flags & 4u)!=0u {
            let uv_overlay=raw_uv*p.surface.x;
            overlay=bindings::sample_macro_map(slot,vec2<f32>(uv_overlay.x,1.0-uv_overlay.y)).r;
        }
        // ocean_defaultPS tones the PCA normal down by the macro overlay alone.
        let tonedown=2.0*p.water[1].z*saturate(2.0*overlay-0.5);
        let nt=normalize(max(abs(normalize(mix(vec3<f32>(0.0,0.0,1.0),pca,tonedown))),vec3<f32>(0.001)));
        let n=nt.xzy;
        let rv=vd-2.0*n*dot(vd,n);
        var cube=vec3<f32>(0.0);
        if (flags & 64u)!=0u { cube=bindings::sample_environment(slot,vec3<f32>(-rv.x,-rv.y,rv.z),log2(frame::view.viewport.w/640.0)).rgb; }
        let olm=bindings::sample_lightmap(slot,i.uv_b+0.01*nt.xy*vec2<f32>(1.0,-1.0),0.0).rgb;
        // Skate 2's ocean_defaultPS is this one with 4.5·Pc²·C for the lightmap and the specular times Pc² (xemu,
        // brief 10-29): installed pages are page² = 3·Pc²·C, C summing to 1.
        var olm2=olm*olm;
        var spec=1.0;
        if s2 { spec=dot(olm2,vec3<f32>(1.0/3.0)); olm2*=1.5; }
        let fres=0.25+0.75*pow(1.0-abs(dot(n,vd)),5.0);
        let u=normalize(vec3<f32>(-n.z,0.0,n.x));
        let v=cross(n,u);
        let ax=p.water[2].z/p.water[2].x;
        let ay=p.water[2].z/p.water[2].y;
        let h=normalize(sun+vd);
        let eu=dot(h,u)/ax;
        let ev=dot(h,v)/ay;
        let den=sqrt(abs(dot(n,sun)*dot(n,vd)))*12.566371*ax*ay;
        let ward=exp(-2.0*(eu*eu+ev*ev)/(1.0+dot(h,n)))/max(den,1e-6);
        lin=(cube*olm2*fres+ward*p.water[0].rgb*spec)*p.water[1].y*weather::day_scale(day);
        alpha=1.0;
    } else if MAY_WATER && (fam==30u || fam==33u) {
        let t=frame_state.clock.x;
        // Convert to original UVs for scale/scroll, then back to flipped rows.
        let raw_uv=vec2<f32>(i.uv.x,1.0-i.uv.y);
        let uv_scale=select(1.0,p.water[3].x,fam==33u);
        let uv1=raw_uv*p.water[2].xy*uv_scale+p.water[1].xy*t;
        let uv2=raw_uv*p.water[2].zw*uv_scale+p.water[1].zw*t;
        let n1=bindings::sample_normal_map(slot,vec2<f32>(uv1.x,1.0-uv1.y));
        let n2=bindings::sample_normal_map(slot,vec2<f32>(uv2.x,1.0-uv2.y));
        var vn=normalize((2.0*n1.rgb+2.0*n2.rgb-2.0)*p.water[0].xzw);
        var water_n=normalize(vn.x*kt+vn.y*kb+vn.z*wn);
        var sample_uv=i.uv;
        if fam==33u {
            let c1=bindings::sample_detail_map(slot,vec2<f32>(uv1.x,1.0-uv1.y))*2.0-1.0;
            let c2=bindings::sample_detail_map(slot,vec2<f32>(uv2.x,1.0-uv2.y))*2.0-1.0;
            let a1=n1*2.0-1.0;
            let a2=n2*2.0-1.0;
            // water_defaultPS instructions 22..54: the native mean is XYZ,
            // weights are R/G/B pairs. FrameState stores the ocean's R/B/G
            // arrangement; recover those original registers here.
            let mean=frame_state.pca[0].xzy;
            let pca1=vec3<f32>(dot(a1,frame_state.pca[1])+dot(c1,frame_state.pca[2])+mean.x,
                dot(a1,frame_state.pca[5])+dot(c1,frame_state.pca[6])+mean.y,
                dot(a1,frame_state.pca[3])+dot(c1,frame_state.pca[4])+mean.z);
            let pca2=vec3<f32>(dot(a2,frame_state.pca[1])+dot(c2,frame_state.pca[2])+mean.x,
                dot(a2,frame_state.pca[5])+dot(c2,frame_state.pca[6])+mean.y,
                dot(a2,frame_state.pca[3])+dot(c2,frame_state.pca[4])+mean.z);
            let first=vec3<f32>((pca2.x*2.0-1.0)*p.water[0].z,
                1.0+2.0*(pca2.y-1.0)*p.water[0].z,(pca2.z*2.0-1.0)*p.water[0].z);
            let second=vec3<f32>((pca1.x*2.0-1.0)*p.water[0].w,
                1.0+2.0*(pca1.y-1.0)*p.water[0].w,(pca1.z*2.0-1.0)*p.water[0].w);
            water_n=first*inverseSqrt(max(dot(first,first),1e-12));
            let refract=second*inverseSqrt(max(dot(second,second),1e-12));
            sample_uv+=0.02*refract.xz*vec2<f32>(1.0,-1.0);
            d=bindings::sample_diffuse(slot,sample_uv).rgb;
            d*=d;
            vn=water_n;
        }
        let water_lm_uv=i.uv_b+0.01*vn.xz*vec2<f32>(1.0,-1.0);
        var wlm=bindings::sample_lightmap(slot,water_lm_uv,0.0).rgb;
        if fam==33u {
            // Native tf3 fetches at all four half-texel corners, averages,
            // then squares. Squaring each tap would change baked lighting.
            let texel=0.5/vec2<f32>(bindings::lightmap_dimensions(slot));
            wlm=(bindings::sample_lightmap(slot,water_lm_uv+texel,0.0).rgb
                +bindings::sample_lightmap(slot,water_lm_uv-texel,0.0).rgb
                +bindings::sample_lightmap(slot,water_lm_uv+texel*vec2<f32>(-1.0,1.0),0.0).rgb
                +bindings::sample_lightmap(slot,water_lm_uv+texel*vec2<f32>(1.0,-1.0),0.0).rgb)*0.25;
        }
        var lml=wlm*wlm;
        if frame_state.shadow.w>0.0 {
            let vz=(frame::view.view_from_world*i.world_position).z;
            for(var id=0u;id<frame::lights.n_directional_lights;id+=1u) {
                if (frame::lights.directional_lights[id].flags & 5u)==5u {
                    // TU3's water overrides add g_envattributes[0] (material_envattributes/default) to the shadow;
                    // Skate 2's flowing water keeps the adapter floor.
                    let floor=select(vec3<f32>(0.05,0.09,0.13),frame_state.shadow.rgb,s2);
                    lml=min(lml,vec3<f32>(fetch_directional_shadow(id,i.world_position,wn,vz))+floor); break;
                }
            }
        }
        lml=weather::daylight(lml,day);
        let kd=dot(water_n,vec3<f32>(0.58*sign(sun.x),0.62*sign(sun.y),0.39))*2.39562;
        var wm=vec2<f32>(0.0);
        if (flags & 16u)!=0u { wm=saturate(bindings::sample_specular_map(slot,sample_uv).xz-p.water[3].y); }
        let reflected_light=2.0*water_n*dot(water_n,sun)-sun;
        let ks=pow(max(saturate(dot(vd,reflected_light)),1e-6),p.water[3].z);
        var spec=ks*wm.x*vec3<f32>(2.1,1.8,1.5)*saturate(lml.g-0.1);
        if (flags & 64u)!=0u {
            let rv=vd-2.0*water_n*dot(vd,water_n);
            let cube=bindings::sample_environment(slot,vec3<f32>(-rv.x,-rv.y,rv.z),log2(frame::view.viewport.w/640.0)).rgb;
            let lum=0.3*saturate(4.0*wm.y-2.6);
            spec+=cube*(lml.g+lum*(1.0-lml.g))*wm.y*1.5;
        }
        lin=(lml*kd*d+spec)*p.water[0].y;
        alpha=max(spec.g,p.water[3].w);
    } else if MAY_TREE && (fam == 9u || fam == 10u) {
        lin = d * weather::daylight(max(lm*lm*select(1.0,1.5,s2),vec3<f32>(p.family.y)),day) * p.family.x;
        if fam == 9u { lin *= p.family.z; }
        // Skate 1 tree.psh: diffuse·LUT(lm)·diffuse_scale 1.7·dotlight, Tree_Sunlight_Mid_Range (1, 0) makes dotlight 1.
        if s1 { lin = s1_untone(a.rgb*sqrt(weather::daylight(lm*lm,day))*1.7)/p.mode.w; }
        alpha = a.a;
        // Not retail (user 10-19): seasonal.rs's Halloween cobwebs and Christmas lights on the Super Ultra Mega Park's
        // trees. Web threads show across the card's gaps too.
        if frame_state.snow.z > 0.0 && weather::in_mega_park(i.world_position.xz) {
            let card = normalize(cross(dp1,dp2));
            if frame_state.snow.z == 1.0 {
                let web = weather::tree_web(i.world_position.xyz,card,max(length(dp1),length(dp2)));
                let light = weather::daylight(max(lm*lm*select(1.0,1.5,s2),vec3<f32>(p.family.y)),day)*p.family.x;
                lin = mix(lin,light*0.85,web);
                alpha = max(alpha,web);
                test = max(test,web);
            } else {
                lin += weather::fairy_lights(i.world_position.xyz,card,frame_state.clock.x,d)/p.mode.w;
            }
        }
    } else if MAY_LAMP && (fam == 11u || fam == 12u) {
        lin = d;
        // Incandescent (12) glows at night too: the street lamps and lit signs.
        if fam == 11u { lin *= weather::daylight(vec3<f32>(p.family.w),day); }
        // #33 Skate 3 baseincandescent (videoscreen's too): diffuse²·m_params.y, the vault rows in water[0]
        // (render-parameters.json): incandescent.default 1, videoscreen 0.25.
        if fam == 12u && !s2 { lin = d*p.water[0].y; }
        // Skate 2 baseincandescent: diffuse²·lerp(1, lightmap, m_params.x)·m_params.y.
        if fam == 12u && s2 {
            let l4 = s2_lightmap4(slot,i.uv_b);
            lin = d*mix(vec3<f32>(1.0),weather::daylight(l4,day),p.family.x)*p.family.y;
            // zbiasincandescentbacklit (8192) alpha-tests lerp(1, lm_val, m_params.x)·diffuse.a;
            // lm_val = Pc² = mean(l4)/1.5.
            if (flags & 8192u) != 0u { test = mix(1.0,dot(l4,vec3<f32>(2.0/9.0)),p.family.x)*a.a; }
        }
    } else if MAY_WORLD && s1 {
        // Skate 1 lightmap.psh / transparentlightmap.psh: ambient (0) + diffuse·palette (1)·LUT(lm_val)·diffuse_scale
        // (1.7), in gamma ('default' material row). Alpha as the Skate 3 path draws these families.
        lin = s1_untone(a.rgb*sqrt(baked)*1.7)/p.mode.w;
        if fam >= 7u { alpha = a.a; }
    } else if MAY_WORLD && s2 {
        // Skate 2 world pixel shaders, decoded from the retail ucode (GRADING_AUDIT_S2.md,
        // Lighting part 2): base/decalenvironment (1, 3), default/alphatestdefaultenvironment
        // (2, 7), and two variants of 1: transparentenvironment (bit 2048: vertex normal, no
        // overlay, the diffuse term times diffuse.a, output alpha diffuse.a²) and decalzbias
        // (4096: no decal, overlay, detail or cube, kd from the unnormalised normal-map texel,
        // output alpha diffuse.a). The authored sun stands in for g_vLightDir (no static source).
        let env = fam == 1u || fam == 3u;
        let clear = (flags & 2048u) != 0u;
        let zdecal = (flags & 4096u) != 0u;
        var ov = vec3<f32>(1.0);
        var kd = 1.0;
        var sh = mix(skater,1.0,0.6*weather::cover(rain));
        if env && !clear {
            var dxy = vec2<f32>(0.5);
            if !zdecal {
                if (flags & 8u) != 0u && (flags & 512u) == 0u && !terrain { d = mix(d,art.rgb*art.rgb,art.a*p.decal.x); }
                // ApplyOverlay: 2·base·overlay.
                if (flags & 256u) == 0u { ov = 2.0*saturate((overlay_sample-0.5)*p.surface.y+0.5); }
                if (flags & 2u) != 0u { dxy = detail; }
            }
            if terrain {
                // baseterrain / decalterrain: tangent normals lerp by the vertex normal's height, albedo and spec
                // masks by the mapped normal's; blend = 1 - |pow(|n.y|, m_params[1].x/.z) - .y/.w|, unclamped.
                // kd from the unnormalised lerped texel; the spec masks skip the overlay; decalterrain's decal
                // goes on after the overlay.
                let bn = 1.0-abs(pow(max(abs(i.world_normal.y),1e-6),p.family.x)-p.family.y);
                let vn = mix(nm*2.0-1.0,slope_nm*2.0-1.0,bn);
                wn = normalize(vn.x*kt+vn.y*kb+vn.z*wn);
                let bw = 1.0-abs(pow(max(abs(wn.y),1e-6),p.family.z)-p.family.w);
                d = mix(d,slope*slope,bw)*ov;
                masks = mix(masks,slope_masks,bw);
                if fam == 3u && (flags & 8u) != 0u && (flags & 512u) == 0u { d = mix(d,art.rgb*art.rgb,art.a); }
                ov = vec3<f32>(1.0);
                kd = (0.64*sign(dot(kt,sun))*vn.x+0.64*sign(dot(kb,sun))*vn.y+0.420465*vn.z)*2.3735468;
            } else {
                let raw = vec3<f32>(nm.xy*2.0+dxy*2.0-2.0,nm.z*2.0-1.0);
                let vnd = raw*inverseSqrt(max(dot(raw,raw),1e-12));
                wn = normalize(vnd.x*kt+vnd.y*kb+vnd.z*wn);
                let kn = select(vnd,raw,zdecal);
                kd = (0.64*sign(dot(kt,sun))*kn.x+0.64*sign(dot(kb,sun))*kn.y+0.420465*kn.z)*2.3735468;
            }
        }
        // The shadow map darkens only where the (mapped) normal faces the light.
        if env && dot(wn,sun) <= 0.0 { sh = 1.0; }
        let interp = saturate((s2_v-0.04)*50.0)*(1.0-sh);
        lin = weather::daylight(mix(s2_lit,vec3<f32>(0.07,0.09,0.1),interp),day)*kd*d*ov;
        if clear { lin *= a.a; alpha = a.a*a.a; test = alpha; }
        if zdecal { alpha = a.a; }
        if env {
            let lum = weather::daylight(vec3<f32>(mix(min(s2_v,0.04),s2_v,sh)),day).g;
            let m = ov*masks;
            var spec = vec3<f32>(0.0);
            if (flags & 16u) != 0u {
                let rl = 2.0*wn*dot(wn,sun)-sun;
                spec = m.x*pow(max(saturate(dot(vd,rl)),1e-6),10.0+290.0*m.y)*vec3<f32>(18.0,15.0,10.5)*saturate(lum-0.1);
            }
            if (flags & 64u) != 0u && !zdecal {
                let rv = vd-2.0*wn*dot(vd,wn);
                let cube = bindings::sample_environment(slot,vec3<f32>(-rv.x,-rv.y,rv.z),log2(frame::view.viewport.w/640.0)).rgb;
                spec += m.z*cube*(lum+(1.0-lum)*0.3*saturate(4.0*(m.z-1.5)))*1.5;
            }
            // Fresnel 1 + 4·(1 - N·E)^8, squared three times as the ucode does.
            let f1 = 1.0-saturate(dot(wn,vd));
            let f2 = f1*f1;
            let f4 = f2*f2;
            lin += spec*(1.0+4.0*f4*f4);
        }
        if fam >= 7u { alpha = a.a; }
    } else if MAY_WORLD {
        if (fam == 3u || fam == 4u) && (flags & 8u) != 0u && (flags & 512u) == 0u { d = mix(d,art.rgb*art.rgb,art.a*p.decal.x); }
        if (flags & 4u) != 0u && fam < 13u && (flags & 256u) == 0u { d *= saturate((overlay_sample-0.5)*p.surface.y+0.5); }
        var kd = 0.93429;
        if (fam <= 6u || fam == 13u) && (flags & 1u) != 0u {
            var dxy = vec2<f32>(0.5);
            if (flags & 2u) != 0u && fam != 2u { dxy = detail; }
            if (fam == 5u || fam == 6u || fam == 13u) && (flags & 128u) != 0u { dxy = detail; }
            let raw = vec3<f32>(nm.xy*2.0+dxy*2.0-2.0,nm.z*2.0-1.0);
            var vnd = raw;
            if fam != 2u && fam != 6u { vnd = raw * inverseSqrt(max(dot(raw,raw),1e-12)); }
            var tt = kt;
            var bb = kb;
            if fam == 5u || fam == 6u || fam == 13u {
                let up = vec3<f32>(0.0,1.0,0.0)-wn*wn.y;
                if length(up) > 0.05 {
                    let b2 = normalize(up);
                    let t2 = cross(b2,wn);
                    tt = t2*select(-1.0,1.0,dot(t2,kt)>=0.0);
                    bb = b2*select(-1.0,1.0,dot(b2,kb)>=0.0);
                }
            }
            wn = normalize(raw.x*tt+raw.y*bb+wn*max(raw.z,0.05));
            kd = (vnd.x*0.58*sign(dot(kt,sun))+vnd.y*0.62*sign(dot(kb,sun))+vnd.z*0.39)*2.39562;
        }
        var lml = baked;
        lin = lml*kd*d;
        if fam == 13u { lin = lml*d*a.a; }
        if (flags & 16u) != 0u {
            let lit = vec3<f32>(-0.14,0.5,0.9);
            let reflected = lit-2.0*wn*dot(wn,lit);
            let bp = saturate(dot(vd,-reflected));
            let ks = pow(max(bp,1e-6),10.0+290.0*masks.y);
            lin += ks*vec3<f32>(2.1,1.8,1.5)*lml.g*masks.x;
        }
        if (fam == 5u || fam == 6u || fam == 13u) && (flags & 64u) != 0u {
            let rv = vd-2.0*wn*dot(vd,wn);
            let dir = vec3<f32>(-rv.x,-rv.y,rv.z);
            let cube = bindings::sample_environment(slot,dir,log2(frame::view.viewport.w/640.0)).rgb;
            let rl = 0.3*saturate(4.0*masks.z-2.6);
            let lum = lml.g+rl*(1.0-lml.g);
            lin += cube*lum*masks.z*1.5;
        }
        if fam >= 7u { alpha = a.a; }
        if fam == 13u { alpha *= alpha; }
    }
    if snow>0.0 {
        // Settled snow, lit by the same baked light; logos and decals are buried.
        let xz=i.world_position.xz;
        if fam>=1u && fam<=8u {
            lin=mix(lin,weather::daylight(weather::snow_light(sky_light),day)*mix(0.45,1.0,skater)*weather::snow_albedo(xz)*0.93,weather::snow_cover(sky_facing,xz,sky_light.g,snow));
        } else if fam==9u || fam==10u {
            var light=weather::daylight(max(lm*lm*select(1.0,1.5,s2),vec3<f32>(p.family.y)),day)*p.family.x;
            if fam==9u { light*=p.family.z; }
            lin=mix(lin,light*weather::snow_albedo(xz),weather::frost(i.world_position.xyz,sky_facing,snow));
        } else if fam==11u {
            lin=mix(lin,weather::snow_albedo(xz)*weather::daylight(vec3<f32>(p.family.w),day),weather::snow_cover(sky_facing,xz,1.0,snow));
        }
    }
    if rain>0.0 && fam>=1u && fam<=8u {
        let w=weather::wet(i.world_position.xz,level,rain,frame_state.clock.x,vd,length(rpos),baked.g,frame::fog.base_color.rgb);
        lin=lin*w.x+w.yzw/p.mode.w;
    }
    if clouds>0.0 && fam>=9u && fam<=11u { lin*=mix(vec3<f32>(1.0),weather::tint(),weather::cover(clouds)); }
    // Skate 2 zone fog (retail_grading.rs) replaces the material's when the frame state flags it (ramp.w).
    var fog_ramp = p.fog_ramp;
    var fog_color = p.fog_color;
    if frame_state.fog_ramp.w > 0.5 {
        fog_ramp = frame_state.fog_ramp;
        fog_color = vec4<f32>(frame_state.fog_colour.rgb/2.5, frame_state.fog_colour.a);
    }
    // The haze darkens with the sky at night.
    fog_color = vec4<f32>(fog_color.rgb*frame_state.day_sky.rgb, fog_color.a);
    var f = saturate(length(rpos)*fog_ramp.x+fog_ramp.y);
    if fog_ramp.z != 1.0 { f = pow(max(f,1e-6),fog_ramp.z); }
    var fog_a = 1.0+fog_color.a*f;
    if fam <= 8u || fam == 13u { fog_a *= p.surface.w; }
    var xe = max((lin*fog_a+fog_color.rgb*f)*p.mode.w,vec3<f32>(0.0));
    if s1 {
        // Skate 1 PS_Fog_Bloom_Tone / PS_Fog_Tone: lerp(colour, fog_colour, Fog) in gamma, over |view.xz|
        // (ComputeFogFactor). The ramp is Fog/max and the colour linear·max (environment.py fog_parameters).
        let fmax = -fog_color.a;
        var fs = saturate(length((frame::view.view_from_world*i.world_position).xz)*fog_ramp.x+fog_ramp.y);
        if fog_ramp.z != 1.0 { fs = pow(max(fs,1e-6),fog_ramp.z); }
        let fog_v = s1_tone(fog_color.rgb*p.mode.w/max(fmax,1e-6));
        xe = s1_untone(mix(s1_tone(max(lin*p.mode.w,vec3<f32>(0.0))),fog_v,fmax*fs));
    }
    // Weather fog: the camera's DistanceFog colour and density, thickening with
    // distance; it equals Bevy's exponential fog on PBR props at 100 m, as on
    // characters. Transparent when dry.
    let fd=length(rpos);
    xe=mix(xe,frame::fog.base_color.rgb,frame::fog.base_color.a*(1.0-exp(-fd*frame::fog.be.x*(fd+600.0)/700.0)));
    // Reduced curve is the full curve with the linear input capped at one.
    if fam == 8u { xe = min(xe,vec3<f32>(1.0)); }
    if p.foliage_debug.w != 0.0 { return vec4<f32>(p.foliage_debug.rgb, 1.0); }
#ifndef RETAIL_NO_ALPHA_TEST
    if test < p.mode.z { discard; }
#endif
    return vec4<f32>(xe,alpha);
}
