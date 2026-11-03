//! Native-resolution pause UI over a separately scaled 3D render target.
use crate::difficulty::Difficulty;
use bevy::{
    camera::RenderTarget,
    core_pipeline::prepass::{DepthPrepass, NormalPrepass},
    pbr::{ScreenSpaceAmbientOcclusion, ScreenSpaceAmbientOcclusionQualityLevel},
    image::ImageSampler,
    prelude::*,
    render::{
        experimental::occlusion_culling::OcclusionCulling,
        render_resource::{Extent3d, TextureFormat},
        renderer::RenderAdapter,
    },
    window::{
        Monitor, MonitorSelection, PresentMode, PrimaryMonitor, PrimaryWindow, VideoModeSelection,
        WindowMode, WindowPosition,
    },
};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicU16, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
    time::{Duration, Instant},
};

const RESOLUTIONS: &[(u32, u32)] = &[
    (1280, 720),
    (1280, 800),
    (1600, 900),
    (1920, 1080),
    (1920, 1200),
    (2560, 1440),
    (2560, 1600),
    (3840, 2160),
];
const SCALES: &[u32] = &[25, 50, 67, 75, 85, 100];
const LIMITS: &[u32] = &[0, 30, 60, 90, 120, 144, 165, 240];
// The original picker holds at most eight choices (see coremenu.GetMaxSubOptions).
const BRIGHTNESS: &[u32] = &[70, 80, 90, 100, 110, 120, 130, 140];
/// Anti-Aliasing post passes; MSAA sample counts follow them in the picker.
const POST_AA: &[&str] = &["Off", "FXAA", "SMAA"];
/// Anisotropic Filtering: the world texture samplers' clamp (1 is off).
const ANISOTROPY_LEVELS: &[u16] = &[1, 2, 4, 8, 16];
const ANISOTROPY_DEFAULT: u16 = 8;
/// What skate_world's map texture samplers use; set from the settings before the first map loads.
pub(crate) static ANISOTROPY: AtomicU16 = AtomicU16::new(ANISOTROPY_DEFAULT);
/// Sharpening: label and contrast-adaptive sharpening strength.
const SHARPENING: &[(&str, f32)] = &[("Off", 0.), ("Low", 0.3), ("Medium", 0.6), ("High", 0.9)];
/// NVIDIA DLSS choices (dlss.rs maps them to Bevy's modes); 0 is off. The row exists only in `dlss` feature builds on a
/// GPU that supports it (`Menu::dlss_supported`). While one is on, DLSS owns the internal resolution and anti-aliasing:
/// Render Scale, Anti-Aliasing and Ambient Occlusion (Bevy's SSAO ignores the lower internal resolution) step aside.
pub(crate) const DLSS_MODES: [&str; 7] = ["Off", "DLAA", "Quality", "Balanced", "Performance", "Ultra Performance", "Auto"];
/// Not retail (user 10-25): Settings > Audio > Text Chat.
const CHAT_CHOICES: [&str; 3] = ["Off", "Filtered", "Unfiltered"];
/// Label, rain and snow strength; visual only, gameplay never reads it.
const WEATHER: &[(&str, f32, f32)] = &[
    ("Clear", 0., 0.),
    ("Light Rain", 0.5, 0.),
    ("Heavy Rain", 1., 0.),
    ("Light Snow", 0., 0.5),
    ("Heavy Snow", 0., 1.),
    // Not retail (user 10-19): Halloween is clear everywhere; Christmas lays a full snow cover everywhere under a
    // lighter haze than Heavy Snow, with a gentle fall (retail_weather::snowfall). seasonal.rs decorates only the
    // Super Ultra Mega Park.
    ("Halloween", 0., 0.),
    ("Christmas", 0., 0.5),
];
/// Not retail (user 10-14, feature #4): Graphics > Preset. What a preset sets: render scale, (post AA, MSAA samples),
/// shadow quality, ambient occlusion, anisotropy, texture quality, draw distance, sharpening, the [`EFFECTS`] and DLSS.
pub(crate) type Look = (u32, (u32, u32), u32, bool, u16, u32, u32, u32, [bool; 5], u32);
/// Performance (Fortnite's Performance Mode in spirit): 67% scale sharpened back, FXAA, the shortest draw distance,
/// low textures and shadows, and no AO, bloom, depth of field or motion blur (colour grading and the lens stay).
/// Not retail (user 10-19): Performance mode is meant to look worse for frame rate: 50% scale upscaled smoothly (no
/// sharpening), every post effect off, and while its row is exactly in effect ([`GraphicsSettings::performance`]) it
/// also caps world textures ([`performance_texture`]), turns every shadow map off and draws to 300 m.
/// ponytail: the world's lighting shaders are unchanged; a cheaper lighting path would be its own work.
const PRESETS: [(&str, Look); 2] = [
    ("Performance", (50, (1, 1), 0, false, 1, 0, 0, 0, [false; 5], 0)),
    ("Default", (100, (0, 4), SHADOW_QUALITY_DEFAULT, false, ANISOTROPY_DEFAULT, 2, 2, 0, [true; 5], 0)),
];
impl GraphicsSettings {
    fn look(&self) -> Look {
        (self.scale, (self.post_aa, self.samples), self.shadow_quality, self.ambient_occlusion, self.anisotropy,
         self.texture_quality, self.draw_distance, self.sharpening, self.effects, self.dlss)
    }
    fn set_look(&mut self, look: Look) {
        (self.scale, (self.post_aa, self.samples), self.shadow_quality, self.ambient_occlusion, self.anisotropy,
         self.texture_quality, self.draw_distance, self.sharpening, self.effects, self.dlss) = look;
    }
    /// Not retail (user 10-19): Performance mode: the Performance preset's rows, unchanged (one changed row is Custom).
    fn performance(&self) -> bool {
        self.look() == PRESETS[0].1
    }
}

/// Not retail (user 10-14, feature #5): label, then (opacity, metres to half of it); None follows the weather.
pub(crate) const FOG: [(&str, Option<(f32, f32)>); 11] = [
    ("Weather", None),
    ("Off", Some((0., 300.))),
    ("Light (far)", Some((0.5, 500.))),
    ("Light", Some((0.5, 250.))),
    ("Light (near)", Some((0.5, 100.))),
    ("Medium (far)", Some((0.7, 400.))),
    ("Medium", Some((0.7, 180.))),
    ("Medium (near)", Some((0.7, 70.))),
    ("Heavy (far)", Some((0.9, 300.))),
    ("Heavy", Some((0.9, 120.))),
    ("Heavy (near)", Some((0.9, 40.))),
];
/// Field of View slider: degrees added to the retail vertical FOV (lens 12 mm,
/// ~67 degrees at 16:9; skate-core camera/manager_frame.rs). Never wider than +10.
const FOV_RANGE: std::ops::RangeInclusive<i32> = -15..=10;
const FOV_RETAIL: i32 = 67;
/// Shadow Quality labels; retail_character::shadow_quality maps the index.
const SHADOW_QUALITIES: &[&str] = &["Low", "Medium", "High"];
pub(crate) const SHADOW_QUALITY_DEFAULT: u32 = 1;
/// Camera Angle choices: label, camera::STYLES index and Low. Skate 3's own row
/// (Control Settings > Camera Angle: Low/High), then the older games' cameras.
/// Skate 1 had a single camera, so it has one choice.
const CAMERA_ANGLES: [(&str, u32, bool); 5] = [
    ("Low", 0, true),
    ("High", 0, false),
    ("Skate 2 Low", 1, true),
    ("Skate 2 High", 1, false),
    ("Skate 1", 2, false),
];
/// Edit Skater rooms: label and private/native-backdrops package stem. The Skate 2
/// room is Skate 2's own (old_room.py); it takes Skate 2's own skater spots and cameras (cas_room::Skate2Cas).
pub(crate) const ROOMS: &[(&str, &str)] = &[
    ("Skate 3", "CreateSkater"),
    ("Skate 2", "CreateSkater_Skate2"),
];
/// Two older placements of the same Skate 2 room, once rows of [`ROOMS`]: a save that chose one opens Skate 2.
const OLD_SKATE2_ROOMS: [&str; 2] = ["CreateSkater_Skate2Couch", "CreateSkater_Skate2Door"];

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
enum DisplayMode {
    #[default]
    Windowed,
    Borderless,
    /// Exclusive, at the chosen resolution when the monitor offers it.
    Fullscreen,
}
const DISPLAY_MODES: &[(DisplayMode, &str)] = &[
    (DisplayMode::Windowed, "Windowed"),
    (DisplayMode::Borderless, "Borderless"),
    (DisplayMode::Fullscreen, "Fullscreen"),
];

/// A Game Settings row in the Start menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Setting {
    DisplayMode,
    /// Which display Borderless and Fullscreen fill (and a window opens on): Automatic or one of [`Menu::monitors`].
    Monitor,
    Resolution,
    RenderScale,
    Msaa,
    FpsLimit,
    VSync,
    Occlusion,
    Brightness,
    TimeOfDay,
    Weather,
    /// A [`FOG`] row.
    Fog,
    /// A [`PRESETS`] row (or Custom).
    Preset,
    CameraAngle,
    CameraShake,
    InvertX,
    InvertY,
    Hud,
    HudStyle,
    /// Not retail (user 10-26): Settings > World's Colour Grading row, Off ([`EFFECTS`]' first bit, as the Performance
    /// preset clears it) or the game whose look shows ([`HUD_STYLES`]). This player's own, online too.
    Look,
    /// One of [`EFFECTS`]: a Skate 1/2 map post effect.
    Effect(usize),
    /// A [`DRAW_DISTANCES`] row.
    DrawDistance,
    /// Low, Medium or High (retail) world textures, on the next map load.
    TextureQuality,
    EditSkaterRoom,
    /// Other players' Graphic Creator designs.
    PlayerGraphics,
    /// A slider (`Menu::slide`), not a picker.
    FieldOfView,
    ShadowQuality,
    AmbientOcclusion,
    Anisotropy,
    Sharpening,
    /// A [`DLSS_MODES`] row (dlss.rs); listed only where DLSS is supported.
    Dlss,
    /// Retail Control Settings "Vibration" (option 19, ID_GAMESETTINGS_VIBRATION_TOGGLE).
    Vibration,
    /// Retail Control Settings "Hall Of Meat Mode" (ID_GAMESETTINGS_HOM_MODE_OPTIONS).
    HallOfMeat,
    /// Retail option 24 "Units" (ID_GAMESETTINGS_UNITS_TOGGLE): Metric or Imperial.
    Units,
    /// Retail Game Settings "Subtitles" (ID_GAMESETTINGS_SUBTITLES): the movies' subtitle lines.
    Subtitles,
    /// Not retail (3.3): Discord Rich Presence on or off.
    DiscordStatus,
    /// Not retail (online safety 3.12, 3.2): online Text Chat, Chat From (Everyone / Friends) and Custom Models
    /// (Everyone / Friends / Nobody); player_list reads them.
    Chat,
    ChatFrom,
    CustomModels,
    /// Not retail (online safety Q6): Reduce Flashing, off by default.
    ReduceFlashing,
    /// Not retail (perf B3): render each frame right after its update, not one frame behind; applies on restart.
    LowLatency,
    /// Not retail (user 10-26): the FPS box (fps_overlay) shows, on by default.
    FpsCounter,
    /// Not retail (user 11-02): the next start goes straight into the game (boot_flow::Flow::skip_intro); off by default.
    SkipIntro,
    /// Not retail (user 10-26): the Voice Chat page's rows (core_menu::voice_categories), in its order, and the Audio
    /// tab's Output Device.
    VoiceChat,
    VoiceFrom,
    MicMode,
    /// Set by the next key pressed after Accept (core_menu), not a picker.
    PushToTalkKey,
    PushToTalkButton,
    /// Sliders (`Menu::slide`), as Field of View.
    OpenMicSensitivity,
    Microphone,
    MicVolume,
    MicTest,
    NoiseSuppression,
    VoiceVolume,
    LowerGameSound,
    OutputDevice,
}
fn position<T: PartialEq>(values: &[T], value: T) -> usize {
    values.iter().position(|v| *v == value).unwrap_or(0)
}
/// A Push to Talk Key as the row shows it: "KeyV" is "V", "Digit4" "4", "ShiftLeft" "Shift Left", "Mouse4" "Mouse 4".
pub(crate) fn key_label(key: &str) -> String {
    if let Some(number) = key.strip_prefix("Mouse") {
        return format!("Mouse {number}");
    }
    let key = key.strip_prefix("Key").or_else(|| key.strip_prefix("Digit")).filter(|k| k.len() == 1).unwrap_or(key);
    let mut label = String::new();
    for (i, c) in key.char_indices() {
        if c.is_ascii_uppercase() && key[..i].ends_with(|p: char| p.is_ascii_lowercase()) {
            label.push(' ');
        }
        label.push(c);
    }
    label
}
/// Push to Talk Button's choices (user 10-26): label, XInput bit, and what the button gives up while it only talks, from
/// retail's control text (english.json: ID_CHALLENGE_TUT_*, ID_SKATEPARK_*, ID_HINTS_TEXT_17/24, ID_TIPS_LOADER_13) and
/// the port's replay editor (replay/hud.rs). A, B and Start are left out, so the menus stay usable.
pub(crate) const PTT_BUTTONS: [(&str, u16, &str); 8] = [
    ("None", 0, "Hold a controller button to talk as well as the key; that button then does nothing else"),
    ("X", 0x4000, "X only talks: push with A; no park editor New Object, no replay Copy Camera"),
    ("Y", 0x8000, "Y only talks: no getting off or on the board, no drop-ins, no park editor Delete, no settings search"),
    ("LB", 0x0100, "LB only talks: no session markers, next song, Object Dropper, park editor or photos"),
    ("RB", 0x0200, "RB only talks: no darkslides or no-foot inverts, no grabbing objects, no shoving people"),
    ("Back", 0x0020, "Back only talks: no park editor Quick Menu, no replay Paste Camera"),
    ("L3", 0x0040, "L3 only talks: no park editor object reset"),
    ("R3", 0x0080, "R3 only talks: no Reset Roll in the replay editor"),
];
/// HUD Style choices: label and the game whose HUD shows (None follows the map).
const HUD_STYLES: [(&str, Option<u8>); 4] = [("Match Map", None), ("Skate 3", Some(3)), ("Skate 2", Some(2)), ("Skate 1", Some(1))];
fn on_off(on: bool) -> (Vec<String>, usize) {
    (vec!["Off".into(), "On".into()], usize::from(on))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct GraphicsSettings {
    display_mode: DisplayMode,
    /// The chosen display's system name (`\\.\DISPLAY2`); None follows the window. Kept while it is unplugged.
    monitor: Option<String>,
    width: u32,
    height: u32,
    scale: u32,
    /// MSAA samples; 1 while a POST_AA pass is chosen.
    samples: u32,
    /// A POST_AA index (0: none).
    post_aa: u32,
    fps: u32,
    /// GPU occlusion culling, off by default since perf 10-10: its depth prepass encodes every opaque draw twice more,
    /// costing more than it culls (1080p, RTX 2050: Skate 1 San Van 70 -> 90 FPS off, New San Van 83 -> 96). Saved
    /// under a new key so files written with the old default (on) start off.
    #[serde(rename = "occlusion_culling")]
    occlusion: bool,
    /// A world_time::TIMES index.
    time_of_day: u32,
    /// A WEATHER index.
    weather: u32,
    /// A [`FOG`] index.
    fog: u32,
    vsync: bool,
    /// V-Sync "Fast": Mailbox (no tearing, the newest frame shows at each refresh) where the GPU offers it, else FIFO.
    vsync_fast: bool,
    /// Percent; scales the presented world, not the menus.
    brightness: u32,
    hud: bool,
    /// A HUD_STYLES index: the map's own game's HUD, or Skate 3's, 2's or 1's on every map.
    hud_style: u32,
    /// Not retail (user 10-14, feature #9): a HUD_STYLES index for the screen effects' game (retail_grading.rs).
    look: u32,
    /// Settings saved before the effects were split: false turns every [`EFFECTS`] row off. Read, never written.
    #[serde(skip_serializing)]
    screen_effects: bool,
    /// The [`EFFECTS`] rows, in order; all on is retail.
    effects: [bool; 5],
    /// A [`DRAW_DISTANCES`] index; High (2) is the full view.
    draw_distance: u32,
    /// Texture Quality: 0 Low, 1 Medium, 2 High (the full textures).
    texture_quality: u32,
    camera_low: bool,
    /// A camera::STYLES index: the Skate 3, Skate 2 or Skate 1 camera.
    camera_style: u32,
    camera_shake: bool,
    invert_look: [bool; 2],
    /// A ROOMS package stem.
    edit_skater_room: String,
    /// In FOV_RANGE: degrees added to the camera's vertical FOV.
    fov_offset: i32,
    /// A SHADOW_QUALITIES index.
    shadow_quality: u32,
    /// Screen-space ambient occlusion; turns MSAA off while on.
    ambient_occlusion: bool,
    /// An ANISOTROPY_LEVELS value.
    anisotropy: u16,
    /// A SHARPENING index.
    sharpening: u32,
    /// A DLSS_MODES index (0: off). Kept by builds without DLSS, which ignore it.
    dlss: u32,
    /// Show other players' Graphic Creator designs (stock logos always show).
    player_graphics: bool,
    /// Controller rumble (retail game settings +163, seeded from the dashboard's Vibration
    /// preference, whose default is on).
    vibration: bool,
    /// Free-skate Hall of Meat (retail game settings +184; off by default).
    hall_of_meat: bool,
    /// Units (retail settings+168, mirrored to the locale's +4): imperial, else metric. Retail starts imperial and
    /// switches to metric on a European console (0x824FFAB0, 0x825009F0); the PC has no region, so imperial (user's choice).
    imperial: bool,
    /// Learn > On Screen Help: [Trick Analyzer, Manual Meter]. Retail resets both each boot; kept here (user's choice).
    on_screen_help: [bool; 2],
    /// Subtitles (retail game settings +157; 0 for English, so off).
    subtitles: bool,
    /// Discord Rich Presence (discord_presence.rs): what you play, shown to your Discord friends. Owner decision: on keeps
    /// the earlier behaviour.
    discord_status: bool,
    /// Online: text chat shows, only friends' chat, whose own models show (player_list::CUSTOM_MODELS).
    chat: bool,
    /// Not retail (user 10-25): Text Chat "Filtered" (the default) masks swearing; slurs are masked either way.
    chat_filter: bool,
    chat_friends: bool,
    custom_models: u32,
    /// Not retail (online safety Q6): dimmer lightning, slower session weather / time changes, other players' mod
    /// boxes keep their first colour.
    reduce_flashing: bool,
    /// Low Latency: no pipelined rendering (app.rs). Measured 10-17 on New San Vanelona: input to GPU done 21 -> 14 ms,
    /// 154 -> 129 FPS. Read once at startup.
    low_latency: bool,
    /// Not retail (user 10-26): the FPS box shows (fps_overlay); on, as it always was before the row.
    fps_counter: bool,
    /// Not retail (user 11-02): Skip Intro, read once at startup (boot.rs).
    skip_intro: bool,
    /// Audio Settings volumes and the Music Player's soundtrack choices (music.rs).
    audio: crate::music::AudioSettings,
    /// Voice chat (user 10-26, Settings > Audio > Voice Chat): on for a new player, from everyone.
    voice_chat: bool,
    voice_friends_only: bool,
    /// Mic Mode: Open Mic, else Push to Talk (the default).
    open_mic: bool,
    /// Push to Talk Key: a KeyCode's Debug name ("KeyV"), or "Mouse4" / "Mouse5" (the side buttons).
    ptt_key: String,
    /// Push to Talk Button: a [`PTT_BUTTONS`] bit (0: none).
    ptt_pad: u16,
    /// Open Mic Sensitivity, 1..=10 (skate_voice::dsp::line_dbfs).
    open_mic_sensitivity: u8,
    /// An input device by name; None: the Windows default. Kept while it is unplugged.
    microphone: Option<String>,
    /// Mic Volume, percent: 0..=200 in 10 % steps.
    mic_volume: u16,
    /// Noise Suppression: 0 Off, 1 Standard (RNNoise), 2 High (DeepFilterNet).
    noise: u8,
    /// Voice Volume, percent: 0..=100 in 10 % steps.
    voice_volume: u8,
    /// Lower Game Sound While Talking.
    duck_for_voice: bool,
    /// Settings > Audio > Output Device by name (None: the Windows default); opened at startup (vendored bevy_audio).
    output_device: Option<String>,
}
impl Default for GraphicsSettings {
    fn default() -> Self {
        Self {
            display_mode: DisplayMode::Windowed,
            monitor: None,
            width: 1280,
            height: 720,
            scale: 100,
            samples: 4,
            post_aa: 0,
            fps: 0,
            occlusion: false,
            time_of_day: 0,
            weather: 0,
            fog: 0,
            vsync: false,
            vsync_fast: false,
            brightness: 100,
            hud: true,
            hud_style: 0,
            look: 0,
            screen_effects: true,
            effects: [true; 5],
            draw_distance: 2,
            texture_quality: 2,
            // Retail's Camera Angle default is Low (game settings +188 = 0; getter 0x8260D9A0, 0 = LOW).
            camera_low: true,
            camera_style: 0,
            camera_shake: true,
            invert_look: [false; 2],
            edit_skater_room: ROOMS[0].1.into(),
            fov_offset: 0,
            shadow_quality: SHADOW_QUALITY_DEFAULT,
            ambient_occlusion: false,
            anisotropy: ANISOTROPY_DEFAULT,
            sharpening: 0,
            dlss: 0,
            player_graphics: true,
            vibration: true,
            hall_of_meat: false,
            imperial: true,
            on_screen_help: [false; 2],
            subtitles: false,
            discord_status: true,
            chat: true,
            chat_filter: true,
            chat_friends: false,
            // ponytail: owner decision (online safety 3.2): Everyone, as before; Friends is the safer default.
            custom_models: 0,
            reduce_flashing: false,
            low_latency: false,
            fps_counter: true,
            skip_intro: false,
            audio: Default::default(),
            voice_chat: true,
            voice_friends_only: false,
            open_mic: false,
            ptt_key: "KeyV".into(),
            ptt_pad: 0,
            open_mic_sensitivity: 5,
            microphone: None,
            mic_volume: 100,
            noise: 1,
            voice_volume: 100,
            duck_for_voice: false,
            output_device: None,
        }
    }
}
impl GraphicsSettings {
    /// No saved settings yet. On integrated (and software) GPUs the heaviest defaults, 4x MSAA, Medium shadows and
    /// 8x anisotropy, cost the most memory bandwidth, so those start on FXAA, Low and 4x. Every row stays in
    /// Settings, and nothing changes once a settings file exists.
    /// Not retail (user 10-12): a first run starts in exclusive Fullscreen, measured faster than Borderless on the
    /// RTX 2050 (University 159 -> 176 FPS, Skate 1 San Vanelona 116 -> 122). 1080p, or the monitor's own mode when it
    /// has no 1080p one (fullscreen_mode falls back to Current).
    fn first_run(device: wgpu::DeviceType) -> Self {
        let base = Self { display_mode: DisplayMode::Fullscreen, width: 1920, height: 1080, ..Self::default() };
        match device {
            wgpu::DeviceType::IntegratedGpu | wgpu::DeviceType::Cpu => {
                info!("Integrated GPU: first-run graphics start on FXAA, Low shadows and 4x anisotropy");
                Self { samples: 1, post_aa: 1, shadow_quality: 0, anisotropy: 4, ..base }
            }
            _ => base,
        }
    }
    fn validated(mut self) -> Self {
        if self.texture_quality > 2 {
            self.texture_quality = 2;
        }
        if self.draw_distance as usize >= DRAW_DISTANCES.len() {
            self.draw_distance = 2;
        }
        if !self.screen_effects {
            (self.screen_effects, self.effects) = (true, [false; 5]);
        }
        if self.time_of_day as usize >= crate::world_time::TIMES.len() {
            self.time_of_day = 0;
        }
        if self.look as usize >= HUD_STYLES.len() {
            self.look = 0;
        }
        if self.fog as usize >= FOG.len() {
            self.fog = 0;
        }
        if self.weather as usize >= WEATHER.len() {
            self.weather = 0;
        }
        if self.camera_style > 2 {
            self.camera_style = 0;
        }
        if self.hud_style as usize >= HUD_STYLES.len() {
            self.hud_style = 0;
        }
        if !RESOLUTIONS.contains(&(self.width, self.height)) {
            (self.width, self.height) = (1280, 720);
        }
        if !SCALES.contains(&self.scale) {
            self.scale = 100;
        }
        if ![1, 2, 4, 8].contains(&self.samples) {
            self.samples = 4;
        }
        if self.post_aa as usize >= POST_AA.len() {
            self.post_aa = 0;
        }
        if self.post_aa > 0 {
            self.samples = 1;
        }
        if !ANISOTROPY_LEVELS.contains(&self.anisotropy) {
            self.anisotropy = ANISOTROPY_DEFAULT;
        }
        if self.sharpening as usize >= SHARPENING.len() {
            self.sharpening = 0;
        }
        if self.dlss as usize >= DLSS_MODES.len() {
            self.dlss = 0;
        }
        if !LIMITS.contains(&self.fps) {
            self.fps = 0;
        }
        if !BRIGHTNESS.contains(&self.brightness) {
            self.brightness = 100;
        }
        if OLD_SKATE2_ROOMS.contains(&self.edit_skater_room.as_str()) {
            self.edit_skater_room = ROOMS[1].1.into();
        } else if !ROOMS.iter().any(|r| r.1 == self.edit_skater_room) {
            self.edit_skater_room = ROOMS[0].1.into();
        }
        if !FOV_RANGE.contains(&self.fov_offset) {
            self.fov_offset = 0;
        }
        if self.shadow_quality as usize >= SHADOW_QUALITIES.len() {
            self.shadow_quality = SHADOW_QUALITY_DEFAULT;
        }
        self.audio.volumes = self.audio.volumes.map(|v| v.min(10));
        self.audio.rain = self.audio.rain.min(10);
        // Voice chat (user 10-26): the rows' ranges, volumes in 10 % steps; a pad button only from the picker.
        self.mic_volume = self.mic_volume.min(200) / 10 * 10;
        self.voice_volume = self.voice_volume.min(100) / 10 * 10;
        self.open_mic_sensitivity = self.open_mic_sensitivity.clamp(1, 10);
        if self.noise > 2 {
            self.noise = 1;
        }
        if !PTT_BUTTONS.iter().any(|b| b.1 == self.ptt_pad) {
            self.ptt_pad = 0;
        }
        if self.ptt_key.is_empty() || self.ptt_key.len() > 32 {
            self.ptt_key = "KeyV".into();
        }
        self
    }
    fn internal_size(&self, window: UVec2) -> UVec2 {
        (window * self.scale / 100).max(UVec2::ONE)
    }
}
/// A connected display, as the Monitor row lists it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MonitorInfo {
    name: String,
    size: UVec2,
    hz: Option<u32>,
    primary: bool,
}
impl MonitorInfo {
    /// "Display 2: 1920 x 1080, 144 Hz (Primary)": Windows names displays `\\.\DISPLAYn`.
    fn label(&self) -> String {
        let number = self.name.trim_start_matches(|c: char| !c.is_ascii_digit());
        let mut label = if number.is_empty() { self.name.clone() } else { format!("Display {number}") };
        label += &format!(": {} x {}", self.size.x, self.size.y);
        if let Some(hz) = self.hz {
            label += &format!(", {hz} Hz");
        }
        if self.primary {
            label += " (Primary)";
        }
        label
    }
}

#[derive(Resource)]
pub(crate) struct Menu {
    pub(crate) open: bool,
    selected: usize,
    settings: GraphicsSettings,
    path: PathBuf,
    supported_msaa: Vec<u32>,
    difficulty: Difficulty,
    /// Skate 2 physics (user 10-20, not tested in play): the saved choice and the four D-pad gesture slots.
    physics: crate::skate2_physics::settings::Physics,
    skate2_gestures: [u32; 4],
    status: String,
    maps: Vec<crate::map_library::Entry>,
    selected_map: usize,
    multiplayer: bool,
    browser: bool,
    /// The original Start-button crossbar owns the paused screen; this panel is hidden.
    pub(crate) crossbar: bool,
    /// A row action queued by the crossbar, performed exactly as if chosen here.
    pending: Option<(usize, i32)>,
    /// The crossbar consumed this frame's press; the panel must not act on it again.
    handed_off: bool,
    /// A guest in an online session: the host's difficulty, weather and time of day apply, and the player can't
    /// change them (online_moves).
    pub(crate) guest: bool,
    /// A guest the host made co-host: their weather and time of day picks go to the host (`world_request`).
    pub(crate) cohost: bool,
    /// A co-host's Settings > World pick for online_moves to send to the host.
    pub(crate) world_request: Option<[u32; 2]>,
    /// An online session's [weather, time of day] (WEATHER and world_time::TIMES indices): shown and used instead of
    /// the player's own, which come back after the session. Never saved.
    session_world: Option<[u32; 2]>,
    /// Not retail (user 10-19): on a real Halloween or Christmas with the weather left Clear, that season's mode
    /// (seasonal::today) until the player picks a weather. Never saved.
    seasonal: Option<u32>,
    /// Vibration was just turned on: the rumble system plays retail's confirmation pulse and clears this.
    pub(crate) vibration_buzz: bool,
    /// Bumped by every settings change: the paused world (camera::present) renders afresh to show it.
    pub(crate) revision: u64,
    /// Connected displays, by name (list_monitors); the picker's 8-row limit keeps the first seven.
    monitors: Vec<MonitorInfo>,
    /// This GPU runs DLSS (dlss.rs sets it in `dlss` feature builds); false hides the DLSS row and ignores the setting.
    pub(crate) dlss_supported: bool,
    /// The loaded map is New San Vanelona (`skate2_physics` sets it when a map loads, as `dlss_supported` is set):
    /// the only map Skate 2 physics runs on in this build.
    pub(crate) on_nsv: bool,
    /// Not retail (user 10-25): the loaded map is the Super Ultra Mega Park's (seasonal.rs sets it, as `on_nsv` is
    /// set). Halloween and Christmas are offered and in effect only there; elsewhere they read as Clear.
    pub(crate) seasons: bool,
    /// Voice chat's Mic Test (user 10-26): on only while a page shows its row (core_menu turns it off); never saved.
    pub(crate) mic_test: bool,
    /// The mic's level (dBFS) for the Mic Test meter, written by the voice plugin.
    pub(crate) mic_level_dbfs: f32,
    /// The input and output devices by name, read on a thread at start and as Game Settings or the Voice Chat page
    /// opens ([`Self::refresh_voice_devices`]).
    pub(crate) voice_devices: Option<(Vec<String>, Vec<String>)>,
    /// A read under way; [`Self::take_voice_devices`] takes its lists.
    pub(crate) voice_devices_reading: Option<Mutex<Receiver<(Vec<String>, Vec<String>)>>>,
    /// Why voice chat opened no microphone (written by the voice plugin), shown on the Microphone row.
    pub(crate) mic_error: Option<String>,
}
impl Menu {
    /// FPS pass look check (verification.rs SKATE_CAPTURE_TOGGLE, diagnostics): puts `back`, else Performance mode's
    /// look, in effect and returns the look it replaced. Never saved.
    pub(crate) fn diagnostic_look(&mut self, back: Option<Look>) -> Look {
        let replaced = self.settings.look();
        self.settings.set_look(back.unwrap_or(PRESETS[0].1));
        replaced
    }
    /// The [`DLSS_MODES`] index in effect: the saved choice where DLSS runs, else 0 (off).
    pub(crate) fn dlss(&self) -> u32 {
        if self.dlss_supported { self.settings.dlss } else { 0 }
    }
    /// The 3D image's size: Render Scale's share of the window, or the whole window while DLSS (which picks its own
    /// internal resolution and upscales to this) is on.
    fn internal_size(&self, window: UVec2) -> UVec2 {
        if self.dlss() != 0 { window.max(UVec2::ONE) } else { self.settings.internal_size(window) }
    }
    /// The CAS closed: back to gameplay, unless it was opened over the Start-button crossbar (Edit Skaters), which
    /// is still up and unpauses through its own close.
    pub(crate) fn cas_closed(&mut self) {
        if !self.crossbar {
            self.open = false;
        }
    }
    pub(crate) fn map_labels(&self) -> Vec<String> {
        self.maps.iter().map(|m| m.label.clone()).collect()
    }
    pub(crate) fn selected_map(&self) -> usize {
        self.selected_map
    }
    /// G1: the saved map that loaded behind the boot is the one selected, as setup selects a map loaded before it.
    pub(crate) fn select_map(&mut self, path: Option<&std::path::Path>) {
        if let Some(i) = self.maps.iter().position(|m| m.path.as_deref() == path) {
            self.selected_map = i;
        }
    }
    pub(crate) fn difficulty(&self) -> Difficulty {
        self.difficulty
    }
    /// An online session's difficulty (or the player's own again after it): applied, and kept by map loads, not saved.
    pub(crate) fn adopt_difficulty(
        &mut self,
        difficulty: Difficulty,
        physics: &mut crate::physics::GamePhysics,
        config: &mut crate::config::Config,
    ) {
        self.difficulty = difficulty;
        physics.set_difficulty(difficulty);
        config.difficulty = difficulty;
    }
    pub(crate) fn set_difficulty(
        &mut self,
        difficulty: Difficulty,
        physics: &mut crate::physics::GamePhysics,
        config: &mut crate::config::Config,
    ) {
        if self.guest {
            self.status = "The host sets the difficulty in an online session".into();
            return;
        }
        self.difficulty = difficulty;
        physics.set_difficulty(difficulty);
        config.difficulty = difficulty;
        self.status = match difficulty.save(&config.asset_root) {
            Ok(()) => "Difficulty saved".into(),
            Err(e) => format!("Applied, but could not save: {e}"),
        };
    }
    pub(crate) fn physics(&self) -> crate::skate2_physics::settings::Physics {
        self.physics
    }
    /// Saved; `skate2_physics` acts on it. Not tested in play.
    pub(crate) fn set_physics(&mut self, physics: crate::skate2_physics::settings::Physics, root: &std::path::Path) -> Result<(), String> {
        self.physics = physics;
        let saved = crate::skate2_physics::settings::Gameplay::update(root, |gameplay| gameplay.physics = physics);
        self.status = match &saved {
            Ok(()) => "Physics saved".into(),
            Err(e) => format!("Applied, but could not save: {e}"),
        };
        saved
    }
    /// An online session's physics (the host's, user 10-28), or the player's own again after it: applied, not saved.
    pub(crate) fn adopt_physics(&mut self, physics: crate::skate2_physics::settings::Physics) {
        self.physics = physics;
    }
    pub(crate) fn skate2_gestures(&self) -> [u32; 4] {
        self.skate2_gestures
    }
    /// Slot 0..=3 (D-pad Up, Down, Left, Right) takes one of Skate 2's 35 gestures. Anything else is refused: a bad
    /// index sent to the sim ends it.
    pub(crate) fn set_skate2_gesture(&mut self, slot: usize, gesture: u32, root: &std::path::Path) -> Result<(), String> {
        if slot >= self.skate2_gestures.len() || gesture as usize >= crate::skate2_physics::settings::GESTURES.len() {
            self.status = "That is not one of Skate 2's 35 gestures".into();
            return Err(format!("gesture {gesture} for slot {slot} is out of range"));
        }
        self.skate2_gestures[slot] = gesture;
        let saved = crate::skate2_physics::settings::Gameplay::update(root, |gameplay| gameplay.skate2_gestures[slot] = gesture);
        self.status = match &saved {
            Ok(()) => "Gestures saved".into(),
            Err(e) => format!("Applied, but could not save: {e}"),
        };
        saved
    }
    /// Whether the Camera Angle row's Low is chosen (Skate 1's single camera ignores it).
    pub(crate) fn camera_low(&self) -> bool {
        self.settings.camera_low
    }
    /// Invert Look X and Y.
    pub(crate) fn invert_look(&self) -> [bool; 2] {
        self.settings.invert_look
    }
    #[cfg(test)]
    pub(crate) fn status(&self) -> &str {
        &self.status
    }
    /// Continue in this panel from the crossbar at `row` (on the multiplayer page when
    /// requested), optionally performing that row's action.
    pub(crate) fn hand_off(
        &mut self,
        row: usize,
        multiplayer: bool,
        perform: bool,
        map: Option<usize>,
    ) {
        self.crossbar = false;
        self.open = true;
        self.multiplayer = multiplayer;
        self.browser = false;
        self.selected = row;
        if let Some(map) = map.filter(|&m| m < self.maps.len()) {
            self.selected_map = map;
        }
        self.pending = perform.then_some((row, 1));
        self.handed_off = true;
    }
    pub(crate) fn session_world(&self) -> Option<[u32; 2]> {
        self.session_world
    }
    /// A [weather, time of day] from the network as [`Self::set_session_world`] keeps it.
    pub(crate) fn clamp_world([weather, time]: [u32; 2]) -> [u32; 2] {
        [weather.min(WEATHER.len() as u32 - 1), time.min(crate::world_time::TIMES.len() as u32 - 1)]
    }
    /// online_moves: the session's weather and time of day (None after it). Values from the network are clamped.
    pub(crate) fn set_session_world(&mut self, world: Option<[u32; 2]>) {
        let world = world.map(Self::clamp_world);
        if world != self.session_world {
            (self.session_world, self.revision) = (world, self.revision + 1);
        }
    }
    /// [weather, time of day] in effect: the session's, else the player's own. Off the [`Self::seasons`] map a
    /// Halloween or Christmas reads as Clear; the choice itself is kept for that map.
    fn world(&self) -> [u32; 2] {
        let [weather, time] =
            self.session_world.unwrap_or([self.seasonal.unwrap_or(self.settings.weather), self.settings.time_of_day]);
        [if (weather as usize) < self.weathers() { weather } else { 0 }, time]
    }
    /// The WEATHER rows offered here: the seasons (the last rows) only on their map.
    fn weathers(&self) -> usize {
        if self.seasons { WEATHER.len() } else { crate::seasonal::Season::Halloween.weather() as usize }
    }
    /// Not retail (user 10-19): the [`crate::seasonal::Season`] the weather in effect picks, if any.
    pub(crate) fn season(&self) -> Option<crate::seasonal::Season> {
        crate::seasonal::Season::from_weather(self.world()[0])
    }
    /// A world_time::TIMES index.
    pub(crate) fn time_of_day(&self) -> u32 {
        self.world()[1]
    }
    pub(crate) fn diagnostic_settings(&self) -> String {
        format!("{:?}", self.settings)
    }
    /// Online, the panel refuses map loads. One a crossbar pick handed here (Change Location, a Challenge Map spot)
    /// also closes it: the player never chose this panel, and an open one stops the world (user 10-09: stuck after
    /// unpausing in online freeskate).
    fn refuse_map_load(&mut self, handed_off: bool) {
        self.status = "Leave multiplayer before switching maps".into();
        if handed_off {
            warn!("Map load refused online");
            self.open = false;
        }
    }
    pub(crate) fn transition_finished(&mut self, status: String, resume: bool) {
        self.status = status;
        self.open = !resume;
    }
    /// camera::STYLES index of the chosen camera.
    pub(crate) fn camera_style(&self) -> usize {
        self.settings.camera_style as usize
    }
    /// 0 when dry, 1 for the heaviest rain.
    pub(crate) fn rain(&self) -> f32 {
        WEATHER[self.world()[0] as usize].1
    }
    /// 0 without snow; any snow covers the world, 1 also has it falling.
    pub(crate) fn snow(&self) -> f32 {
        WEATHER[self.world()[0] as usize].2
    }
    /// The Fog setting's (opacity, half distance), or None to follow the weather. A local view choice: online too.
    pub(crate) fn fog(&self) -> Option<(f32, f32)> {
        FOG[self.settings.fog as usize].1
    }
    pub(crate) fn hud_visible(&self) -> bool {
        self.settings.hud
    }
    /// The game (1, 2 or 3) whose HUD shows on a map made for `map_game`.
    pub(crate) fn hud_game(&self, map_game: u8) -> u8 {
        HUD_STYLES[self.settings.hud_style as usize].1.unwrap_or(map_game)
    }
    /// The game whose screen effects show (None: the map's own).
    pub(crate) fn look(&self) -> Option<u8> {
        HUD_STYLES[self.settings.look as usize].1
    }
    #[cfg(test)]
    pub(crate) fn effects(&self) -> [bool; 5] {
        self.settings.effects
    }
    pub(crate) fn hall_of_meat(&self) -> bool {
        self.settings.hall_of_meat
    }
    pub(crate) fn vibration(&self) -> bool {
        self.settings.vibration
    }
    pub(crate) fn reduce_flashing(&self) -> bool {
        self.settings.reduce_flashing
    }
    pub(crate) fn fps_counter(&self) -> bool {
        self.settings.fps_counter
    }
    pub(crate) fn audio(&self) -> &crate::music::AudioSettings {
        &self.settings.audio
    }
    pub(crate) fn on_screen_help(&self) -> [bool; 2] {
        self.settings.on_screen_help
    }
    pub(crate) fn set_on_screen_help(&mut self, help: [bool; 2]) {
        if help != self.settings.on_screen_help {
            self.settings.on_screen_help = help;
            self.save_settings();
        }
    }
    /// Audio Settings / Music Player changes; saved when they differ.
    pub(crate) fn set_audio(&mut self, audio: crate::music::AudioSettings) {
        if audio != self.settings.audio {
            self.settings.audio = audio;
            self.save_settings();
        }
    }
    /// Radians added to the gameplay camera's vertical field of view.
    pub(crate) fn fov_offset(&self) -> f32 {
        (self.settings.fov_offset as f32).to_radians()
    }
    /// A SHADOW_QUALITIES index: 0 Low, 1 Medium, 2 High.
    pub(crate) fn shadow_quality(&self) -> u32 {
        self.settings.shadow_quality
    }
    /// The chosen Edit Skater room's native-backdrops package stem.
    pub(crate) fn edit_skater_room(&self) -> &str {
        &self.settings.edit_skater_room
    }
    /// Picker labels for `setting` and the index of its current value.
    pub(crate) fn choices(&self, setting: Setting) -> (Vec<String>, usize) {
        let s = &self.settings;
        let labels = |values: &[u32], label: fn(u32) -> String| {
            values.iter().map(|&v| label(v)).collect::<Vec<_>>()
        };
        match setting {
            Setting::DisplayMode => (
                DISPLAY_MODES.iter().map(|m| m.1.to_string()).collect(),
                DISPLAY_MODES
                    .iter()
                    .position(|m| m.0 == s.display_mode)
                    .unwrap_or(0),
            ),
            Setting::Resolution => (
                RESOLUTIONS
                    .iter()
                    .map(|(w, h)| format!("{w} x {h}"))
                    .collect(),
                position(RESOLUTIONS, (s.width, s.height)),
            ),
            Setting::Monitor => {
                let shown = &self.monitors[..self.monitors.len().min(7)];
                let at = shown.iter().position(|m| Some(&m.name) == s.monitor.as_ref()).map_or(0, |i| i + 1);
                (std::iter::once("Automatic".to_string()).chain(shown.iter().map(MonitorInfo::label)).collect(), at)
            }
            Setting::RenderScale => (
                labels(SCALES, |v| format!("{v}%")),
                position(SCALES, s.scale),
            ),
            Setting::Msaa => {
                let modes = self.aa_modes();
                (
                    modes
                        .iter()
                        .map(|&(post, samples)| {
                            if samples > 1 { format!("MSAA {samples}x") } else { POST_AA[post as usize].into() }
                        })
                        .collect(),
                    position(&modes, (s.post_aa, s.samples)),
                )
            }
            Setting::FpsLimit => (
                labels(LIMITS, |v| {
                    if v == 0 {
                        "Unlimited".into()
                    } else {
                        v.to_string()
                    }
                }),
                position(LIMITS, s.fps),
            ),
            Setting::VSync => (["Off", "On", "Fast"].map(String::from).to_vec(), usize::from(s.vsync) + usize::from(s.vsync && s.vsync_fast)),
            Setting::Occlusion => on_off(s.occlusion),
            Setting::Brightness => (
                labels(BRIGHTNESS, |v| format!("{v}%")),
                position(BRIGHTNESS, s.brightness),
            ),
            Setting::TimeOfDay => (
                crate::world_time::TIMES.map(String::from).to_vec(),
                self.world()[1] as usize,
            ),
            Setting::Weather => (
                WEATHER[..self.weathers()].iter().map(|w| w.0.to_string()).collect(),
                self.world()[0] as usize,
            ),
            Setting::Fog => (FOG.iter().map(|f| f.0.to_string()).collect(), s.fog as usize),
            // "Custom" while the rows match no preset.
            Setting::Preset => (
                std::iter::once("Custom").chain(PRESETS.iter().map(|p| p.0)).map(String::from).collect(),
                PRESETS.iter().position(|p| p.1 == s.look()).map_or(0, |i| i + 1),
            ),
            Setting::CameraShake => on_off(s.camera_shake),
            Setting::CameraAngle => (
                CAMERA_ANGLES.iter().map(|a| a.0.to_string()).collect(),
                // Skate 1 ignores Low/High, so it matches either.
                CAMERA_ANGLES
                    .iter()
                    .position(|&(_, style, low)| {
                        style == s.camera_style && (style == 2 || low == s.camera_low)
                    })
                    .unwrap_or(1),
            ),
            Setting::InvertX | Setting::InvertY => (
                vec!["Normal".into(), "Inverted".into()],
                usize::from(s.invert_look[usize::from(setting == Setting::InvertY)]),
            ),
            Setting::Hud => on_off(s.hud),
            Setting::HudStyle => (HUD_STYLES.iter().map(|h| h.0.to_string()).collect(), s.hud_style as usize),
            Setting::Look => (
                std::iter::once("Off").chain(HUD_STYLES.iter().map(|h| h.0)).map(String::from).collect(),
                if s.effects[0] { s.look as usize + 1 } else { 0 },
            ),
            Setting::Effect(i) => on_off(s.effects[i.min(4)]),
            Setting::TextureQuality => (["Low", "Medium", "High"].map(String::from).to_vec(), s.texture_quality as usize),
            Setting::DrawDistance => (DRAW_DISTANCES.iter().map(|d| d.0.to_string()).collect(), s.draw_distance as usize),
            Setting::EditSkaterRoom => (
                ROOMS.iter().map(|r| r.0.to_string()).collect(),
                ROOMS
                    .iter()
                    .position(|r| r.1 == s.edit_skater_room)
                    .unwrap_or(0),
            ),
            Setting::PlayerGraphics => (vec!["Hide".into(), "Show".into()], usize::from(s.player_graphics)),
            // One "choice": the slider's value (core_menu draws the bar while it moves).
            Setting::FieldOfView => (vec![self.fov_label()], 0),
            Setting::ShadowQuality => (
                SHADOW_QUALITIES.iter().map(|q| q.to_string()).collect(),
                s.shadow_quality as usize,
            ),
            Setting::AmbientOcclusion => on_off(s.ambient_occlusion),
            Setting::Anisotropy => (
                ANISOTROPY_LEVELS.iter().map(|&v| if v == 1 { "Off".into() } else { format!("{v}x") }).collect(),
                position(ANISOTROPY_LEVELS, s.anisotropy),
            ),
            Setting::Sharpening => (
                SHARPENING.iter().map(|s| s.0.to_string()).collect(),
                s.sharpening as usize,
            ),
            Setting::Dlss => (DLSS_MODES.map(String::from).to_vec(), s.dlss as usize),
            Setting::Vibration => on_off(s.vibration),
            Setting::HallOfMeat => on_off(s.hall_of_meat),
            // ID_GAMESETTINGS_UNITS_METRIC / _IMPERIAL, in the option's value order (0, 1).
            Setting::Units => (vec!["Metric".into(), "Imperial".into()], usize::from(s.imperial)),
            Setting::Subtitles => on_off(s.subtitles),
            Setting::DiscordStatus => on_off(s.discord_status),
            Setting::Chat => (CHAT_CHOICES.map(String::from).to_vec(), if !s.chat { 0 } else if s.chat_filter { 1 } else { 2 }),
            Setting::ChatFrom => (vec!["Everyone".into(), "Friends".into()], usize::from(s.chat_friends)),
            Setting::CustomModels => (vec!["Everyone".into(), "Friends".into(), "Nobody".into()], s.custom_models.min(2) as usize),
            Setting::ReduceFlashing => on_off(s.reduce_flashing),
            Setting::LowLatency => on_off(s.low_latency),
            Setting::FpsCounter => on_off(s.fps_counter),
            Setting::SkipIntro => on_off(s.skip_intro),
            Setting::VoiceChat => on_off(s.voice_chat),
            Setting::VoiceFrom => (vec!["Everyone".into(), "Friends".into()], usize::from(s.voice_friends_only)),
            Setting::MicMode => (vec!["Push to Talk".into(), "Open Mic".into()], usize::from(s.open_mic)),
            // One "choice", the key: core_menu sets it from the next one pressed.
            Setting::PushToTalkKey => (vec![key_label(&s.ptt_key)], 0),
            Setting::PushToTalkButton => (
                PTT_BUTTONS.iter().map(|b| b.0.to_string()).collect(),
                PTT_BUTTONS.iter().position(|b| b.1 == s.ptt_pad).unwrap_or(0),
            ),
            Setting::OpenMicSensitivity | Setting::MicVolume | Setting::VoiceVolume => (vec![self.slider(setting).1], 0),
            // Default and the first 7 devices (the Monitor row's cap); a saved one that is not plugged in reads Default.
            Setting::Microphone | Setting::OutputDevice => {
                let inputs = setting == Setting::Microphone;
                let names: &[String] = match &self.voice_devices {
                    Some((i, o)) => if inputs { i.as_slice() } else { o.as_slice() },
                    None => &[],
                };
                let shown = &names[..names.len().min(7)];
                let saved = if inputs { &s.microphone } else { &s.output_device };
                let at = shown.iter().position(|n| Some(n) == saved.as_ref()).map_or(0, |i| i + 1);
                (std::iter::once("Default (Windows)".to_string()).chain(shown.iter().cloned()).collect(), at)
            }
            Setting::MicTest => on_off(self.mic_test),
            Setting::NoiseSuppression => (vec!["Off".into(), "Standard".into(), "High".into()], usize::from(s.noise.min(2))),
            Setting::LowerGameSound => on_off(s.duck_for_voice),
        }
    }
    /// A slider's place in 0..=1 and its value line.
    pub(crate) fn slider(&self, setting: Setting) -> (f32, String) {
        let s = &self.settings;
        match setting {
            Setting::FieldOfView => (self.fov_fraction(), self.fov_label()),
            Setting::OpenMicSensitivity => (f32::from(s.open_mic_sensitivity.saturating_sub(1)) / 9., s.open_mic_sensitivity.to_string()),
            Setting::MicVolume => (f32::from(s.mic_volume) / 200., format!("{}%", s.mic_volume)),
            Setting::VoiceVolume => (f32::from(s.voice_volume) / 100., format!("{}%", s.voice_volume)),
            _ => (0., String::new()),
        }
    }
    /// Push to Talk Key's new key (core_menu takes it from the next press).
    pub(crate) fn set_ptt_key(&mut self, key: String) {
        self.settings.ptt_key = key;
        self.save_settings();
    }
    pub(crate) fn open_mic(&self) -> bool {
        self.settings.open_mic
    }
    pub(crate) fn open_mic_sensitivity(&self) -> u8 {
        self.settings.open_mic_sensitivity
    }
    /// Settings > Audio > Voice Chat as the voice thread takes it (voice.rs); a saved High runs as Standard where High
    /// would not keep up (skate_voice::HIGH_NEEDS_FMA).
    pub(crate) fn voice_config(&self) -> skate_voice::engine::Config {
        use skate_voice::dsp::{Mode, Noise};
        let s = &self.settings;
        let high = !skate_voice::HIGH_NEEDS_FMA || std::is_x86_feature_detected!("fma");
        skate_voice::engine::Config {
            enabled: s.voice_chat,
            mode: if s.open_mic { Mode::OpenMic { sensitivity: s.open_mic_sensitivity } } else { Mode::PushToTalk },
            noise: match s.noise {
                0 => Noise::Off,
                2 if high => Noise::High,
                _ => Noise::Standard,
            },
            mic_volume: s.mic_volume,
            voice_volume: s.voice_volume,
            mic_test: self.mic_test,
        }
    }
    /// Push to Talk Key: a KeyCode's Debug name, or "Mouse4" / "Mouse5".
    pub(crate) fn ptt_key(&self) -> &str {
        &self.settings.ptt_key
    }
    /// Settings' Microphone (None: Windows' default).
    pub(crate) fn microphone(&self) -> Option<&str> {
        self.settings.microphone.as_deref()
    }
    pub(crate) fn duck_for_voice(&self) -> bool {
        self.settings.duck_for_voice
    }
    /// Reads the microphones' and outputs' names again, on a thread: Game Settings or its Voice Chat page is opening.
    /// The lists so far stay until [`Self::take_voice_devices`] has the new ones.
    pub(crate) fn refresh_voice_devices(&mut self) {
        self.refresh_voice_devices_with(list_voice_devices);
    }
    fn refresh_voice_devices_with(&mut self, read: impl FnOnce() -> (Vec<String>, Vec<String>) + Send + 'static) {
        // A read done meanwhile (the one at start) is kept, not dropped with its channel.
        self.take_voice_devices();
        self.voice_devices_reading = read_on_a_thread(read);
    }
    /// Whether a read came in: its lists are [`Self::voice_devices`] now (an open settings page redraws its rows).
    pub(crate) fn take_voice_devices(&mut self) -> bool {
        let Some(reading) = self.voice_devices_reading.as_mut() else { return false };
        match reading.get_mut().unwrap_or_else(PoisonError::into_inner).try_recv() {
            Ok(lists) => {
                (self.voice_devices, self.voice_devices_reading) = (Some(lists), None);
                true
            }
            Err(TryRecvError::Empty) => false,
            // The thread ended without its lists (the device layer panicked): the lists so far stay.
            Err(TryRecvError::Disconnected) => {
                self.voice_devices_reading = None;
                false
            }
        }
    }
    /// Anti-Aliasing choices as (POST_AA index, MSAA samples): Off, FXAA, SMAA, then each supported MSAA count.
    fn aa_modes(&self) -> Vec<(u32, u32)> {
        (0..POST_AA.len() as u32)
            .map(|post| (post, 1))
            .chain(self.supported_msaa.iter().filter(|&&n| n > 1).map(|&n| (0, n)))
            .collect()
    }
    pub(crate) fn discord_status(&self) -> bool {
        self.settings.discord_status
    }
    /// "67 (Default)", "72 (+5)".
    pub(crate) fn fov_label(&self) -> String {
        match self.settings.fov_offset {
            0 => format!("{FOV_RETAIL} (Default)"),
            offset => format!("{} ({offset:+})", FOV_RETAIL + offset),
        }
    }
    /// The slider's position in 0..=1.
    pub(crate) fn fov_fraction(&self) -> f32 {
        (self.settings.fov_offset - FOV_RANGE.start()) as f32 / (FOV_RANGE.end() - FOV_RANGE.start()) as f32
    }
    /// Move a slider setting one step and save; false when it is already at that end.
    pub(crate) fn slide(&mut self, setting: Setting, step: i32) -> bool {
        let s = &mut self.settings;
        let moved = match setting {
            Setting::FieldOfView => {
                let offset = (s.fov_offset + step).clamp(*FOV_RANGE.start(), *FOV_RANGE.end());
                std::mem::replace(&mut s.fov_offset, offset) != offset
            }
            // Voice chat (user 10-26): Sensitivity a step at a time, the volumes 10 % at a time.
            Setting::OpenMicSensitivity => {
                let value = (i32::from(s.open_mic_sensitivity) + step).clamp(1, 10) as u8;
                std::mem::replace(&mut s.open_mic_sensitivity, value) != value
            }
            Setting::MicVolume => {
                let value = (i32::from(s.mic_volume) + 10 * step).clamp(0, 200) as u16;
                std::mem::replace(&mut s.mic_volume, value) != value
            }
            Setting::VoiceVolume => {
                let value = (i32::from(s.voice_volume) + 10 * step).clamp(0, 100) as u8;
                std::mem::replace(&mut s.voice_volume, value) != value
            }
            _ => false,
        };
        if moved {
            self.save_settings();
        }
        moved
    }
    /// Not retail (user 10-27: Performance Mode "should require a restart before its applied so ask people if they want
    /// to restart"): picking `index` on `setting` turns Performance mode on or off (core_menu asks first). Changes
    /// nothing.
    pub(crate) fn restarts(&self, setting: Setting, index: usize) -> bool {
        self.picked(setting, index).performance() != self.settings.performance()
    }
    /// Apply the picker choice `index` (as listed by `choices`) and save.
    pub(crate) fn choose(&mut self, setting: Setting, index: usize) {
        // In a session: the host changes the session's (not saved), a co-host asks the host to, other guests nothing.
        if let (Setting::Weather | Setting::TimeOfDay, Some(world)) = (setting, self.session_world.as_mut()) {
            if self.guest && !self.cohost {
                self.status = "The host sets the weather and time of day in an online session".into();
            } else {
                let at = usize::from(setting == Setting::TimeOfDay);
                world[at] = index as u32;
                let clamped = Some(*world);
                self.set_session_world(clamped);
                if self.guest {
                    self.world_request = self.session_world;
                }
            }
            return;
        }
        match setting {
            Setting::Weather => self.seasonal = None,
            // 0x8260DF40 pulses the pad when Vibration turns on.
            Setting::Vibration => self.vibration_buzz |= index == 1 && !self.settings.vibration,
            // Not saved: it runs while a page shows its row (core_menu::keeps_mic_test).
            Setting::MicTest => self.mic_test = index == 1,
            _ => {}
        }
        self.settings = self.picked(setting, index);
        self.save_settings();
    }
    /// The settings with the picker choice `index` (as listed by `choices`) on `setting`.
    fn picked(&self, setting: Setting, index: usize) -> GraphicsSettings {
        let aa = self.aa_modes().get(index).copied().unwrap_or((0, 1));
        let mut s = self.settings.clone();
        let on = index == 1;
        match setting {
            Setting::DisplayMode => {
                s.display_mode = DISPLAY_MODES[index.min(DISPLAY_MODES.len() - 1)].0
            }
            Setting::Resolution => {
                (s.width, s.height) = RESOLUTIONS[index.min(RESOLUTIONS.len() - 1)]
            }
            Setting::Monitor => s.monitor = index.checked_sub(1).and_then(|i| self.monitors.get(i)).map(|m| m.name.clone()),
            Setting::RenderScale => s.scale = SCALES[index.min(SCALES.len() - 1)],
            Setting::Msaa => (s.post_aa, s.samples) = aa,
            Setting::FpsLimit => s.fps = LIMITS[index.min(LIMITS.len() - 1)],
            Setting::VSync => (s.vsync, s.vsync_fast) = (index >= 1, index == 2),
            Setting::Occlusion => s.occlusion = on,
            Setting::Brightness => s.brightness = BRIGHTNESS[index.min(BRIGHTNESS.len() - 1)],
            Setting::TimeOfDay => s.time_of_day = index.min(crate::world_time::TIMES.len() - 1) as u32,
            Setting::Weather => s.weather = index.min(WEATHER.len() - 1) as u32,
            Setting::Fog => s.fog = index.min(FOG.len() - 1) as u32,
            Setting::Preset => {
                if let Some((_, look)) = index.checked_sub(1).and_then(|i| PRESETS.get(i)) {
                    // MSAA the GPU lacks falls back to none.
                    let samples = if self.supported_msaa.contains(&look.1.1) { look.1.1 } else { 1 };
                    s.set_look((look.0, (look.1.0, samples), look.2, look.3, look.4, look.5, look.6, look.7, look.8, look.9));
                }
            }
            Setting::CameraShake => s.camera_shake = on,
            Setting::CameraAngle => {
                let (_, style, low) = CAMERA_ANGLES[index.min(CAMERA_ANGLES.len() - 1)];
                (s.camera_style, s.camera_low) = (style, low);
            }
            Setting::InvertX => s.invert_look[0] = on,
            Setting::InvertY => s.invert_look[1] = on,
            Setting::Hud => s.hud = on,
            Setting::HudStyle => s.hud_style = index.min(HUD_STYLES.len() - 1) as u32,
            Setting::Look => {
                s.effects[0] = index > 0;
                if index > 0 {
                    s.look = (index - 1).min(HUD_STYLES.len() - 1) as u32;
                }
            }
            Setting::Effect(i) => s.effects[i.min(4)] = on,
            Setting::TextureQuality => s.texture_quality = index.min(2) as u32,
            Setting::DrawDistance => s.draw_distance = index.min(DRAW_DISTANCES.len() - 1) as u32,
            Setting::EditSkaterRoom => {
                s.edit_skater_room = ROOMS[index.min(ROOMS.len() - 1)].1.into()
            }
            Setting::PlayerGraphics => s.player_graphics = on,
            // A slider: `slide` moves it.
            Setting::FieldOfView => {}
            Setting::ShadowQuality => {
                s.shadow_quality = index.min(SHADOW_QUALITIES.len() - 1) as u32
            }
            Setting::AmbientOcclusion => s.ambient_occlusion = on,
            Setting::Anisotropy => s.anisotropy = ANISOTROPY_LEVELS[index.min(ANISOTROPY_LEVELS.len() - 1)],
            Setting::Sharpening => s.sharpening = index.min(SHARPENING.len() - 1) as u32,
            Setting::Dlss => s.dlss = index.min(DLSS_MODES.len() - 1) as u32,
            Setting::Vibration => s.vibration = on,
            Setting::HallOfMeat => s.hall_of_meat = on,
            Setting::Units => s.imperial = on,
            Setting::Subtitles => s.subtitles = on,
            Setting::DiscordStatus => s.discord_status = on,
            Setting::Chat => (s.chat, s.chat_filter) = (index >= 1, index != 2),
            Setting::ChatFrom => s.chat_friends = on,
            Setting::CustomModels => s.custom_models = index.min(2) as u32,
            Setting::ReduceFlashing => s.reduce_flashing = on,
            Setting::LowLatency => s.low_latency = on,
            Setting::FpsCounter => s.fps_counter = on,
            Setting::SkipIntro => s.skip_intro = on,
            Setting::VoiceChat => s.voice_chat = on,
            Setting::VoiceFrom => s.voice_friends_only = on,
            Setting::MicMode => s.open_mic = on,
            Setting::PushToTalkButton => s.ptt_pad = PTT_BUTTONS[index.min(PTT_BUTTONS.len() - 1)].1,
            Setting::Microphone | Setting::OutputDevice => {
                let inputs = setting == Setting::Microphone;
                let names = self.voice_devices.as_ref().map(|(i, o)| if inputs { i } else { o });
                let name = index.checked_sub(1).and_then(|i| names?.get(i)).cloned();
                *if inputs { &mut s.microphone } else { &mut s.output_device } = name;
            }
            Setting::NoiseSuppression => s.noise = index.min(2) as u8,
            Setting::LowerGameSound => s.duck_for_voice = on,
            // Mic Test is no setting (`choose`); the key comes from the next one pressed (set_ptt_key), the sliders
            // from `slide`.
            Setting::MicTest | Setting::PushToTalkKey | Setting::OpenMicSensitivity | Setting::MicVolume | Setting::VoiceVolume => {}
        }
        s
    }
    fn save_settings(&mut self) {
        self.revision += 1;
        if crate::hero_clip::running() {
            return; // a clip run's window settings are its own, never the player's graphics.json
        }
        let save = (|| -> Result<(), String> {
            crate::saves::write_atomic(
                &self.path,
                &serde_json::to_vec_pretty(&self.settings).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())
        })();
        self.status = match save {
            Ok(()) => {
                crate::autosave_tag::saving();
                "Saved".into()
            }
            Err(e) => {
                warn!("Game settings: could not save: {e}");
                format!("Could not save: {e}")
            }
        };
    }
}
/// Bevy's TimePlugin holds every Messages buffer until a FixedUpdate pass has seen it, and none runs while
/// Time<Virtual> is paused: a long offline pause kept every message written meanwhile (asset events from the animated
/// menus, input, sounds). Paused, nothing in FixedUpdate is waiting, so they clear each frame as without a fixed step.
fn release_messages_while_paused(time: Res<Time<Virtual>>, mut registry: ResMut<bevy::ecs::message::MessageRegistry>) {
    if time.is_paused() {
        registry.should_update = bevy::ecs::message::ShouldUpdateMessages::Ready;
    }
}
pub(crate) fn gameplay_active(menu: Option<Res<Menu>>) -> bool {
    menu.is_none_or(|m| !m.open)
}
/// The world simulates: no menu, or a menu online, where nothing pauses (user 10-24, build 10: the skater froze in
/// place on the other players' screens while its owner had the Start menu up, the big desync).
pub(crate) fn simulation_active(menu: Option<Res<Menu>>, net: Option<Res<crate::multiplayer::Multiplayer>>) -> bool {
    menu.is_none_or(|m| !m.open) || net.is_some_and(|n| n.active())
}
/// The 3D world's render image, shown by the presentation camera.
#[derive(Resource)]
pub(crate) struct SceneTarget(pub(crate) Handle<Image>);
#[derive(Resource)]
pub(crate) struct FramePacer(Instant);
/// Just-in-time frame start (perf 10-18), see [`just_in_time`]. app.rs inserts it unless SKATE_JIT=0 (benchmarks).
#[derive(Resource)]
pub(crate) struct JustInTime {
    /// Rendering on its own thread: the frame's idle wait is for that thread, else for a swapchain image.
    pipelined: bool,
    delay: Duration,
}
impl JustInTime {
    pub(crate) fn new(pipelined: bool) -> Self {
        Self { pipelined, delay: Duration::ZERO }
    }
}
/// The presented world image; Screen Brightness tints it.
#[derive(Component)]
pub(crate) struct SceneView;
#[derive(Component)]
struct MenuRoot;
#[derive(Component)]
pub(crate) struct MenuRow(usize);
#[derive(Component)]
struct MenuLabel(usize);
#[derive(Component)]
struct StatusLabel;

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct MenuInput;

/// The presentation camera must exist before overlays select their UI target.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct PresentationSetup;

pub(crate) struct GraphicsMenuPlugin;
impl Plugin for GraphicsMenuPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(FramePacer(Instant::now()))
            .init_resource::<crate::world_time::WorldTime>()
            // The first map loads at Startup (world::spawn) and reads ANISOTROPY.
            .add_systems(PreStartup, |config: Res<crate::config::Config>, adapter: Res<RenderAdapter>| {
                publish(&load_settings(&config, &adapter).1)
            })
            .add_systems(PostStartup, setup.in_set(PresentationSetup))
            .add_systems(First, release_messages_while_paused.before(bevy::ecs::message::MessageUpdateSystems))
            .add_systems(
                PreUpdate,
                interact.in_set(MenuInput).after(bevy::input::InputSystems),
            )
            .add_systems(
                Update,
                (crate::world_time::tick, crate::map_render::advance_day, list_monitors, apply, prop_ranges, labels).chain(),
            )
            // Not retail (user 10-19): Performance mode's textures and shadows.
            .add_systems(Update, (performance_reload, performance_shadows).after(apply))
            .add_systems(PostUpdate, performance_textures.after(bevy::asset::AssetEventSystems))
            .add_systems(
                PostUpdate,
                crate::map_render::position_celestial_bodies
                    .before(bevy::transform::TransformSystems::Propagate),
            )
            // Before PreUpdate polls the pads: a capped frame samples input after its sleep, not before it
            // (perf 10-10: sleeping in Last added up to a frame of pad latency whenever the cap bit).
            .add_systems(First, pace);
    }
}
fn load_settings(config: &crate::config::Config, adapter: &RenderAdapter) -> (PathBuf, GraphicsSettings) {
    let path = crate::saves::settings_dir(&config.asset_root).join("graphics.json");  // Not retail (user 10-13)
    let settings = match crate::saves::load_json::<GraphicsSettings>(&path) {
        crate::saves::Loaded::Loaded(s) | crate::saves::Loaded::Restored(s, _) => s,
        crate::saves::Loaded::Missing => GraphicsSettings::first_run(adapter.get_info().device_type),
        crate::saves::Loaded::Damaged(_) | crate::saves::Loaded::Unreadable => GraphicsSettings::default(),
    }
    .validated();
    (path, settings)
}
/// Settings > Low Latency, read before the app is built: the render plugins can't change once it runs.
pub(crate) fn low_latency(config: &crate::config::Config) -> bool {
    // The first read of graphics.json: a damaged one is restored here, before load_settings reads it again.
    crate::saves::load_json::<GraphicsSettings>(&crate::saves::settings_dir(&config.asset_root).join("graphics.json"))
        .ok()
        .is_some_and(|s| s.low_latency)
}
/// Settings > Gameplay > Skip Intro, read as the boot screens set up (boot.rs), before the menu's own load.
pub(crate) fn skip_intro(config: &crate::config::Config) -> bool {
    crate::saves::load_json::<GraphicsSettings>(&crate::saves::settings_dir(&config.asset_root).join("graphics.json"))
        .ok()
        .is_some_and(|s| s.skip_intro)
}
/// Settings > Audio > Output Device, read before the app is built: bevy_audio opens its output once, as it starts.
pub(crate) fn output_device(config: &crate::config::Config) -> Option<String> {
    crate::saves::load_json::<GraphicsSettings>(&crate::saves::settings_dir(&config.asset_root).join("graphics.json"))
        .ok()
        .and_then(|s| s.output_device)
}
/// Settings > World's post effects on Skate 1 and 2 maps (retail_grading.rs gates its params by them): label and help.
pub(crate) const EFFECTS: [(&str, &str); 5] = [
    ("Colour Grading", "The map's own colour look (Skate 1 and 2 maps change it by district), or Skate 3's, 2's or 1's on every map. Only you see yours"),
    ("Bloom", "Skate 1 and 2 maps: the glow around bright light; Skate 3 cutscenes' flashes"),
    ("Lens & Vignette", "Skate 1 and 2 maps: fisheye lens, dark corners and Skate 1's film grunge"),
    ("Depth of Field", "Skate 1 maps: the far distance softly blurred"),
    ("Motion Blur", "Skate 1 maps: blur at speed"),
];
/// Draw Distance: label, the camera's far plane, how far movable props draw (None: as far as the camera) and the
/// remote-skater cull, in metres. High is the view before the setting existed (Bevy's 1000 m far, no prop range,
/// skaters to 400 m). The static map's 512 m cells are only cut by the far plane: ranging them would drop whole
/// blocks. Collision never reads any of it. ponytail: no LOD meshes exist (the .skate maps carry HighLOD only).
pub(crate) const DRAW_DISTANCES: [(&str, f32, Option<f32>, f32); 3] =
    [("Low", 400., Some(120.), 150.), ("Medium", 650., Some(250.), 250.), ("High", 1000., None, 400.)];
pub(crate) static TEXTURE_QUALITY: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(2);
/// How many times map textures are halved at load: High 0, Medium 1, Low 2.
/// Not retail (user 10-19): Performance mode halves until the larger side is under 128 (`shrink` stops at 64).
pub(crate) fn texture_steps() -> u32 {
    if performance() {
        return 16;
    }
    2 - u32::from(TEXTURE_QUALITY.load(Ordering::Relaxed).min(2))
}
/// Not retail (user 10-19): Performance mode ([`GraphicsSettings::performance`]), for loads outside the ECS.
pub(crate) static PERFORMANCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub(crate) fn performance() -> bool {
    PERFORMANCE.load(Ordering::Relaxed)
}
pub(crate) static DRAW_DISTANCE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(2);
/// The [`DRAW_DISTANCES`] row in use.
/// Not retail (user 10-19): Performance mode pulls Low's far plane in to 300 m and remote skaters to 120 m.
pub(crate) fn draw_distance() -> (&'static str, f32, Option<f32>, f32) {
    if performance() {
        return ("Performance", 300., DRAW_DISTANCES[0].2, 120.);
    }
    DRAW_DISTANCES[(DRAW_DISTANCE.load(Ordering::Relaxed) as usize).min(2)]
}
/// Not retail (user 10-19): Performance mode's cap on a texture outside the UI (loaded skater, board and traffic
/// textures, the sky): RGBA8 halved until its larger side is under 128, with a box-filtered mip chain and linear
/// filtering, so it reads as very low resolution and smooth. Returns whether it changed. Compressed, array and
/// render-target images, and ones already that small, are left alone (the map's go through `texture_steps`).
pub(crate) fn performance_texture(image: &mut Image) -> bool {
    performance() && cap_texture(image)
}
fn cap_texture(image: &mut Image) -> bool {
    use bevy::render::render_resource::{TextureDimension, TextureUsages};
    let d = &image.texture_descriptor;
    let (w, h) = (d.size.width, d.size.height);
    let top = (w * h * 4) as usize;
    if !matches!(d.format, TextureFormat::Rgba8Unorm | TextureFormat::Rgba8UnormSrgb)
        || d.dimension != TextureDimension::D2
        || d.size.depth_or_array_layers != 1
        || d.usage.contains(TextureUsages::RENDER_ATTACHMENT)
        || w.max(h) < 128
        || image.data.as_ref().is_none_or(|data| data.len() < top)
    {
        return false;
    }
    // The top level only: a chain made before (customiser_parts::mip) is rebuilt from it.
    let (rgba, w, h) = crate::retail_render::shrink(&image.data.as_ref().unwrap()[..top], w, h, 1, 16);
    let (bytes, levels) = crate::retail_render::mip_chain(&rgba, w, h, 1);
    image.data = Some(bytes);
    image.texture_descriptor.size = Extent3d { width: w, height: h, depth_or_array_layers: 1 };
    image.texture_descriptor.mip_level_count = levels;
    match &mut image.sampler {
        ImageSampler::Descriptor(s) => {
            use bevy::image::ImageFilterMode::Linear;
            (s.mag_filter, s.min_filter, s.mipmap_filter) = (Linear, Linear, Linear);
        }
        sampler => *sampler = ImageSampler::linear(),
    }
    true
}
/// Not retail (user 10-19): an asset-server image is a world texture (a skater part's map, a glTF model's texture)
/// unless it is the one UI picture loaded by path (custom_models' preview cards); the UI's own images are built in
/// code and have no path.
fn world_texture(path: &bevy::asset::AssetPath) -> bool {
    !path.path().ends_with("preview.png")
}
/// Not retail (user 10-19): Performance mode caps every world texture the asset server loads, as it arrives and
/// before extraction uploads it (and, for GPU-only images, drops the RAM copy).
fn performance_textures(
    mut events: MessageReader<AssetEvent<Image>>,
    menu: Res<Menu>,
    server: Res<AssetServer>,
    mut images: ResMut<Assets<Image>>,
) {
    if !menu.settings.performance() {
        events.clear();
        return;
    }
    for event in events.read() {
        let (AssetEvent::Added { id } | AssetEvent::Modified { id }) = *event else { continue };
        if server.get_path(id).is_some_and(|p| world_texture(&p))
            && let Some(image) = images.get_mut_untracked(id)
        {
            cap_texture(image);
        }
    }
}
/// Not retail (user 10-19): entering or leaving Performance mode reloads the world image files already loaded
/// (skater and board maps), so `performance_textures` caps them, or they come back whole, at once. glTF-embedded
/// textures and the map's change on their next load: reloading a glTF would respawn its scenes.
fn performance_reload(
    menu: Res<Menu>,
    server: Res<AssetServer>,
    images: Res<Assets<Image>>,
    mut was: Local<Option<bool>>,
) {
    let on = menu.settings.performance();
    if was.replace(on).is_none_or(|was| was == on) {
        return;
    }
    let paths: std::collections::HashSet<_> = images
        .ids()
        .filter_map(|id| server.get_path(id))
        .filter(|p| p.label().is_none() && world_texture(p))
        .map(|p| p.into_owned())
        .collect();
    info!("Performance mode {}: reloading {} textures", if on { "on" } else { "off" }, paths.len());
    for path in paths {
        server.reload(path);
    }
}
/// Not retail (user 10-19): Performance mode turns every shadow map off (the character visibility, player, rain
/// shelter and custom-map sun lights): the shaders then skip their shadow taps. Leaving it turns them back on.
#[derive(Component)]
struct PerformanceShadowOff;
fn performance_shadows(
    mut commands: Commands,
    mut lights: Query<(Entity, &mut DirectionalLight, Has<PerformanceShadowOff>)>,
) {
    let on = performance();
    for (entity, mut light, off) in &mut lights {
        if on && light.shadows_enabled {
            light.shadows_enabled = false;
            commands.entity(entity).insert(PerformanceShadowOff);
        } else if !on && off {
            light.shadows_enabled = true;
            commands.entity(entity).remove::<PerformanceShadowOff>();
        }
    }
}
/// Movable props fade out past the Draw Distance's prop range (hidden, never despawned: their physics runs on).
fn prop_ranges(
    mut commands: Commands,
    menu: Res<Menu>,
    added: Query<Entity, Added<crate::physics::props::PropPiece>>,
    all: Query<Entity, With<crate::physics::props::PropPiece>>,
    mut applied: Local<Option<u32>>,
) {
    use bevy::camera::visibility::VisibilityRange;
    let level = menu.settings.draw_distance;
    let pieces: Vec<Entity> = if *applied != Some(level) { all.iter().collect() } else { added.iter().collect() };
    *applied = Some(level);
    let end = DRAW_DISTANCES[(level as usize).min(2)].2;
    for piece in pieces {
        match end {
            // Pieces sit at the map origin with their shape in the mesh: range by the box's centre.
            Some(end) => commands.entity(piece).insert(VisibilityRange { start_margin: 0.0..0.0, end_margin: end..end + 10., use_aabb: true }),
            None => commands.entity(piece).remove::<VisibilityRange>(),
        };
    }
}
/// [`GraphicsSettings::effects`] as bits (1 << row), for the render world.
pub(crate) static EFFECT_BITS: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0b1_1111);
/// Settings read outside the ECS: map texture loads and the camera rig.
fn publish(settings: &GraphicsSettings) {
    DRAW_DISTANCE.store(settings.draw_distance.min(2) as u8, Ordering::Relaxed);
    TEXTURE_QUALITY.store(settings.texture_quality.min(2) as u8, Ordering::Relaxed);
    PERFORMANCE.store(settings.performance(), Ordering::Relaxed); // Not retail (user 10-19): Performance mode
    EFFECT_BITS.store(settings.effects.iter().rev().fold(0, |bits, &on| bits << 1 | u8::from(on)), Ordering::Relaxed);
    ANISOTROPY.store(settings.anisotropy, Ordering::Relaxed);
    crate::camera::SHAKE.store(settings.camera_shake, Ordering::Relaxed);
    crate::custom_graphic::SHOW_OTHERS.store(settings.player_graphics, Ordering::Relaxed);
    crate::hud_runtime::IMPERIAL.store(settings.imperial, Ordering::Relaxed);
    crate::retail_ui::SUBTITLES.store(settings.subtitles, Ordering::Relaxed);
    crate::player_list::CHAT.store(settings.chat, Ordering::Relaxed);
    crate::player_list::CHAT_FILTER.store(settings.chat_filter, Ordering::Relaxed);
    crate::player_list::CHAT_FRIENDS.store(settings.chat_friends, Ordering::Relaxed);
    crate::player_list::CUSTOM_MODELS.store(settings.custom_models.min(2) as u8, Ordering::Relaxed);
    // Voice chat (user 10-26): whose voices are heard, and the pad's talk button (none while Voice Chat is Off).
    crate::player_list::VOICE.store(settings.voice_chat, Ordering::Relaxed);
    crate::player_list::VOICE_FRIENDS.store(settings.voice_friends_only, Ordering::Relaxed);
    crate::input::platform::PTT_PAD.store(if settings.voice_chat { settings.ptt_pad } else { 0 }, Ordering::Relaxed);
}
fn setup(
    mut commands: Commands,
    config: Res<crate::config::Config>,
    mut images: ResMut<Assets<Image>>,
    mut window: Single<&mut Window, With<PrimaryWindow>>,
    cameras: Query<Entity, With<Camera3d>>,
    adapter: Res<RenderAdapter>,
    mut time: ResMut<Time<Virtual>>,
) {
    let (path, settings) = load_settings(&config, &adapter);
    let supported_msaa: Vec<_> = [1, 2, 4, 8]
        .into_iter()
        .filter(|&samples| {
            [
                // perf 10-19: Rg11b10Ufloat under SKATE_HDR_RG11B10=1 (vendor/README.md).
                bevy::render::view::ViewTarget::hdr_format(),
                TextureFormat::Rgba8UnormSrgb,
                TextureFormat::Depth32Float,
            ]
            .into_iter()
            .all(|format| {
                adapter
                    .get_texture_format_features(format)
                    .flags
                    .sample_count_supported(samples)
            })
        })
        .collect();
    let mut settings = settings;
    // Reproducible A/B override; normal launches use the saved menu setting.
    match std::env::var("SKATE_OCCLUSION").as_deref() {
        Ok("0") => settings.occlusion = false,
        Ok("1") => settings.occlusion = true,
        _ => {}
    }
    if !supported_msaa.contains(&settings.samples) {
        settings.samples = 1;
    }
    if crate::hero_clip::is_active(&config) {
        // the hidden clip run: a small window whatever the player saved (in memory only; menus are what save)
        settings.display_mode = DisplayMode::Windowed;
        (settings.width, settings.height) = (480, 360);
        settings.vsync = false; // a hidden window's vsync present can stall the frame loop
    }
    // perf 10-18: SKATE_FRAME_LATENCY=1|2 sets the swapchain's frame latency (wgpu's default 2). Read once by Bevy, at
    // the window's first extraction, so before the first frame.
    if let Some(latency) = std::env::var("SKATE_FRAME_LATENCY").ok().and_then(|v| v.parse().ok()).and_then(std::num::NonZeroU32::new) {
        window.desired_maximum_frame_latency = Some(latency);
    }
    // A resize queued with a fullscreen mode would shrink the fullscreen window.
    if settings.display_mode == DisplayMode::Windowed {
        window
            .resolution
            .set_physical_resolution(settings.width, settings.height);
    }
    let size = settings.internal_size(window.physical_size());
    let mut image = Image::new_target_texture(size.x, size.y, TextureFormat::Rgba8UnormSrgb, None);
    image.data = None; // GPU-drawn: no CPU copy (perf 10-18, see apt_canvas::Canvas::new)
    image.copy_on_resize = false;
    image.sampler = ImageSampler::linear();
    let target = images.add(image);
    for camera in &cameras {
        commands.entity(camera).insert((
            RenderTarget::Image(target.clone().into()),
            msaa(settings.samples),
        ));
        // Render the world before the presentation camera consumes its image.
        commands.entity(camera).insert(Camera {
            order: -1,
            ..default()
        });
    }
    let output = commands
        .spawn((Camera2d, Msaa::Off, IsDefaultUiCamera))
        .id();
    // perf 10-18: it draws only UI (bevy_ui ignores RenderLayers). On layer 0 every map batch was frustum-tested
    // against it and listed in its VisibleEntities for extraction. SKATE_PRESENT_LAYER0=1 restores that.
    if !std::env::var("SKATE_PRESENT_LAYER0").is_ok_and(|v| v == "1") {
        commands.entity(output).insert(bevy::camera::visibility::RenderLayers::none());
    }
    commands.spawn((
        SceneView,
        Node {
            width: percent(100),
            height: percent(100),
            position_type: PositionType::Absolute,
            ..default()
        },
        ImageNode::new(target.clone()),
        UiTargetCamera(output),
    ));
    commands.spawn((MenuRoot, UiTargetCamera(output), GlobalZIndex(10), Node {
        display: Display::None, width:percent(100), height:percent(100), align_items:AlignItems::Center,
        justify_content:JustifyContent::Center, position_type:PositionType::Absolute, ..default()
    }, BackgroundColor(Color::srgba(0.015,0.025,0.04,0.88)))).with_children(|root| {
        root.spawn((Node { width:px(560),max_width:percent(95),padding:UiRect::all(px(18)),flex_direction:FlexDirection::Column,row_gap:px(4),border_radius:BorderRadius::all(px(12)),..default() },
            BackgroundColor(Color::srgb(0.035,0.055,0.08)))).with_children(|panel| {
            panel.spawn((Text::new("GAME MENU"),TextFont {font_size:32.,..default()},TextColor(Color::WHITE)));
            panel.spawn((Text::new("GAMEPLAY & GRAPHICS"),TextFont {font_size:16.,..default()},TextColor(Color::srgb(0.4,0.85,0.85))));
            for i in 0..17 {
                panel.spawn((Button, MenuRow(i), Node {width:percent(100),min_height:px(26),padding:UiRect::all(px(3)),align_items:AlignItems::Center,border_radius:BorderRadius::all(px(5)),..default()},
                    BackgroundColor(Color::srgb(0.08,0.11,0.15)))).with_children(|row| {
                    row.spawn((MenuLabel(i),Text::new(""),TextFont {font_size:18.,..default()},TextColor(Color::WHITE)));
                });
            }
            panel.spawn((StatusLabel,Text::new(""),TextFont {font_size:15.,..default()},TextColor(Color::srgb(0.65,0.75,0.8))));
            panel.spawn((Text::new("Click to cycle | Up/Down select | Left/Right change\nEsc resume | Changes save automatically"),TextFont {font_size:14.,..default()},TextColor(Color::srgb(0.65,0.75,0.8))));
        });
    });
    commands.insert_resource(SceneTarget(target));
    let maps = crate::map_library::discover(&config.asset_root);
    let selected_map = maps
        .iter()
        .position(|m| m.path.as_ref() == config.map_path.as_ref())
        .unwrap_or(0);
    if config.start_paused {
        time.pause();
    }
    let seasonal = (settings.weather == 0).then(crate::seasonal::today).flatten().map(|s| s.weather());
    let gameplay = crate::skate2_physics::settings::Gameplay::load(&config.asset_root);
    commands.insert_resource(Menu {
        open: config.start_paused,
        selected: 0,
        settings,
        path,
        supported_msaa,
        difficulty: config.difficulty,
        physics: gameplay.physics,
        skate2_gestures: gameplay.skate2_gestures,
        status: String::new(),
        maps,
        selected_map,
        multiplayer: false,
        browser: false,
        crossbar: false,
        pending: None,
        handed_off: false,
        guest: false,
        cohost: false,
        world_request: None,
        session_world: None,
        seasonal,
        vibration_buzz: false,
        revision: 0,
        monitors: Vec::new(),
        dlss_supported: false,
        on_nsv: false,
        seasons: false,
        mic_test: false,
        mic_level_dbfs: -120.,
        voice_devices: None,
        voice_devices_reading: read_on_a_thread(list_voice_devices),
        mic_error: None,
    });
}
/// The microphones' and outputs' names (tens of ms, more with Bluetooth or USB devices).
fn list_voice_devices() -> (Vec<String>, Vec<String>) {
    (skate_voice::device::inputs(), skate_voice::device::outputs())
}
/// `read` on its own thread (review 10-27: never on the main one, whose world never pauses online).
fn read_on_a_thread(read: impl FnOnce() -> (Vec<String>, Vec<String>) + Send + 'static) -> Option<Mutex<Receiver<(Vec<String>, Vec<String>)>>> {
    let (send, reading) = mpsc::sync_channel(1);
    let thread = std::thread::Builder::new().name("voice devices".into()).spawn(move || {
        let _ = send.send(read());
    });
    match thread {
        Ok(_) => Some(Mutex::new(reading)),
        Err(error) => {
            error!("Voice chat: no thread to list the devices: {error}");
            None
        }
    }
}
fn msaa(samples: u32) -> Msaa {
    match samples {
        2 => Msaa::Sample2,
        4 => Msaa::Sample4,
        8 => Msaa::Sample8,
        _ => Msaa::Off,
    }
}
fn cycle<T: PartialEq + Copy>(values: &[T], value: T, direction: i32) -> T {
    let index = values.iter().position(|x| *x == value).unwrap_or(0) as i32;
    values[(index + direction).rem_euclid(values.len() as i32) as usize]
}
pub(crate) fn interact(
    mut config: ResMut<crate::config::Config>,
    mut transition: ResMut<crate::map_transition::MapTransition>,
    mut customiser: ResMut<crate::customiser::Customiser>,
    mut custom_models: ResMut<crate::custom_models::CustomModels>,
    (mut mods, panel): (
        ResMut<crate::modding::ModMenu>,
        Res<crate::modding::EnabledPanel>,
    ),
    nav: Res<crate::customiser::Navigation>,
    mut physics: ResMut<crate::physics::GamePhysics>,
    keys: Res<ButtonInput<KeyCode>>,
    mut menu: ResMut<Menu>,
    mut time: ResMut<Time<Virtual>>,
    buttons: Query<(&Interaction, &MenuRow), Changed<Interaction>>,
    (mut exit, mut sounds): (MessageWriter<AppExit>, MessageWriter<crate::skate_audio::FrontEndSoundRequest>),
    mut net: ResMut<crate::multiplayer::Multiplayer>,
    mut typing: MessageReader<bevy::input::keyboard::KeyboardInput>,
    mut travel: ResMut<crate::teleport_menu::Travel>,
) {
    if transition.busy() {
        menu.open = true;
        time.pause();
        return;
    }
    if mods.open
        || travel.open
        || travel.closed_this_frame
        || customiser.open
        || custom_models.open
        || menu.crossbar
    {
        return;
    }
    let handed_off = std::mem::take(&mut menu.handed_off);
    let mut action = menu.pending.take();
    // Start only closes this panel (a crossbar hand-off opened it). The Skate 3 crossbar is the one Start menu: a
    // press it refuses (a match that blocks pause, Party Play's turns) opens nothing here.
    if !handed_off && menu.open && (keys.just_pressed(KeyCode::Escape) || nav.pressed & 0x10 != 0) {
        menu.open = false;
    }
    for event in typing.read() {
        if !menu.open
            || !menu.multiplayer
            || menu.browser
            || menu.selected != 3
            || !event.state.is_pressed()
        {
            continue;
        }
        if event.key_code == KeyCode::Backspace {
            net.join_code.pop();
        }
        if let Some(text) = &event.text {
            for ch in text.chars().filter(|c| c.is_ascii_hexdigit() || *c == '-') {
                if net.join_code.len() < 40 {
                    net.join_code.push(ch);
                }
            }
        }
    }
    if menu.open && !handed_off {
        let rows = if menu.multiplayer {
            11
        } else {
            17
        };
        if !panel.focused {
            // Retail's list screens: `core_nav_up/down/left/right` and `core_a_button`
            // (`GlobalFEPlaySound` ids; see `core_menu_runtime` for the crossbar's own).
            let mut play = |id: u64| {
                sounds.write(crate::skate_audio::FrontEndSoundRequest(id));
            };
            if keys.just_pressed(KeyCode::ArrowUp) || nav.pressed & 1 != 0 {
                menu.selected = (menu.selected + rows - 1) % rows;
                play(0x73D7_CC18_FE21_DBE9);
            }
            if keys.just_pressed(KeyCode::ArrowDown) || nav.pressed & 2 != 0 {
                menu.selected = (menu.selected + 1) % rows;
                play(0x7F97_5D2B_6425_9AC2);
            }
            if keys.just_pressed(KeyCode::ArrowLeft) || nav.pressed & 4 != 0 {
                action = Some((menu.selected, -1));
                play(0xAD8B_F9A0_B499_7B49);
            }
            if keys.just_pressed(KeyCode::ArrowRight) || nav.pressed & 8 != 0 {
                action = Some((menu.selected, 1));
                play(0xD8EE_38A6_1C45_EA28);
            } else if keys.just_pressed(KeyCode::Enter) || nav.pressed & 0x1000 != 0 {
                action = Some((menu.selected, 1));
                play(0x373D_38D7_EA16_7A71);
            }
        }
        for (interaction, row) in &buttons {
            if panel.dragging() {
                continue;
            }
            if *interaction == Interaction::Pressed {
                menu.selected = row.0;
                action = Some((row.0, 1));
            }
        }
    }
    // A picker setting one step along its choices (saved by `choose`).
    let step = |menu: &mut Menu, setting: Setting, direction: i32| {
        let (choices, index) = menu.choices(setting);
        menu.choose(setting, (index as i32 + direction).rem_euclid(choices.len() as i32) as usize);
    };
    if let Some((row, direction)) = action {
        if menu.browser {
            match row {
                0 => {
                    net.browse(0);
                }
                1..=5 => net.join_row(row - 1),
                6 => {
                    let page = net.browser_page.saturating_sub(1);
                    net.browse(page);
                }
                7 => {
                    let page = net.browser_page + 1;
                    if page * 5 < net.browser_total {
                        net.browse(page);
                    }
                }
                8 => menu.open = false,
                9 => {
                    exit.write(AppExit::Success);
                }
                10 => {
                    menu.browser = false;
                    menu.selected = 6;
                }
                _ => {}
            }
        } else if menu.multiplayer {
            match row {
                0 => net.local(true),
                1 => net.local(false),
                2 => net.steam(true),
                3 => {}
                4 => net.steam(false),
                5 => net.leave(),
                6 => {
                    net.browse(0);
                    menu.browser = true;
                    menu.selected = 0;
                }
                8 => menu.open = false,
                9 => {
                    exit.write(AppExit::Success);
                }
                10 => {
                    menu.multiplayer = false;
                    menu.selected = 11;
                }
                _ => {}
            }
        } else {
            match row {
                0 => {
                    let size = cycle(
                        RESOLUTIONS,
                        (menu.settings.width, menu.settings.height),
                        direction,
                    );
                    (menu.settings.width, menu.settings.height) = size;
                }
                1 => menu.settings.scale = cycle(SCALES, menu.settings.scale, direction),
                2 => step(&mut menu, Setting::Msaa, direction),
                3 => menu.settings.fps = cycle(LIMITS, menu.settings.fps, direction),
                4 => menu.settings.occlusion = !menu.settings.occlusion,
                5 => {
                    let difficulty = cycle(&Difficulty::ALL, menu.difficulty, direction);
                    menu.set_difficulty(difficulty, &mut physics, &mut config);
                }
                6 => {
                    menu.selected_map = (menu.selected_map as i32 + direction)
                        .rem_euclid(menu.maps.len() as i32)
                        as usize;
                    menu.status = "Choose Load map to switch".into();
                }
                7 => {
                    if net.active() {
                        menu.refuse_map_load(handed_off);
                    } else {
                        net.leave();
                        transition.request(menu.maps[menu.selected_map].clone());
                        menu.status = "Loading map...".into();
                    }
                }
                8 => menu.open = false,
                9 => {
                    exit.write(AppExit::Success);
                }
                10 => {
                    custom_models.request_stock();
                    customiser.begin();
                }
                11 => {
                    menu.multiplayer = true;
                    menu.selected = 0;
                }
                // A swapped model would desync what other players see.
                12 if net.active() => menu.status = "Custom models can't be changed in an online session".into(),
                12 => custom_models.begin(),
                13 => menu.status = "Updates are handled by the launcher".into(),
                14 => travel.open = true,
                15 => mods.begin(),
                16 => step(&mut menu, Setting::TimeOfDay, direction),
                _ => {}
            }
        }
        if row < 5 && !menu.multiplayer && !menu.browser {
            menu.save_settings();
        }
    }
    if menu.open && !net.active() {
        time.pause();
    } else {
        time.unpause();
    }
}
pub(crate) fn apply(
    mut commands: Commands,
    menu: Res<Menu>,
    mut window: Single<&mut Window, With<PrimaryWindow>>,
    target: Res<SceneTarget>,
    mut images: ResMut<Assets<Image>>,
    mut cameras: Query<(Entity, &mut Msaa), With<Camera3d>>,
    mut scene: Query<&mut ImageNode, With<SceneView>>,
    physics: Option<ResMut<crate::physics::GamePhysics>>,
    mut previous: Local<Option<GraphicsSettings>>,
    monitors: Query<(Entity, &Monitor, Has<PrimaryMonitor>)>,
    mut chosen: Local<Option<Entity>>,
    mut last_dlss: Local<Option<u32>>,
) {
    let s = &menu.settings;
    // DLSS (always 0 without the dlss feature) replaces MSAA, the post AA passes and SSAO; dlss.rs adds its camera parts.
    let dlss = menu.dlss();
    let dlss_changed = last_dlss.replace(dlss) != Some(dlss);
    // An unplugged choice falls back to the window's own display.
    let display = s.monitor.as_ref().and_then(|name| monitors.iter().find(|(_, m, _)| m.name.as_ref() == Some(name))).map(|(e, ..)| e);
    if previous.as_ref().is_none_or(|p| {
        p.width != s.width || p.height != s.height || p.display_mode != s.display_mode
    }) || *chosen != display
    {
        let selection = display.map_or(MonitorSelection::Current, MonitorSelection::Entity);
        window.mode = match s.display_mode {
            DisplayMode::Windowed => WindowMode::Windowed,
            DisplayMode::Borderless => WindowMode::BorderlessFullscreen(selection),
            DisplayMode::Fullscreen => WindowMode::Fullscreen(
                selection,
                fullscreen_mode(&monitors, display, &window, UVec2::new(s.width, s.height)),
            ),
        };
        // Fullscreen modes size the window themselves; leaving them restores this size.
        if s.display_mode == DisplayMode::Windowed {
            window.resolution.set_physical_resolution(s.width, s.height);
            if let Some(target) = display.filter(|t| *chosen != Some(*t)) {
                window.position = WindowPosition::Centered(MonitorSelection::Entity(target));
            }
        }
        *chosen = display;
    }
    if previous
        .as_ref()
        .is_none_or(|p| (p.vsync, p.vsync_fast) != (s.vsync, s.vsync_fast))
    {
        window.present_mode = present_mode(s);
    }
    if previous
        .as_ref()
        .is_none_or(|p| p.brightness != menu.settings.brightness)
    {
        // Linear gain on the world image only; menus and HUD keep authored levels.
        let gain = menu.settings.brightness as f32 / 100.;
        for mut image in &mut scene {
            image.color = Color::linear_rgb(gain, gain, gain);
        }
    }
    let camera = crate::physics::camera_output::CameraOptions {
        low: menu.settings.camera_low,
        invert_look: menu.settings.invert_look,
    };
    if let Some(mut physics) = physics.filter(|p| p.camera != camera) {
        physics.camera = camera;
    }
    let ao = menu.settings.ambient_occlusion && dlss == 0;
    if dlss_changed
        || previous
            .as_ref()
            .is_none_or(|p| p.samples != menu.settings.samples || p.ambient_occlusion != menu.settings.ambient_occlusion)
    {
        // Bevy's SSAO needs single-sampled views; the MSAA choice returns with AO off.
        for (_, mut samples) in &mut cameras {
            *samples = if ao || dlss != 0 { Msaa::Off } else { msaa(menu.settings.samples) };
        }
    }
    if dlss_changed || previous.as_ref().is_none_or(|p| p.post_aa != s.post_aa || p.sharpening != s.sharpening) {
        use bevy::anti_alias::{contrast_adaptive_sharpening::ContrastAdaptiveSharpening, fxaa::Fxaa, smaa::Smaa};
        for (entity, _) in &cameras {
            let mut camera = commands.entity(entity);
            camera.remove::<(Fxaa, Smaa, ContrastAdaptiveSharpening)>();
            match if dlss != 0 { 0 } else { s.post_aa } {
                1 => {
                    camera.insert(Fxaa::default());
                }
                2 => {
                    camera.insert(Smaa::default());
                }
                _ => {}
            }
            let strength = SHARPENING[s.sharpening as usize].1;
            if strength > 0. {
                camera.insert(ContrastAdaptiveSharpening { enabled: true, sharpening_strength: strength, denoise: false });
            }
        }
    }
    if previous.as_ref().is_none_or(|p| {
        p.anisotropy != s.anisotropy || p.camera_shake != s.camera_shake || p.player_graphics != s.player_graphics
            || p.imperial != s.imperial || p.subtitles != s.subtitles || p.effects != s.effects || p.draw_distance != s.draw_distance
            || p.texture_quality != s.texture_quality || p.performance() != s.performance()
            || p.chat != s.chat || p.chat_filter != s.chat_filter || p.chat_friends != s.chat_friends || p.custom_models != s.custom_models
            || p.voice_chat != s.voice_chat || p.voice_friends_only != s.voice_friends_only || p.ptt_pad != s.ptt_pad
    }) {
        publish(s);
    }
    if dlss_changed
        || previous.as_ref().is_none_or(|p| {
            p.occlusion != menu.settings.occlusion || p.ambient_occlusion != menu.settings.ambient_occlusion
                || p.performance() != s.performance()
        })
    {
        for (entity, _) in &cameras {
            let mut camera = commands.entity(entity);
            if menu.settings.occlusion {
                camera.insert(OcclusionCulling);
            } else {
                camera.remove::<OcclusionCulling>();
            }
            // Occlusion culling, SSAO and DLSS read the depth prepass. FPS pass 10-23: on by default, for the main
            // pass's early depth rejection (each pixel shaded once): +4-12 % FPS on Skate 1 and NSV, lower latency,
            // the same pixels outside Performance mode (SKATE_DEPTH_PREPASS=0 turns it off). Not in Performance mode:
            // its pixels are cheap, the prepass bought it nothing (Skate 1 142 -> 146 fps, same) and its tail was worse.
            let prepass = crate::config::perf_on("SKATE_DEPTH_PREPASS") && !s.performance();
            if menu.settings.occlusion || ao || dlss != 0 || prepass {
                camera.insert(DepthPrepass);
            } else {
                camera.remove::<DepthPrepass>();
            }
            if ao {
                camera.insert((
                    NormalPrepass,
                    ScreenSpaceAmbientOcclusion {
                        quality_level: ScreenSpaceAmbientOcclusionQualityLevel::Low,
                        ..default()
                    },
                ));
            } else {
                camera.remove::<(ScreenSpaceAmbientOcclusion, NormalPrepass)>();
            }
        }
        info!(
            "GPU occlusion culling: {}, ambient occlusion: {ao}",
            menu.settings.occlusion
        );
    }
    // A window of no size (minimised, or its monitor unplugged) keeps the last target: Bevy's SSAO builds a 5-mip
    // depth chain off it, which a target under 16 px can't hold (a fatal wgpu validation error).
    let size = menu.internal_size(window.physical_size());
    let sized = window.physical_size().min_element() > 0 && size.max_element() >= 16;
    if let Some(image) = images.get(&target.0).filter(|_| sized) {
        if image.size() != size {
            images.get_mut(&target.0).unwrap().resize(Extent3d {
                width: size.x,
                height: size.y,
                depth_or_array_layers: 1,
            });
        }
    }
    *previous = Some(menu.settings.clone());
}
/// Settings > Video > Monitor's list, refreshed when displays come and go.
fn list_monitors(mut menu: ResMut<Menu>, monitors: Query<(&Monitor, Has<PrimaryMonitor>)>) {
    let mut list: Vec<MonitorInfo> = monitors
        .iter()
        .filter_map(|(m, primary)| {
            let hz = m.refresh_rate_millihertz.map(|mhz| (mhz + 500) / 1000);
            Some(MonitorInfo { name: m.name.clone()?, size: m.physical_size(), hz, primary })
        })
        .collect();
    list.sort_by(|a, b| a.name.cmp(&b.name));
    if menu.monitors != list {
        menu.monitors = list;
    }
}
/// The fastest video mode of `size` on the chosen monitor, else the window's (else the primary one);
/// the desktop mode when that monitor has no such resolution.
/// V-Sync Off / On / Fast. Mailbox falls back to FIFO where the GPU lacks it (vendor/bevy_render: never to tearing).
fn present_mode(s: &GraphicsSettings) -> PresentMode {
    match (s.vsync, s.vsync_fast) {
        (false, _) => PresentMode::AutoNoVsync,
        (true, false) => PresentMode::AutoVsync,
        (true, true) => PresentMode::Mailbox,
    }
}
fn fullscreen_mode(
    monitors: &Query<(Entity, &Monitor, Has<PrimaryMonitor>)>,
    target: Option<Entity>,
    window: &Window,
    size: UVec2,
) -> VideoModeSelection {
    let center = match window.position {
        WindowPosition::At(at) => Some(at + (window.physical_size() / 2).as_ivec2()),
        _ => None,
    };
    let contains = |m: &Monitor| {
        center.is_some_and(|c| {
            let offset = c - m.physical_position;
            offset.cmpge(IVec2::ZERO).all() && offset.as_uvec2().cmplt(m.physical_size()).all()
        })
    };
    monitors
        .iter()
        .max_by_key(|(e, m, primary)| (Some(*e) == target, contains(m), *primary))
        .and_then(|(_, m, _)| {
            m.video_modes
                .iter()
                .filter(|v| v.physical_size == size)
                .max_by_key(|v| (v.refresh_rate_millihertz, v.bit_depth))
        })
        .map_or(VideoModeSelection::Current, |v| {
            VideoModeSelection::Specific(*v)
        })
}
fn labels(
    menu: Res<Menu>,
    transition: Res<crate::map_transition::MapTransition>,
    time: Res<Time<Real>>,
    customiser: Res<crate::customiser::Customiser>,
    custom_models: Res<crate::custom_models::CustomModels>,
    travel: Res<crate::teleport_menu::Travel>,
    mods: Res<crate::modding::ModMenu>,
    net: Res<crate::multiplayer::Multiplayer>,
    skate2: Option<Res<crate::skate2_physics::Skate2Physics>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut root: Single<&mut Node, With<MenuRoot>>,
    mut labels: Query<(&MenuLabel, &mut Text), Without<StatusLabel>>,
    mut status: Single<&mut Text, With<StatusLabel>>,
    mut buttons: Query<
        (&MenuRow, &Interaction, &mut BackgroundColor, &mut Node),
        Without<MenuRoot>,
    >,
) {
    // Writing a Node marks it changed and reruns UI layout; only write a new value.
    let display = if menu.open
        && !menu.crossbar
        && !mods.open
        && !travel.open
        && !customiser.open
        && !custom_models.open
    {
        Display::Flex
    } else {
        Display::None
    };
    if root.display != display {
        root.display = display;
    }
    // Hidden (closed, or under the crossbar / customiser / another panel): text written now would only be
    // re-shaped for nothing every frame (Bevy measures Text under Display::None too). The frame it shows writes it.
    if display == Display::None {
        return;
    }
    let s = &menu.settings;
    let size = menu.internal_size(window.physical_size());
    for (label, mut text) in &mut labels {
        let chosen = |setting| {
            let (choices, index) = menu.choices(setting);
            choices.into_iter().nth(index).unwrap_or_default()
        };
        // Only a new string marks the Text changed (a change re-shapes it).
        let new: String = if menu.browser {
            match label.0 {
                0 => "Refresh public Steam lobbies".into(),
                1..=5 => net
                    .browser_rows
                    .get(label.0 - 1)
                    .map(|r| {
                        format!(
                            "{} | {}/{} | #{}{}",
                            r.map,
                            r.players,
                            r.capacity,
                            r.id % 100000,
                            if r.compatible {
                                ""
                            } else {
                                " | incompatible physics/protocol"
                            }
                        )
                    })
                    .unwrap_or_else(|| "--".into()),
                6 => "Previous page".into(),
                7 => "Next page".into(),
                8 => "Resume".into(),
                9 => "Quit game".into(),
                _ => "Back to multiplayer".into(),
            }
        } else if menu.multiplayer {
            match label.0 {
                0 => "Host local test (no Steam)".into(),
                1 => "Join local test (no Steam)".into(),
                2 => "Host via Steam / Spacewar".into(),
                3 => format!(
                    "Join code: {}{}",
                    net.join_code,
                    if menu.selected == 3 { "_" } else { "" }
                ),
                4 => "Join via Steam / Spacewar".into(),
                5 => "Leave multiplayer".into(),
                6 => "Browse public Steam lobbies".into(),
                7 => "Solo / local play does not require Steam".into(),
                8 => "Resume".into(),
                9 => "Quit game".into(),
                _ => "Back to gameplay & graphics".into(),
            }
        } else {
            match label.0 {
                0 => format!("Resolution          {} x {}", s.width, s.height),
                1 => format!(
                    "Internal resolution   {}%  ({} x {})",
                    s.scale, size.x, size.y
                ),
                2 => format!("Anti-aliasing        {}", chosen(Setting::Msaa)),
                3 => format!(
                    "FPS limit             {}",
                    if s.fps == 0 {
                        "Unlimited".into()
                    } else {
                        s.fps.to_string()
                    }
                ),
                4 => format!(
                    "Occlusion culling     {}",
                    if s.occlusion { "On" } else { "Off" }
                ),
                5 => format!("Difficulty            {}", menu.difficulty.label()),
                6 => format!(
                    "Map                   {}",
                    menu.maps[menu.selected_map].label
                ),
                7 => {
                    if transition.busy() {
                        "Loading map...".into()
                    } else {
                        "Load map".into()
                    }
                }
                8 => "Resume".into(),
                9 => "Quit game".into(),
                10 => "Character customiser".into(),
                12 => "Custom models".into(),
                13 => "Updates".into(),
                15 => "Mods".into(),
                14 => "Teleport…".into(),
                16 => format!("Time of day           {}", chosen(Setting::TimeOfDay)),
                _ => "Multiplayer".into(),
            }
        };
        if **text != new {
            **text = new;
        }
    }
    let new = if transition.busy() {
        format!(
            "{} {}\nGameplay is paused. Please wait.",
            ["|", "/", "-", "\\"][(time.elapsed_secs() * 4.) as usize % 4],
            transition.label()
        )
    } else if menu.browser {
        net.browser_status.clone()
    } else if menu.multiplayer {
        format!(
            "{}{}",
            net.status,
            if net.host_code.is_empty() {
                String::new()
            } else {
                format!("\nYour connection code: {}", net.host_code)
            }
        )
    } else if let Some(notice) = skate2.as_ref().and_then(|skate2| skate2.notice.clone()) {
        // Skate 2 physics' notice (loading, reloading, stopped) while it lasts. Not tested in play.
        notice
    } else {
        menu.status.clone()
    };
    if ***status != new {
        ***status = new;
    }
    for (row, interaction, mut color, mut node) in &mut buttons {
        let display = if menu.multiplayer && row.0 >= 11 {
            Display::None
        } else {
            Display::Flex
        };
        if node.display != display {
            node.display = display;
        }
        color.set_if_neq(BackgroundColor(if row.0 == menu.selected || *interaction == Interaction::Hovered {
            Color::srgb(0.10, 0.30, 0.34)
        } else {
            Color::srgb(0.08, 0.11, 0.15)
        }));
    }
}
pub(crate) fn pace(
    menu: Option<Res<Menu>>,
    winit: Res<bevy::winit::WinitSettings>,
    windows: Query<&Window>,
    mut pacer: ResMut<FramePacer>,
    jit: Option<ResMut<JustInTime>>,
    (time, boot, customiser, monitors, replay): (
        Res<Time<Virtual>>,
        Option<Res<crate::boot::BootActive>>,
        Option<Res<crate::customiser::Customiser>>,
        Query<&Monitor>,
        Option<Res<crate::replay::hud::ReplayHud>>,
    ),
) {
    let Some(menu) = menu else {
        return;
    };
    // A minimized window stays `focused` here (measured), but it has no size.
    let foreground = windows
        .iter()
        .any(|w| w.focused && w.physical_width() > 0 && w.physical_height() > 0);
    // C3: menus, the boot and the pause draw little, and uncapped they ran hundreds of frames a second at full GPU and
    // CPU (heat, fans, a throttled laptop back in the game). With no limit set they run at the display's refresh.
    let screen = menu.open || time.is_paused() || boot.is_some_and(|b| b.0) || customiser.is_some_and(|c| c.open);
    // ponytail: the fastest display's refresh, not the one the window is on; unknown leaves them unlimited.
    let refresh = monitors.iter().filter_map(|m| m.refresh_rate_millihertz).max().map_or(0, |mhz| (mhz + 500) / 1000);
    // Render speed (10-27): an export renders as fast as the machine can, alt-tabbed too, and with no late start.
    let export = replay.and_then(|r| r.export_rendering());
    let fps = frame_cap(menu.settings.fps, screen, refresh, export, &winit, foreground);
    pacer.0 = if fps > 0 {
        next_deadline(pacer.0, Duration::from_secs_f64(1. / f64::from(fps)), Instant::now(), std::thread::sleep)
    } else {
        Instant::now()
    };
    if let Some(mut jit) = jit {
        // On this thread, a V-Sync wait is the display's: a frame started that late plus a hiccup would miss its
        // refresh and stutter. Pipelined, the render thread still waits for the display after it: safe either way.
        let vsync = !jit.pipelined
            && windows.iter().any(|w| !matches!(w.present_mode, PresentMode::AutoNoVsync | PresentMode::Immediate));
        let waited = if jit.pipelined {
            &bevy::render::pipelined_rendering::RENDER_WAIT_NANOS
        } else {
            &bevy::render::view::window::ACQUIRE_WAIT_NANOS
        };
        let waited = Duration::from_nanos(waited.load(std::sync::atomic::Ordering::Relaxed));
        jit.delay = if vsync || export.is_some() { Duration::ZERO } else { just_in_time(jit.delay, waited) };
        if !jit.delay.is_zero() {
            std::thread::sleep(jit.delay);
        }
    }
}
/// A frame that ends idle, waiting on the renderer, starts later by about that much instead: the pads are read that
/// much closer to the screen at the same frame rate (what NVIDIA Reflex does). The delay creeps up half the way to
/// last frame's wait less MARGIN (at most STEP a frame, so one hitch can't stall the next), drops at once to fit a
/// shorter wait, and with none at all (the renderer idled on us, by an unknown amount) at least halves.
/// ponytail: reacts to last frame only; a frame with a physics step (+~2 ms) pulls it down for its neighbours too.
fn just_in_time(delay: Duration, waited: Duration) -> Duration {
    const MARGIN: Duration = Duration::from_micros(800);
    const STEP: Duration = Duration::from_micros(500);
    const NO_WAIT: Duration = Duration::from_micros(100);
    const MAX: Duration = Duration::from_millis(20);
    if waited < NO_WAIT {
        delay.saturating_sub(MARGIN.max(delay / 2))
    } else if waited < MARGIN {
        delay.saturating_sub(MARGIN - waited)
    } else {
        (delay + ((waited - MARGIN) / 2).min(STEP)).min(MAX)
    }
}
/// The cap `pace` keeps: the player's limit, a menu `screen` at the display's refresh with none set, the background's
/// when alt-tabbed. `export`: Some(rendering) while a replay export runs; a frame it renders runs uncapped (render
/// speed 10-27), alt-tabbed too. Review M4: one waiting on its encoder draws nothing new and paces as a menu does, so
/// a software encoder (a weaker PC's) gets the CPU.
fn frame_cap(fps: u32, screen: bool, refresh: u32, export: Option<bool>, winit: &bevy::winit::WinitSettings, foreground: bool) -> u32 {
    match export {
        Some(true) => 0,
        _ => background_limit(screen_limit(fps, screen || export.is_some(), refresh), winit, foreground),
    }
}
/// The player's limit, or on a menu `screen` with none set (0), the display's `refresh` (0: unknown).
fn screen_limit(fps: u32, screen: bool, refresh: u32) -> u32 {
    if fps == 0 && screen { refresh } else { fps }
}
/// Frames are due at fixed steps from the last deadline, so a sleep that overshoots is taken back from the next
/// frame (resetting to "now" every frame let each overshoot add up: a 60 cap ran ~57 with uneven frame times).
/// A frame that runs late starts a fresh schedule instead of rushing to catch up.
fn next_deadline(last: Instant, period: Duration, now: Instant, sleep: impl FnOnce(Duration)) -> Instant {
    let due = last + period;
    match due.checked_duration_since(now) {
        Some(wait) => {
            sleep(wait);
            due
        }
        None => now,
    }
}
/// The frame limit to apply. WinitSettings asks for a reactive rate when no window
/// is focused, but Bevy 0.18 on Windows still updates flat out (196 a second while
/// minimized, measured 2026-09-29), so the limiter enforces it. The benchmark
/// harness sets continuous mode and so keeps measuring regardless of focus.
fn background_limit(fps: u32, winit: &bevy::winit::WinitSettings, focused: bool) -> u32 {
    match winit.unfocused_mode {
        bevy::winit::UpdateMode::Reactive { wait, .. } if !focused && !wait.is_zero() => {
            let background = (1. / wait.as_secs_f64()).round() as u32;
            if fps == 0 {
                background
            } else {
                fps.min(background)
            }
        }
        _ => fps,
    }
}

#[cfg(test)]
mod background_tests {
    #[test]
    fn a_handed_off_map_load_refused_online_gives_back_the_world() {
        let mut menu = super::test_menu();
        menu.hand_off(7, false, true, Some(0));
        menu.refuse_map_load(true);
        assert!(!menu.open,"the crossbar's pick must not strand the player in the panel");
        menu.open = true;
        menu.refuse_map_load(false);
        assert!(menu.open, "chosen in the panel itself: it stays up with the notice");
    }
    #[test]
    fn integrated_gpus_start_lighter_and_discrete_keep_the_defaults() {
        use super::GraphicsSettings;
        let light = GraphicsSettings::first_run(wgpu::DeviceType::IntegratedGpu).validated();
        assert_eq!((light.samples, light.post_aa, light.shadow_quality, light.anisotropy), (1, 1, 0, 4));
        assert_eq!(light.scale, 100, "resolution is left alone");
        for device in [wgpu::DeviceType::DiscreteGpu, wgpu::DeviceType::Other, wgpu::DeviceType::VirtualGpu] {
            let first = GraphicsSettings::first_run(device);
            let display = GraphicsSettings { display_mode: first.display_mode, width: first.width, height: first.height, ..GraphicsSettings::default() };
            assert_eq!(first, display);
        }
    }
    #[test]
    fn a_first_run_starts_in_exclusive_fullscreen_at_1080p() {
        use super::{DisplayMode, GraphicsSettings};
        for device in [wgpu::DeviceType::DiscreteGpu, wgpu::DeviceType::IntegratedGpu] {
            let first = GraphicsSettings::first_run(device).validated();
            assert_eq!((first.display_mode, first.width, first.height), (DisplayMode::Fullscreen, 1920, 1080));
        }
        // A saved file keeps whatever the player chose.
        let saved: GraphicsSettings = serde_json::from_str(r#"{"display_mode":"Borderless"}"#).unwrap();
        assert_eq!(saved.display_mode, DisplayMode::Borderless);
    }
    #[test]
    fn frame_cap_holds_its_rate_through_oversleep_and_resets_after_a_late_frame() {
        use std::time::{Duration, Instant};
        let start = Instant::now();
        let period = Duration::from_millis(10);
        // On time: sleeps the remainder and the deadline, not the (overshot) wake time, carries on.
        let mut slept = Duration::ZERO;
        let due = super::next_deadline(start, period, start + Duration::from_millis(4), |d| slept = d);
        assert_eq!((due, slept), (start + period, Duration::from_millis(6)));
        // 60 frames that each reach the limiter 0.7 ms past their deadline (an overslept wake plus work) hold the cap.
        let mut last = start;
        for _ in 0..60 {
            let woke = last + Duration::from_micros(700);
            last = super::next_deadline(last, period, woke.max(last), |_| {});
        }
        assert_eq!(last - start, period * 60);
        // A late frame neither sleeps nor schedules catch-up frames.
        let late = start + Duration::from_millis(25);
        assert_eq!(super::next_deadline(start, period, late, |_| panic!("slept")), late);
    }
    #[test]
    fn unfocused_windows_are_capped_unless_benchmarking() {
        let game = bevy::winit::WinitSettings {
            focused_mode: bevy::winit::UpdateMode::Continuous,
            unfocused_mode: bevy::winit::UpdateMode::reactive_low_power(
                std::time::Duration::from_secs_f64(1. / 60.),
            ),
        };
        assert_eq!(
            super::background_limit(0, &game, true),
            0,
            "focused: uncapped setting kept"
        );
        assert_eq!(super::background_limit(0, &game, false), 60);
        assert_eq!(
            super::background_limit(30, &game, false),
            30,
            "a lower user cap wins"
        );
        assert_eq!(super::background_limit(144, &game, false), 60);
        assert_eq!(
            super::background_limit(0, &bevy::winit::WinitSettings::continuous(), false),
            0,
            "benchmarks opt out"
        );
    }
    /// C3: a menu with no limit set runs at the display's refresh; the player's own limit and gameplay are untouched.
    #[test]
    fn menus_without_a_limit_run_at_the_refresh_rate() {
        assert_eq!(super::screen_limit(0, true, 144), 144);
        assert_eq!(super::screen_limit(0, false, 144), 0, "gameplay");
        assert_eq!(super::screen_limit(240, true, 144), 240, "the player's limit");
        assert_eq!(super::screen_limit(0, true, 0), 0, "unknown refresh");
    }
    /// Render speed (10-27) and review M4: an export's frames run uncapped, alt-tabbed too; an export waiting on its
    /// encoder draws nothing new and paces as a menu does (the refresh, the player's limit, the background's).
    #[test]
    fn an_export_waiting_on_its_encoder_paces_as_a_menu() {
        let game = bevy::winit::WinitSettings {
            focused_mode: bevy::winit::UpdateMode::Continuous,
            unfocused_mode: bevy::winit::UpdateMode::reactive_low_power(std::time::Duration::from_secs_f64(1. / 60.)),
        };
        assert_eq!(super::frame_cap(0, false, 144, Some(true), &game, true), 0, "rendering");
        assert_eq!(super::frame_cap(0, false, 144, Some(true), &game, false), 0, "rendering alt-tabbed");
        assert_eq!(super::frame_cap(0, false, 144, Some(false), &game, true), 144, "waiting: the refresh");
        assert_eq!(super::frame_cap(240, false, 144, Some(false), &game, true), 240, "waiting: the player's limit");
        assert_eq!(super::frame_cap(0, false, 144, Some(false), &game, false), 60, "waiting alt-tabbed: the background's");
        assert_eq!(super::frame_cap(0, false, 144, None, &game, true), 0, "gameplay as before");
    }
    /// Perf 10-18: the just-in-time start settles short of the renderer's idle wait and backs off at once.
    #[test]
    fn just_in_time_settles_short_of_the_wait_and_backs_off_fast() {
        use std::time::Duration;
        // A renderer that leaves 3 ms idle: the delay settles at 3 ms less the margin, never past it.
        let us = Duration::from_micros;
        let mut delay = Duration::ZERO;
        for _ in 0..60 {
            delay = super::just_in_time(delay, us(3000).saturating_sub(delay));
            assert!(delay <= us(2200), "{delay:?}");
        }
        assert!(delay > us(2190), "{delay:?}");
        assert_eq!(super::just_in_time(us(2000), us(500)), us(1700), "a shorter wait: down to fit it at once");
        assert_eq!(super::just_in_time(us(2000), Duration::ZERO), us(1000), "no wait: halved");
        assert_eq!(super::just_in_time(us(500), Duration::ZERO), Duration::ZERO, "at least the margin comes off");
        assert_eq!(super::just_in_time(us(1000), us(500_000)), us(1500), "a hitch adds one step");
    }
}

#[cfg(test)]
pub(crate) fn test_menu() -> Menu {
    Menu {
        open: false,
        selected: 0,
        settings: GraphicsSettings::default(),
        difficulty: Difficulty::Easy,
        physics: Default::default(),
        skate2_gestures: crate::skate2_physics::settings::DEFAULT_GESTURES,
        // One file per menu: tests that save run in parallel.
        path: std::env::temp_dir().join(format!(
            "skate3rust-settings-test/graphics-{}.json",
            {
                static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            }
        )),
        supported_msaa: vec![1, 2, 4, 8],
        status: String::new(),
        multiplayer: false,
        browser: false,
        crossbar: false,
        pending: None,
        handed_off: false,
        guest: false,
        cohost: false,
        world_request: None,
        session_world: None,
        seasonal: None,
        vibration_buzz: false,
        revision: 0,
        monitors: Vec::new(),
        dlss_supported: false,
        on_nsv: false,
        // The seasons' tests run on their map; `seasons_only_on_the_mega_parks_map` leaves it.
        seasons: true,
        mic_test: false,
        mic_level_dbfs: -120.,
        voice_devices: None,
        voice_devices_reading: None,
        mic_error: None,
        maps: vec![crate::map_library::Entry {
            label: "Test world".into(),
            path: None,
        }],
        selected_map: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// v12 list 2: only Skate 3 and Skate 2 rooms; a save on the old Couch or Door room opens Skate 2.
    #[test]
    fn the_old_skate2_room_rows_open_the_skate2_room() {
        assert_eq!(ROOMS.iter().map(|r| r.0).collect::<Vec<_>>(), ["Skate 3", "Skate 2"]);
        for (saved, opens) in [("CreateSkater_Skate2Couch", "CreateSkater_Skate2"), ("CreateSkater_Skate2Door", "CreateSkater_Skate2"), ("Nowhere", "CreateSkater")] {
            let s = serde_json::from_str::<GraphicsSettings>(&format!(r#"{{"edit_skater_room": "{saved}"}}"#)).unwrap().validated();
            assert_eq!(s.edit_skater_room, opens, "{saved}");
        }
    }
    /// Bevy clears Messages only after a FixedUpdate pass, and none runs while Time<Virtual> is paused (the offline
    /// Start menu): every message written during a pause stayed buffered until it ended.
    #[test]
    fn messages_clear_while_time_is_paused() {
        #[derive(Message)]
        struct Ping;
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<Ping>()
            .add_systems(First, release_messages_while_paused.before(bevy::ecs::message::MessageUpdateSystems))
            .add_systems(Update, |mut ping: MessageWriter<Ping>| {
                ping.write(Ping);
            });
        app.update();
        app.world_mut().resource_mut::<Time<Virtual>>().pause();
        for _ in 0..600 {
            app.update();
        }
        let buffered = app.world().resource::<bevy::ecs::message::Messages<Ping>>().len();
        assert!(buffered <= 2, "{buffered} messages kept over a 600-frame pause");
    }
    /// User 10-26: the FPS counter shows unless turned off, also for a graphics.json saved before the row existed.
    #[test]
    fn the_fps_counter_shows_until_turned_off() {
        let mut menu = test_menu();
        menu.settings = serde_json::from_str(r#"{"hud": true}"#).unwrap();
        assert!(menu.fps_counter());
        menu.choose(Setting::FpsCounter, 0);
        assert!(!menu.fps_counter());
        assert_eq!(menu.choices(Setting::FpsCounter), (vec!["Off".into(), "On".into()], 0));
    }
    /// User 10-26: a graphics.json saved before voice chat gets its defaults (on, from everyone, Push to Talk on V, no
    /// pad button, Standard noise suppression, full volumes, the Windows devices); a hand-edited one is clamped.
    #[test]
    fn old_settings_files_get_voice_defaults() {
        let s: GraphicsSettings = serde_json::from_str(r#"{"hud": true}"#).unwrap();
        assert!(s.voice_chat && !s.voice_friends_only && !s.open_mic && !s.duck_for_voice);
        assert_eq!((s.ptt_key.as_str(), s.ptt_pad, s.open_mic_sensitivity, s.noise), ("KeyV", 0, 5, 1));
        assert_eq!((s.mic_volume, s.voice_volume, s.microphone.as_deref(), s.output_device.as_deref()), (100, 100, None, None));
        let wild = r#"{"mic_volume": 999, "voice_volume": 255, "open_mic_sensitivity": 0, "noise": 7, "ptt_pad": 4096, "ptt_key": ""}"#;
        let s = serde_json::from_str::<GraphicsSettings>(wild).unwrap().validated();
        assert_eq!((s.mic_volume, s.voice_volume, s.open_mic_sensitivity, s.noise, s.ptt_pad, s.ptt_key.as_str()), (200, 100, 1, 1, 0, "KeyV"));
        let s = serde_json::from_str::<GraphicsSettings>(r#"{"mic_volume": 155, "voice_volume": 47, "open_mic_sensitivity": 11, "ptt_pad": 512}"#).unwrap().validated();
        assert_eq!((s.mic_volume, s.voice_volume, s.open_mic_sensitivity, s.ptt_pad), (150, 40, 10, 512), "10 % steps; RB is a picker bit");
    }
    /// The voice sliders: Mic Volume 0-200 % and Voice Volume 0-100 % in 10 % steps, Open Mic Sensitivity 1-10.
    #[test]
    fn voice_sliders_step_and_clamp() {
        let mut menu = test_menu();
        menu.settings = serde_json::from_str("{}").unwrap();
        for _ in 0..10 {
            assert!(menu.slide(Setting::MicVolume, 1));
        }
        assert!(!menu.slide(Setting::MicVolume, 1), "200 % is the top");
        assert_eq!((menu.settings.mic_volume, menu.slider(Setting::MicVolume)), (200, (1., "200%".into())));
        for _ in 0..10 {
            assert!(menu.slide(Setting::VoiceVolume, -1));
        }
        assert!(!menu.slide(Setting::VoiceVolume, -1), "0 % is the bottom");
        assert_eq!((menu.settings.voice_volume, menu.slider(Setting::VoiceVolume)), (0, (0., "0%".into())));
        for _ in 0..4 {
            assert!(menu.slide(Setting::OpenMicSensitivity, -1));
        }
        assert!(!menu.slide(Setting::OpenMicSensitivity, -1));
        assert_eq!(menu.settings.open_mic_sensitivity, 1);
        for _ in 0..20 {
            menu.slide(Setting::OpenMicSensitivity, 1);
        }
        assert_eq!((menu.settings.open_mic_sensitivity, menu.slider(Setting::OpenMicSensitivity)), (10, (1., "10".into())));
        assert_eq!(menu.slider(Setting::FieldOfView), (menu.fov_fraction(), menu.fov_label()));
        assert!(menu.slide(Setting::FieldOfView, 1) && !menu.slide(Setting::Hud, 1), "Field of View still slides, a picker never");
    }
    /// A CAS opened from Edit Skaters (over the crossbar) leaves the pause up when it closes.
    #[test]
    fn closing_the_cas_over_the_crossbar_keeps_the_pause() {
        let mut menu = test_menu();
        (menu.open, menu.crossbar) = (true, true);
        menu.cas_closed();
        assert!(menu.open);
        menu.crossbar = false;
        menu.cas_closed();
        assert!(!menu.open);
    }
    /// Review 10-27: reading the microphones and outputs takes tens of ms (more with Bluetooth or USB devices) and
    /// the world never pauses online, so opening Game Settings reads them on a thread; the lists so far stay meanwhile.
    #[test]
    fn voice_devices_are_read_off_the_main_thread() {
        let mut menu = test_menu();
        menu.voice_devices = Some((vec!["Old Mic".into()], vec![]));
        let main = std::thread::current().id();
        let (go, wait) = std::sync::mpsc::channel::<()>();
        menu.refresh_voice_devices_with(move || {
            assert_ne!(std::thread::current().id(), main, "read on the main thread");
            let _ = wait.recv();
            (vec!["USB Mic".into()], vec!["Speakers".into()])
        });
        assert_eq!(menu.voice_devices, Some((vec!["Old Mic".into()], vec![])), "the lists so far, meanwhile");
        assert!(!menu.take_voice_devices(), "not read yet");
        go.send(()).unwrap();
        let start = Instant::now();
        while !menu.take_voice_devices() {
            assert!(start.elapsed() < Duration::from_secs(5), "the read never came");
            std::thread::yield_now();
        }
        assert_eq!(menu.voice_devices, Some((vec!["USB Mic".into()], vec!["Speakers".into()])));
    }
    #[test]
    fn every_setting_choice_reads_back_and_fits_the_picker() {
        use Setting::*;
        let mut menu = test_menu();
        menu.voice_devices = Some((vec!["Headset Mic".into(), "USB Mic".into()], vec!["Speakers".into()]));
        for setting in [
            Setting::DisplayMode,
            Setting::Monitor,
            Resolution,
            RenderScale,
            Msaa,
            FpsLimit,
            VSync,
            Occlusion,
            Brightness,
            TimeOfDay,
            Weather,
            CameraAngle,
            CameraShake,
            Anisotropy,
            Sharpening,
            InvertX,
            InvertY,
            Hud,
            HudStyle,
            Look,
            Effect(0),
            Effect(1),
            Effect(2),
            Effect(3),
            Effect(4),
            DrawDistance,
            TextureQuality,
            EditSkaterRoom,
            PlayerGraphics,
            FieldOfView,
            ShadowQuality,
            AmbientOcclusion,
            Vibration,
            HallOfMeat,
            Units,
            Subtitles,
            DiscordStatus,
            Chat,
            ChatFrom,
            CustomModels,
            ReduceFlashing,
            LowLatency,
            FpsCounter,
            SkipIntro,
            VoiceChat,
            VoiceFrom,
            MicMode,
            PushToTalkKey,
            PushToTalkButton,
            OpenMicSensitivity,
            Microphone,
            MicVolume,
            MicTest,
            NoiseSuppression,
            VoiceVolume,
            LowerGameSound,
            OutputDevice,
        ] {
            let (labels, _) = menu.choices(setting);
            assert!(
                (1..=8).contains(&labels.len()),
                "{setting:?}: {} choices",
                labels.len()
            );
            for index in 0..labels.len() {
                menu.choose(setting, index);
                assert_eq!(menu.choices(setting).1, index, "{setting:?} choice {index}");
            }
        }
        // Voice chat (user 10-26): Default and the devices (the first 7, as Monitor); the pad button's eight.
        assert_eq!(menu.choices(Microphone).0, ["Default (Windows)", "Headset Mic", "USB Mic"]);
        assert_eq!(menu.choices(OutputDevice).0, ["Default (Windows)", "Speakers"]);
        assert_eq!(menu.choices(PushToTalkButton).0, ["None", "X", "Y", "LB", "RB", "Back", "L3", "R3"]);
        menu.choose(PushToTalkButton, 3);
        assert_eq!(menu.settings.ptt_pad, 0x0100, "LB");
        menu.choose(Microphone, 2);
        menu.voice_devices = Some(((0..9).map(|i| format!("Mic {i}")).collect(), Vec::new()));
        assert_eq!(menu.choices(Microphone), (std::iter::once("Default (Windows)".to_string()).chain((0..7).map(|i| format!("Mic {i}"))).collect(), 0));
        assert_eq!(menu.settings.microphone.as_deref(), Some("USB Mic"), "an unplugged mic is kept, the row reads Default");
        // HUD Style: exactly one game's HUD, the map's own by default.
        menu.choose(HudStyle, 0);
        assert_eq!([1, 2, 3].map(|g| menu.hud_game(g)), [1, 2, 3]);
        menu.choose(HudStyle, 1);
        assert_eq!([1, 2, 3].map(|g| menu.hud_game(g)), [3, 3, 3]);
        menu.choose(HudStyle, 3);
        assert_eq!([1, 2, 3].map(|g| menu.hud_game(g)), [1, 1, 1]);
        // Colour Grading (user 10-26): Off (the Performance preset's), or one game's look on every map.
        assert_eq!(menu.choices(Look).0, ["Off", "Match Map", "Skate 3", "Skate 2", "Skate 1"]);
        menu.choose(Look, 3);
        assert_eq!((menu.look(), menu.effects()[0]), (Some(2), true));
        menu.choose(Look, 0);
        assert_eq!((menu.choices(Look).1, menu.effects()[0]), (0, false));
        menu.choose(Look, 1);
        assert_eq!((menu.look(), menu.effects()[0]), (None, true));
        // Anti-Aliasing: post passes and MSAA exclude each other.
        assert_eq!(menu.choices(Msaa).0, ["Off", "FXAA", "SMAA", "MSAA 2x", "MSAA 4x", "MSAA 8x"]);
        menu.choose(Msaa, 2);
        assert_eq!((menu.settings.post_aa, menu.settings.samples), (2, 1));
        menu.choose(Msaa, 4);
        assert_eq!((menu.settings.post_aa, menu.settings.samples), (0, 4));
        // Camera choices reach the physics-side options.
        menu.choose(CameraAngle, 0);
        menu.choose(InvertX, 0);
        menu.choose(InvertY, 1);
        assert!(menu.settings.camera_low && menu.settings.invert_look == [false, true]);
        let saved: GraphicsSettings =
            serde_json::from_slice(&std::fs::read(&menu.path).unwrap()).unwrap();
        assert_eq!(saved, menu.settings);
    }
    #[test]
    fn vibration_defaults_on_saves_and_pulses_when_turned_on() {
        let mut menu = test_menu();
        assert!(menu.vibration());
        assert_eq!(menu.choices(Setting::Vibration), (vec!["Off".into(), "On".into()], 1));
        menu.choose(Setting::Vibration, 0);
        assert!(!menu.vibration() && !menu.vibration_buzz);
        menu.choose(Setting::Vibration, 1);
        assert!(menu.vibration() && menu.vibration_buzz);
        let saved: GraphicsSettings = serde_json::from_slice(&std::fs::read(&menu.path).unwrap()).unwrap();
        assert!(saved.vibration);
        // Settings saved before the option existed load with it on.
        let old: GraphicsSettings = serde_json::from_str("{}").unwrap();
        assert!(old.vibration);
    }
    /// User 10-25: Text Chat is Off, Filtered (swearing masked) or Unfiltered; slurs are masked in every mode
    /// (chat_filter). Filtered is the default, for settings saved before it too.
    #[test]
    fn text_chat_is_off_filtered_or_unfiltered() {
        let mut menu = test_menu();
        assert_eq!(menu.choices(Setting::Chat), (vec!["Off".into(), "Filtered".into(), "Unfiltered".into()], 1));
        menu.choose(Setting::Chat, 2);
        assert!(menu.settings.chat && !menu.settings.chat_filter);
        menu.choose(Setting::Chat, 0);
        assert_eq!(menu.choices(Setting::Chat).1, 0);
        menu.choose(Setting::Chat, 1);
        assert!(menu.settings.chat && menu.settings.chat_filter);
        let old: GraphicsSettings = serde_json::from_str(r#"{"chat":true}"#).unwrap();
        assert!(old.chat && old.chat_filter);
    }
    /// A minimised window, or one whose monitor was unplugged, reports 0 x 0: the scene keeps its last size, as a
    /// 1 x 1 target crashes Bevy's SSAO (its depth chain is 5 mips; user crash report 10-11, "Monitor removed").
    #[test]
    fn a_window_of_no_size_keeps_the_scene_target() {
        let mut app = App::new();
        let mut images = Assets::<Image>::default();
        let target = images.add(Image::new_target_texture(1280, 720, TextureFormat::Rgba8UnormSrgb, None));
        let mut menu = test_menu();
        menu.settings.ambient_occlusion = true;
        app.insert_resource(SceneTarget(target.clone())).insert_resource(images).insert_resource(menu).add_systems(Update, apply);
        let window = app.world_mut().spawn((Window::default(), PrimaryWindow)).id();
        app.world_mut().spawn((Camera3d::default(), Msaa::Off));
        let size = |app: &App| app.world().resource::<Assets<Image>>().get(&target).unwrap().size();
        let resize = |app: &mut App, w, h| app.world_mut().get_mut::<Window>(window).unwrap().resolution.set_physical_resolution(w, h);
        app.update();
        let before = size(&app);
        for (w, h) in [(0, 0), (1, 1), (0, 900)] {
            resize(&mut app, w, h);
            app.update();
            assert_eq!(size(&app), before, "{w} x {h}");
        }
        resize(&mut app, 1600, 900);
        app.update();
        assert_eq!(size(&app), UVec2::new(1600, 900) * test_menu().settings.scale / 100);
    }
    /// DLSS off (the default, or no supporting GPU, or a build without the feature) leaves the camera exactly as it was
    /// before the DLSS row existed: same components, same MSAA, same scene size, whatever the saved DLSS value.
    #[test]
    fn dlss_off_leaves_the_camera_unchanged() {
        let camera_after = |supported: bool, dlss: u32, post_aa: usize, ao: bool| {
            let mut app = App::new();
            let mut images = Assets::<Image>::default();
            let target = images.add(Image::new_target_texture(1280, 720, TextureFormat::Rgba8UnormSrgb, None));
            let mut menu = test_menu();
            menu.dlss_supported = supported;
            menu.settings.dlss = dlss;
            menu.settings.ambient_occlusion = ao;
            menu.choose(Setting::Msaa, post_aa);
            app.insert_resource(SceneTarget(target.clone())).insert_resource(images).insert_resource(menu).add_systems(Update, apply);
            app.world_mut().spawn((Window::default(), PrimaryWindow));
            let id = app.world_mut().spawn((Camera3d::default(), Msaa::Off)).id();
            app.update();
            let mut names: Vec<String> = app.world().inspect_entity(id).unwrap().map(|c| c.name().to_string()).collect();
            names.sort();
            let size = app.world().resource::<Assets<Image>>().get(&target).unwrap().size();
            (names, *app.world().get::<Msaa>(id).unwrap(), size)
        };
        for post_aa in [0, 1, 2, 4] {
            for ao in [false, true] {
                let today = camera_after(false, 0, post_aa, ao);
                assert!(!today.0.iter().any(|n| ["TemporalJitter", "MipBias", "MotionVectorPrepass", "Hdr"].iter().any(|c| n.ends_with(c))));
                assert_eq!(camera_after(true, 0, post_aa, ao), today, "supported, off");
                for saved in [3, 6, 99] {
                    assert_eq!(camera_after(false, saved, post_aa, ao), today, "unsupported, saved {saved}");
                }
            }
        }
    }
    #[test]
    fn dlss_setting_round_trips_and_unknown_values_are_off() {
        let mut menu = test_menu();
        assert_eq!(menu.choices(Setting::Dlss), (DLSS_MODES.map(String::from).to_vec(), 0));
        menu.choose(Setting::Dlss, 4);
        let saved: GraphicsSettings = serde_json::from_slice(&std::fs::read(&menu.path).unwrap()).unwrap();
        assert_eq!(saved.validated().dlss, 4);
        assert_eq!(menu.dlss(), 0, "no supporting GPU: off whatever is saved");
        menu.dlss_supported = true;
        assert_eq!(menu.dlss(), 4);
        assert_eq!(menu.internal_size(UVec2::new(1600, 900)), UVec2::new(1600, 900), "DLSS sets the render scale");
        for old in [r#"{"hud":true}"#, r#"{"dlss":7}"#, r#"{"dlss":4000000000}"#] {
            assert_eq!(serde_json::from_str::<GraphicsSettings>(old).unwrap().validated().dlss, 0, "{old}");
        }
    }
    /// FPS pass 10-23: Performance mode's pixels are cheap and the prepass bought it nothing (Skate 1 142 -> 146 fps,
    /// same; a worse tail), so that look keeps the shipped pipeline; leaving it brings the prepass back.
    #[test]
    fn performance_mode_drops_the_depth_prepass() {
        let mut app = App::new();
        let mut images = Assets::<Image>::default();
        let target = images.add(Image::new_target_texture(1280, 720, TextureFormat::Rgba8UnormSrgb, None));
        app.insert_resource(SceneTarget(target))
            .insert_resource(images)
            .insert_resource(test_menu())
            .add_systems(Update, apply);
        app.world_mut().spawn((Window::default(), PrimaryWindow));
        let camera = app.world_mut().spawn((Camera3d::default(), Msaa::Off)).id();
        app.update();
        let prepass = crate::config::perf_on("SKATE_DEPTH_PREPASS");
        assert_eq!(app.world().entity(camera).contains::<DepthPrepass>(), prepass);
        let normal = app.world_mut().resource_mut::<Menu>().diagnostic_look(None);
        app.update();
        assert!(!app.world().entity(camera).contains::<DepthPrepass>(), "Performance mode");
        app.world_mut().resource_mut::<Menu>().diagnostic_look(Some(normal));
        app.update();
        assert_eq!(app.world().entity(camera).contains::<DepthPrepass>(), prepass, "back from Performance mode");
    }
    #[test]
    fn culling_can_toggle_with_msaa_and_render_scale_changes() {
        let mut app = App::new();
        let mut images = Assets::<Image>::default();
        let target = images.add(Image::new_target_texture(
            1280,
            720,
            TextureFormat::Rgba8UnormSrgb,
            None,
        ));
        let mut menu = test_menu();
        menu.settings.occlusion = true;
        app.insert_resource(SceneTarget(target.clone()))
            .insert_resource(images)
            .insert_resource(menu)
            .add_systems(Update, apply);
        app.world_mut().spawn((Window::default(), PrimaryWindow));
        let camera = app.world_mut().spawn((Camera3d::default(), Msaa::Off)).id();
        app.update();
        assert!(app.world().entity(camera).contains::<OcclusionCulling>());
        assert!(app.world().entity(camera).contains::<DepthPrepass>());
        {
            let mut menu = app.world_mut().resource_mut::<Menu>();
            menu.settings.occlusion = false;
            menu.settings.samples = 1;
            menu.settings.scale = 67;
        }
        app.update();
        assert!(!app.world().entity(camera).contains::<OcclusionCulling>());
        // FPS pass 10-23: the depth prepass stays on its own (SKATE_DEPTH_PREPASS=0 drops it).
        let prepass = crate::config::perf_on("SKATE_DEPTH_PREPASS");
        assert_eq!(app.world().entity(camera).contains::<DepthPrepass>(), prepass);
        assert_eq!(*app.world().get::<Msaa>(camera).unwrap(), Msaa::Off);
        assert_eq!(
            app.world()
                .resource::<Assets<Image>>()
                .get(&target)
                .unwrap()
                .size(),
            UVec2::new(857, 482)
        );
        {
            let mut menu = app.world_mut().resource_mut::<Menu>();
            menu.settings.occlusion = true;
            menu.settings.samples = 8;
        }
        app.update();
        assert!(app.world().entity(camera).contains::<OcclusionCulling>());
        assert!(app.world().entity(camera).contains::<DepthPrepass>());
        assert_eq!(*app.world().get::<Msaa>(camera).unwrap(), Msaa::Sample8);
        // Ambient occlusion: depth + normal prepasses and no MSAA, even without culling.
        {
            let mut menu = app.world_mut().resource_mut::<Menu>();
            menu.settings.occlusion = false;
            menu.settings.ambient_occlusion = true;
        }
        app.update();
        let entity = app.world().entity(camera);
        assert!(entity.contains::<ScreenSpaceAmbientOcclusion>());
        assert!(entity.contains::<NormalPrepass>() && entity.contains::<DepthPrepass>());
        assert!(!entity.contains::<OcclusionCulling>());
        assert_eq!(*entity.get::<Msaa>().unwrap(), Msaa::Off);
        // Off again: exactly the earlier pipeline, the MSAA choice restored.
        app.world_mut().resource_mut::<Menu>().settings.ambient_occlusion = false;
        app.update();
        let entity = app.world().entity(camera);
        assert!(!entity.contains::<ScreenSpaceAmbientOcclusion>());
        assert!(!entity.contains::<NormalPrepass>() && entity.contains::<DepthPrepass>() == prepass);
        assert_eq!(*entity.get::<Msaa>().unwrap(), Msaa::Sample8);
        // FXAA and sharpening are camera passes; FXAA replaces MSAA.
        {
            let mut menu = app.world_mut().resource_mut::<Menu>();
            menu.choose(Setting::Msaa, 1);
            menu.choose(Setting::Sharpening, 2);
        }
        app.update();
        let entity = app.world().entity(camera);
        assert!(entity.contains::<bevy::anti_alias::fxaa::Fxaa>());
        assert!(entity.contains::<bevy::anti_alias::contrast_adaptive_sharpening::ContrastAdaptiveSharpening>());
        assert_eq!(*entity.get::<Msaa>().unwrap(), Msaa::Off);
    }
    #[test]
    fn the_monitor_row_lists_each_display_and_keeps_an_unplugged_choice() {
        let mut menu = test_menu();
        assert_eq!(menu.choices(Setting::Monitor), (vec!["Automatic".to_string()], 0));
        let screen = |name: &str, w, h, hz, primary| MonitorInfo { name: name.into(), size: UVec2::new(w, h), hz, primary };
        menu.monitors = vec![screen(r"\\.\DISPLAY1", 2560, 1440, Some(144), true), screen(r"\\.\DISPLAY2", 1920, 1080, None, false)];
        assert_eq!(
            menu.choices(Setting::Monitor).0,
            ["Automatic", "Display 1: 2560 x 1440, 144 Hz (Primary)", "Display 2: 1920 x 1080"]
        );
        menu.choose(Setting::Monitor, 2);
        assert_eq!(menu.settings.monitor.as_deref(), Some(r"\\.\DISPLAY2"));
        assert_eq!(menu.choices(Setting::Monitor).1, 2);
        // Unplugged: the row reads Automatic, the saved choice stays for when it is back.
        menu.monitors.pop();
        assert_eq!(menu.choices(Setting::Monitor).1, 0);
        assert_eq!(menu.settings.monitor.as_deref(), Some(r"\\.\DISPLAY2"));
        menu.choose(Setting::Monitor, 0);
        assert_eq!(menu.settings.monitor, None);
    }
    #[test]
    fn old_screen_effects_off_turns_every_effect_off_and_each_effect_toggles_alone() {
        let old: GraphicsSettings = serde_json::from_str(r#"{"screen_effects":false}"#).unwrap();
        assert_eq!(old.validated().effects, [false; 5]);
        let new: GraphicsSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(new.validated().effects, [true; 5], "retail: every effect on");
        let mut menu = test_menu();
        menu.choose(Setting::Effect(1), 0);
        assert_eq!(menu.effects(), [true, false, true, true, true]);
        assert_eq!(menu.choices(Setting::Effect(1)), (vec!["Off".to_string(), "On".to_string()], 0));
        let saved = std::fs::read_to_string(&menu.path).unwrap();
        assert!(!saved.contains("screen_effects"), "the old switch is read, never written");
    }
    #[test]
    fn draw_distance_defaults_to_today_and_lower_settings_range_props_live() {
        assert_eq!(GraphicsSettings::default().draw_distance, 2, "High: the full view");
        assert_eq!(DRAW_DISTANCES[2], ("High", 1000., None, 400.));
        let mut app = App::new();
        let mut menu = test_menu();
        menu.choose(Setting::DrawDistance, 0);
        app.insert_resource(menu).add_systems(Update, prop_ranges);
        let early = app.world_mut().spawn(crate::physics::props::PropPiece(0)).id();
        app.update();
        let range = app.world().get::<bevy::camera::visibility::VisibilityRange>(early).cloned().unwrap();
        assert_eq!((range.end_margin.start, range.use_aabb), (DRAW_DISTANCES[0].2.unwrap(), true));
        let late = app.world_mut().spawn(crate::physics::props::PropPiece(1)).id();
        app.update();
        assert!(app.world().get::<bevy::camera::visibility::VisibilityRange>(late).is_some(), "a prop spawned later is ranged too");
        app.world_mut().resource_mut::<Menu>().choose(Setting::DrawDistance, 2);
        app.update();
        assert!(app.world().get::<bevy::camera::visibility::VisibilityRange>(early).is_none(), "High draws every prop");
    }
    #[test]
    fn a_new_install_opens_at_the_consoles_16_9() {
        // Boot screens fit the 1280x720 stage and the loading screen covers it: at 16:9 both frame alike.
        let d = GraphicsSettings::default();
        assert_eq!((d.width, d.height), (1280, 720));
        assert!(RESOLUTIONS.contains(&(1280, 720)));
    }
    #[test]
    fn invalid_saved_values_fall_back() {
        let settings: GraphicsSettings =
            serde_json::from_str(r#"{"width":0,"height":999999,"scale":0,"samples":3,"fps":1}"#)
                .unwrap();
        assert_eq!(settings.validated(), GraphicsSettings::default());
    }
    #[test]
    fn occlusion_culling_starts_off_even_for_files_saved_with_the_old_default() {
        let old: GraphicsSettings = serde_json::from_str(r#"{"occlusion":true}"#).unwrap();
        assert!(!old.occlusion);
        let chosen = GraphicsSettings { occlusion: true, ..default() };
        let saved: GraphicsSettings = serde_json::from_str(&serde_json::to_string(&chosen).unwrap()).unwrap();
        assert!(saved.occlusion, "turning it back on is kept");
    }
    #[test]
    fn graphics_tab_defaults_keep_the_retail_view_and_round_trip() {
        // Settings saved before the Graphics tab: retail FOV, Medium shadows.
        let old: GraphicsSettings = serde_json::from_str(r#"{"hud":true}"#).unwrap();
        let mut menu = test_menu();
        menu.settings = old.validated();
        assert_eq!((menu.fov_offset(), menu.shadow_quality()), (0., SHADOW_QUALITY_DEFAULT));
        assert!(!menu.settings.ambient_occlusion, "AO stays off for old settings files");
        assert_eq!(menu.choices(Setting::FieldOfView), (vec!["67 (Default)".to_string()], 0));
        // A 1-degree slider, never wider than +10.
        assert!(menu.slide(Setting::FieldOfView, 1));
        assert_eq!(menu.fov_label(), "68 (+1)");
        for _ in 0..30 {
            menu.slide(Setting::FieldOfView, 1);
        }
        assert!(!menu.slide(Setting::FieldOfView, 1), "stops at the end");
        assert!((menu.fov_offset() - 10f32.to_radians()).abs() < 1e-6 && menu.fov_fraction() == 1.);
        for _ in 0..30 {
            menu.slide(Setting::FieldOfView, -1);
        }
        assert!((menu.fov_offset() + 15f32.to_radians()).abs() < 1e-6 && menu.fov_fraction() == 0.);
        menu.choose(Setting::ShadowQuality, 2);
        menu.choose(Setting::AmbientOcclusion, 1);
        let saved: GraphicsSettings =
            serde_json::from_slice(&std::fs::read(&menu.path).unwrap()).unwrap();
        assert_eq!((saved.fov_offset, saved.shadow_quality, saved.ambient_occlusion), (-15, 2, true));
        let bad: GraphicsSettings =
            serde_json::from_str(r#"{"fov_offset":25,"shadow_quality":7}"#).unwrap();
        let bad = bad.validated();
        assert_eq!((bad.fov_offset, bad.shadow_quality), (0, SHADOW_QUALITY_DEFAULT));
    }
    #[test]
    fn the_performance_preset_sets_every_row_and_default_undoes_it() {
        let mut menu = test_menu();
        menu.supported_msaa = vec![1, 2, 4];
        assert_eq!(menu.choices(Setting::Preset).1, 2, "the defaults are the Default preset");
        menu.choose(Setting::Preset, 1);
        assert_eq!(menu.choices(Setting::Preset), (vec!["Custom".into(), "Performance".into(), "Default".into()], 1));
        // Not retail (user 10-19): Performance mode: half scale, upscaled unsharpened, FXAA, no post effects.
        let s = &menu.settings;
        assert_eq!((s.scale, s.post_aa, s.samples, s.sharpening, s.effects), (50, 1, 1, 0, [false; 5]));
        assert_eq!((s.shadow_quality, s.ambient_occlusion, s.draw_distance, s.texture_quality), (0, false, 0, 0));
        assert!(s.performance(), "the preset's rows are Performance mode");
        menu.choose(Setting::DrawDistance, 2);
        assert_eq!(menu.choices(Setting::Preset).1, 0, "a changed row is Custom");
        assert!(!menu.settings.performance(), "and Custom is not Performance mode");
        menu.choose(Setting::Preset, 2);
        assert_eq!(menu.settings.look(), GraphicsSettings::default().look());
        assert!(!menu.settings.performance());
    }
    /// Not retail (user 10-27): a pick that turns Performance mode on or off takes a restart (core_menu asks first);
    /// `restarts` names those picks and changes nothing.
    #[test]
    fn picks_that_turn_performance_mode_on_or_off_take_a_restart() {
        let mut menu = test_menu();
        let revision = menu.revision;
        assert!(menu.restarts(Setting::Preset, 1), "Performance, from Default");
        assert!(!menu.restarts(Setting::Preset, 2) && !menu.restarts(Setting::Preset, 0), "Default again, or Custom");
        assert!(!menu.restarts(Setting::DrawDistance, 0) && !menu.restarts(Setting::FpsLimit, 1), "rows outside Performance mode");
        assert!(!menu.settings.performance() && menu.revision == revision, "asking changes nothing");
        menu.choose(Setting::Preset, 1);
        assert!(menu.restarts(Setting::Preset, 2), "Default, from Performance");
        assert!(menu.restarts(Setting::DrawDistance, 2), "a changed row leaves Performance mode (Custom)");
        assert!(!menu.restarts(Setting::DrawDistance, 0), "the row as Performance mode has it");
        assert!(!menu.restarts(Setting::FpsLimit, 1) && !menu.restarts(Setting::Weather, 1), "rows Performance mode doesn't set");
        assert!(menu.settings.performance());
    }
    /// Not retail (user 10-19): in Performance mode a world texture the asset server loads (a skater part's map) is
    /// capped under 128 a side with a mip chain and linear filtering; the UI's (built in code, or a preview card) aren't.
    #[test]
    fn performance_mode_caps_world_textures_but_not_ui_ones() {
        let dir = std::env::temp_dir().join(format!("skate3rust-performance-textures-{}", std::process::id()));
        for name in ["parts/shirt.png", "cards/preview.png"] {
            std::fs::create_dir_all(dir.join(name).parent().unwrap()).unwrap();
            ::image::RgbaImage::from_pixel(512, 256, ::image::Rgba([200, 100, 50, 255])).save(dir.join(name)).unwrap();
        }
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin { file_path: dir.to_string_lossy().into_owned(), ..default() },
            bevy::image::ImagePlugin::default(),
        ));
        // The render plugin registers the PNG loader; there is none here.
        app.register_asset_loader(bevy::image::ImageLoader::new(bevy::image::CompressedImageFormats::NONE));
        let mut menu = test_menu();
        menu.choose(Setting::Preset, 1);
        app.insert_resource(menu).add_systems(PostUpdate, performance_textures.after(bevy::asset::AssetEventSystems));
        app.finish();
        let server = app.world().resource::<AssetServer>().clone();
        let (part, card): (Handle<Image>, Handle<Image>) = (server.load("parts/shirt.png"), server.load("cards/preview.png"));
        let ui = app.world_mut().resource_mut::<Assets<Image>>().add(Image::new_fill(
            Extent3d { width: 512, height: 256, depth_or_array_layers: 1 },
            bevy::render::render_resource::TextureDimension::D2,
            &[1, 2, 3, 255],
            TextureFormat::Rgba8UnormSrgb,
            bevy::asset::RenderAssetUsages::default(),
        ));
        for _ in 0..500 {
            app.update();
            if server.is_loaded(part.id()) && server.is_loaded(card.id()) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        app.update();
        let images = app.world().resource::<Assets<Image>>();
        let size = |h: &Handle<Image>| {
            let d = &images.get(h).unwrap().texture_descriptor;
            (d.size.width, d.size.height, d.mip_level_count)
        };
        assert_eq!(size(&part), (64, 32, 7), "the part's map is capped and mipped");
        assert!(matches!(&images.get(&part).unwrap().sampler, ImageSampler::Descriptor(s) if s.mipmap_filter == bevy::image::ImageFilterMode::Linear));
        assert_eq!(size(&card), (512, 256, 1), "a UI preview card keeps its size");
        assert_eq!(size(&ui), (512, 256, 1), "a UI image built in code keeps its size");
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn weather_is_clear_unless_rain_is_chosen() {
        // Settings saved before the Weather row existed, and unknown values, stay dry.
        let mut menu = test_menu();
        menu.settings = serde_json::from_str::<GraphicsSettings>(r#"{"hud":true}"#)
            .unwrap()
            .validated();
        assert_eq!(menu.rain(), 0.);
        menu.settings = serde_json::from_str::<GraphicsSettings>(r#"{"weather":9}"#)
            .unwrap()
            .validated();
        assert_eq!(menu.rain(), 0.);
        menu.choose(Setting::Weather, 1);
        assert!(menu.rain() > 0. && menu.rain() < 1.);
        menu.choose(Setting::Weather, 2);
        assert_eq!(menu.rain(), 1.);
    }
    #[test]
    fn a_session_shows_the_hosts_world_and_guests_cannot_change_it() {
        let mut menu = test_menu();
        menu.choose(Setting::Weather, 4);
        menu.choose(Setting::TimeOfDay, crate::world_time::CYCLE as usize);
        let own = menu.settings.clone();
        // A session starts Clear and Day whatever the player's own are.
        menu.set_session_world(Some([0, 0]));
        assert_eq!((menu.snow(), menu.time_of_day()), (0., 0));
        // The host changes the session's, unsaved.
        menu.choose(Setting::Weather, 2);
        menu.choose(Setting::TimeOfDay, 1);
        assert_eq!((menu.rain(), menu.time_of_day()), (1., 1));
        assert_eq!(menu.choices(Setting::Weather).1, 2);
        assert_eq!(menu.settings, own);
        // A guest changes nothing (set_difficulty refuses guests the same way).
        menu.guest = true;
        menu.choose(Setting::Weather, 0);
        menu.choose(Setting::TimeOfDay, 0);
        assert_eq!(menu.session_world(), Some([2, 1]));
        // Out-of-range values from the network are clamped; the player's own come back after.
        menu.set_session_world(Some([99, 99]));
        assert_eq!(menu.session_world(), Some([WEATHER.len() as u32 - 1, 2]));
        menu.set_session_world(None);
        assert_eq!((menu.snow(), menu.time_of_day()), (1., crate::world_time::CYCLE));
    }
    #[test]
    fn halloween_is_clear_and_christmas_snows() {
        use crate::seasonal::Season;
        let mut menu = test_menu();
        for (season, snow) in [(Season::Halloween, 0.), (Season::Christmas, 0.5)] {
            assert_eq!(WEATHER[season.weather() as usize].0, format!("{season:?}"));
            menu.choose(Setting::Weather, season.weather() as usize);
            assert_eq!((menu.season(), menu.rain(), menu.snow()), (Some(season), 0., snow));
        }
        // A real Halloween's default holds until the player picks a weather, and is never saved.
        menu.choose(Setting::Weather, 0);
        menu.seasonal = Some(Season::Halloween.weather());
        assert_eq!((menu.season(), menu.settings.weather), (Some(Season::Halloween), 0));
        menu.choose(Setting::Weather, 0);
        assert_eq!(menu.season(), None);
    }
    /// User 10-25: Halloween and Christmas only on the Super Ultra Mega Park's map. Elsewhere they are not offered and
    /// a kept or session choice is Clear (no Christmas snow); back on the park's map the choice returns.
    #[test]
    fn seasons_only_on_the_mega_parks_map() {
        use crate::seasonal::Season;
        let mut menu = test_menu();
        menu.choose(Setting::Weather, Season::Christmas.weather() as usize);
        assert_eq!(menu.choices(Setting::Weather).0.len(), WEATHER.len());
        menu.seasons = false;
        assert_eq!((menu.season(), menu.snow()), (None, 0.));
        assert_eq!(menu.choices(Setting::Weather), (WEATHER[..5].iter().map(|w| w.0.to_string()).collect(), 0));
        menu.set_session_world(Some([Season::Halloween.weather(), 0]));
        assert_eq!(menu.season(), None);
        menu.set_session_world(None);
        menu.seasons = true;
        assert_eq!((menu.season(), menu.snow()), (Some(Season::Christmas), 0.5));
    }
    #[test]
    fn snow_choices_cover_the_world_without_rain() {
        let mut menu = test_menu();
        menu.choose(Setting::Weather, 3);
        assert_eq!((menu.rain(), menu.snow()), (0., 0.5));
        menu.choose(Setting::Weather, 4);
        assert_eq!((menu.rain(), menu.snow()), (0., 1.));
        menu.choose(Setting::Weather, 2);
        assert_eq!(menu.snow(), 0.);
    }
    #[test]
    fn scaled_target_and_cycle_boundaries() {
        let s = GraphicsSettings {
            scale: 50,
            ..default()
        };
        assert_eq!(
            s.internal_size(UVec2::new(1920, 1080)),
            UVec2::new(960, 540)
        );
        assert_eq!(s.internal_size(UVec2::ZERO), UVec2::ONE);
        assert_eq!(cycle(LIMITS, 0, -1), 240);
        assert_eq!(cycle(LIMITS, 240, 1), 0);
    }
    /// F10: never saved -> retail's Low camera; a saved High loads unchanged.
    #[test]
    fn unsaved_camera_is_low_and_saved_high_loads_unchanged() {
        assert!(GraphicsSettings::default().camera_low);
        assert_eq!(test_menu().choices(Setting::CameraAngle).1, 0, "Low");
        let saved: GraphicsSettings = serde_json::from_str(r#"{"camera_low":false,"camera_style":0}"#).unwrap();
        let mut menu = test_menu();
        menu.settings = saved.validated();
        assert_eq!(menu.choices(Setting::CameraAngle).1, 1, "High");
    }
}
