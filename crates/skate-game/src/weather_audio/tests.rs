use super::{assets::Bank, mixer::*};
use std::sync::Arc;

fn rms(buf: &[f32]) -> f32 { (buf.iter().map(|x| x * x).sum::<f32>() / buf.len() as f32).sqrt() }
fn run(m: &mut Mixer, seconds: f32) -> Vec<f32> {
    let mut out = vec![0.; (RATE as f32 * seconds) as usize * 2];
    for chunk in out.chunks_mut(1024) { m.render(chunk); }
    out
}

#[test]
fn clear_weather_is_silent_and_heavy_is_not() {
    let mut m = Mixer::new(Arc::new(Bank::synthetic()), 1);
    m.set(Params { rain: 0., master: 1., ..Default::default() });
    assert!(rms(&run(&mut m, 1.)) < 1e-6);
    m.set(Params { rain: 1., master: 1., ..Default::default() });
    let tail = run(&mut m, 8.);
    assert!(rms(&tail[tail.len() - 48_000..]) > 1e-3);
}

fn max_step(out: &[f32]) -> f32 {
    out.chunks(2).collect::<Vec<_>>().windows(2).map(|w| (w[1][0] - w[0][0]).abs()).fold(0., f32::max)
}

#[test]
fn weather_change_fades_without_clicks() { // Review Focus 1
    let mut m = Mixer::new(Arc::new(Bank::synthetic()), 2);
    m.set(Params { rain: 1., master: 1., ..Default::default() });
    let before = run(&mut m, 10.);
    m.set(Params { rain: 0., master: 1., ..Default::default() });
    let out = run(&mut m, 12.);
    assert!(max_step(&out) < 0.05, "click {}", max_step(&out));
    assert!(rms(&out[out.len() - 48_000..]) < 1e-4, "not silent after fade");
    // About 3 s, not a cut: a second in, it still holds 30% of the heavy rain's level.
    let (start, later) = (rms(&before[before.len() - 96_000..]), rms(&out[96_000..144_000]));
    assert!(later > start * 0.3, "{later} vs {start}");
}

#[test]
fn light_rain_transitions_fade_without_clicks() { // Review Focus 1, review I4
    for (from, to) in [(0.5, 0.), (1., 0.5), (0., 0.5)] {
        let mut m = Mixer::new(Arc::new(Bank::synthetic()), 12);
        m.set(Params { rain: from, master: 1., surfaces: [[1., 1.], [0.; 2], [0.; 2], [0.; 2], [0.; 2]], ..Default::default() });
        run(&mut m, 10.);
        m.set(Params { rain: to, master: 1., surfaces: [[1., 1.], [0.; 2], [0.; 2], [0.; 2], [0.; 2]], ..Default::default() });
        let out = run(&mut m, 12.);
        assert!(max_step(&out) < 0.05, "{from} -> {to}: click {}", max_step(&out));
    }
}

#[test]
fn snow_is_dry() { // Review Focus 1, review I4
    let mut menu = crate::graphics_menu::test_menu();
    for (weather, rain) in [(1, 0.5), (2, 1.), (3, 0.), (4, 0.)] {
        menu.set_session_world(Some([weather, 0]));
        assert_eq!(crate::retail_render::rain_level(Some(&menu), Some(&crate::retail_render::RetailScene(true)), None), rain);
    }
}

#[test]
fn cover_lowers_high_frequencies_and_adds_roof() {
    let bank = Arc::new(Bank::synthetic()); // roof clips are a distinct tone
    let mut open = Mixer::new(bank.clone(), 3);
    let mut covered = Mixer::new(bank, 3);
    let base = Params { rain: 1., master: 1., surfaces: [[0.5, 0.5], [0.; 2], [0.; 2], [0.; 2], [0.; 2]], ..Default::default() };
    open.set(base);
    covered.set(Params { cover: 1., ..base });
    let (a, b) = (run(&mut open, 6.), run(&mut covered, 6.));
    assert!(super::tests_util::hf_energy(&b) < super::tests_util::hf_energy(&a) * 0.25);
    assert!(super::tests_util::tone_energy(&b, super::assets::ROOF_TEST_TONE) > 1e-4);
}

#[test]
fn pause_muffles_but_keeps_playing() {
    let mut m = Mixer::new(Arc::new(Bank::synthetic()), 4);
    m.set(Params { rain: 1., master: 1., ..Default::default() });
    let live = rms(&run(&mut m, 8.)[48_000 * 12..]);
    m.set(Params { rain: 1., master: 1., paused: true, ..Default::default() });
    let paused = rms(&run(&mut m, 3.)[48_000 * 4..]);
    assert!(paused > live * 0.1 && paused < live * 0.5);
}

#[test]
fn master_follows_sfx_volume() {
    let mut a = Mixer::new(Arc::new(Bank::synthetic()), 5);
    let mut b = Mixer::new(Arc::new(Bank::synthetic()), 5);
    a.set(Params { rain: 1., master: 1., ..Default::default() });
    b.set(Params { rain: 1., master: 0.5, ..Default::default() });
    let (ra, rb) = (rms(&run(&mut a, 6.)[48_000 * 8..]), rms(&run(&mut b, 6.)[48_000 * 8..]));
    assert!((rb / ra - 0.25).abs() < 0.03); // master²
}

#[test]
fn missing_folder_gives_an_empty_bank_and_silence() { // Review Focus 5
    let bank = Bank::load(std::path::Path::new("Z:/definitely/not/here"));
    assert!(bank.is_empty());
    let mut m = Mixer::new(Arc::new(bank), 6);
    m.set(Params { rain: 1., master: 1., ..Default::default() });
    assert!(rms(&run(&mut m, 1.)) < 1e-6);
}

#[test]
fn partial_folder_reports_each_bad_clip_and_stays_silent() { // Review Focus 5, review I4
    let dir = std::env::temp_dir().join(format!("weather-audio-partial-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("manifest.json"), r#"{"version": 1, "sample_rate": 48000, "roles": {
        "bed_light": [{"file": "missing.ogg", "loop": true, "lufs": -30.0}],
        "bed_heavy": [{"file": "garbage.ogg", "loop": true, "lufs": -30.0}]}}"#).unwrap();
    std::fs::write(dir.join("garbage.ogg"), b"OggS not really").unwrap();
    let (bank, problems) = Bank::load_report(&dir);
    std::fs::write(dir.join("manifest.json"), r#"{"version": 2, "sample_rate": 48000, "roles": {}}"#).unwrap();
    let (newer, newer_problems) = Bank::load_report(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(bank.is_empty() && newer.is_empty());
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert!(problems[0].starts_with("missing.ogg") && problems[1].starts_with("garbage.ogg"), "{problems:?}");
    assert_eq!(newer_problems.len(), 1);
    let mut m = Mixer::new(Arc::new(bank), 6);
    m.set(Params { rain: 1., master: 1., ..Default::default() });
    assert!(rms(&run(&mut m, 1.)) < 1e-6);
    // The export goes on without weather.
    assert!(super::replay::OfflineWeather::new(Arc::new(Bank::default()), 800, 1600).is_none());
}

use super::thunder::*;
#[test] fn delay_is_speed_of_sound() { assert!((delay_s(3430.) - 10.).abs() < 1e-4); assert!(delay_s(MAX_KM * 1000.) <= 11.7); }
#[test] fn bins_by_distance() { assert_eq!(bin(1200.), "thunder_crack"); assert_eq!(bin(2000.), "thunder_roll"); assert_eq!(bin(3900.), "thunder_rumble"); }
#[test] fn farther_is_darker_and_quieter() {
    assert!(cutoff_hz(4000.) < cutoff_hz(1000.)); assert!((cutoff_hz(1000.) - 4000.).abs() < 1.);
    assert!(gain_db(4000.) < gain_db(1000.)); assert!((gain_db(1000.)).abs() < 1e-4);
}
#[test] fn never_repeats_back_to_back() {
    let mut last = None;
    for seed in 0..200u32 { let i = pick(seed, 4, last); assert_ne!(Some(i), last); last = Some(i); }
}
#[test] fn pan_follows_azimuth() {
    let right = pan(0.5, 0., 2000.); let left = pan(-0.5, 0., 2000.);
    assert!(right[1] > right[0] && left[0] > left[1]);
    let p = pan(0.3, 0., 1000.); assert!(((p[0]*p[0] + p[1]*p[1]) - 1.).abs() < 0.02);
}
#[test] fn strike_sounds_after_the_delay_and_cancel_drops_it() { // Review Focus 2/3 core
    use std::sync::Arc; use super::{assets::Bank, mixer::*};
    let mut m = Mixer::new(Arc::new(Bank::synthetic()), 7);
    m.set(Params { rain: 0., master: 1., ..Default::default() }); // thunder only
    m.strike(Thunder { azimuth: 0., distance_m: 1715., seed: 3 }, delay_s(1715.)); // 5 s
    let mut out = vec![0.; 48_000 * 2 * 7];
    for c in out.chunks_mut(1024) { m.render(c); }
    let before: f32 = out[..48_000 * 2 * 4].iter().map(|x| x.abs()).sum();
    let after: f32 = out[48_000 * 2 * 5..].iter().map(|x| x.abs()).sum();
    assert!(before < 1e-3 && after > 1.);
    m.strike(Thunder { azimuth: 0., distance_m: 3000., seed: 4 }, 2.);
    m.cancel_pending();
    let mut tail = vec![0.; 48_000 * 2 * 40]; // let the first strike finish too
    for c in tail.chunks_mut(1024) { m.render(c); }
    assert!(tail[48_000 * 2 * 38..].iter().all(|x| x.abs() < 1e-6));
}

#[test] fn hiss_scales_with_speed_and_is_silent_when_still_or_dry() {
    use super::wheels::Hiss;
    let render = |speed: f32, rain: f32| { let mut h = Hiss::new(9); let mut b = vec![0.; 48_000 * 2];
        for c in b.chunks_mut(1024) { h.mix_into(c, speed, rain); } (b.iter().map(|x| x*x).sum::<f32>() / b.len() as f32).sqrt() };
    assert!(render(0., 1.) < 1e-5);
    assert!(render(8., 0.) < 1e-5);
    assert!(render(8., 1.) > render(2., 1.) * 2.);
}

#[test] fn thunder_voices_are_reused_from_the_start() { // review I2
    let mut m = Mixer::new(Arc::new(Bank::synthetic()), 8);
    m.set(Params { master: 1., ..Default::default() }); // thunder only
    assert_eq!(m.thunder_ready(), [1, 1, 1]);
    for round in 0..3 {
        m.strike(Thunder { azimuth: 0., distance_m: 1200., seed: round }, 0.);
        assert!(rms(&run(&mut m, 0.5)) > 1e-3, "round {round} is heard");
        assert_eq!(m.thunder_ready(), [0, 1, 1], "round {round}: its voice is out");
        run(&mut m, 4.); // a 3 s clip, up to 6% slower
        assert_eq!(m.thunder_ready(), [1, 1, 1], "round {round}: and back once it ends");
    }
}

#[test] fn a_strike_out_of_range_is_held_to_the_storm() { // review I3
    let render = |distance_m: f32| {
        let mut m = Mixer::new(Arc::new(Bank::synthetic()), 9);
        m.set(Params { master: 1., ..Default::default() });
        m.strike(Thunder { azimuth: 0.2, distance_m, seed: 4 }, 0.);
        run(&mut m, 2.)
    };
    let near = render(1000.);
    assert!(rms(&near) > 1e-3 && near.iter().all(|x| x.is_finite() && x.abs() < 1.));
    assert!(render(0.) == near && render(1e-20) == near, "a distance under 1 km plays as 1 km");
    assert!(render(f32::NAN).iter().all(|&x| x == 0.), "a NaN distance is dropped");
}

#[test] fn paused_wheels_are_still() { // review M4
    let tail = |wheel_speed: f32| {
        let mut m = Mixer::new(Arc::new(Bank::synthetic()), 10);
        m.set(Params { rain: 1., master: 1., paused: true, wheel_speed, ..Default::default() });
        let out = run(&mut m, 4.);
        out[out.len() - 48_000..].to_vec()
    };
    let (rolling, still) = (tail(8.), tail(0.));
    let hiss = rms(&rolling.iter().zip(&still).map(|(a, b)| a - b).collect::<Vec<_>>());
    assert!(hiss < 1e-6, "{hiss}");
}

#[test] fn heavy_rain_does_not_decode_the_light_hard_layer() { // review M5
    let mut m = Mixer::new(Arc::new(Bank::synthetic()), 11);
    m.set(Params { rain: 1., master: 1., surfaces: [[1., 1.], [0.; 2], [0.; 2], [0.; 2], [0.; 2]], ..Default::default() });
    run(&mut m, 15.);
    assert_eq!(m.layer_gain("hard_light"), [0.; 2]);
    assert!(m.layer_gain("hard_heavy")[0] > 0.1);
}

// Task 6: the census, against a 40x40 m ground (Hard material 0 at x < 0, metal_sheet 30 at x > 0) and an
// optional 6x6 m concrete roof at y = 4 (faces down), built as physics/climbing/tests.rs does.
use super::census::{Census, class_of};
use bevy::math::Vec3;
fn synthetic_world(roof: bool) -> skate_core::physics::board_world::BoardWorld {
    synthetic_world_with(if roof { &[[[-3., 4., -3.], [3., 4., -3.], [3., 4., 3.], [-3., 4., 3.]]] } else { &[] })
}
/// The ground plus `extra` Hard quads (their winding sets the face normal).
fn synthetic_world_with(extra: &[[[f32; 3]; 4]]) -> skate_core::physics::board_world::BoardWorld {
    use skate_core::{math::Vector3, physics::{board_world::{BoardWorld, WorldTriangle, query_metadata::{Bounds, QueryMesh, QueryMetadata, QueryPool}}, collision::TriangleFeature, contact::RetailContactMaterial, drive_frames::RetailAffineTransform}};
    let mut triangles = Vec::new();
    let mut quad = |p: [[f32; 3]; 4], tag: u32| for ids in [[0, 1, 2], [0, 2, 3]] {
        triangles.push(WorldTriangle::from_vertices(ids.map(|i| Vector3::new(p[i][0], p[i][1], p[i][2])),
            RetailContactMaterial { static_friction: 0.8, dynamic_friction: 0.6, restitution: 0. }, tag, TriangleFeature::ONE_SIDED, [1.; 3], 0.).unwrap());
    };
    quad([[-20., 0., -20.], [-20., 0., 20.], [0., 0., 20.], [0., 0., -20.]], 0);
    quad([[0., 0., -20.], [0., 0., 20.], [20., 0., 20.], [20., 0., -20.]], 30);
    extra.iter().for_each(|&p| quad(p, 0));
    let count = triangles.len();
    BoardWorld::with_query_metadata(triangles, QueryMetadata { packed_surfaces: vec![0; count], static_edges: vec![], island_flags: 0,
        meshes: vec![QueryMesh { geometry: 1, rejection_flags: 0, triangle_range: 0..count, pool: QueryPool::Ground, matching_group: 0,
            local_bounds: Bounds { min: Vector3::new(-20., -1., -20.), max: Vector3::new(20., 25., 20.) },
            local_to_world: RetailAffineTransform::IDENTITY, world_to_local: RetailAffineTransform::IDENTITY }] }).unwrap()
}
#[test] fn census_hears_metal_on_the_right() {
    let world = synthetic_world(false);
    let c = Census::sample(&world, Vec3::new(0., 1.6, 0.), Vec3::X, 0.);
    let s = c.surfaces();
    assert!(s[Surface::Metal as usize][1] > 0.8 && s[Surface::Hard as usize][0] > 0.8, "{s:?}");
    assert!(c.cover < 0.05);
}
#[test] fn roof_overhead_is_cover_not_ground() {
    let world = synthetic_world(true);
    let c = Census::sample(&world, Vec3::new(0., 1.6, 0.), Vec3::X, 0.);
    assert!(c.cover >= 0.6, "{c:?}");
    let s = c.surfaces(); // ground under the roof still counts
    assert!(s[Surface::Hard as usize][0] > 0.5, "{s:?}");
}
#[test] fn unknown_material_is_hard() { assert_eq!(class_of(143), Surface::Hard); }
#[test] fn cover_edge_does_not_flutter() { // Review Focus 4, review I4: the mixer's own cover smoothing
    let mut m = Mixer::new(Arc::new(Bank::synthetic()), 13);
    let mut seen = vec![];
    for i in 0..20 {
        // The census flipping at a roof's edge, every census.
        m.set(Params { rain: 1., master: 1., cover: if i % 2 == 0 { 0.6 } else { 0. }, ..Default::default() });
        run(&mut m, 0.25);
        if i >= 4 { seen.push(m.cover_level()); }
    }
    assert!(seen.iter().all(|v| (0.15..=0.45).contains(v)), "{seen:?}");
}
#[test] fn walls_are_not_cover() { // review M6
    // A building front 5 m to the right, facing the camera; the 45° ray that way hits it 5 m up.
    let world = synthetic_world_with(&[[[5., 0., -20.], [5., 0., 20.], [5., 20., 20.], [5., 20., -20.]]]);
    let camera = Vec3::new(0., 1.6, 0.);
    let wall = super::census::cast(&world, camera, camera + Vec3::new(1., 1., 0.).normalize() * 20.);
    assert!(wall.is_some_and(|(_, _, ny)| ny.abs() < 0.1), "the ray hits the wall: {wall:?}");
    let c = Census::sample(&world, camera, Vec3::X, 0.);
    assert!(c.cover < 0.05, "{c:?}");
}
#[test] fn census_rays_stay_in_budget() { // review M7
    // The worst case: a roof over every ring spot (each down ray casts again from under it).
    let world = synthetic_world_with(&[[[-20., 4., -20.], [-20., 4., 20.], [20., 4., 20.], [20., 4., -20.]]]);
    let rays = || super::census::RAYS.with(std::cell::Cell::take);
    rays();
    let mut c = Census::default();
    let mut per_part = vec![];
    for part in 0..Census::PARTS {
        c.sample_part(&world, Vec3::new(0., 1.6, 0.), Vec3::X, 0.3, part);
        per_part.push(rays());
    }
    let census: usize = per_part.iter().sum::<usize>() + 1; // + the skater's cover ray
    assert!(per_part.iter().all(|&n| n <= 16), "{per_part:?} rays in a frame");
    assert!((48..=54).contains(&census), "{census} rays a census (the re-casts happened)");
    let per_second = census as f32 / (Census::PARTS as f32 * super::CENSUS_EVERY);
    assert!(per_second <= 110., "{per_second} rays a second");
}

// Task 7: the storm's distance and the driver's cancel.
#[test] fn storm_strike_carries_a_distance_in_range() {
    let mut storm = crate::retail_render::weather_tests_api::storm(42);
    let mut fired = None;
    for _ in 0..4000 { storm.step(1. / 60., true); if storm.age() == Some(0.) { fired = Some(storm.distance_m()); break; } }
    let d = fired.expect("a strike within ~67 s");
    assert!((1000. ..=4000.).contains(&d));
}
/// The driver alone (no audio) on a retail map in WEATHER `weather` (0 clear, 1 light, 2 heavy rain).
fn drive_app(weather: u32) -> bevy::prelude::App {
    use bevy::prelude::*;
    let mut app = App::new();
    app.add_plugins(MinimalPlugins).add_plugins(super::WeatherAudioPlugin { headless: true });
    let mut menu = crate::graphics_menu::test_menu();
    menu.set_session_world(Some([weather, 0]));
    app.insert_resource(menu).insert_resource(crate::retail_render::RetailScene(true));
    app
}
fn flash(app: &mut bevy::prelude::App, distance_m: f32) {
    use crate::retail_render::LightningStrike;
    app.world_mut().resource_mut::<bevy::ecs::message::Messages<LightningStrike>>().write(LightningStrike { azimuth: 0.3, distance_m, seed: 5 });
}
/// What the mixer thread would take from the link: the rain, the cancel and the strikes (cleared, as it does).
fn take(app: &bevy::prelude::App) -> (f32, bool, Vec<(Thunder, f32)>) {
    let link = app.world().resource::<super::WeatherLink>().0.clone();
    let mut l = link.lock().unwrap();
    (l.params.rain, std::mem::take(&mut l.cancel), l.strikes.iter_mut().filter_map(Option::take).collect())
}
#[test] fn pending_thunder_is_cancelled_when_heavy_rain_stops() { // Review Focus 2
    let mut app = drive_app(2);
    app.update();
    flash(&mut app, 3000.);
    app.update();
    let (_, cancel, strikes) = take(&app);
    assert!(!cancel && strikes.len() == 1, "the strike is queued while heavy");
    assert!((strikes[0].1 - delay_s(3000.)).abs() < 1e-4);
    app.world_mut().resource_mut::<crate::graphics_menu::Menu>().set_session_world(Some([1, 0])); // Light Rain
    app.update();
    let (_, cancel, strikes) = take(&app);
    assert!(cancel && strikes.is_empty());
    super::duck::publish_rain(0.); // review M10: the global must not leak into other tests
}
#[test] fn no_rain_or_thunder_behind_the_boot_or_a_loading_screen() { // review I1
    let mut app = drive_app(2);
    app.insert_resource(crate::boot::BootActive(true)).insert_resource(crate::tip_loader::LoadingHold(false));
    for (boot, loading) in [(true, false), (false, true), (false, false)] {
        app.world_mut().resource_mut::<crate::boot::BootActive>().0 = boot;
        app.world_mut().resource_mut::<crate::tip_loader::LoadingHold>().0 = loading;
        flash(&mut app, 2000.);
        app.update();
        let (rain, cancel, strikes) = take(&app);
        let up = boot || loading;
        assert_eq!((rain, cancel, strikes.len()), if up { (0., true, 0) } else { (1., false, 1) }, "boot {boot}, loading {loading}");
    }
    super::duck::publish_rain(0.);
}

// Task 8: the ambience duck.
#[test] fn rain_ducks_nature_beds_more_than_city_ones() {
    use super::duck::duck_scale;
    let want = [10f32.powf(-15. / 20.), 10f32.powf(-6. / 20.), 10f32.powf(-6. / 20.)];
    for (name, w) in ["amb_birds_park", "amb_city_traffic", "amb_unknown"].into_iter().zip(want) {
        assert!((duck_scale(name, 1.) - w).abs() < 0.01, "{name}");
        assert_eq!(duck_scale(name, 0.), 1.);
    }
    assert!((duck_scale(super::bed_name(5), 0.5) - 10f32.powf(-6. / 20.)).abs() < 0.01); // 05_dt_parks, light
}
#[test] fn the_duck_comes_back_to_exactly_one() { // review M9: at 1.0 the duck is skipped (retail 1:1)
    let mut duck = super::AmbienceDuck::default();
    for _ in 0..600 { duck.step_with("amb_birds_park", 1., 0.05); }
    let last = (0..1200).map(|_| duck.step_with("amb_birds_park", 0., 0.05)).last();
    assert_eq!(last, Some(1.));
}
#[test] fn the_export_worker_ducks_to_the_recorded_rain() { // review M11
    let ducked = std::thread::spawn(|| {
        super::export_rain(1.);
        let mut duck = super::AmbienceDuck::default();
        (0..1200).map(|_| duck.step("amb_birds_park", 0.05)).last()
    });
    let ducked = ducked.join().unwrap().unwrap();
    assert!((ducked - 10f32.powf(-15. / 20.)).abs() < 1e-3, "{ducked}");
}
#[test] fn scale_gain_touches_only_the_gain_bits() { // review M13 (skate-audio-core's duck)
    use skate_audio_core::{Guest, mixmap::controller::{read_gain, scale_gain, set_outputs}};
    const CTRL: u32 = 0x1000;
    const OUTPUTS: u32 = 0x1100;
    let mut g = Guest::single(CTRL, 0x200);
    set_outputs(&mut g, CTRL, OUTPUTS).unwrap();
    // Output 0 in the low half (gain 0x4000, its top bit set), output 1 in the high half (gain 0x7FFF).
    g.set_u32(OUTPUTS, 0x7FFF_C000).unwrap();
    scale_gain(&mut g, CTRL, 0, 1.).unwrap();
    assert_eq!(g.u32(OUTPUTS).unwrap(), 0x7FFF_C000, "1.0 changes nothing");
    scale_gain(&mut g, CTRL, 0, 0.5).unwrap();
    assert_eq!(g.u32(OUTPUTS).unwrap(), 0x7FFF_A000, "output 0: half the gain, bit 15 kept");
    scale_gain(&mut g, CTRL, 1, 0.5).unwrap();
    assert_eq!(g.u32(OUTPUTS).unwrap(), 0x4000_A000, "output 1 is the high half");
    assert_eq!(read_gain(&g, CTRL, 1).unwrap(), 0x4000);
}

// Task 9: replays and the export.
#[test] fn replay_round_trips_the_weather_track() {
    use crate::replay::{editor::Editor, file, record::{self, Ring}};
    use crate::retail_render::LightningStrike;
    let strike = LightningStrike { azimuth: -0.7, distance_m: 2345., seed: 9 };
    let mut ring = Ring::default();
    for i in 0..130 {
        let mut f = record::tests::frame(i, 1);
        f.rain = if i < 60 { 0.5 } else { 1. };
        f.strike = (i == 70).then_some(strike);
        ring.push(f);
    }
    ring.flush();
    let bytes = file::encode(&file::Header::new("m", None, &ring), &ring, &Editor::new(false));
    let (_, loaded, _) = file::decode(&bytes).expect("loads");
    for i in [0, 59, 60, 70, 129] {
        let f = loaded.get(i).unwrap();
        assert_eq!((f.rain, f.strike), (if i < 60 { 0.5 } else { 1. }, (i == 70).then_some(strike)), "frame {i}");
    }
    // A version 2 block (before the weather): one frame, identity camera, nothing else. It loads dry.
    let mut raw = vec![2u8, 1, 0];
    raw.extend_from_slice(&0f64.to_le_bytes());
    raw.push(0);
    raw.extend_from_slice(&1f32.to_le_bytes());
    raw.extend_from_slice(&[0, 0, 0, 3, 0, 0, 0, 0]); // camera: position, largest, rotation, scale flag
    raw.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0]); // no actors, no props, no audio
    let old = record::decode_block(&zstd::bulk::compress(&raw, 3).unwrap()).expect("a version 2 block loads");
    assert_eq!((old[0].rain, old[0].strike), (0., None));
}
#[test] fn scrub_back_past_a_strike_does_not_double_fire() { // Review Focus 3
    use super::replay::ReplayThunder;
    use crate::retail_render::LightningStrike;
    const F: usize = 300;
    let strikes = [(F as f64 / 60., LightningStrike { azimuth: 0., distance_m: 2000., seed: 1 })];
    let time = |i: usize| Some(i as f64 / 60.);
    let at = |i: usize| i as f64 / 60.;
    let mut t = ReplayThunder::default();
    let mut fired = 0;
    for i in 0..=F + 10 { fired += t.step(i, at(i), true, 1., time, strikes.into_iter()).strikes.len(); }
    assert_eq!(fired, 1);
    assert!(t.step(F - 120, at(F - 120), false, 1., time, strikes.into_iter()).cancel, "the seek cancels");
    let mut again = 0;
    for i in F - 119..=F + 10 {
        let step = t.step(i, at(i), true, 1., time, strikes.into_iter());
        assert!(!step.cancel || i == F - 119, "only the resume cancels");
        again += step.strikes.len();
    }
    assert_eq!(again, 1, "exactly one strike after the seek");
}
#[test] fn replay_pause_holds_thunder_and_a_seek_lands_in_it() { // review M1, M2
    use super::replay::{ReplayThunder, Step};
    use crate::retail_render::LightningStrike;
    // A strike at 5 s (frame 300), 3430 m away: its thunder comes 10 s later.
    let strikes = [(5., LightningStrike { azimuth: 0., distance_m: 3430., seed: 1 })];
    let time = |i: usize| Some(i as f64 / 60.);
    let at = |i: usize| i as f64 / 60.;
    let mut t = ReplayThunder::default();
    let mut step = |i: usize, playing: bool| t.step(i, at(i), playing, 1., time, strikes.into_iter());
    let fired: usize = (0..=360).map(|i| step(i, true).strikes.len()).sum();
    assert_eq!(fired, 1);
    // Paused at 6 s: the thunder pending is dropped, and nothing more while the playhead holds.
    assert!(step(360, false).cancel);
    assert_eq!(step(360, false), Step::default());
    // Resumed: it is queued again with the 9 s it has left.
    let resume = step(360, true);
    assert!(resume.cancel && resume.strikes.len() == 1 && (resume.strikes[0].1 - 9.).abs() < 1e-3, "{resume:?}");
    assert!(step(361, true).strikes.is_empty(), "and not again as play goes on");
    // A seek to 8 s, past the flash: its thunder is 7 s off.
    let seek = step(480, true);
    assert!(seek.cancel && seek.strikes.len() == 1 && (seek.strikes[0].1 - 7.).abs() < 1e-3, "{seek:?}");
    // A seek past the thunder (16 s): nothing.
    let late = step(960, true);
    assert!(late.cancel && late.strikes.is_empty());
}
#[test] fn drive_follows_the_replay_and_drops_its_thunder_after() { // Review Focus 3, review I4 + M3
    use crate::replay::{Replay, record::{self, Ring}};
    use crate::retail_render::LightningStrike;
    use bevy::prelude::App;
    let mut ring = Ring::default();
    for i in 0..600 {
        let mut f = record::tests::frame(i, 1);
        f.rain = 1.;
        f.strike = (i == 300).then_some(LightningStrike { azimuth: 0., distance_m: 2000., seed: 3 });
        ring.push(f);
    }
    let mut replay = Replay::default();
    replay.install(ring);
    replay.enter();
    let mut app = drive_app(0); // the live weather is Clear
    app.insert_resource(replay);
    app.update();
    let (rain, cancel, strikes) = take(&app);
    assert!(rain == 1. && cancel && strikes.is_empty(), "the recorded rain; the live thunder dropped");
    fn play_at(app: &mut App, t: f64) -> (f32, bool, Vec<(Thunder, f32)>) {
        {
            let mut replay = app.world_mut().resource_mut::<Replay>();
            replay.playing = true;
            replay.seek(t);
        }
        app.update();
        take(app)
    }
    assert!(play_at(&mut app, 4.9).2.is_empty());
    let (_, cancel, strikes) = play_at(&mut app, 5.05);
    assert!(!cancel && strikes.len() == 1, "playing through the flash queues it");
    assert!((strikes[0].1 - (delay_s(2000.) - 0.05)).abs() < 0.02, "{strikes:?}");
    let (_, cancel, strikes) = play_at(&mut app, 4.5);
    assert!(cancel && strikes.is_empty(), "scrubbing back drops it");
    let again: usize = [4.7, 4.9, 5.1, 5.3].into_iter().map(|t| play_at(&mut app, t).2.len()).sum();
    assert_eq!(again, 1, "playing through again fires it once");
    // Review M3: back to live play in heavy rain, the replay's thunder on its way is dropped.
    app.world_mut().resource_mut::<crate::graphics_menu::Menu>().set_session_world(Some([2, 0]));
    app.world_mut().resource_mut::<Replay>().exit();
    app.update();
    assert!(take(&app).1, "leaving the replay cancels");
    super::duck::publish_rain(0.);
}
#[test] fn a_replay_strike_out_of_range_does_not_load() { // review I3
    use crate::replay::record;
    let block = |distance: f32| {
        // One version 3 frame: identity camera, nothing else, heavy rain and a strike.
        let mut raw = vec![3u8, 1, 0];
        raw.extend_from_slice(&0f64.to_le_bytes());
        raw.push(0);
        raw.extend_from_slice(&1f32.to_le_bytes());
        raw.extend_from_slice(&[0, 0, 0, 3, 0, 0, 0, 0]);
        raw.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0]);
        raw.extend_from_slice(&[200, 1]);
        raw.extend_from_slice(&0.5f32.to_le_bytes());
        raw.extend_from_slice(&distance.to_le_bytes());
        raw.extend_from_slice(&7u32.to_le_bytes());
        record::decode_block(&zstd::bulk::compress(&raw, 3).unwrap())
    };
    assert_eq!(block(2000.).unwrap()[0].strike.map(|s| s.distance_m), Some(2000.));
    for bad in [0., 1e-3, 999., 4001., f32::INFINITY, f32::NAN] {
        assert!(block(bad).is_err(), "{bad} m");
    }
}
#[test] fn a_version_1_save_loads_dry() { // review M13
    use crate::replay::{editor::Editor, file, record::{self, Ring}};
    let mut ring = Ring::default();
    ring.push(record::tests::frame(0, 1));
    let header = file::Header { version: 1, ..file::Header::new("m", None, &ring) };
    // Its block is version 2: no weather bytes and no version 4 joint count. A lone bone-less skater with no props
    // or audio ends with `u16 joints 0, u32 props 0, u8 audio 0, u8 rain 0, u8 strike 0`.
    let skater = record::Actor { id: 0, root: bevy::prelude::Transform::IDENTITY, bones: Vec::new(), joints: Vec::new(),
        visible: true };
    let dry = record::RecFrame { actors: vec![skater], ..Default::default() };
    let mut raw = zstd::stream::decode_all(record::encode_block(&[dry]).as_slice()).unwrap();
    let tail = raw.len() - 9;
    assert_eq!(raw[tail..], [0, 0, 0, 0, 0, 0, 0, 0, 0]);
    raw[0] = 2;
    raw.truncate(raw.len() - 2);
    raw.drain(tail..tail + 2);
    let chunk = |out: &mut Vec<u8>, bytes: &[u8]| {
        out.extend((bytes.len() as u32).to_le_bytes());
        out.extend(bytes);
    };
    let mut bytes = file::MAGIC.to_vec();
    bytes.extend(1u16.to_le_bytes());
    chunk(&mut bytes, &serde_json::to_vec(&header).unwrap());
    bytes.extend(1u32.to_le_bytes());
    chunk(&mut bytes, &zstd::bulk::compress(&raw, 3).unwrap());
    chunk(&mut bytes, &serde_json::to_vec(&file::timeline_json(&Editor::new(false))).unwrap());
    let path = std::env::temp_dir().join(format!("weather-audio-v1-{}.s3replay", std::process::id()));
    std::fs::write(&path, &bytes).unwrap();
    let (head, loaded) = (file::header(&path), file::load(&path));
    let _ = std::fs::remove_file(&path);
    assert_eq!(head.map(|h| h.version), Some(1));
    let (_, ring, _) = loaded.expect("a version 1 save loads");
    let frame = ring.get(0).unwrap();
    assert_eq!((ring.len(), frame.rain, frame.strike), (1, 0., None));
}
#[test] fn a_replay_shows_the_recorded_rain_and_lightning() { // review M13
    use crate::replay::{Replay, record::{self, Ring}};
    use crate::retail_render::{LightningStrike, weather_tests_api::recorded};
    let mut ring = Ring::default();
    for i in 0..600 {
        let mut f = record::tests::frame(i, 1);
        f.rain = 1.;
        f.strike = (i == 300).then_some(LightningStrike { azimuth: 0.8, distance_m: 2000., seed: 42 });
        ring.push(f);
    }
    let mut replay = Replay::default();
    replay.install(ring);
    assert!(recorded(&replay).is_none(), "not in a replay: the live weather");
    replay.enter();
    replay.seek(5.05);
    let (rain, flash, strike) = recorded(&replay).unwrap();
    assert_eq!((rain, strike), (1., Some((0.8, 42.))));
    assert!(flash > 0.1, "{flash}");
    replay.seek(4.9);
    assert_eq!(recorded(&replay).unwrap().1, 0., "before the flash");
    replay.seek(6.);
    assert_eq!(recorded(&replay).unwrap().2, None, "after it");
}
#[test] fn export_contains_thunder_at_the_recorded_time() {
    use super::replay::{OfflineWeather, ReplayThunder, export_tick};
    use crate::replay::{Replay, record::{self, Ring}};
    use crate::retail_render::LightningStrike;
    // 10 s recorded dry (the bed muted), a strike at 1 s, 1715 m away: its thunder lands at 6 s.
    let mut ring = Ring::default();
    for i in 0..601 {
        let mut f = record::tests::frame(i, 1);
        f.strike = (i == 60).then_some(LightningStrike { azimuth: 0., distance_m: 1715., seed: 2 });
        ring.push(f);
    }
    let mut replay = Replay::default();
    replay.install(ring);
    let mut weather = OfflineWeather::new(Arc::new(Bank::synthetic()), 800, 1600).unwrap();
    let mut thunder = ReplayThunder::default();
    let mut out = Vec::new();
    for frame in 0..300u32 { // 30 fps: two 60 Hz ticks a frame
        let ticks: Vec<_> = (frame * 2..frame * 2 + 2).map(|tick| export_tick(&replay, tick as f32 / 600., 1., &mut thunder)).collect();
        out.extend(weather.frame(&ticks));
    }
    assert_eq!(out.len(), 48_000 * 10 * 2);
    let energy = |a: f32, b: f32| out[(a * 96_000.) as usize..(b * 96_000.) as usize].iter().map(|x| x * x).sum::<f32>();
    assert!(energy(2., 4.) < 1e-9, "{}", energy(2., 4.));
    assert!(energy(6., 7.) > 1e-3, "{}", energy(6., 7.));
}
