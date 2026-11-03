//! Original loading screen (main/tiploader), shown over everything while a map
//! loads (pause menu Load map, Challenge Map travel, multiplayer map changes).
//! Missing or invalid prepared assets leave the pause panel's loading text.
use crate::{
    apt_canvas::{Canvas, movie_textures},
    config::Config,
    map_transition::MapTransition,
    scoring_hud::{HudComposite, HudMaterial},
    tip_loader_runtime::Runtime,
};
use bevy::{prelude::*, window::PrimaryWindow};
use std::{path::PathBuf, sync::Arc};

const LAYER: usize = 34;
/// The movie is authored at 16 ms per frame.
const FRAME: f32 = 1. / 60.;
/// Outro fallback if the movie never reports OutroComplete.
const CLOSE_TIMEOUT: f32 = 0.6;
/// The cover's close rate (0x825E6BF8, not immediate: -0.04 from 0x82216FEC), added once per update by 0x825E69A0,
/// which stops at 0 (the 25th update in f32): the black behind the screen fades out over its outro.
const COVER_STEP: f32 = 0.04;
/// Online audit 10-28 (s3re): after a session respawn, retail raises the black again (0x826EBA38) and lifts it once
/// everything within 30.0 has streamed in and 15 more frames have passed (0x826EB8E0). Our map is resident from its
/// load, so only the 15 frames are left, at the loader's 60 Hz: a join's cover (here) and the trip back after a
/// match (match_core) hold this long after they land.
pub(crate) const SETTLE: f32 = 15. * FRAME;

#[derive(Component)]
struct Overlay;

/// Skate 3's Lua loading-screen hold (OpenLoadingScreen / CloseLoadingScreen, TU3 natives 0x82840578 /
/// 0x82840460: one requester bit, 1 << 1, of the shared cover controller 0x825e6bf8). An S3 online mode
/// sets it from match start until its script's CloseLoadingScreen point; the cover stays while either
/// the map load or this hold is up.
#[derive(Resource, Default)]
pub(crate) struct LoadingHold(pub bool);

/// Joining another player's session (online_session): the cover stays from the join's map load until their lobby is
/// joined and they are seen, so the player never stands alone on the map in between. Its own flag, because the modes
/// write `LoadingHold` every frame.
#[derive(Resource, Default, PartialEq)]
pub(crate) struct JoinCover(pub bool);

/// A Skate 3 match's start (match_core::starts_covered): the cover from its start until its mode's own takes over. Its
/// own flag, as JoinCover's.
#[derive(Resource, Default, PartialEq)]
pub(crate) struct MatchCover(pub bool);

/// The loading screen is up and opaque (not closing), or none can show: what is behind it moves unseen (User 10-26:
/// online_session's spawn, online_moves' leave, a match's start slot). As `animate` last drew it.
#[derive(Resource, Default, PartialEq)]
pub(crate) struct Covered(pub bool);

#[derive(Resource)]
struct TipLoader {
    source: Arc<serde_json::Value>,
    wheel: Option<Arc<serde_json::Value>>,
    canvas: Canvas,
    runtime: Option<Runtime>,
    /// perf 10-18: the next load's screen booted ahead, as core_menu's Booted (Runtime::load ran on the frame a load
    /// began): setup's check, then a worker's after each show. SKATE_TIP_PREBOOT=0 boots on that frame.
    ready: Option<Runtime>,
    spare: Option<std::thread::JoinHandle<Result<Runtime, String>>>,
    closing: Option<f32>,
    /// The cover's alpha (0x830CFE14 +304 +12): 1 while loading, falling by [`COVER_STEP`] once closing.
    cover: f32,
    accumulated: f32,
    loads: u32,
}

pub(crate) struct TipLoaderPlugin;
impl Plugin for TipLoaderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LoadingHold>().init_resource::<JoinCover>().init_resource::<MatchCover>().init_resource::<Covered>()
            .add_systems(PostStartup, setup.after(crate::graphics_menu::PresentationSetup))
            .add_systems(Update, (resize_target, animate).chain());
    }
}

/// What is left of a join's [`SETTLE`] after this frame: full while its cover is up, then counting down.
fn settle(left: f32, join: bool, dt: f32) -> f32 {
    if join { SETTLE } else { (left - dt).max(0.) }
}

fn root(asset_root: &std::path::Path) -> PathBuf {
    std::env::var_os("SKATE_TIP_LOADER_ROOT").map(PathBuf::from).unwrap_or_else(|| asset_root.join("private/tip-loader"))
}
/// What setup reads, for the startup prefetch (apt_canvas::prefetch).
pub(crate) fn manifest(asset_root: &std::path::Path) -> PathBuf {
    root(asset_root).join("runtime/tiploader.json")
}
/// Each load opens on a different tip.
fn seed(load: u32) -> u32 {
    load.wrapping_mul(0x9e37_79b9) ^ std::process::id()
}
/// Boots load `loads + 1`'s screen on a worker.
fn prepare(loader: &mut TipLoader) {
    if !crate::config::perf_on("SKATE_TIP_PREBOOT") {
        return;
    }
    let (source, wheel, seed) = (loader.source.clone(), loader.wheel.clone(), seed(loader.loads + 1));
    let boot = move || Runtime::load(&source, wheel.as_deref(), seed);
    loader.spare = std::thread::Builder::new().name("tip-preboot".into()).spawn(boot).ok();
}

fn setup(
    mut commands: Commands,
    config: Res<Config>,
    cameras: Query<Entity, With<IsDefaultUiCamera>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut images: ResMut<Assets<Image>>,
    mut composites: ResMut<Assets<HudComposite>>,
) {
    let Ok(output) = cameras.single() else { return };
    let root = root(&config.asset_root);
    let mut canvas = Canvas::new(&mut commands, &mut images, &mut composites, &window, root.clone(), LAYER, -6);
    // Edge to edge, as on the console. ponytail: plain cover crop; 16:10 loses 64 stage px per side and
    // "Loading..." (x 98) and the wheel (to x 1180) stay; a near-square or portrait window would crop them.
    canvas.cover(&mut commands);
    let result = (|| -> Result<(serde_json::Value, Option<serde_json::Value>, Runtime), String> {
        // perf 10-18: main() prefetches it.
        let mut source: serde_json::Value = match crate::apt_canvas::prefetched_json(&root.join("runtime/tiploader.json")) {
            Some(json) => json,
            None => serde_json::from_slice(
                &std::fs::read(root.join("runtime/tiploader.json")).map_err(|e| format!("{}: {e}", root.display()))?,
            )
            .map_err(|e| e.to_string())?,
        };
        if source["format"] != "skate3-tip-loader" || source["version"] != 1 {
            return Err("Unsupported loading screen manifest".into());
        }
        // ponytail: a loading screen prepared before its glyphs were (F17) borrows the music screens' copy of the
        // same miscboot.big art, by absolute path; a hud refresh puts them in its own manifest.
        if source.get("buttons").is_none() {
            source["buttons"] = lend_buttons(&config.asset_root.join("private/music-ui"));
        }
        // Run the movie once now, so a defective install falls back before a map loads. perf 10-18: with the wheel
        // when it boots (that one run checks both; it ran twice), kept as the first load's screen.
        // The corner wheel is optional: without it the screen still shows.
        let booted = std::fs::read(root.join("runtime/autosave.json"))
            .map_err(|e| e.to_string())
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|e| e.to_string()))
            .and_then(|wheel| {
                if wheel["format"] != "skate3-busy-tag" || wheel["version"] != 1 {
                    return Err("Unsupported busy tag manifest".into());
                }
                Runtime::load(&source, Some(&wheel), seed(1)).map(|runtime| (wheel, runtime))
            })
            .inspect_err(|error| warn!("Loading wheel unavailable ({error})"));
        let (wheel, runtime) = match booted {
            Ok((wheel, runtime)) => (Some(wheel), runtime),
            Err(_) => (None, Runtime::load(&source, None, seed(1))?),
        };
        for movie in std::iter::once(&source).chain(&wheel) {
            for (path, size) in movie_textures(movie)? {
                canvas.preload(&mut images, &path, size)?;
            }
        }
        Ok((source, wheel, runtime))
    })();
    let (source, wheel, booted) = match result {
        Ok(value) => value,
        Err(error) => {
            warn!("Original loading screen unavailable ({error}); map loads show the pause panel's progress. Prepare it with tools/prepare_tip_loader.py");
            return;
        }
    };
    let full = || Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() };
    // Full screen over the paused world and every menu, black as the console shows it.
    commands
        .spawn((
            Overlay,
            UiTargetCamera(output),
            GlobalZIndex(25),
            Pickable::IGNORE,
            BackgroundColor(Color::BLACK),
            Node { display: Display::None, ..full() },
        ))
        .with_child((MaterialNode(canvas.composite.clone()), Pickable::IGNORE, full()));
    let ready = crate::config::perf_on("SKATE_TIP_PREBOOT").then_some(booted);
    let (source, wheel) = (Arc::new(source), wheel.map(Arc::new));
    commands.insert_resource(TipLoader { source, wheel, canvas, runtime: None, ready, spare: None, closing: None, cover: 1., accumulated: 0., loads: 0 });
    info!("Original loading screen loaded from {}", root.display());
}

/// Another screen folder's controller glyphs (`buttons` in its chyron manifest), their paths made absolute.
fn lend_buttons(root: &std::path::Path) -> serde_json::Value {
    let Ok(bytes) = std::fs::read(root.join("runtime/chyron.json")) else { return serde_json::Value::Null };
    let Ok(mut manifest) = serde_json::from_slice::<serde_json::Value>(&bytes) else { return serde_json::Value::Null };
    let mut buttons = manifest["buttons"].take();
    for texture in buttons.as_object_mut().into_iter().flat_map(|b| b.values_mut()) {
        if let Some(rgba) = texture["rgba"].as_str() {
            texture["rgba"] = root.join(rgba).to_string_lossy().into_owned().into();
        }
    }
    buttons
}

fn resize_target(
    loader: Option<ResMut<TipLoader>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut images: ResMut<Assets<Image>>,
    mut composites: ResMut<Assets<HudComposite>>,
) {
    if let Some(mut loader) = loader {
        loader.canvas.resize(&window, &mut images, &mut composites);
    }
}

#[allow(clippy::too_many_arguments)]
fn animate(
    mut commands: Commands,
    loader: Option<ResMut<TipLoader>>,
    transition: Res<MapTransition>,
    hold: Res<LoadingHold>,
    (join, start, mut covered): (Res<JoinCover>, Res<MatchCover>, ResMut<Covered>),
    boot: Option<Res<crate::boot::BootActive>>,
    real: Res<Time<Real>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<HudMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut cameras: Query<&mut Camera>,
    mut overlays: Query<(&mut Node, &mut BackgroundColor), With<Overlay>>,
    mut settling: Local<f32>,
) {
    *settling = settle(*settling, join.0, real.delta_secs());
    // G1: the saved map loading behind the boot shows the boot's own loading tag instead (boot_flow Step::Loading).
    let busy = (transition.busy() && !(transition.startup() && boot.is_some_and(|b| b.0))) || hold.0 || join.0 || start.0
        || *settling > 0.;
    // Without the screen nothing covers, so nothing waits for it.
    let Some(mut loader) = loader else {
        covered.set_if_neq(Covered(busy));
        return;
    };
    let loader = &mut *loader;
    let mut failed = false;
    if busy && loader.runtime.is_none() {
        loader.loads += 1;
        let ahead = loader.ready.take().map(Ok).or_else(|| loader.spare.take().and_then(|spare| spare.join().ok()));
        match ahead.unwrap_or_else(|| Runtime::load(&loader.source, loader.wheel.as_deref(), seed(loader.loads))) {
            Ok(runtime) => {
                info!("LOADING_SCREEN_SHOWN");
                loader.runtime = Some(runtime);
                loader.closing = None;
                loader.cover = 1.;
                loader.accumulated = 0.;
                prepare(loader);
            }
            Err(error) => {
                error!("Original loading screen stopped: {error}");
                failed = true;
            }
        }
    }
    if let Some(runtime) = loader.runtime.as_mut() {
        let mut result = Ok(());
        if !busy && loader.closing.is_none() {
            result = runtime.close();
            loader.closing = Some(0.);
        }
        loader.accumulated = (loader.accumulated + real.delta_secs()).min(4. * FRAME);
        while loader.accumulated >= FRAME && result.is_ok() {
            loader.accumulated -= FRAME;
            result = runtime.advance();
            // StateLoading's load done (0x82706850) releases its cover requester without `immediate`.
            if loader.closing.is_some() {
                loader.cover = (loader.cover - COVER_STEP).max(0.);
            }
        }
        if let Some(closing) = loader.closing.as_mut() {
            *closing += real.delta_secs();
            if runtime.bindings.outro_complete || *closing >= CLOSE_TIMEOUT {
                info!("LOADING_SCREEN_HIDDEN");
                loader.runtime = None;
                loader.closing = None;
            }
        } else if let Err(error) = result {
            error!("Original loading screen stopped: {error}");
            loader.runtime = None;
        }
    }
    let active = loader.runtime.is_some();
    // Up at full black from the frame it shows (cover 1) until it closes; one that failed to show covers nothing.
    covered.set_if_neq(Covered(busy && (failed || active && loader.closing.is_none())));
    loader.canvas.set_active(active, &mut cameras);
    for (mut node, mut background) in &mut overlays {
        let display = if active { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
        let colour = Color::BLACK.with_alpha(loader.cover);
        if background.0 != colour {
            background.0 = colour;
        }
    }
    let draws = match loader.runtime.as_ref().map(Runtime::draws) {
        Some(Ok(draws)) => draws,
        Some(Err(error)) => {
            error!("Loading screen geometry: {error}");
            Vec::new()
        }
        None => Vec::new(),
    };
    loader.canvas.draw(&draws, &mut commands, &mut images, &mut meshes, &mut materials);
}

#[cfg(test)]
mod tests {
    /// perf 10-18: the screen booted ahead for load n shuffles as load n would.
    #[test]
    fn each_load_boots_ahead_with_its_own_seed() {
        assert_eq!(super::seed(2), 2u32.wrapping_mul(0x9e37_79b9) ^ std::process::id());
        assert_ne!(super::seed(1), super::seed(2));
    }

    /// Online audit 10-28 (s3re): a join's cover holds 15 frames past the join landing, as retail's settle.
    #[test]
    fn a_joins_cover_holds_fifteen_frames_after_it_lands() {
        use super::{FRAME, SETTLE, settle};
        assert_eq!(settle(0., true, FRAME), SETTLE, "up while joining");
        let mut left = settle(0., true, FRAME);
        for frame in 0..14 {
            left = settle(left, false, FRAME);
            assert!(left > 0., "frame {frame}: still black");
        }
        assert!(settle(left, false, FRAME) < 1e-6, "the 15th frame lifts it");
        assert_eq!(settle(0., false, FRAME), 0., "no join, no hold");
    }
}
