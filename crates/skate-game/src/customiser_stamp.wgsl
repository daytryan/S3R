// StandardMaterial entry point; the added block only composes skin-stamp albedo.
// Bevy 0.18.1 pbr.wgsl, MIT/Apache-2.0, copyright Bevy contributors.
struct SkinStamp {
    // Per zone, two rows mapping TEXCOORD1 to the graphic's UVs (i_customGraphic).
    rows: array<vec4<f32>, 8>,
    // y: hair opacity map; z: 1 lays graphics over cloth, 0 multiplies tattoos into skin.
    enabled: vec4<f32>,
    // Per zone on/off.
    zones: vec4<f32>,
}
@group(#{MATERIAL_BIND_GROUP}) @binding(100) var stamp_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var stamp_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(108) var stamp_texture_1: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(109) var stamp_sampler_1: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(110) var stamp_texture_2: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(111) var stamp_sampler_2: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(112) var stamp_texture_3: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(113) var stamp_sampler_3: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(103) var hair_opacity: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(104) var hair_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var<uniform> stamp: SkinStamp;
#import bevy_pbr::{
    pbr_types,
    pbr_functions::alpha_discard,
    pbr_fragment::pbr_input_from_standard_material,
    decal::clustered::apply_decals,
}

#ifdef PREPASS_PIPELINE
#import bevy_pbr::{
    prepass_io::{VertexOutput, FragmentOutput},
    pbr_deferred_functions::deferred_output,
}
#else
#import skate_character_lighting::{CharacterParams, CharacterLighting, lit_by, shade_character, character_normal, character_albedo}
#import bevy_pbr::pbr_bindings
@group(#{MATERIAL_BIND_GROUP}) @binding(105) var retail_mask: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(106) var retail_mask_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(107) var<uniform> retail: CharacterParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(114) var<storage, read> world_light: CharacterLighting;
#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
    pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT,
}
#endif

#ifdef VISIBILITY_RANGE_DITHER
#import bevy_pbr::pbr_functions::visibility_range_dither;
#endif

#ifdef MESHLET_MESH_MATERIAL_PASS
#import bevy_pbr::meshlet_visibility_buffer_resolve::resolve_vertex_output
#endif

#ifdef OIT_ENABLED
#import bevy_core_pipeline::oit::oit_draw
#endif // OIT_ENABLED

#ifdef FORWARD_DECAL
#import bevy_pbr::decal::forward::get_forward_decal_info
#endif

#ifndef PREPASS_PIPELINE
// Pieces lit by the world take its light, SH and key from the shared buffer; team photos (key.w < 0) and tiles
// (tint.w 0) keep their own (retail_character.rs SharedLighting).
fn world_lit(p: CharacterParams) -> CharacterParams {
    if p.tint.w != 0.0 && p.key.w >= 0.0 {
        return lit_by(p, world_light);
    }
    return p;
}
#endif

fn stamp_uv(zone: u32, uv: vec2<f32>) -> vec2<f32> {
    let a = stamp.rows[zone * 2u];
    let b = stamp.rows[zone * 2u + 1u];
    return vec2<f32>(dot(a.xy, uv) + a.z, dot(b.xy, uv) + b.z);
}

@fragment
fn fragment(
#ifdef MESHLET_MESH_MATERIAL_PASS
    @builtin(position) frag_coord: vec4<f32>,
#else
    vertex_output: VertexOutput,
    @builtin(front_facing) is_front: bool,
#endif
) -> FragmentOutput {
#ifdef MESHLET_MESH_MATERIAL_PASS
    let vertex_output = resolve_vertex_output(frag_coord);
    let is_front = true;
#endif

    var in = vertex_output;

    // If we're in the crossfade section of a visibility range, conditionally
    // discard the fragment according to the visibility pattern.
#ifdef VISIBILITY_RANGE_DITHER
    visibility_range_dither(in.position, in.visibility_range_dither);
#endif

#ifdef FORWARD_DECAL
    let forward_decal_info = get_forward_decal_info(in);
    in.world_position = forward_decal_info.world_position;
    in.uv = forward_decal_info.uv;
#endif

    // generate a PbrInput struct from the StandardMaterial bindings
    var pbr_input = pbr_input_from_standard_material(in, is_front);
#ifndef PREPASS_PIPELINE
    let rp=world_lit(retail);
    // Preserve authored alpha as specular data before opacity/tattoos/discard.
    let retail_albedo=textureSample(pbr_bindings::base_color_texture,pbr_bindings::base_color_sampler,in.uv);
    let retail_mapped=character_normal(in.world_position,in.world_normal,in.uv,
        textureSample(pbr_bindings::normal_map_texture,pbr_bindings::normal_map_sampler,in.uv).xy);
    let retail_n=select(normalize(in.world_normal),retail_mapped,rp.options.x>0.0);
    if rp.light.w>0.0 && rp.tint.w>0.0 {
        // enabled.w (customiser_parts::grip_material): 1 a Solid grip, the flat colour over the grip's own specular
        // mask and normals; 2 a Realistic one, the tape's scuffs around the colour (the tape shows 0.276 on average)
        // and its screws (far brighter) left as they are. ponytail: the non-retail fallback below keeps the tinted texture.
        let albedo=character_albedo(retail_albedo.rgb);
        var shade=mix(albedo,vec3<f32>(1.0),min(stamp.enabled.w,1.0))*rp.tint.rgb;
        if stamp.enabled.w>1.5 {
            let shown=sqrt(dot(albedo,vec3<f32>(1.0/3.0)));
            let scuff=shown/0.276;
            shade=mix(scuff*scuff*rp.tint.rgb,albedo,smoothstep(0.45,0.6,shown));
        }
        pbr_input.material.base_color=vec4<f32>(shade,retail_albedo.a);
    }
#endif



#ifdef VERTEX_UVS_B
    // The diffuse alpha is NOT coverage. Hair has a separate red-channel
    // opacity map on secondary UVs (cac_hair_defaultVS/PS).
    let hair_alpha = textureSample(hair_opacity, hair_sampler, in.uv_b).r;
    pbr_input.material.base_color.a = mix(pbr_input.material.base_color.a, hair_alpha, stamp.enabled.y);
    // cacstamp_skin_defaultPS: skin * (1 + (decal.rgb - 1) * decal.a); cloth graphics
    // lie over the cloth. Its VS transforms TEXCOORD1 by the two i_customGraphic rows; the
    // decal shows wherever that lands inside it (no zone mask: a wide logo's start overflows).
    // Every zone samples (uniform control flow) and masks afterwards.
    var decals = array<vec4<f32>, 4>(
        textureSample(stamp_texture, stamp_sampler, stamp_uv(0u, in.uv_b)),
        textureSample(stamp_texture_1, stamp_sampler_1, stamp_uv(1u, in.uv_b)),
        textureSample(stamp_texture_2, stamp_sampler_2, stamp_uv(2u, in.uv_b)),
        textureSample(stamp_texture_3, stamp_sampler_3, stamp_uv(3u, in.uv_b)));
    var albedo = pbr_input.material.base_color.rgb;
    for (var zone = 0u; zone < 4u; zone++) {
        let uv = stamp_uv(zone, in.uv_b);
        let inside = all(uv >= vec2<f32>(0.0)) && all(uv <= vec2<f32>(1.0));
        var decal = decals[zone];
#ifndef PREPASS_PIPELINE
        // Retail blends the decal in the diffuse's space (cacstamp PS), as base_color above.
        if rp.light.w>0.0 && rp.tint.w>0.0 {
            decal = vec4<f32>(character_albedo(decal.rgb), decal.a);
        }
#endif
        let opacity = decal.a * stamp.zones[zone] * select(0.0, 1.0, inside);
        albedo = mix(albedo * mix(vec3<f32>(1.0), decal.rgb, opacity), mix(albedo, decal.rgb, opacity), stamp.enabled.z);
    }
    pbr_input.material.base_color = vec4<f32>(albedo, pbr_input.material.base_color.a);
#endif

    // alpha discard
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);

    // clustered decals
    apply_decals(&pbr_input);

#ifdef PREPASS_PIPELINE
    // write the gbuffer, lighting pass id, and optionally normal and motion_vector textures
    let out = deferred_output(in, pbr_input);
#else
    // in forward mode, we calculate the lit color immediately, and then apply some post-lighting effects here.
    // in deferred mode the lit color and these effects will be calculated in the deferred lighting shader
    var out: FragmentOutput;
    if rp.light.w > 0.0 && rp.tint.w > 0.0 {
        let mask = textureSample(retail_mask, retail_mask_sampler, in.uv).r;
        out.color = shade_character(rp, retail_n, pbr_input.world_position,
            pbr_input.material.base_color.rgb,
            select(retail_albedo.a, mask, rp.options.y > 0.0),
            pbr_input.material.base_color.a);
    } else if (pbr_input.material.flags & STANDARD_MATERIAL_FLAGS_UNLIT_BIT) == 0u {
        out.color = apply_pbr_lighting(pbr_input);
    } else {
        out.color = pbr_input.material.base_color;
    }

    // apply in-shader post processing (fog, alpha-premultiply, and also tonemapping, debanding if the camera is non-hdr)
    // note this does not include fullscreen postprocessing effects like bloom.
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
#endif

#ifdef OIT_ENABLED
    let alpha_mode = pbr_input.material.flags & pbr_types::STANDARD_MATERIAL_FLAGS_ALPHA_MODE_RESERVED_BITS;
    if alpha_mode != pbr_types::STANDARD_MATERIAL_FLAGS_ALPHA_MODE_OPAQUE {
        // The fragments will only be drawn during the oit resolve pass.
        oit_draw(in.position, out.color);
        discard;
    }
#endif // OIT_ENABLED

#ifdef FORWARD_DECAL
        out.color.a = min(forward_decal_info.alpha, out.color.a);
#endif

        return out;
}
