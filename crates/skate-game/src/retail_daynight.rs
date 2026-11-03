//! Time of day on retail maps: how the world, sky and characters look at
//! `WorldTime::hour`. Noon is the authored daylight, untouched. Night comes from Skate 3's
//! After Dark DLC (Sanatorium, `SanitariumDLC`): its sky, and its night park's lightmaps
//! against the day twin's (docs/retail-renderer.md). Custom maps light themselves
//! (map_render::advance_day).
use bevy::prelude::*;

/// What the hour does. Identity ([`DAY`]) at noon, in the Edit Skater room, on custom
/// maps and on maps whose own sky is night.
#[derive(Resource, Clone, Copy, PartialEq, Debug)]
pub(crate) struct DayLook {
    /// Baked light: scale and tint, and contrast (night flattens it).
    pub light: Vec3,
    pub contrast: f32,
    /// The day sky's and fogs' tint, and how far the sky has turned to night.
    pub sky: Vec3,
    pub night: f32,
    /// Horizon glow toward the sun.
    pub glow: Vec3,
    /// Characters' key light colour and probe ambient tint.
    pub key: Vec3,
    pub ambient: Vec3,
    /// Auto-exposure: target luminance scale (night reads darker than day, as the
    /// meter would otherwise brighten it back) and bound lift (dark corners stay readable).
    pub exposure: Vec2,
}
impl Default for DayLook {
    fn default() -> Self {
        DAY
    }
}

pub(crate) const DAY: DayLook = DayLook {
    light: Vec3::ONE,
    contrast: 1.,
    sky: Vec3::ONE,
    night: 0.,
    glow: Vec3::ZERO,
    key: Vec3::ONE,
    ambient: Vec3::ONE,
    exposure: Vec2::ONE,
};
/// After Dark. Its night park (Downtown_Night_Park) against the day twin (DownTownSkatePark),
/// area-weighted lightmap² p10..p90: night = 0.16 * day^0.69. Their ratio's tint (0.83, 1.04,
/// 1.15) carries the park's warm lamps, so the moon here is bluer, as Sanatorium's is.
/// Character probes (SH band 0) there are 0.30/0.35/0.41 of the day's; the key is the moon.
/// After Dark runs the day maps' meter (exposure.json), which brightens a dark scene back
/// toward day; night aims it at about half.
// ponytail: one fit for every map; the tint, key and meter target are judged by eye against
// Sanatorium (After Dark's character light data keeps the day key). Per-map night
// lightmaps would need a bake.
const NIGHT: DayLook = DayLook {
    light: Vec3::new(0.12, 0.155, 0.2),
    contrast: 0.69,
    sky: Vec3::new(0.3, 0.33, 0.42),
    night: 1.,
    glow: Vec3::ZERO,
    key: Vec3::new(0.15, 0.2, 0.26),
    ambient: Vec3::new(0.27, 0.34, 0.45),
    exposure: Vec2::new(0.55, 1.25),
};
/// Hour, look. Smoothstepped between neighbours; 0 and 24 are the same night.
const KEYS: [(f32, DayLook); 11] = [
    (0., NIGHT),
    (5., NIGHT),
    // Dawn: the sun on the horizon, the sky still half night.
    (6., DayLook {
        light: Vec3::new(0.42, 0.33, 0.33),
        contrast: 0.8,
        sky: Vec3::new(0.45, 0.42, 0.55),
        night: 0.35,
        glow: Vec3::new(0.54, 0.27, 0.12),
        key: Vec3::new(0.51, 0.3, 0.18),
        ambient: Vec3::new(0.5, 0.5, 0.62),
        exposure: Vec2::new(0.7, 1.15),
    }),
    (7.5, DayLook {
        light: Vec3::new(0.85, 0.74, 0.62),
        contrast: 0.95,
        sky: Vec3::new(0.85, 0.82, 0.8),
        night: 0.,
        glow: Vec3::new(0.35, 0.18, 0.06),
        key: Vec3::new(1., 0.82, 0.62),
        ambient: Vec3::new(0.85, 0.85, 0.9),
        exposure: Vec2::new(0.92, 1.),
    }),
    (10., DAY),
    (14., DAY),
    (16.5, DayLook {
        light: Vec3::new(0.92, 0.82, 0.68),
        contrast: 0.97,
        sky: Vec3::new(0.92, 0.88, 0.82),
        night: 0.,
        glow: Vec3::new(0.2, 0.1, 0.03),
        key: Vec3::new(1., 0.85, 0.65),
        ambient: Vec3::new(0.9, 0.88, 0.88),
        exposure: Vec2::ONE,
    }),
    // Sunset: warm, low, the horizon burning.
    (18., DayLook {
        light: Vec3::new(0.75, 0.42, 0.25),
        contrast: 0.85,
        sky: Vec3::new(0.7, 0.45, 0.4),
        night: 0.1,
        glow: Vec3::new(1.2, 0.5, 0.15),
        key: Vec3::new(1., 0.5, 0.25),
        ambient: Vec3::new(0.6, 0.52, 0.55),
        exposure: Vec2::new(0.9, 1.05),
    }),
    (19., DayLook {
        light: Vec3::new(0.24, 0.2, 0.26),
        contrast: 0.75,
        sky: Vec3::new(0.35, 0.3, 0.42),
        night: 0.6,
        glow: Vec3::new(0.35, 0.12, 0.08),
        key: Vec3::new(0.35, 0.25, 0.3),
        ambient: Vec3::new(0.4, 0.38, 0.5),
        exposure: Vec2::new(0.68, 1.2),
    }),
    (20., NIGHT),
    (24., NIGHT),
];

/// The look at `hour` (wrapped to 0..24).
pub(crate) fn at(hour: f32) -> DayLook {
    let hour = hour.rem_euclid(24.);
    let i = KEYS.iter().rposition(|(h, _)| *h <= hour).unwrap_or(0).min(KEYS.len() - 2);
    let ((h0, a), (h1, b)) = (KEYS[i], KEYS[i + 1]);
    let t = ((hour - h0) / (h1 - h0)).clamp(0., 1.);
    let s = t * t * (3. - 2. * t);
    // a + (b - a) * s, so equal neighbours (the noon plateau) stay bit-exact.
    let mix3 = |a: Vec3, b: Vec3| a + (b - a) * s;
    let mix = |a: f32, b: f32| a + (b - a) * s;
    DayLook {
        light: mix3(a.light, b.light),
        contrast: mix(a.contrast, b.contrast),
        sky: mix3(a.sky, b.sky),
        night: mix(a.night, b.night),
        glow: mix3(a.glow, b.glow),
        key: mix3(a.key, b.key),
        ambient: mix3(a.ambient, b.ambient),
        exposure: a.exposure + (b.exposure - a.exposure) * s,
    }
}

/// Up at 6, down at 18, highest (the authored sun) at noon; 15 degrees of azimuth an hour.
/// At midnight the moon, opposite, stands where the sun was authored, as the baked shadows do.
fn sun(authored: Vec3, hour: f32) -> Vec3 {
    let elevation = authored.y.clamp(-1., 1.).asin() * ((hour - 6.) * std::f32::consts::PI / 12.).sin();
    let azimuth = authored.z.atan2(authored.x) + (hour - 12.) * std::f32::consts::PI / 12.;
    Vec3::new(elevation.cos() * azimuth.cos(), elevation.sin(), elevation.cos() * azimuth.sin())
}

/// Light that is not a lightmap (props' ambient), as retail_weather.wgsl's day_scale.
fn day_scale(look: &DayLook) -> Vec3 {
    let level = if look.contrast == 1. { 0.25 } else { 0.25f32.powf(look.contrast) };
    look.light * level * 4.
}

pub(super) fn install(app: &mut App) {
    let hours = std::env::var("SKATE_DAY_HOUR")
        .map_or(Vec::new(), |v| v.split(',').filter_map(|h| h.trim().parse().ok()).collect());
    app.init_resource::<DayLook>().insert_resource(Forced { hours, ..default() }).add_systems(
        Update,
        (step_forced, apply).chain().after(crate::world_time::tick).before(super::weather::WeatherSet),
    );
}

/// Diagnostics: SKATE_DAY_HOUR=<h> holds the retail look at that hour. A list
/// (SKATE_DAY_HOUR=6,12,20) with --verify shows each for three seconds from 20 s,
/// saving <capture>_<h>.png, so one run (one window) shoots a whole day; give
/// SKATE_VERIFY_SECONDS room for it.
#[derive(Resource, Default)]
struct Forced {
    hours: Vec<f32>,
    shown: usize,
    captured: usize,
    clock: f32,
}
fn step_forced(mut commands: Commands, time: Res<Time<Real>>, config: Option<Res<crate::config::Config>>, mut forced: ResMut<Forced>) {
    let Some(path) = config.and_then(|c| c.verification_capture.clone()) else { return };
    if forced.hours.len() < 2 || forced.captured == forced.hours.len() {
        return;
    }
    forced.clock += time.delta_secs();
    let slot = (forced.clock - 20.) / 3.;
    if slot < 0. {
        return;
    }
    forced.shown = (slot as usize).min(forced.hours.len() - 1);
    // Late in each slot, once the exposure meter has settled.
    if slot.fract() > 0.8 && forced.captured == forced.shown {
        let stem = path.file_stem().map_or("capture".into(), |s| s.to_string_lossy().into_owned());
        let file = path.with_file_name(format!("{stem}_{}.png", forced.hours[forced.shown]));
        commands.spawn(bevy::render::view::screenshot::Screenshot::primary_window())
            .observe(bevy::render::view::screenshot::save_to_disk(file));
        forced.captured += 1;
    }
}

#[allow(clippy::too_many_arguments)]
fn apply(
    clock: Option<Res<crate::world_time::WorldTime>>,
    retail: Option<Res<super::RetailScene>>,
    customiser: Option<Res<crate::customiser::Customiser>>,
    authored_night: Query<(), With<super::sky::AuthoredNight>>,
    skies: Query<&MeshMaterial3d<super::RetailSkyMaterial>>,
    sky_materials: Res<Assets<super::RetailSkyMaterial>>,
    mut look: ResMut<DayLook>,
    mut state: ResMut<super::ShadowState>,
    mut exposure: ResMut<super::exposure::Settings>,
    mut ambient: ResMut<GlobalAmbientLight>,
    forced: Res<Forced>,
) {
    let retail = retail.is_some_and(|r| r.0);
    let on = retail && !customiser.is_some_and(|c| c.open) && authored_night.is_empty();
    // Steps of 30 game seconds (0.4 s real): characters' materials change a few
    // times a second, not every frame.
    let hour = forced.hours.get(forced.shown).copied().or(clock.map(|c| c.hour)).map_or(12., |h| (h * 120.).round() / 120.);
    let next = if on { at(hour) } else { DAY };
    look.set_if_neq(next);
    let authored = skies
        .iter()
        .find_map(|m| sky_materials.get(&m.0))
        .map(|m| m.params.sun_direction.truncate())
        .filter(|d| d.length_squared() > 0.5);
    let moving = match authored {
        Some(d) if on && hour != 12. => sun(d, hour).extend(1.),
        _ => Vec4::ZERO,
    };
    let day = [next.light.extend(next.contrast), next.sky.extend(next.night), next.glow.extend(0.), moving];
    if state.7 != day {
        state.7 = day;
    }
    let meter = Vec4::new(next.exposure.x, next.exposure.y, next.exposure.y, 1.);
    if exposure.day != meter {
        exposure.day = meter;
    }
    if retail {
        // map_render's retail scene ambient (350, white) lights the PBR props.
        let scale = day_scale(&next);
        let level = scale.dot(Vec3::new(0.2126, 0.7152, 0.0722));
        let (color, brightness) = if next == DAY {
            (Color::WHITE, crate::map_render::RETAIL_AMBIENT)
        } else {
            (Color::linear_rgb(scale.x / level, scale.y / level, scale.z / level), crate::map_render::RETAIL_AMBIENT * level)
        };
        if ambient.color != color || ambient.brightness != brightness {
            ambient.color = color;
            ambient.brightness = brightness;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn noon_is_authored_night_is_lit_and_midnight_wraps() {
        // Players who keep "Day" see today's look.
        assert_eq!(at(12.), DAY);
        assert_eq!(day_scale(&DAY), Vec3::ONE);
        // The cycle closes: 24 is 0, and the last minutes run into it.
        assert_eq!(at(24.), at(0.));
        assert!((at(23.99).light - at(0.).light).abs().max_element() < 1e-4);
        // Not too dark: lit ground (lightmap² 0.5) keeps a fifth of noon at the meter's
        // fixed baseline, and the meter still aims at half of day's brightness.
        let night = at(crate::world_time::NIGHT_HOUR);
        assert_eq!(night.night, 1.);
        let lit = night.light.dot(Vec3::new(0.2126, 0.7152, 0.0722)) * 0.5f32.powf(night.contrast) * night.exposure.y / 0.5;
        assert!(lit > 0.2 && night.exposure.x >= 0.5, "{lit}");
        // Every hour is finite and moves smoothly.
        for step in 0..=24 * 60 {
            let (a, b) = (at(step as f32 / 60.), at((step + 1) as f32 / 60.));
            assert!(a.light.is_finite() && (a.light - b.light).abs().max_element() < 0.03);
        }
        // The sun is the authored one at noon, set at 18 and the moon (its opposite) at midnight.
        let authored = Vec3::new(-0.4787, 0.736, 0.4787).normalize();
        assert!(sun(authored, 12.).distance(authored) < 1e-5);
        assert!(sun(authored, 18.).y.abs() < 1e-5);
        assert!((-sun(authored, 0.)).distance(authored) < 1e-5);
    }
}
