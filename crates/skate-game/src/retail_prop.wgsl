// PBR props (benches, rails, tables) and fallback world surfaces (glass, terrain):
// Bevy's StandardMaterial forward shading (pbr.wgsl) plus the shared weather snow
// layer and the map's authored distance fog, as retail_world.wgsl applies it.
#import bevy_pbr::{
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    pbr_fragment::pbr_input_from_standard_material,
    pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT,
    decal::clustered::apply_decals,
    forward_io::{VertexOutput, FragmentOutput},
    mesh_bindings::mesh,
    mesh_view_bindings::view,
}
#ifdef VISIBILITY_RANGE_DITHER
#import bevy_pbr::pbr_functions::visibility_range_dither
#endif
#import skate_retail::weather

#ifdef BINDLESS
struct PropIndices { frame_state: u32 }
@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<storage> prop_indices: array<PropIndices>;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var<storage> prop_frames: binding_array<weather::FrameState>;
#else
@group(#{MATERIAL_BIND_GROUP}) @binding(50) var<storage, read> prop_frame: weather::FrameState;
#endif

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
#ifdef VISIBILITY_RANGE_DITHER
    visibility_range_dither(in.position, in.visibility_range_dither);
#endif
#ifdef BINDLESS
    let slot = mesh[in.instance_index].material_and_lightmap_bind_group_slot & 0xffffu;
    let state = prop_indices[slot].frame_state;
    let snow = prop_frames[state].snow.x;
    let fog_ramp = prop_frames[state].fog_ramp;
    let fog_colour = prop_frames[state].fog_colour;
    let day = prop_frames[state].day_light;
    let sky = prop_frames[state].day_sky.rgb;
#else
    let snow = prop_frame.snow.x;
    let fog_ramp = prop_frame.fog_ramp;
    let fog_colour = prop_frame.fog_colour;
    let day = prop_frame.day_light;
    let sky = prop_frame.day_sky.rgb;
#endif
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);
    apply_decals(&pbr_input);
    if snow > 0.0 {
        // Settled snow is white, matte and smooth where the prop faces the sky.
        let xz = in.world_position.xz;
        let s = weather::snow_cover(pbr_input.world_normal.y, xz, 1.0, snow);
        pbr_input.material.base_color = vec4<f32>(
            mix(pbr_input.material.base_color.rgb, weather::snow_albedo(xz), s), pbr_input.material.base_color.a);
        pbr_input.material.metallic = mix(pbr_input.material.metallic, 0.0, s);
        pbr_input.material.perceptual_roughness = mix(pbr_input.material.perceptual_roughness, 0.9, s);
        pbr_input.N = normalize(mix(pbr_input.N, pbr_input.world_normal, s));
    }
    // Time of day (retail_daynight.rs): baked light follows the world's; the ambient
    // light follows it on the CPU. Exact at noon.
    pbr_input.lightmap_light *= weather::day_scale(day);
    var out: FragmentOutput;
    if (pbr_input.material.flags & STANDARD_MATERIAL_FLAGS_UNLIT_BIT) == 0u {
        out.color = apply_pbr_lighting(pbr_input);
    } else {
        out.color = pbr_input.material.base_color;
    }
    // Authored map fog before the weather fog, in retail_world.wgsl's order and form.
    var f = saturate(length(in.world_position.xyz - view.world_position) * fog_ramp.x + fog_ramp.y);
    if fog_ramp.z != 1.0 { f = pow(max(f, 1e-6), fog_ramp.z); }
    out.color = vec4<f32>(out.color.rgb * (1.0 + fog_colour.a * f) + fog_colour.rgb * sky * f, out.color.a);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
