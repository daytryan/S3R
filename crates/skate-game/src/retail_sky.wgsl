#import bevy_pbr::{forward_io::Vertex, mesh_view_bindings::{view, globals, fog}}
#import skate_retail::weather
struct SkyParams { settings: vec4<f32>, sun_direction: vec4<f32>, night: vec4<f32> }
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> p: SkyParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var panorama: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var panorama_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var sun_gradient: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(4) var sun_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(5) var<storage, read> frame_state: weather::FrameState;
@group(#{MATERIAL_BIND_GROUP}) @binding(6) var night_panorama: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(7) var night_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(8) var moon_gradient: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(9) var moon_sampler: sampler;
struct SkyOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) dome_position: vec3<f32>,
}
@vertex
fn vertex(v: Vertex) -> SkyOut {
    var out: SkyOut;
    let world = v.position + vec3<f32>(view.world_position.x,p.settings.x,view.world_position.z);
    out.position = view.clip_from_world*vec4<f32>(world,1.0);
    // Reverse-Z far plane: the authored dome is larger than the camera's far distance.
    out.position.z = 0.0000001*out.position.w;
    out.uv = v.uv;
    out.dome_position = v.position;
    return out;
}
// Native sky_defaultPS: sine-angle lookup in the material's specular gradient,
// alpha controls the HDR core.
fn gradient(t: texture_2d<f32>, s: sampler, dir: vec3<f32>, toward: vec3<f32>, scale: f32) -> vec3<f32> {
    let dot_pl = saturate(dot(dir,toward));
    let sin_angle = sqrt(saturate(1.0-dot_pl*dot_pl));
    let g = textureSample(t,s,vec2<f32>(sin_angle/scale,0.5/16.0));
    return g.rgb*g.rgb/saturate(g.a+0.01);
}
@fragment
fn fragment(i: SkyOut) -> @location(0) vec4<f32> {
    let d = textureSample(panorama,panorama_sampler,i.uv).rgb;
    // Sampled unconditionally: implicit derivatives must be uniform.
    let n = textureSample(night_panorama,night_sampler,i.uv).rgb;
    var linear = d*d;
    let w = frame_state.weather;
    let cover = weather::cover(max(w.x,frame_state.snow.x));
    let dir = normalize(i.dome_position);
    // Time of day (retail_daynight.rs) moves the sun; at noon the authored one.
    let day = frame_state.day_sun.w > 0.0;
    let sun_dir = select(p.sun_direction.xyz,frame_state.day_sun.xyz,day);
    if p.settings.w > 0.0 {
        // World position minus the sky anchor is exactly the local dome position used here.
        linear += gradient(sun_gradient,sun_sampler,dir,sun_dir,p.settings.w)*(1.0-cover);
    }
    var colour = linear*p.settings.y*p.settings.z;
    var land = 0.0;
    if day {
        // Dawn and dusk: the day sky tinted, glowing low toward the sun.
        let low = 1.0-smoothstep(0.0,0.4,abs(dir.y));
        let toward = 0.5+0.5*dot(normalize(dir.xz+vec2<f32>(1e-5,0.0)),normalize(sun_dir.xz+vec2<f32>(1e-5,0.0)));
        colour = colour*frame_state.day_sky.rgb+frame_state.day_glow.rgb*low*low*(0.3+0.7*toward*toward)*p.settings.y*0.35;
        // Night: After Dark's sky (stars, cloud, horizon glow) at its own 0.35
        // multiplier, with this map's skyline kept as a moonlit silhouette. The
        // land mask keeps the day panorama's low, unsaturated, unbright texels.
        land = (1.0-smoothstep(0.12,0.3,d.b-d.r))*(1.0-smoothstep(0.6,0.8,max(d.r,max(d.g,d.b))))*(1.0-smoothstep(0.16,0.29,dir.y));
        let silhouette = dot(linear,vec3<f32>(0.2126,0.7152,0.0722))*vec3<f32>(0.55,0.68,1.0)*0.012;
        var night = silhouette;
        if p.night.x > 0.0 {
            // The moon rises opposite the sun (the Halloween mode draws its own, below).
            let moon = gradient(moon_gradient,moon_sampler,dir,-sun_dir,p.night.x)*select(1.0,0.0,frame_state.snow.z==1.0);
            night = mix(n*n+moon,silhouette,land);
        }
        colour = mix(colour,night*p.settings.y*0.35,frame_state.day_sky.w);
    }
    if cover > 0.0 {
        colour = mix(colour,weather::clouds(dir,globals.time,fog.base_color.rgb),cover);
        if w.y > 0.0 { colour = colour/weather::flash_gain(w.y)+weather::lightning(dir,w.z,w.w,w.y); }
    }
    // Not retail (user 10-19): the Halloween mode's night sky over the Super Ultra Mega Park (seasonal.rs).
    colour = weather::season_sky(colour,frame_state.snow,dir,-sun_dir,land,fog.base_color,p.settings.y*0.35);
    return vec4<f32>(colour,1.0);
}
