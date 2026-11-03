#import bevy_pbr::{forward_io::VertexOutput, mesh_view_bindings as frame}
#import skate_character_lighting::{CharacterParams, CharacterLighting, lit_by, shade_character, character_normal, character_albedo}
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> p: CharacterParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var diffuse: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var diffuse_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var normal_map: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(4) var normal_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(5) var mask_map: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(6) var mask_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(7) var coverage_map: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(8) var coverage_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(9) var<storage, read> world_light: CharacterLighting;

@fragment
fn fragment(i: VertexOutput) -> @location(0) vec4<f32> {
    let albedo = textureSample(diffuse,diffuse_sampler,i.uv);
    let d=character_albedo(albedo.rgb)*p.tint.rgb;
    let mapped=character_normal(i.world_position,i.world_normal,i.uv,textureSample(normal_map,normal_sampler,i.uv).xy);
    let vn=select(normalize(i.world_normal),mapped,p.options.x>0.0);
    var smask=albedo.a;
    if p.options.y>0.0 { smask=textureSample(mask_map,mask_sampler,i.uv).r; }
#ifdef VERTEX_UVS_B
    // Only native pieces with a coverage map carry a second UV set (native_roster.py): retail hair and
    // alpha shaders cut by that map's R on TEXCOORD1; their diffuse alpha is the specular mask.
    let alpha=textureSample(coverage_map,coverage_sampler,i.uv_b).r;
#else
    let alpha=albedo.a;
#endif
    return shade_character(lit_by(p, world_light), vn, i.world_position, d, smask, alpha);
}
