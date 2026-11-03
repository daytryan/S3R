//! Skate 3's power-on sequence in front of the world: boot_flow.rs decides, this plugin shows it. The original
//! screens draw on one canvas over a black backdrop (z 55); the movies are the tutorial ports' full-screen
//! player (z 60) with the skip prompt (screens/main/fmv_popup) over them on the same canvas (z 61). While the
//! boot owns the screen the world is paused and the menus never see its keys. Assets: tools/prepare_boot.py
//! (pipeline stage `boot`) into assets/private/boot. Missing assets, any command-line argument, or
//! SKATE_SKIP_BOOT start straight in the world, as before.
use crate::{
    apt_canvas::{Canvas, CompositeOf, movie_textures},
    apt_scene::{Draw, Vertex},
    boot_flow::{COMMANDS, Command, Command2, Flow, GRID, Key, Movie, Popup, Screen as Slot, Source, TextEntry},
    boot_screens::Screen,
    config::Config,
    core_menu_runtime::fe_sound_id,
    scoring_hud::{HudComposite, HudMaterial},
    skate_audio::FrontEndSoundRequest,
};
use bevy::{
    asset::RenderAssetUsages,
    input::{ButtonState, keyboard::KeyboardInput},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
    window::PrimaryWindow,
};
use std::{
    collections::BTreeMap,
    io::Read,
    path::PathBuf,
    process::{Child, Stdio},
    sync::{
        Mutex,
        mpsc::{Receiver, TryRecvError, sync_channel},
    },
    time::Instant,
};

const LAYER: usize = 50;
/// One FE tick and one authored frame (16 ms frames, stepped at 60 Hz as every screen here).
const FRAME: f32 = 1. / 60.;
/// A screen whose outro never reports OutroComplete is dropped after this.
const CLOSE_TIMEOUT: f32 = 1.0;
/// welcome_native 0x82609F30: the camera previews' MoviePlayer centres (x 504 / 776, y 420), scale 1.0.
const PREVIEWS: [(&str, &str, [f32; 2]); 2] = [("cameraLow", "preview:low", [504., 420.]), ("cameraHigh", "preview:high", [776., 420.])];
const PREVIEW_SIZE: [u32; 2] = [256, 144];
/// A 1x1 white texel the prepared boot ships, for solid fills.
pub(crate) const WHITE: &str = "runtime/white.rgba";
/// The English strings, if a language lacks them: ID_WELCOME_NEW_TEAM_NAME and ALL_VALID_ALPHANUMERIC_CHARACTERS.
const DEFAULT_TEAM: &str = "New Team";
const ALL_VALID: &str = r##"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789',.?!@#$*()"/_ -,.' "##;

/// The boot owns the screen (music.rs holds the music off while it does).
#[derive(Resource, Default)]
pub(crate) struct BootActive(pub bool);

#[derive(Component)]
struct Backdrop;
#[derive(Component)]
struct Overlay;

struct Open {
    slot: Slot,
    screen: Screen,
}

/// A camera preview: its vp6 decoded by ffmpeg into a texture the canvas draws at its centre. It loops as retail's does
/// (0x8263BBD8 sets request +0x44, so the decode thread 0x82A916C8 decodes the same request again). ffmpeg's
/// -stream_loop cannot seek the EA container (it stopped after one pass, on the clip's black last frame), so each pass
/// is a new ffmpeg run.
struct Preview {
    frames: Mutex<Receiver<Vec<u8>>>,
    image: Handle<Image>,
    key: &'static str,
    centre: [f32; 2],
    started: Instant,
    shown: u64,
}
impl Preview {
    fn start(images: &mut Assets<Image>, video: &std::path::Path, key: &'static str, centre: [f32; 2]) -> Result<Self, String> {
        let [w, h] = PREVIEW_SIZE;
        let (path, scale) = (video.to_owned(), format!("scale={w}:{h}"));
        let decode = move || -> std::io::Result<Child> {
            skate_data::audio::ffmpeg::command()
                .args(["-v", "error", "-nostdin", "-i"])
                .arg(&path)
                .args(["-an", "-vf", &scale, "-r", "30000/1001", "-f", "rawvideo", "-pix_fmt", "rgba", "-"])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
        };
        let mut child = decode().map_err(|e| format!("cannot run ffmpeg for {}: {e}", video.display()))?;
        let (send, frames) = sync_channel::<Vec<u8>>(4);
        std::thread::Builder::new()
            .name("boot-preview".into())
            .spawn(move || loop {
                let mut stdout = child.stdout.take();
                let mut decoded = 0;
                loop {
                    let mut frame = vec![0u8; (w * h * 4) as usize];
                    if stdout.as_mut().is_none_or(|s| s.read_exact(&mut frame).is_err()) {
                        break;
                    }
                    decoded += 1;
                    // The preview stopped (receiver dropped): so does this pass's ffmpeg.
                    if send.send(frame).is_err() {
                        let _ = child.kill();
                        return;
                    }
                }
                let _ = child.wait();
                // The clip's end: decode it again. A pass that gave no frame stops instead of respawning forever.
                if decoded == 0 {
                    return;
                }
                let Ok(next) = decode() else { return };
                child = next;
            })
            .map_err(|e| e.to_string())?;
        let image = images.add(Image::new_fill(
            Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            TextureDimension::D2,
            &[0, 0, 0, 255],
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        ));
        Ok(Self { frames: Mutex::new(frames), image, key, centre, started: Instant::now(), shown: 0 })
    }
    /// The newest due frame as a new image (true when one came). The canvas's material keeps the GPU texture it was
    /// built with (Bevy makes a modified Image a new texture; apt_canvas rebinds a draw only on a new handle), so
    /// frames written into one image never showed (first boot 10-29 #2).
    fn advance(&mut self, images: &mut Assets<Image>) -> bool {
        let due = (self.started.elapsed().as_secs_f64() * 30000. / 1001.) as u64;
        let mut latest = None;
        let frames = self.frames.get_mut().unwrap();
        while self.shown <= due {
            match frames.try_recv() {
                Ok(frame) => {
                    latest = Some(frame);
                    self.shown += 1;
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        let Some(frame) = latest else { return false };
        let [width, height] = PREVIEW_SIZE;
        let size = Extent3d { width, height, depth_or_array_layers: 1 };
        self.image =
            images.add(Image::new(size, TextureDimension::D2, frame, TextureFormat::Rgba8UnormSrgb, RenderAssetUsages::RENDER_WORLD));
        true
    }
    fn draw(&self) -> Draw {
        let [w, h] = PREVIEW_SIZE.map(|v| v as f32);
        let [x0, y0] = [self.centre[0] - w / 2., self.centre[1] - h / 2.];
        quad(self.key, PREVIEW_SIZE, [x0, y0, x0 + w, y0 + h], [1.; 4])
    }
    /// Dropping the receiver ends the decode thread, which kills its ffmpeg.
    fn stop(self, images: &mut Assets<Image>) {
        images.remove(&self.image);
    }
}

pub(crate) fn quad(texture: &str, size: [u32; 2], [x0, y0, x1, y1]: [f32; 4], multiply: [f32; 4]) -> Draw {
    let points = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
    let uvs = [[0., 0.], [1., 0.], [1., 1.], [0., 1.]];
    Draw {
        texture: texture.into(),
        size,
        vertices: [0, 1, 2, 0, 2, 3].map(|i| Vertex { position: points[i], uv: uvs[i] }).to_vec(),
        multiply,
        add: [0.; 4],
    }
}

#[derive(Resource)]
struct Boot {
    root: PathBuf,
    movies: serde_json::Value,
    manifests: BTreeMap<&'static str, serde_json::Value>,
    flow: Flow,
    canvas: Canvas,
    open: Option<Open>,
    closing: Vec<(Screen, f32)>,
    popup: Option<crate::popup_runtime::Runtime>,
    skip: Option<Screen>,
    tag: Option<Screen>,
    tag_on: bool,
    movie: Option<crate::challenge::movie::Movie>,
    /// The movies' soundtracks, read off the main thread (the intro's is 74 MB) and handed over as they finish.
    soundtracks: Mutex<Receiver<(&'static str, crate::skate_audio::StreamPcm)>>,
    loaded: BTreeMap<&'static str, Handle<crate::skate_audio::StreamPcm>>,
    /// The playing movie's soundtrack when it came from `loaded`.
    movie_audio: Option<Entity>,
    previews: Vec<Preview>,
    accumulated: f32,
    /// Keys since the last update, in order.
    pending: Vec<(Source, Key)>,
    typed: String,
    pads: [u16; 4],
    /// The boot is over and holds the loading screen (`tip_loader::LoadingHold`) until the world is up.
    loading: bool,
    /// The boot raised the shared `LoadingHold` itself (it was down), so only then does its release lower it.
    raised_hold: bool,
    profile: String,
    font: Option<crate::apt_text::Font>,
    career: PathBuf,
    settings: PathBuf,
    /// The new career's slot 63 logo picker and slot 68 Preset Skaters (team_select.rs), made on first use.
    team: Option<crate::team_select::Manager>,
}

/// What the boot remembers between runs (the career save's new-career fields).
#[derive(serde::Serialize, serde::Deserialize, Default)]
pub(crate) struct Career {
    pub(crate) intro_done: bool,
    pub(crate) team_name: Option<String>,
    pub(crate) tutorial: Option<bool>,
}

/// The boot's `career.json` in `settings`, or the default when it is missing or damaged past its backup.
pub(crate) fn load_career(settings: &std::path::Path) -> Career {
    crate::saves::load_json::<Career>(&settings.join("career.json")).ok().unwrap_or_default()
}

pub(crate) struct BootPlugin;
impl Plugin for BootPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BootActive>()
            .add_systems(PostStartup, setup.after(crate::graphics_menu::PresentationSetup))
            .add_systems(
                PreUpdate,
                (
                    // Ahead of every menu that opens on a key (the Challenge Map first, then the crossbar and the panel).
                    input
                        .after(bevy::input::InputSystems)
                        .after(crate::customiser::navigation)
                        .before(crate::challenge_map::interact)
                        .before(crate::core_menu::interact),
                    // graphics_menu::interact unpauses a closed menu's world; the boot keeps it paused.
                    // After a load's publish unpauses the world (map_transition::poll), so the boot's pause wins.
                    hold.after(crate::graphics_menu::MenuInput).after(crate::map_transition::MapTransitionSet),
                ),
            )
            .add_systems(Update, (resize_target, update).chain());
    }
}

/// Runs the boot unless the game was started for something else (a test map, a teleport, a session, a check);
/// `--assets` (a preview build) still shows it.
pub(crate) fn wanted(args: impl IntoIterator<Item = String>) -> bool {
    const SKIP: [&str; 7] = ["--map", "--test-world", "--teleport", "--verify", "--check-assets", "--net-", "--hero-clip"];
    std::env::var_os("SKATE_SKIP_BOOT").is_none() && !args.into_iter().any(|a| SKIP.iter().any(|s| a.starts_with(s)))
}

/// First launch only on a fresh install: no career.json and none of the settings an earlier run wrote. An install
/// that already has settings is a returning boot: it gets a retail-default career (so it stays returning) and
/// nothing else is touched. A damaged career.json comes back from its backup (saves::load); with none, it is kept as
/// .bad and the boot is returning. One that can't be read is a returning boot and is not saved over.
fn first_boot(settings: &std::path::Path, default_team: &str) -> bool {
    let path = settings.join("career.json");
    match crate::saves::load_json::<Career>(&path) {
        crate::saves::Loaded::Loaded(c) | crate::saves::Loaded::Restored(c, _) => return !c.intro_done,
        crate::saves::Loaded::Unreadable => return false,
        crate::saves::Loaded::Damaged(_) => {}
        crate::saves::Loaded::Missing => if !["graphics.json", "gameplay.json", "character.json"].iter().any(|f| settings.join(f).exists()) {
            return true;
        },
    }
    let career = Career { intro_done: true, team_name: Some(default_team.to_owned()), tutorial: None };
    if let Err(error) = save_career(&path, &career) {
        warn!("Career save {}: {error}", path.display());
    }
    false
}

fn save_career(path: &std::path::Path, career: &Career) -> std::io::Result<()> {
    crate::saves::write_atomic(path, &serde_json::to_vec_pretty(career).unwrap_or_default())?;
    crate::autosave_tag::saving();
    Ok(())
}

#[allow(clippy::too_many_arguments)]
/// The boot screens' manifests, `runtime/<name>.json` under [`root`].
const SCREENS: [&str; 9] = ["esrb", "pressstart", "welcomescreen", "coachfrank", "team_management_boot", "fmv_popup", "autosave", "save_warning", "sk8popup"];
fn root(asset_root: &std::path::Path) -> PathBuf {
    std::env::var_os("SKATE_BOOT_ROOT").map(PathBuf::from).unwrap_or_else(|| asset_root.join("private/boot"))
}
/// What setup reads, for the startup prefetch (apt_canvas::prefetch); nothing when the sequence won't run.
pub(crate) fn manifests(asset_root: &std::path::Path) -> Vec<PathBuf> {
    if !wanted(std::env::args().skip(1)) {
        return Vec::new();
    }
    let root = root(asset_root);
    std::iter::once("boot").chain(SCREENS).map(|name| root.join(format!("runtime/{name}.json"))).collect()
}

fn setup(
    mut commands: Commands,
    config: Res<Config>,
    cameras: Query<Entity, With<IsDefaultUiCamera>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut images: ResMut<Assets<Image>>,
    mut composites: ResMut<Assets<HudComposite>>,
    mut active: ResMut<BootActive>,
    career: Option<Res<crate::skate3_career::glue::Career>>,
) {
    if !wanted(std::env::args().skip(1)) {
        return;
    }
    let Ok(output) = cameras.single() else { return };
    let root = root(&config.asset_root);
    let mut canvas = Canvas::new(&mut commands, &mut images, &mut composites, &window, root.clone(), LAYER, -5);
    let loaded = (|| -> Result<(serde_json::Value, BTreeMap<&'static str, serde_json::Value>), String> {
        let read = |name: &str| -> Result<serde_json::Value, String> {
            let path = root.join(format!("runtime/{name}.json"));
            if let Some(json) = crate::apt_canvas::prefetched_json(&path) {
                return Ok(json);
            }
            serde_json::from_slice(&std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?).map_err(|e| e.to_string())
        };
        let index = read("boot")?;
        if index["format"] != "skate3-boot" || index["version"] != 1 {
            return Err("Unsupported boot manifest".into());
        }
        let mut manifests = BTreeMap::new();
        for name in SCREENS {
            let json = read(name)?;
            if json["version"] != 1 || !matches!(json["format"].as_str(), Some("skate3-boot" | "skate3-popup")) {
                return Err(format!("Unsupported boot screen manifest {name}"));
            }
            for (path, size) in movie_textures(&json)? {
                canvas.preload(&mut images, &path, size)?;
            }
            manifests.insert(name, json);
        }
        Ok((index["movies"].clone(), manifests))
    })();
    let (movies, manifests) = match loaded {
        Ok(value) => value,
        Err(error) => {
            warn!("Startup sequence unavailable ({error}); starting in the world. Prepare it with tools/prepare_boot.py");
            return;
        }
    };
    let language = &manifests["welcomescreen"]["language"];
    let text = |id: &str| language[id].as_str().unwrap_or_default().to_owned();
    let settings = crate::saves::settings_dir(&config.asset_root);  // Not retail (user 10-13)
    let default_team = Some(text("ID_WELCOME_NEW_TEAM_NAME")).filter(|t| !t.is_empty()).unwrap_or_else(|| DEFAULT_TEAM.into());
    let first = first_boot(&settings, &default_team);
    let valid = Some(text("ALL_VALID_ALPHANUMERIC_CHARACTERS")).filter(|t| !t.is_empty()).unwrap_or_else(|| ALL_VALID.into());
    let mut flow = Flow::new(first, &default_team, &valid);
    let skip_intro = crate::graphics_menu::skip_intro(&config);
    if skip_intro {
        flow.skip_intro();
    }
    // Spec 11.1: with the Skate 3 career on, a new career asks Unlock Everything?, on unlocks.json's answer.
    if let Some(career) = career.as_ref().filter(|c| c.state.is_some()) {
        flow.ask_unlock_everything(career.store.everything);
    }
    flow.set_default_boss(&Some(text("ID_DEFAULT_PLAYER_NAME")).filter(|t| !t.is_empty()).unwrap_or_else(|| "The Legend".into()));
    // A returning boot plays only the EA logo and (when idle) the attract video.
    let stems: &[Movie] = if first { &[Movie::EaIntro, Movie::Attract, Movie::Intro, Movie::CoachFrankIntro] } else { &[Movie::EaIntro, Movie::Attract] };
    // Skip Intro plays neither the EA logo nor the title's attract video.
    let stems = stems.iter().filter(|m| !(skip_intro && matches!(m, Movie::EaIntro | Movie::Attract)));
    let wavs: Vec<_> = stems.filter_map(|m| Some((m.stem(), root.join(movies[m.stem()]["audio"].as_str()?)))).collect();
    let (send, soundtracks) = sync_channel(1);
    let reader = std::thread::Builder::new().name("boot-soundtracks".into()).spawn(move || {
        for (stem, wav) in wavs {
            match crate::challenge::movie::read_wav(&wav) {
                Ok(pcm) => {
                    if send.send((stem, pcm)).is_err() {
                        return;
                    }
                }
                Err(error) => warn!("Boot movie {stem} audio unavailable: {error}"),
            }
        }
    });
    if let Err(error) = reader {
        warn!("Boot soundtracks load on first play instead: {error}");
    }
    let skip = Screen::level(&manifests["fmv_popup"]).inspect_err(|e| warn!("Movie skip prompt unavailable: {e}")).ok();
    let tag = Screen::level(&manifests["autosave"]).inspect_err(|e| warn!("Loading tag unavailable: {e}")).ok();
    let font = crate::apt_text::TextAssets::load(&manifests["welcomescreen"])
        .ok()
        .and_then(|t| t.font_named("Futura Std Medium").and_then(|id| t.fonts.get(&id).cloned()));
    let full = || Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() };
    commands.spawn((Backdrop, UiTargetCamera(output), GlobalZIndex(55), Pickable::IGNORE, BackgroundColor(Color::BLACK), full()));
    commands.spawn((
        Overlay,
        MaterialNode(canvas.composite.clone()),
        CompositeOf(canvas.camera()),
        UiTargetCamera(output),
        GlobalZIndex(61),
        Pickable::IGNORE,
        full(),
    ));
    let profile = std::env::var("USERNAME").ok().filter(|n| !n.is_empty()).unwrap_or_else(|| "Player".into());
    commands.insert_resource(Boot {
        root,
        movies,
        manifests,
        flow,
        canvas,
        open: None,
        closing: Vec::new(),
        popup: None,
        skip,
        tag,
        tag_on: false,
        movie: None,
        soundtracks: Mutex::new(soundtracks),
        loaded: BTreeMap::new(),
        movie_audio: None,
        previews: Vec::new(),
        accumulated: 0.,
        pending: Vec::new(),
        typed: String::new(),
        pads: [0; 4],
        loading: false,
        raised_hold: false,
        profile,
        font,
        career: settings.join("career.json"),
        settings,
        team: None,
    });
    active.0 = true;
    info!("BOOT_STARTED first_boot={first}");
}

/// Edge-detected keys from every pad and the keyboard; the boot's own and nobody else's while it owns the screen.
#[allow(clippy::too_many_arguments)]
fn input(
    boot: Option<ResMut<Boot>>,
    active: Res<BootActive>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut nav: ResMut<crate::customiser::Navigation>,
    mut typing: MessageReader<KeyboardInput>,
    mut pads: Local<[crate::input::platform::CapabilityCache; 4]>,
) {
    let (Some(mut boot), true) = (boot, active.0) else { return };
    let boot = &mut *boot;
    if boot.flow.in_create_skater() {
        boot.pads = [u16::MAX; 4];
        typing.clear();
        return;
    }
    const BUTTONS: [(u16, Key); 9] = [
        (0x1, Key::Up),
        (0x2, Key::Down),
        (0x4, Key::Left),
        (0x8, Key::Right),
        (0x10, Key::Start),
        (0x1000, Key::A),
        (0x2000, Key::B),
        (0x4000, Key::X),
        (0x8000, Key::Y),
    ];
    for slot in 0..4 {
        let current = crate::input::platform::poll_cached(slot, &mut pads[slot]).map_or(0, |p| {
            let stick = |v: i16, high: u16, low: u16| if v > 16000 { high } else if v < -16000 { low } else { 0 };
            p.state.buttons | stick(p.state.left[1], 1, 2) | stick(p.state.left[0], 8, 4)
        });
        // A button held from before (or through the CAS) counts once released.
        let previous = boot.pads[slot];
        let pressed = current & !previous;
        let source = Source::Pad(slot as u8);
        for (bit, key) in BUTTONS {
            if pressed & bit != 0 {
                boot.pending.push((source, key));
            }
        }
        if previous & 0x10 != 0 && current & 0x10 == 0 && previous != u16::MAX {
            boot.pending.push((source, Key::StartReleased));
        }
        boot.pads[slot] = if previous == u16::MAX && current != 0 { u16::MAX } else { current };
    }
    let text_entry = boot.flow.keyboard_open();
    keyboard_keys(typing.read(), text_entry, &mut boot.pending, &mut boot.typed);
    // Nobody else sees these keys (the key events stay readable, the menus read ButtonInput).
    keys.reset_all();
    nav.pressed = 0;
}

/// The FE accumulator after a frame of `dt` seconds, at most four 60 Hz ticks' worth (no catch-up burst after a
/// hitch), and the seconds the cap dropped.
fn fe_clock(accumulated: f32, dt: f32) -> (f32, f32) {
    let capped = (accumulated + dt).min(4. * FRAME);
    (capped, accumulated + dt - capped)
}

/// The engine's keys from this frame's key events: Escape = Start (its release accepts the title), Enter = A,
/// Backspace = B, arrows = D-pad. While the team name is typed, letters type, Enter is Done, Escape cancels and
/// Backspace deletes. Events, not ButtonInput: `input` resets ButtonInput every frame, which loses releases.
fn keyboard_keys<'a>(
    events: impl IntoIterator<Item = &'a KeyboardInput>,
    text_entry: bool,
    pending: &mut Vec<(Source, Key)>,
    typed: &mut String,
) {
    for event in events {
        let key = match (event.state, event.key_code) {
            (ButtonState::Released, KeyCode::Escape) if !text_entry => Some(Key::StartReleased),
            (ButtonState::Released, _) => None,
            // A held Enter/Escape repeats; one press is one choice.
            (_, KeyCode::Enter | KeyCode::NumpadEnter | KeyCode::Escape) if event.repeat => None,
            (_, KeyCode::Enter | KeyCode::NumpadEnter) => Some(if text_entry { Key::Start } else { Key::A }),
            (_, KeyCode::Escape) => Some(if text_entry { Key::B } else { Key::Start }),
            (_, KeyCode::Backspace) => Some(if text_entry { Key::X } else { Key::B }),
            (_, KeyCode::ArrowUp) => Some(Key::Up),
            (_, KeyCode::ArrowDown) => Some(Key::Down),
            (_, KeyCode::ArrowLeft) => Some(Key::Left),
            (_, KeyCode::ArrowRight) => Some(Key::Right),
            _ => {
                if let (true, Some(text)) = (text_entry, &event.text) {
                    typed.push_str(text);
                }
                None
            }
        };
        if let Some(key) = key {
            if text_entry || !event.repeat {
                pending.push((Source::Keyboard, key));
            }
        }
    }
}

/// The world is held from the boot's first frame until the loading screen lets go: retail has no skater in the
/// world before the career's load, so nothing of it (a spawn landing, its rolling) may sound or move behind the
/// boot. The player audio meanwhile runs the front end's frames with the top FE slot's audio state
/// (`skate_audio::set_fe_audio_state`).
fn hold(boot: Option<Res<Boot>>, active: Res<BootActive>, mut time: ResMut<Time<Virtual>>) {
    let state = match boot.as_deref() {
        Some(b) if active.0 => Some(b.flow.audio_state()),
        // TipsLoader (slot 52): "tiploader".
        Some(b) if b.loading => Some(crate::boot_flow::AUDIO_TIPS_LOADER),
        _ => None,
    };
    crate::skate_audio::set_fe_audio_state(state);
    match world_pause(state, boot.is_some_and(|b| b.flow.in_create_skater())) {
        Some(true) => time.pause(),
        Some(false) => time.unpause(),
        None => {}
    }
}

/// Whether the boot pauses (`Some(true)`) or runs (`Some(false)`) the world this frame; `None` leaves it to the
/// others. The CAS runs its own preview world: the camera and the room are placed from its presentation frames, and
/// nothing else unpauses the world under an open customiser (graphics_menu::interact returns early).
fn world_pause(fe_state: Option<u32>, create_skater: bool) -> Option<bool> {
    fe_state.map(|_| !create_skater)
}

fn resize_target(
    boot: Option<ResMut<Boot>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut images: ResMut<Assets<Image>>,
    mut composites: ResMut<Assets<HudComposite>>,
) {
    if let Some(mut boot) = boot {
        boot.canvas.resize(&window, &mut images, &mut composites);
    }
}

fn screen_name(slot: Slot) -> (&'static str, f64) {
    match slot {
        Slot::Esrb => ("esrb", 0.),
        Slot::PressStart => ("pressstart", 0.),
        Slot::Welcome => ("welcomescreen", 0.),
        Slot::TeamPrompt => ("welcomescreen", 10.),
        Slot::TeamManagement => ("team_management_boot", 0.),
        Slot::CoachFrank => ("coachfrank", 0.),
    }
}

/// Everything update needs from the rest of the game.
#[derive(bevy::ecs::system::SystemParam)]
struct Game<'w> {
    config: ResMut<'w, Config>,
    menu: ResMut<'w, crate::graphics_menu::Menu>,
    physics: ResMut<'w, crate::physics::GamePhysics>,
    customiser: ResMut<'w, crate::customiser::Customiser>,
    custom_models: ResMut<'w, crate::custom_models::CustomModels>,
    transition: Res<'w, crate::map_transition::MapTransition>,
    parts: Res<'w, crate::customiser_parts::Parts>,
    photos: ResMut<'w, crate::team_photo::Photos>,
    friends: Res<'w, crate::team_select::FriendSource>,
    presence: Option<ResMut<'w, crate::presence::Presence>>,
    loading: Option<ResMut<'w, crate::tip_loader::LoadingHold>>,
    /// Retail Skate 3's career (absent in the boot's own test apps).
    career: Option<ResMut<'w, crate::skate3_career::glue::Career>>,
}

#[allow(clippy::too_many_arguments)]
fn update(
    mut commands: Commands,
    boot: Option<ResMut<Boot>>,
    mut active: ResMut<BootActive>,
    mut pad: ResMut<crate::input::ActivePad>,
    mut world: Game,
    real: Res<Time<Real>>,
    mut sounds: MessageWriter<FrontEndSoundRequest>,
    mut images: ResMut<Assets<Image>>,
    mut audio: ResMut<Assets<crate::skate_audio::StreamPcm>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<HudMaterial>>,
    mut cameras: Query<&mut Camera>,
    ui_cameras: Query<Entity, With<IsDefaultUiCamera>>,
    mut backdrops: Query<&mut Node, (With<Backdrop>, Without<Overlay>)>,
    mut overlays: Query<&mut Node, (With<Overlay>, Without<Backdrop>)>,
) {
    let Some(mut boot) = boot else { return };
    let b = &mut *boot;
    if !active.0 {
        // StateLoading's load done (0x82706850: tag off, cover requester 7 released): our world is already
        // loaded, so the loading screen closes as soon as no map load is running.
        if b.loading && !world.transition.busy() {
            b.loading = false;
            if let Some(hold) = world.loading.as_mut().filter(|_| std::mem::take(&mut b.raised_hold)) {
                hold.0 = false;
            }
            info!("BOOT_LOADING_DONE");
        }
        // Finished: the canvas camera renders its last, empty frame, then stays off.
        b.canvas.set_active(false, &mut cameras);
        return;
    }
    let mut play = |ids: &mut Vec<u64>| {
        for id in ids.drain(..) {
            sounds.write(FrontEndSoundRequest(id));
        }
    };
    let mut fe = Vec::new();
    while let Ok((stem, pcm)) = b.soundtracks.get_mut().unwrap().try_recv() {
        b.loaded.insert(stem, audio.add(pcm));
    }
    // Keys first, then the frame's ticks; each step's commands are carried out before the next.
    let typed = std::mem::take(&mut b.typed);
    b.flow.type_text(&typed);
    // The shared retail active pad (input+28): the FE filter reads it and Press Start writes it.
    b.flow.set_active_pad(pad.0);
    for (source, key) in std::mem::take(&mut b.pending) {
        b.flow.input(source, key);
        run(b, &mut commands, &mut images, &mut audio, &ui_cameras, &mut world, &mut fe);
    }
    if pad.0 != b.flow.active_pad() {
        pad.0 = b.flow.active_pad();
    }
    if b.flow.in_create_skater() && !world.customiser.open {
        b.flow.create_skater_closed();
        run(b, &mut commands, &mut images, &mut audio, &ui_cameras, &mut world, &mut fe);
    }
    let lost;
    (b.accumulated, lost) = fe_clock(b.accumulated, real.delta_secs());
    b.flow.movie_clock((lost * 1000.) as u32);
    while b.accumulated >= FRAME && !b.flow.finished() {
        b.accumulated -= FRAME;
        b.flow.tick(!world.transition.busy());
        run(b, &mut commands, &mut images, &mut audio, &ui_cameras, &mut world, &mut fe);
        advance(b, &mut fe);
    }
    if let Some(movie) = b.movie.as_mut() {
        if movie.advance(&mut images) {
            stop_movie(b, &mut commands, &mut images);
            b.flow.movie_finished();
            run(b, &mut commands, &mut images, &mut audio, &ui_cameras, &mut world, &mut fe);
        }
    }
    let fresh: Vec<_> =
        b.previews.iter_mut().filter_map(|p| p.advance(&mut images).then(|| (p.key, p.image.clone()))).collect();
    for (key, image) in fresh {
        b.canvas.insert_texture(key, image);
    }
    play(&mut fe);
    // The cover is black: TipsLoader (slot 52) opens under the boot this frame and the boot leaves on the next,
    // so no frame of the world shows between them.
    if b.flow.finished() && !b.loading {
        b.loading = true;
        if let Some(hold) = world.loading.as_mut() {
            b.raised_hold = !hold.0;
            hold.0 = true;
        }
    } else if b.flow.finished() {
        finish(b, &mut commands, &mut images);
        active.0 = false;
        b.canvas.set_active(false, &mut cameras);
        b.canvas.draw(&[], &mut commands, &mut images, &mut meshes, &mut materials);
        for mut node in backdrops.iter_mut().chain(overlays.iter_mut()) {
            node.display = Display::None;
        }
        info!("BOOT_FINISHED");
        return;
    }
    // The CAS shows itself; the boot waits behind it.
    let shown = !b.flow.in_create_skater();
    let display = if shown { Display::Flex } else { Display::None };
    for mut node in backdrops.iter_mut().chain(overlays.iter_mut()) {
        if node.display != display {
            node.display = display;
        }
    }
    b.canvas.set_active(shown, &mut cameras);
    let draws = if shown { draws(b) } else { Vec::new() };
    b.canvas.draw(&draws, &mut commands, &mut images, &mut meshes, &mut materials);
}

/// One authored frame of every live movie, and the sounds they played.
fn advance(b: &mut Boot, fe: &mut Vec<u64>) {
    let mut failed = false;
    if let Some(open) = b.open.as_mut() {
        if let Err(error) = open.screen.advance() {
            error!("Boot screen {:?} stopped: {error}", open.slot);
            failed = true;
        } else {
            fe.extend(open.screen.take_sounds());
        }
    }
    if failed {
        b.open = None;
        b.flow.screen_failed();
    }
    b.closing.retain_mut(|(screen, seconds)| {
        *seconds += FRAME;
        screen.advance().is_ok() && !screen.bindings.outro_complete && *seconds < CLOSE_TIMEOUT
    });
    if let Some(popup) = b.popup.as_mut() {
        if let Err(error) = popup.advance() {
            error!("Boot popup stopped: {error}");
            b.popup = None;
            b.flow.popup_failed();
        }
    }
    for level in [b.skip.as_mut(), b.tag.as_mut()].into_iter().flatten() {
        let _ = level.advance();
    }
    if let Some(team) = b.team.as_mut().filter(|t| t.is_open()) {
        if let Err(error) = team.advance() {
            error!("Boot team screen stopped: {error}");
            b.team = None;
            b.flow.team_closed();
        }
    }
}

/// The team manager, made on first use from the prepared team data (None, and the step skipped, without it).
fn team_manager<'a>(b: &'a mut Boot, world: &Game<'_>) -> Option<&'a mut crate::team_select::Manager> {
    if b.team.is_none() {
        match crate::team_select::Data::load(&b.root) {
            Ok(data) => {
                let mut team = crate::team_select::load_team(&data, &b.settings);
                if let Some(name) = b.flow.outcome().team_name.clone() {
                    team.name = name;
                }
                b.team = Some(crate::team_select::Manager::new(data, team, b.settings.clone(), world.friends.0.clone()));
            }
            Err(error) => warn!("Team screens unavailable ({error}); skipped. Prepare them with tools/prepare_boot.py"),
        }
    }
    b.team.as_mut()
}

/// The team manager's requests: photos, imports, sounds; its close moves the flow on.
fn serve_team(b: &mut Boot, world: &mut Game<'_>, fe: &mut Vec<u64>) {
    let Boot { team, flow, root, open, .. } = b;
    let Some(manager) = team.as_mut() else { return };
    crate::team_select::collect_photos(manager, &mut world.photos);
    let mut services = crate::team_select::Services {
        customiser: &mut world.customiser,
        custom_models: &mut world.custom_models,
        parts: &world.parts,
        photos: &mut world.photos,
        root,
        presence: world.presence.as_deref_mut(),
    };
    let mut swapped = None;
    for out in manager.take() {
        match crate::team_select::serve(manager, out, &mut services, &mut swapped) {
            crate::team_select::Served::Sound(id) => fe.push(id),
            crate::team_select::Served::Closed => flow.team_closed(),
            crate::team_select::Served::Done => {}
        }
    }
    // The boot opens no teammate's CAS (mode 2 and the first member's import only): nothing to swap back.
    debug_assert!(swapped.is_none(), "the boot cannot put a swapped profile back");
    // Slot 67's card shows the first member's photo once it is drawn.
    if let Some(open) = open.as_mut().filter(|o| o.slot == Slot::TeamManagement) {
        let name = crate::team_select::photo_name(0);
        if let Some(photo) = manager.image(&name).filter(|p| open.screen.bindings.team.images.get(&name).is_none_or(|t| t.rgba != p.rgba)) {
            let photo = photo.clone();
            let movie = &mut open.screen.bindings.movie;
            open.screen.bindings.team.set_image(movie, &name, photo);
        }
    }
}

/// Carries out the flow's commands (and whatever they lead to) until it has none left. The team manager is served
/// first on every call (each tick at least), not only after a command: a photo that finishes between key presses,
/// and what the manager asks for from its own update (a redraw, its close), are taken up at once.
fn run(
    b: &mut Boot,
    commands: &mut Commands,
    images: &mut Assets<Image>,
    audio: &mut Assets<crate::skate_audio::StreamPcm>,
    ui_cameras: &Query<Entity, With<IsDefaultUiCamera>>,
    world: &mut Game<'_>,
    fe: &mut Vec<u64>,
) {
    serve_team(b, world, fe);
    for _ in 0..64 {
        let batch = b.flow.take();
        if batch.is_empty() {
            return;
        }
        for command in batch {
            carry_out(b, command, commands, images, audio, ui_cameras, world, fe);
        }
        serve_team(b, world, fe);
    }
}

#[allow(clippy::too_many_arguments)]
fn carry_out(
    b: &mut Boot,
    command: Command,
    commands: &mut Commands,
    images: &mut Assets<Image>,
    audio: &mut Assets<crate::skate_audio::StreamPcm>,
    ui_cameras: &Query<Entity, With<IsDefaultUiCamera>>,
    world: &mut Game<'_>,
    fe: &mut Vec<u64>,
) {
    match command {
        Command::Open(slot) => {
            let (name, state) = screen_name(slot);
            let natives = (slot == Slot::TeamManagement).then(|| team_manager(b, world)).flatten().map(|m| {
                m.request_photo(0);
                (m.natives(), m.team.boss.clone())
            });
            // Slot 67's nameText is Game.GetProfileName(): the boss's name ("Enter your name:" is the profile name).
            let boss = natives.as_ref().map(|(_, boss)| b.flow.outcome().boss_name.clone().unwrap_or_else(|| boss.clone()));
            let natives = natives.map(|(n, _)| n);
            match Screen::load(&b.manifests[name], state, boss.as_deref().unwrap_or(&b.profile), Vec::new()) {
                Ok(mut screen) => {
                    if let Some(natives) = natives {
                        screen.bindings.team = natives;
                    }
                    // The native first-team-member state shows the panel (its constructor hides it).
                    if slot == Slot::TeamManagement {
                        let _ = screen.set_state(0.).inspect_err(|e| error!("team_management_boot: {e}"));
                    }
                    fe.extend(screen.take_sounds());
                    b.open = Some(Open { slot, screen });
                }
                Err(error) => {
                    error!("Boot screen {name} failed to load: {error}");
                    b.flow.screen_failed();
                }
            }
        }
        Command::Close(slot) => {
            if let Some(mut open) = b.open.take_if(|o| o.slot == slot) {
                if open.screen.close().is_ok() {
                    b.closing.push((open.screen, 0.));
                }
            }
        }
        Command::SetState(slot, state) => {
            if let Some(open) = b.open.as_mut().filter(|o| o.slot == slot) {
                if let Err(error) = open.screen.set_state(state) {
                    error!("Boot screen {slot:?} SetState({state}): {error}");
                }
                fe.extend(open.screen.take_sounds());
            }
        }
        Command::ForwardScreen(key) => {
            let Some(apt) = key.apt() else { return };
            let picked = b.open.as_mut().map(|open| {
                let picked = open.screen.input(apt);
                fe.extend(open.screen.take_sounds());
                picked
            });
            match picked {
                Some(Ok(Some(index))) => b.flow.menu_select(index),
                Some(Err(error)) => {
                    error!("Boot screen input: {error}");
                    b.flow.screen_failed();
                }
                _ => {}
            }
        }
        Command::ForwardPopup(key) => {
            let Some(apt) = key.apt() else { return };
            let picked = b.popup.as_mut().map(|popup| {
                let picked = popup.input(apt);
                fe.append(&mut popup.bindings.sounds);
                picked
            });
            match picked {
                Some(Ok(Some(index))) => b.flow.popup_chose(index),
                Some(Err(error)) => {
                    error!("Boot popup input: {error}");
                    b.flow.popup_failed();
                }
                _ => {}
            }
        }
        Command::OpenPopup(popup) => {
            let json = &b.manifests[if popup.save_warning() { "save_warning" } else { "sk8popup" }];
            let loaded = crate::popup_runtime::Runtime::load(json, popup.description(), popup.options(), popup.cancelable()).and_then(|mut runtime| {
                // Unlock Everything? opens on Earn It unless that was the saved answer.
                if popup == Popup::UnlockEverything && !b.flow.unlock_cursor_on_yes() {
                    runtime.input(crate::core_menu_runtime::Key::Down)?;
                    runtime.bindings.sounds.clear();
                }
                Ok(runtime)
            });
            match loaded {
                Ok(runtime) => b.popup = Some(runtime),
                Err(error) => {
                    error!("Boot popup {popup:?} failed to load: {error}");
                    b.flow.popup_failed();
                }
            }
        }
        Command::ClosePopup => b.popup = None,
        Command::UnlockEverything(on) => {
            if let Some(career) = world.career.as_mut() {
                career.store.set_everything(on);
                if let Err(error) = career.store.write(&career.settings) {
                    warn!("Skate 3 career unlocks.json: {error}");
                }
            }
        }
        Command::PlayMovie(movie) => {
            let entry = &b.movies[movie.stem()];
            let video = b.root.join(entry["video"].as_str().unwrap_or_default());
            // Not read yet: the player reads it itself (a hitch, as before).
            let loaded = b.loaded.get(movie.stem()).cloned();
            let wav = entry["audio"].as_str().map(|a| b.root.join(a)).filter(|_| loaded.is_none());
            let started = ui_cameras
                .single()
                .map_err(|e| e.to_string())
                .and_then(|camera| crate::challenge::movie::Movie::start(commands, images, audio, camera, &video, wav.as_deref()));
            match started {
                Ok(player) => {
                    b.movie = Some(player);
                    *crate::retail_ui::MOVIE.lock().unwrap_or_else(|e| e.into_inner()) = Some((movie.stem().into(), Instant::now()));
                    b.movie_audio = loaded.map(|pcm| commands.spawn((AudioPlayer(pcm), PlaybackSettings::DESPAWN)).id());
                }
                Err(error) => {
                    warn!("Boot movie {} unavailable ({error}); skipped", movie.stem());
                    b.flow.movie_finished();
                }
            }
        }
        Command::StopMovie => {
            stop_movie(b, commands, images);
        }
        Command::MovieSkip(show) => {
            if let Some(skip) = b.skip.as_mut() {
                let shown = skip.call_global(if show { "ShowMovieSkip" } else { "HideMovieSkip" });
                // HideMovieSkip plays on from "end" to the frame that hides the prompt, but stop() there re-runs
                // in this runtime (and mid-intro it leaves the prompt up): hide it outright, as that frame does.
                if let Err(error) = shown.and_then(|()| if show { Ok(()) } else { skip.hide() }) {
                    warn!("Movie skip prompt: {error}");
                }
            }
        }
        Command::LoadingTag(on) => {
            b.tag_on = on;
            if let Some(tag) = b.tag.as_mut() {
                let _ = tag.call_global(if on { "Load_EnableTag" } else { "Load_HideTag" });
            }
        }
        Command::Sound(key, _flag) => fe.push(fe_sound_id(key)),
        Command::CameraPreviews(on) => {
            for preview in b.previews.drain(..) {
                preview.stop(images);
            }
            if on {
                for (stem, key, centre) in PREVIEWS {
                    let video = b.root.join(b.movies[stem]["video"].as_str().unwrap_or_default());
                    match Preview::start(images, &video, key, centre) {
                        Ok(preview) => {
                            b.canvas.insert_texture(key, preview.image.clone());
                            b.previews.push(preview);
                        }
                        Err(error) => warn!("Camera preview {stem} unavailable: {error}"),
                    }
                }
            }
        }
        Command::Difficulty(index) => {
            let difficulty = crate::difficulty::Difficulty::ALL[index.min(2)];
            world.menu.set_difficulty(difficulty, &mut world.physics, &mut world.config);
        }
        // Skate 3's Low/High only: a Skate 2 or Skate 1 camera style the player chose stays.
        Command::CameraHigh(high) if world.menu.camera_style() == 0 => {
            world.menu.choose(crate::graphics_menu::Setting::CameraAngle, usize::from(high))
        }
        Command::CameraHigh(_) => {}
        Command::OpenKeyboard | Command::CloseKeyboard => {}
        Command::OpenTeamLogos | Command::OpenImport => {
            let logos = command == Command::OpenTeamLogos;
            let opened = team_manager(b, world).map(|m| if logos { m.open_new_career_logos() } else { m.open_import(0, 2) });
            match opened {
                Some(Ok(())) => {}
                Some(Err(error)) => {
                    error!("Boot team screen: {error}");
                    b.flow.team_closed();
                }
                None => b.flow.team_closed(),
            }
        }
        Command::ForwardTeam(key) => {
            if let Some(Err(error)) = b.team.as_mut().map(|m| m.input(key)) {
                error!("Boot team screen input: {error}");
                b.team = None;
                b.flow.team_closed();
            }
        }
        Command::OpenCreateSkater => {
            // A new career's boss is the one made here: a team.json kept from an earlier career may still skate as
            // a teammate (Select Skater, not retail), so it goes back to the boss and the customiser to character.json.
            let boss = crate::team_data::look_path(&b.settings, 0);
            if world.customiser.profile_path() != boss.as_path() {
                if let Some(manager) = b.team.as_mut() {
                    manager.team.active = 0;
                }
                if let Err(error) = crate::team_data::reset_active(&b.settings) {
                    warn!("Team save: {error}");
                }
                let look = crate::team_select::member_profile(&b.settings, 0, &world.parts);
                world.customiser.swap_profile(boss, look);
            }
            world.custom_models.request_stock();
            world.customiser.begin();
        }
        Command::Finish(outcome) => {
            // The new team: its name and the boss's (slot 67); the logo was saved when it was picked.
            if let Some(manager) = b.team.as_mut().filter(|_| outcome.first_boot) {
                if let Some(name) = &outcome.team_name {
                    manager.team.name = name.clone();
                }
                if let Some(boss) = &outcome.boss_name {
                    manager.team.set_member_name(0, boss);
                }
                if let Err(error) = manager.team.save(&b.settings) {
                    warn!("Team save: {error}");
                }
            }
            if outcome.first_boot
                && let Some(career) = world.career.as_mut()
            {
                // Spec 11.3: the new career's world load and first sign-up, by Coach Frank's answer.
                let career = &mut **career;
                if let Some(state) = career.state.as_mut() {
                    crate::skate3_career::glue::start_new(state, &mut career.core, outcome.tutorial.unwrap_or(false));
                }
            } else if let Some(career) = world.career.as_mut() {
                let career = &mut **career;
                if let Some(state) = career.state.as_mut() {
                    crate::skate3_career::glue::resume(state, &mut career.core);
                }
            }
            if outcome.first_boot {
                let career = Career { intro_done: true, team_name: outcome.team_name, tutorial: outcome.tutorial };
                if let Err(error) = save_career(&b.career, &career) {
                    warn!("Career save {}: {error}", b.career.display());
                }
            }
        }
    }
}

fn stop_movie(b: &mut Boot, commands: &mut Commands, images: &mut Assets<Image>) {
    if let Some(movie) = b.movie.take() {
        movie.stop(commands, images);
        *crate::retail_ui::MOVIE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
    if let Some(audio) = b.movie_audio.take() {
        commands.entity(audio).try_despawn();
    }
}

fn finish(b: &mut Boot, commands: &mut Commands, images: &mut Assets<Image>) {
    stop_movie(b, commands, images);
    b.loaded.clear();
    for preview in b.previews.drain(..) {
        preview.stop(images);
    }
    // ponytail: the parsed manifests stay with the resource (a few MB); drop them if memory matters.
    (b.open, b.popup, b.skip, b.tag) = (None, None, None, None);
    b.closing.clear();
}

/// The frame: the screens (or, over a movie, only its skip prompt), the previews, the loading tag, the team-name
/// keyboard and a popup, bottom to top.
fn draws(b: &Boot) -> Vec<Draw> {
    let mut out = Vec::new();
    let mut add = |screen: Option<&Screen>| {
        if let Some(Ok(draws)) = screen.map(Screen::draws) {
            out.extend(draws);
        }
    };
    if b.movie.is_some() {
        add(b.skip.as_ref());
        return out;
    }
    for (screen, _) in &b.closing {
        add(Some(screen));
    }
    add(b.open.as_ref().map(|o| &o.screen));
    out.extend(b.previews.iter().map(Preview::draw));
    // StateLoading's cover over the screens. The loading tag stays over it: the cover's update 0x825E69A0 itself
    // raises the tag once it has been black a second, so the tag is meant to be seen on it.
    let cover = b.flow.cover();
    if cover > 0. {
        out.push(quad(WHITE, [1, 1], [0., 0., 1280., 720.], [0., 0., 0., cover]));
    }
    if b.tag_on {
        if let Some(Ok(draws)) = b.tag.as_ref().map(Screen::draws) {
            out.extend(draws);
        }
    }
    if let Some(team) = b.team.as_ref().filter(|t| t.is_open()) {
        out.extend(team.draws(b.font.as_ref()));
    }
    if b.flow.keyboard_open() {
        out.extend(keyboard_draws(b));
    }
    if let Some(popup) = &b.popup {
        if let Ok(draws) = crate::apt_scene::draw(&popup.bindings.movie, &popup.vm, &popup.bindings.shapes) {
            out.extend(draws);
        }
    }
    out
}

/// Text in the boot's Futura at `height` stage units, left-aligned at `[x, top]` (a name in another script: its
/// system-font glyphs as extra draws, text_fallback).
pub(crate) fn label(font: &crate::apt_text::Font, text: &str, [x, top]: [f32; 2], height: f32, colour: [f32; 4]) -> Vec<Draw> {
    let (sx, sy) = (font.scale[0] * height, font.scale[1] * height);
    let mut x = x + font.offset[0] * sx;
    let y = top + font.offset[1] * sy;
    let mut out = vec![Draw { texture: font.texture.clone(), size: font.size, vertices: Vec::new(), multiply: colour, add: [0.; 4] }];
    for c in crate::text_fallback::nfc(text).chars() {
        let Some((g, texture, size)) = font.place(c) else { continue };
        let quad = crate::apt_scene::glyph_quad(font, &g, size, [x, y], [sx, sy]);
        if texture == font.texture {
            out[0].vertices.extend(quad);
        } else if size[0] > 0 {
            out.push(Draw { texture: texture.to_owned(), size, vertices: quad.to_vec(), multiply: colour, add: [0.; 4] });
        }
        x = g.x_advance.mul_add(sx, x);
    }
    out.retain(|d| !d.vertices.is_empty());
    out
}

/// Decision 9's text entry: a panel in the front end's colours with the title, the name being typed and the
/// controller keyboard (the Xbox Guide keyboard's layout of ALL_VALID_ALPHANUMERIC_CHARACTERS).
fn keyboard_draws(b: &Boot) -> Vec<Draw> {
    let Some(font) = b.font.as_ref() else { return Vec::new() };
    let language = &b.manifests["welcomescreen"]["language"];
    let header = language[b.flow.keyboard_header()].as_str().unwrap_or("Enter New Team Name");
    keyboard_panel(font, &b.flow.keyboard, header, &b.manifests["welcomescreen"]["buttons"])
}

/// The keyboard panel: `header`, `entry`'s text and keys, the legend in `buttons`' glyphs (a manifest's "buttons").
/// Also Edit Skaters' name and team-name keyboards (team_select).
pub(crate) fn keyboard_panel(font: &crate::apt_text::Font, entry: &TextEntry, header: &str, buttons: &serde_json::Value) -> Vec<Draw> {
    const PANEL: [f32; 4] = [300., 150., 980., 630.];
    const KEY: [f32; 2] = [62., 48.];
    let mut out = vec![
        quad(WHITE, [1, 1], [0., 0., 1280., 720.], [0., 0., 0., 0.6]),
        quad(WHITE, [1, 1], PANEL, [0.08, 0.08, 0.08, 0.95]),
        quad(WHITE, [1, 1], [PANEL[0], PANEL[1], PANEL[2], PANEL[1] + 4.], [0.93, 0.62, 0.0, 1.]),
        quad(WHITE, [1, 1], [330., 220., 950., 268.], [1., 1., 1., 0.12]),
    ];
    out.extend(label(font, header, [330., 168.], 26., [1.; 4]));
    out.extend(label(font, &format!("{}_", entry.text), [342., 230.], 26., [1.; 4]));
    let at = |row: usize, column: usize| [330. + column as f32 * KEY[0], 290. + row as f32 * KEY[1]];
    let mut keys: Vec<(usize, usize, usize, String)> = Vec::new();
    for (row, chars) in GRID.iter().enumerate() {
        for (column, c) in chars.chars().enumerate() {
            let c = if entry.shift { c.to_ascii_uppercase() } else { c };
            keys.push((row, column, 1, c.to_string()));
        }
    }
    let mut column = 0;
    for (key, span) in COMMANDS {
        let name = match key {
            Command2::Char(c) => c.to_string(),
            Command2::Shift => "Shift".into(),
            Command2::Space => "Space".into(),
            Command2::Backspace => "Back".into(),
            Command2::Done => "Done".into(),
        };
        keys.push((GRID.len(), column, span, name));
        column += span;
    }
    for (row, column, span, name) in keys {
        let [x, y] = at(row, column);
        let selected = row == entry.row && (column..column + span).contains(&entry.column);
        let fill = if selected { [0.93, 0.62, 0.0, 1.] } else { [1., 1., 1., 0.1] };
        out.push(quad(WHITE, [1, 1], [x + 2., y + 2., x + span as f32 * KEY[0] - 2., y + KEY[1] - 2.], fill));
        let colour = if selected { [0., 0., 0., 1.] } else { [1.; 4] };
        out.extend(label(font, &name, [x + 12., y + 10.], 22., colour));
    }
    // Legend: the 360 glyphs, as the screens' button help shows them.
    let mut x = 330.;
    for (button, text) in [("A", "Select"), ("B", "Cancel"), ("X", "Backspace"), ("Y", "Space"), ("Start", "Done")] {
        let glyph = &buttons[button];
        if let (Some(path), Some(w), Some(h)) = (glyph["rgba"].as_str(), glyph["width"].as_u64(), glyph["height"].as_u64()) {
            out.push(quad(path, [w as u32, h as u32], [x, 592., x + w as f32, 592. + h as f32], [1.; 4]));
            x += w as f32 + 4.;
        }
        out.extend(label(font, text, [x, 596.], 20., [1.; 4]));
        x += font.width(text, 20.) + 22.;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_launch_create_skater_runs_the_world() {
        // Bug 10-09 #2: the boot pauses the world from its first frame; nothing else unpauses it while the CAS is
        // open (graphics_menu::interact returns early), so the CAS had no presentation frame: the camera stayed at
        // the origin under the map and the room was never placed.
        let fe = Some(crate::boot_flow::AUDIO_TIPS_LOADER);
        assert_eq!(world_pause(fe, true), Some(false), "the CAS runs its preview world");
        assert_eq!(world_pause(fe, false), Some(true), "the boot's other screens hold it");
        assert_eq!(world_pause(None, false), None, "after the boot the world is left alone");
    }

    #[test]
    fn a_slow_frame_runs_at_most_four_ticks_and_reports_the_rest() {
        let (accumulated, lost) = fe_clock(0., 0.5);
        assert_eq!(accumulated, 4. * FRAME);
        assert!((lost - (0.5 - 4. * FRAME)).abs() < 1e-6);
        assert_eq!(fe_clock(0.005, FRAME), (0.005 + FRAME, 0.), "an ordinary frame loses nothing");
    }

    #[test]
    fn every_boot_screen_has_a_manifest_name() {
        for slot in [Slot::Esrb, Slot::PressStart, Slot::Welcome, Slot::TeamPrompt, Slot::TeamManagement, Slot::CoachFrank] {
            let (name, state) = screen_name(slot);
            assert!(!name.is_empty());
            assert_eq!(state == 10., slot == Slot::TeamPrompt, "the team-name prompt is the welcome movie in state 10");
        }
        for movie in [Movie::EaIntro, Movie::Attract, Movie::Intro, Movie::CoachFrankIntro] {
            assert!(!movie.stem().is_empty());
        }
        assert!(Popup::Autosave.save_warning() && !Popup::EmptyName.save_warning());
    }

    #[derive(Resource, Default)]
    struct Keys(Vec<(Source, Key)>);

    /// Escape pressed on one frame and released on the next accepts the title (the release comes from the key
    /// events: the boot resets ButtonInput every frame).
    #[test]
    fn keyboard_escape_press_then_release_accepts_the_title() {
        let mut app = App::new();
        app.add_message::<KeyboardInput>().init_resource::<Keys>().init_resource::<ButtonInput<KeyCode>>().add_systems(
            Update,
            |mut events: MessageReader<KeyboardInput>, mut keys: ResMut<ButtonInput<KeyCode>>, mut out: ResMut<Keys>| {
                keyboard_keys(events.read(), false, &mut out.0, &mut String::new());
                keys.reset_all();
            },
        );
        let escape = |state, repeat| KeyboardInput {
            key_code: KeyCode::Escape,
            logical_key: bevy::input::keyboard::Key::Escape,
            state,
            text: None,
            repeat,
            window: Entity::PLACEHOLDER,
        };
        app.world_mut().write_message(escape(ButtonState::Pressed, false));
        app.update();
        app.world_mut().write_message(escape(ButtonState::Pressed, true));
        app.update();
        app.world_mut().write_message(escape(ButtonState::Released, false));
        app.update();
        let keys = std::mem::take(&mut app.world_mut().resource_mut::<Keys>().0);
        assert_eq!(keys, [(Source::Keyboard, Key::Start), (Source::Keyboard, Key::StartReleased)], "a held repeat adds nothing");

        let mut flow = Flow::new(false, DEFAULT_TEAM, ALL_VALID);
        crate::boot_flow::tests::to_title(&mut flow);
        for (source, key) in keys {
            flow.input(source, key);
        }
        assert!(flow.take().contains(&Command::SetState(Slot::PressStart, 2.)), "the title accepted");
    }

    #[test]
    fn team_name_box_ignores_held_enter_and_escape() {
        let event = |key_code, repeat| KeyboardInput {
            key_code,
            logical_key: bevy::input::keyboard::Key::Enter,
            state: ButtonState::Pressed,
            text: None,
            repeat,
            window: Entity::PLACEHOLDER,
        };
        let mut pending = Vec::new();
        let events = [event(KeyCode::Enter, false), event(KeyCode::Enter, true), event(KeyCode::Escape, true), event(KeyCode::Backspace, true)];
        keyboard_keys(&events, true, &mut pending, &mut String::new());
        assert_eq!(pending, [(Source::Keyboard, Key::Start), (Source::Keyboard, Key::X)], "held Backspace keeps deleting");
    }

    fn settings_dir(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("skate3-boot-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (file, text) in files {
            std::fs::write(dir.join(file), text).unwrap();
        }
        dir
    }

    #[test]
    fn a_fresh_install_is_a_first_boot() {
        let dir = settings_dir("fresh", &[]);
        assert!(first_boot(&dir, DEFAULT_TEAM));
        assert!(!dir.join("career.json").exists(), "written when the first boot finishes");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn existing_settings_without_a_career_are_a_returning_boot() {
        let dir = settings_dir("existing", &[("graphics.json", "{\"x\":1}"), ("gameplay.json", "{}")]);
        assert!(!first_boot(&dir, DEFAULT_TEAM));
        assert_eq!(std::fs::read_to_string(dir.join("graphics.json")).unwrap(), "{\"x\":1}", "nothing overwritten");
        assert_eq!(std::fs::read_to_string(dir.join("gameplay.json")).unwrap(), "{}");
        let career: Career = serde_json::from_slice(&std::fs::read(dir.join("career.json")).unwrap()).unwrap();
        assert!(career.intro_done && career.team_name.as_deref() == Some("New Team"));
        assert!(!first_boot(&dir, DEFAULT_TEAM), "and stays returning");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_corrupt_career_is_a_returning_boot() {
        let dir = settings_dir("corrupt", &[("career.json", "{not json"), ("character.json", "{}")]);
        assert!(!first_boot(&dir, DEFAULT_TEAM));
        let bad: Vec<_> = dir.read_dir().unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("career.json.bad-")).collect();
        assert_eq!(std::fs::read_to_string(bad[0].path()).unwrap(), "{not json", "kept aside, not overwritten");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn only_test_and_session_arguments_skip_the_boot() {
        if std::env::var_os("SKATE_SKIP_BOOT").is_some() {
            return;
        }
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(wanted(args(&[])));
        assert!(wanted(args(&["--assets", "D:/assets"])), "a preview build shows the boot");
        for skip in ["--map", "--test-world", "--teleport=dist_x", "--verify", "--check-assets", "--net-host", "--hero-clip"] {
            assert!(!wanted(args(&[skip])), "{skip}");
        }
    }

    /// The keyboard panel over the prepared welcome manifest (SKATE_BOOT_ROOT; PNG in SKATE_BOOT_OUT).
    #[test]
    #[ignore = "requires the prepared boot screens"]
    fn keyboard_panel_renders() {
        let json = crate::boot_screens::tests::manifest("welcomescreen");
        let font = crate::apt_text::TextAssets::load(&json).unwrap();
        let font = font.fonts[&font.font_named("Futura Std Medium").unwrap()].clone();
        assert!(!label(&font, "New Team", [0., 0.], 26., [1.; 4]).is_empty());
    }

    /// First boot 10-29 #2: a preview frame reaches the canvas as a new image (apt_canvas rebinds a draw only on a new
    /// handle), and the clip loops like retail's player (+0x44, decode thread 0x82A916C8); -stream_loop stopped at 190.
    #[test]
    #[ignore = "requires the prepared boot movies (SKATE_BOOT_ROOT) and ffmpeg"]
    fn camera_previews_show_new_frames_and_loop() {
        let root = PathBuf::from(std::env::var("SKATE_BOOT_ROOT").unwrap());
        let mut images = Assets::<Image>::default();
        let mut preview = Preview::start(&mut images, &root.join("movies/cameraLow.vp6"), "preview:low", [504., 420.]).unwrap();
        let first = preview.image.id();
        // 190 frames a pass at 29.97 fps (6.34 s): 10 s is well past the first loop.
        let end = Instant::now() + std::time::Duration::from_secs(10);
        while Instant::now() < end {
            let _ = preview.advance(&mut images);
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        assert_ne!(preview.image.id(), first, "frames go into new images");
        assert!(preview.shown > 250, "still playing past the clip's 190 frames: {}", preview.shown);
        preview.stop(&mut images);
    }
}
