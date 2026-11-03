#define_import_path skate_retail::weather
// Visual-only rain shared by the world, sky, rain and screen shaders. The overcast
// colour and fog come from the camera's DistanceFog (retail_weather.rs). These
// are adapter choices, not recovered native constants.

// Shared frame state (retail_shadow.rs). weather: rain strength, lightning
// flash, bolt azimuth, bolt seed. snow: cover, snowfall. Time of day
// (retail_daynight.rs, all identity at noon): day_light is the baked-light scale
// (rgb) and contrast (w); day_sky the sky and fog tint (rgb) and night blend (w);
// day_glow the horizon glow toward the sun (rgb); day_sun the moving sun (xyz,
// w 1 when the sky draws it instead of the authored one).
struct FrameState { shadow: vec4<f32>, clock: vec4<f32>, pca: array<vec4<f32>, 7>, weather: vec4<f32>, snow: vec4<f32>, fog_ramp: vec4<f32>, fog_colour: vec4<f32>, day_light: vec4<f32>, day_sky: vec4<f32>, day_glow: vec4<f32>, day_sun: vec4<f32> }

// Baked daylight at this time of day: night compresses it toward After Dark's flatter,
// dimmer moonlight (contrast below 1), then scales and tints it. Exact at noon.
fn daylight(light: vec3<f32>, l: vec4<f32>) -> vec3<f32> {
    if l.w == 1.0 { return light * l.rgb; }
    return pow(max(light, vec3<f32>(0.0)), vec3<f32>(l.w)) * l.rgb;
}
// One scale for light that is not a lightmap (ocean, PBR props): daylight at a
// typical lightmap level. Exactly one at noon.
fn day_scale(l: vec4<f32>) -> vec3<f32> { return daylight(vec3<f32>(0.25), l) * 4.0; }

// Cool, dim daylight under cloud.
fn tint() -> vec3<f32> { return vec3<f32>(0.62, 0.66, 0.72); }
// Sky and light are fully overcast from light rain up; heavier rain adds drops,
// puddles and fog.
fn cover(rain: f32) -> f32 { return saturate(rain * 2.0); }
// Whole-view brightening during a lightning flash (final screen pass). The sky
// divides it back out so the bolt stays visible against unsaturated cloud.
fn flash_gain(flash: f32) -> f32 { return 1.0 + 0.45 * flash; }
// Final-image darkening under cloud, applied after the auto-exposure meter so the
// meter cannot brighten a stormy scene back to a sunny level.
fn gloom(rain: f32) -> f32 { return 1.0 - 0.18 * cover(rain); }

fn hash(cell: vec2<f32>) -> f32 {
    let q = bitcast<vec2<u32>>(vec2<i32>(cell));
    var h = (q.x * 1597334677u) ^ (q.y * 3812015801u);
    h = h * 747796405u + 2891336453u;
    h = ((h >> ((h >> 28u) + 4u)) ^ h) * 277803737u;
    return f32((h >> 22u) ^ h) / 4294967295.0;
}

fn noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash(i), hash(i + vec2<f32>(1.0, 0.0)), u.x),
        mix(hash(i + vec2<f32>(0.0, 1.0)), hash(i + vec2<f32>(1.0, 1.0)), u.x), u.y);
}

// Sunlit lightmaps under cloud: dimmer, cooler, with baked shadows lifted toward
// the lit level.
fn overcast(light: vec3<f32>, rain: f32) -> vec3<f32> {
    let lifted = pow(max(light, vec3<f32>(0.0)), vec3<f32>(0.7));
    let grey = mix(lifted, vec3<f32>(dot(lifted, vec3<f32>(0.2126, 0.7152, 0.0722))), 0.5);
    return mix(light, grey * tint(), cover(rain));
}

// Puddle depth: fixed in the world, larger in heavier rain. A fine octave breaks
// up the outline and the wide ramp fades each puddle into the soaked ground.
fn puddle(xz: vec2<f32>, rain: f32) -> f32 {
    let n = 0.6 * noise(xz * 0.16) + 0.3 * noise(xz * 0.45 + 17.0) + 0.1 * noise(xz * 1.9 + 3.0);
    let edge = mix(0.68, 0.62, rain);
    return smoothstep(edge - 0.07, edge + 0.08, n);
}

// Slope of drop rings: two offset layers of cells, at most one ring per cell.
fn ripples(xz: vec2<f32>, t: f32, rain: f32) -> vec2<f32> {
    var slope = vec2<f32>(0.0);
    for (var layer = 0u; layer < 2u; layer++) {
        let q = xz * 2.5 + f32(layer) * vec2<f32>(0.43, 0.71);
        let cell = floor(q);
        let h = hash(cell + f32(layer) * 57.0);
        if h > rain { continue; }
        let centre = cell + 0.3 + 0.4 * vec2<f32>(fract(h * 13.7), hash(cell + 91.0));
        let life = fract(t * 1.1 + h * 7.0);
        let d = q - centre;
        let dist = length(d);
        let x = (dist - life * 0.22) * 40.0;
        if abs(x) < 3.1416 {
            slope += d / max(dist, 1e-4) * sin(x) * (1.0 - life) * (1.0 - life);
        }
    }
    return slope;
}

// Albedo multiplier (x) and added sky reflection (yzw, scene units) for rain-soaked
// ground. `level` is 1 on flat ground, `light` the local lightmap brightness.
fn wet(xz: vec2<f32>, level: f32, rain: f32, t: f32, view_dir: vec3<f32>, distance: f32, light: f32, sky: vec3<f32>) -> vec4<f32> {
    let soaked = cover(rain);
    let pool = puddle(xz, rain) * level;
    var s = vec2<f32>(0.0);
    let near = 1.0 - smoothstep(12.0, 30.0, distance);
    if near > 0.0 && level > 0.0 { s = ripples(xz, t, rain) * near * mix(0.08, 0.35, pool); }
    let n = normalize(vec3<f32>(-s.x, 1.0, -s.y));
    let r = reflect(-view_dir, n);
    let fresnel = mix(0.04, 0.08, pool) + 0.92 * pow(1.0 - saturate(dot(n, view_dir)), 5.0);
    let shine = mix(0.3 * level, 0.9, pool) * soaked * saturate(light * 1.5);
    let reflection = sky * mix(0.8, 1.3, saturate(r.y)) * fresnel * shine;
    return vec4<f32>(1.0 - soaked * (0.15 + 0.25 * level) - 0.3 * pool, reflection);
}

// Overcast dome: layered cloud noise drifting with the wind, brighter overhead,
// fading into the fog colour `sky` at the horizon.
fn clouds(direction: vec3<f32>, t: f32, sky: vec3<f32>) -> vec3<f32> {
    let up = saturate(direction.y);
    let p = direction.xz / (up + 0.12) * 1.8 + vec2<f32>(0.012, 0.006) * t;
    let n = 0.5 * noise(p) + 0.3 * noise(p * 2.3 + 5.0) + 0.2 * noise(p * 5.1 + 11.0);
    let shade = mix(0.6, 1.05, smoothstep(0.25, 0.8, n)) * (1.0 - 0.15 * up);
    return sky * mix(1.0, shade, smoothstep(0.0, 0.25, up));
}

// Piecewise-linear noise in -0.5..0.5: the zig-zag of a bolt.
fn jag(x: f32, seed: f32) -> f32 {
    let i = floor(x);
    return mix(hash(vec2<f32>(i, seed)), hash(vec2<f32>(i + 1.0, seed)), fract(x)) - 0.5;
}

// Sky light from a strike: the clouds glow toward the bolt, which runs from the
// cloud base down to the horizon at `azimuth`. `flash` is 0..1.
fn lightning(direction: vec3<f32>, azimuth: f32, seed: f32, flash: f32) -> vec3<f32> {
    let elevation = asin(clamp(direction.y, -1.0, 1.0));
    let toward = normalize(vec3<f32>(cos(azimuth), 0.4, sin(azimuth)));
    let glow = pow(saturate(dot(direction, toward)), 8.0) * 0.3 + 0.03;
    let wander = jag(elevation * 14.0, seed) * 0.08 + jag(elevation * 45.0, seed + 7.0) * 0.025;
    let delta = atan2(direction.z, direction.x) - azimuth - wander;
    let across = abs(atan2(sin(delta), cos(delta)));
    let along = step(0.0, elevation) * (1.0 - smoothstep(0.35, 0.45, elevation));
    let bolt = (exp(-across / 0.003) * 4.0 + exp(-across / 0.02) * 0.6) * along;
    return vec3<f32>(0.8, 0.85, 1.0) * (glow + bolt) * flash;
}

// Light on settled snow: baked sun and shade are closer under cloud, keeping
// enough of the baked shading to show the shape of the ground.
fn snow_light(baked: vec3<f32>) -> vec3<f32> { return pow(max(baked, vec3<f32>(0.0)), vec3<f32>(0.75)) * 0.9; }

// Settled snow colour: broad drifts and a fine grain, so it is not a flat sheet.
fn snow_albedo(xz: vec2<f32>) -> vec3<f32> {
    return vec3<f32>(0.8, 0.83, 0.88) * (0.88 + 0.07 * noise(xz * 0.35) + 0.05 * noise(xz * 3.0));
}

// Snow cover (0..1): the layer settles where a surface faces the sky, with a
// ragged edge on slopes, and stays off surfaces the baked `light` shows are
// sheltered. Any snow setting lays the full layer.
fn snow_cover(up: f32, xz: vec2<f32>, light: f32, snow: f32) -> f32 {
    let ragged = 0.5 * (noise(xz * 1.3) + noise(xz * 4.1 + 9.0)) - 0.5;
    return smoothstep(0.3, 0.6, up + 0.3 * ragged) * smoothstep(0.03, 0.12, light) * cover(snow);
}

// Snow caught in foliage: clumps on the upper, sky-facing side of branches.
fn frost(position: vec3<f32>, up: f32, snow: f32) -> f32 {
    let clumps = smoothstep(0.4, 0.7, noise(position.xz * 1.7 + position.y * 0.9));
    return clumps * saturate(0.35 + up) * 0.85 * cover(snow);
}

// Something landing on the lens, at most one per cell and only in some cycles:
// offset from its centre in radii (xy), seconds since it landed (z) and its
// radius in UV heights (w, 0 when there is none).
fn lens_hit(uv: vec2<f32>, aspect: f32, t: f32, amount: f32) -> vec4<f32> {
    let g = uv * vec2<f32>(aspect, 1.0) * 7.0;
    let cell = floor(g);
    let h = hash(cell + 311.0);
    let period = mix(7.0, 4.0, amount);
    let cycle = t / period + h;
    if hash(cell + vec2<f32>(floor(cycle), 57.0)) > 0.35 { return vec4<f32>(0.0); }
    let life = fract(cycle) * period;
    let centre = cell + 0.3 + 0.4 * vec2<f32>(hash(cell + 17.0), hash(cell + 29.0));
    let radius = mix(0.12, 0.28, h) * (1.0 - 0.25 * smoothstep(0.0, 2.2, life));
    return vec4<f32>((g - centre) / radius, life, radius / 7.0);
}

// Only the outer band of the screen gets lens hits; the centre stays clear.
fn lens_band(uv: vec2<f32>) -> f32 {
    return smoothstep(0.6, 0.9, max(abs(uv.x - 0.5), abs(uv.y - 0.5)) * 2.0);
}

// Rain drops on the lens near the screen edges, each landing at once and fading
// over about two seconds. Returns the UV offset of the view through the drop (xy),
// its opacity (z, zero almost everywhere and faint at most) and its rim (w).
fn screen_drop(uv: vec2<f32>, aspect: f32, t: f32, rain: f32) -> vec4<f32> {
    let hit = lens_hit(uv, aspect, t, rain);
    let r2 = dot(hit.xy, hit.xy);
    let fade = 1.0 - smoothstep(0.3, 2.2, hit.z);
    if hit.w <= 0.0 || fade <= 0.0 || r2 >= 1.0 { return vec4<f32>(0.0); }
    // A drop is a small lens: the view through it is magnified and flipped.
    return vec4<f32>(-hit.xy * hit.w * 1.4 / vec2<f32>(aspect, 1.0), fade * lens_band(uv) * (1.0 - r2 * r2) * 0.5, smoothstep(0.5, 1.0, r2));
}


// Not retail (user 10-19): seasonal.rs's Halloween and Christmas modes. snow.z is the season (1 Halloween, 2
// Christmas; only on University) and snow.w how close the camera is to the Super Ultra Mega Park (1 inside).
fn in_mega_park(xz: vec2<f32>) -> bool {
    // seasonal::PARK's outline, as its bounding box minus the cut-out north-west corner, grown by seasonal::MARGIN
    // (the trees just outside face the park too).
    return xz.x > 86.0 && xz.x < 516.0 && xz.y > -814.0 && xz.y < -489.0 && !(xz.x < 185.0 && xz.y > -587.0);
}
// The Halloween night sky: a big, low harvest moon on the night moon's bearing (`moon`; behind the skyline's
// `land`) glowing orange on the haze about it, and the horizon lost in the mist (`fog`: the camera's colour and
// opacity). `scale` is the night sky's brightness.
fn season_sky(colour: vec3<f32>, snow: vec4<f32>, dir: vec3<f32>, moon: vec3<f32>, land: f32, fog: vec4<f32>, scale: f32) -> vec3<f32> {
    if snow.z != 1.0 { return colour; }
    let bearing = normalize(vec3<f32>(moon.x, 0.0, moon.z) + vec3<f32>(1e-4, 0.0, 0.0));
    let m = normalize(bearing * 0.92 + vec3<f32>(0.0, 0.38, 0.0));
    let angle = acos(clamp(dot(dir, m), -1.0, 1.0));
    let radius = 0.055;
    // Its face: soft darker seas, dimming toward the limb.
    let side = normalize(cross(m, vec3<f32>(0.0, 1.0, 0.0)));
    let q = vec2<f32>(dot(dir, side), dot(dir, cross(side, m))) / radius;
    let seas = 0.6 * noise(q * 2.2 + 3.0) + 0.4 * noise(q * 5.0 + 9.0);
    let disc = 1.0 - smoothstep(radius * 0.95, radius, angle);
    let face = vec3<f32>(1.0, 0.66, 0.34) * (1.0 - 0.35 * smoothstep(0.45, 0.75, seas)) * (1.0 - 0.3 * saturate(dot(q, q)));
    let halo = vec3<f32>(0.9, 0.4, 0.12) * (exp(-angle / 0.07) * 0.22 + exp(-angle / 0.4) * 0.05) * scale;
    let mist = fog.a * (1.0 - smoothstep(-0.05, 0.35, dir.y));
    var sky = mix(colour * vec3<f32>(0.9, 0.82, 1.15), fog.rgb, mist);
    sky = mix(sky, face * 1.4, disc * (1.0 - land) * (1.0 - mist)) + halo * (1.0 - 0.8 * land);
    return mix(colour, sky, snow.w);
}
fn bulb_colour(k: u32) -> vec3<f32> {
    switch k % 5u {
        case 0u: { return vec3<f32>(1.0, 0.06, 0.04); }
        case 1u: { return vec3<f32>(0.08, 0.9, 0.12); }
        case 2u: { return vec3<f32>(0.1, 0.25, 1.0); }
        case 3u: { return vec3<f32>(1.0, 0.55, 0.05); }
        default: { return vec3<f32>(1.0, 0.85, 0.55); }
    }
}
// Christmas fairy lights strung across a 2D tree card: one bulb per jittered cell on the card's own plane (its
// horizontal axis and height, so both faces agree), twinkling and stepping through the colours. Returns the bulbs
// plus the light they throw on the leaves (`albedo`), in linear colour before exposure.
fn fairy_lights(pos: vec3<f32>, normal: vec3<f32>, t: f32, albedo: vec3<f32>) -> vec3<f32> {
    let cell = vec2<f32>(0.5, 0.4);
    // Rows droop a little between bulbs, like strung wire.
    let plane = card_plane(pos, normal);
    let q = vec2<f32>(plane.x, plane.y + 0.05 * sin(plane.x * 6.2832)) / cell;
    let base = floor(q);
    var light = vec3<f32>(0.0);
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let c = base + vec2<f32>(f32(dx), f32(dy));
            let r = hash(c);
            if r > 0.72 { continue; }
            let at = c + 0.5 + (vec2<f32>(hash(c + vec2<f32>(17.0, 3.0)), hash(c + vec2<f32>(5.0, 41.0))) - 0.5) * 0.6;
            let dist = length((q - at) * cell);
            // A new colour every 2 s or so, staggered bulb by bulb.
            let colour = bulb_colour(u32(hash(c + vec2<f32>(floor(t * 0.5 + r * 4.0), 7.0)) * 5.0));
            let twinkle = 0.55 + 0.45 * sin(t * (2.0 + 3.0 * r) + r * 40.0);
            let core = 1.0 - smoothstep(0.025, 0.05, dist);
            let halo = exp(-dist * dist / 0.012);
            light += colour * twinkle * (core * 2.5 + halo * (albedo * 5.0 + 0.12));
        }
    }
    return light;
}

// The plane of a 2D tree card: its own horizontal axis and height (so both faces agree), in metres.
fn card_plane(pos: vec3<f32>, normal: vec3<f32>) -> vec2<f32> {
    if abs(normal.y) >= 0.9 { return pos.xz; }
    var h = normalize(vec3<f32>(-normal.z, 0.0, normal.x));
    if h.x + 0.001 * h.z < 0.0 { h = -h; }
    return vec2<f32>(dot(pos, h), pos.y);
}
// A thread `width` wide at `dist` from its line, over a pixel `footprint` metres across: thinner than a pixel, it
// fades instead of shimmering.
fn thread(dist: f32, width: f32, footprint: f32) -> f32 {
    let w = max(width, footprint);
    return (1.0 - smoothstep(0.0, w, dist)) * (width / w);
}
// Halloween cobwebs across the park's 2D tree cards: an orb web (spokes and a spiral sagging between them) on some
// 2.4 m cells of the card's plane. Thread coverage, 0 to 1.
fn tree_web(pos: vec3<f32>, normal: vec3<f32>, footprint: f32) -> f32 {
    let size = 2.4;
    let q = card_plane(pos, normal);
    let c = floor(q / size);
    if hash(c + vec2<f32>(3.0, 11.0)) > 0.6 { return 0.0; }
    let radius = size * (0.28 + 0.14 * hash(c + vec2<f32>(9.0, 2.0)));
    let hub = (c + 0.5 + (vec2<f32>(hash(c + vec2<f32>(1.0, 7.0)), hash(c + vec2<f32>(13.0, 5.0))) - 0.5) * 0.25) * size;
    let d = q - hub;
    let r = length(d);
    if r > radius { return 0.0; }
    let spokes = 13.0;
    let f = fract(atan2(d.y, d.x) / 6.2832 * spokes + hash(c));
    let spoke = min(f, 1.0 - f) * 6.2832 / spokes * r;
    let sag = 1.0 + 0.1 * sin(f * 3.14159);
    let spacing = 0.085;
    let ring = abs(fract(r * sag / spacing + 0.5) - 0.5) * spacing / sag;
    let web = max(thread(spoke, 0.008, footprint), thread(ring, 0.006, footprint) * step(0.06, r));
    return web * (1.0 - smoothstep(radius * 0.85, radius, r));
}
