//! Skate 2's own HUD (Settings > Gameplay > HUD Style: on the Skate 2 maps, or everywhere): its trickdisplay2 scoring
//! screen, its hudcentral district title card and, on New_San_Vanelona, its minimap. Prepared by
//! skate2port/old_hud.py into private/hud-skate2. While it is up the Skate 3 scoring HUD stands aside;
//! a missing or failing screen falls back to it.
use crate::{
    apt_canvas::Canvas,
    apt_scene::{self, Draw},
    config::Config,
    hud_runtime,
    map_transition::CurrentMap,
    old_minimap::Minimap,
    physics::SkaterRuntime,
    scoring_hud::{HudComposite, HudMaterial},
    trick_stats,
};
use bevy::{prelude::*, window::PrimaryWindow};
use std::path::{Path, PathBuf};

const LAYER: usize = 35;
/// Skate 2's `fe` one-shots as wavs (skate-data example s2_fe_sounds), shared with the tutorial screens.
const SFX: &str = "private/skate2-tutorial/screens/sfx";
/// The session marker's front-end ids (player_audio/frontend.rs) and Skate 2's own `fe` records of the same names,
/// played instead of Skate 3's while the Skate 2 HUD (and so its cellphone marker panel) is up.
const MARKER_SOUNDS: [(u64, &str); 4] = [
    (0x47fe_75bf_61f1_9941, "cellphone_activate"),
    (0x0d6c_88a3_b91c_828f, "cellphone_place_marker"),
    (0x7f13_5f9f_d28f_7f21, "cellphone_goto_marker"),
    (0x66b3_afe3_b602_918c, "cellphone_marker_error"),
];
/// Skate 2's record for a session marker sound id.
fn marker_sound(id: u64) -> Option<&'static str> {
    MARKER_SOUNDS.iter().find(|(i, _)| *i == id).map(|(_, key)| *key)
}

/// The Skate 2 tutorial's minimap waypoint (world x, z), set by skate2_challenge from event 0x1E/0x1F, and its active
/// lessons' markers (icon, world x, z).
#[derive(Resource, Default)]
pub(crate) struct Skate2Waypoint(pub Option<[f32; 2]>, pub Vec<(String, [f32; 2])>);

pub(crate) fn skate2_map(map: &CurrentMap) -> bool {
    let stem = map.path.as_deref().and_then(Path::file_stem).map(|s| s.to_string_lossy().into_owned());
    stem.is_some_and(|s| s == "New_San_Vanelona" || s.starts_with("Skate_2_"))
}

/// Which game (1, 2 or 3) the map comes from.
pub(crate) fn map_game(map: &CurrentMap) -> u8 {
    if crate::skate1_hud::skate1_map(map) { 1 } else if skate2_map(map) { 2 } else { 3 }
}
/// Settings > HUD Style picks the game whose HUD shows; the map's own without the menu.
pub(crate) fn hud_game(menu: Option<&crate::graphics_menu::Menu>, map: &CurrentMap) -> u8 {
    let game = map_game(map);
    menu.map_or(game, |m| m.hud_game(game))
}

/// Skate 2's race objects on the minimap (Map.AddRaceGate / AddRaceCheckPoint / AddRaceFinish: icon, world x, z), set
/// by skate2_death_race while a race's gates are up.
#[derive(Resource, Default)]
pub(crate) struct Skate2RaceMap(pub Vec<(String, [f32; 2])>);

/// Spot Battle's and Hall of Meat's reset (spot_battle.lua 365, hall_of_meat.lua 274: HUD2.Show(MiniMap, false)) hides
/// the minimap until challenge_cleanup's HUD2.DeleteGroup gives free skate's group back (online: as the results begin).
fn match_hides_minimap(m: &crate::match_core::Match) -> bool {
    use crate::match_core::{Mode, Phase};
    matches!(m.mode, Some(Mode::SpotBattle | Mode::HallOfMeat))
        && matches!(m.phase, Phase::Intro { .. } | Phase::Countdown { .. } | Phase::Running { .. })
}

/// mMiniMap's alpha (colour-transform a_mul) on hudminimap.apt's root timeline, one entry per 60 Hz frame. 'hudintro'
/// is frames 1-10 and 'hudoutro' frames 11-19; at 19 the root goes invisible. Both labels call
/// `Audio.PlayMiniMapTrans()`. The component's SetActive (0x82589910) sends state 1 (intro) on show and state 2
/// (outro) on hide.
const MINIMAP_INTRO: [u8; 10] = [76, 63, 51, 38, 25, 89, 145, 191, 227, 255];
const MINIMAP_OUTRO: [u8; 9] = [195, 136, 76, 102, 127, 153, 102, 50, 0];

/// The minimap's show/hide envelope, advanced by time at the movie's 60 fps.
struct MinimapFade {
    shown: bool,
    frames: f32,
}
impl Default for MinimapFade {
    fn default() -> Self {
        Self { shown: false, frames: f32::INFINITY }
    }
}
impl MinimapFade {
    /// The frame picture's alpha this render, or None once the outro has finished. Only mMiniMap (depth 1) carries the
    /// colour transform; the markers keep full alpha until the root hides.
    /// ponytail: a show during the outro restarts the intro. The AS SetState(1) checks `_visible` first, so retail
    /// may finish the outro instead.
    fn step(&mut self, show: bool, dt: f32) -> Option<f32> {
        if show != self.shown {
            (self.shown, self.frames) = (show, 0.);
        } else {
            self.frames += dt * 60.;
        }
        let n = self.frames as usize;
        let alpha = if show { Some(*MINIMAP_INTRO.get(n).unwrap_or(&255)) } else { MINIMAP_OUTRO.get(n).copied() };
        alpha.filter(|&a| show || a > 0).map(|a| a as f32 / 255.)
    }
}

pub(crate) struct Screen {
    pub(crate) runtime: hud_runtime::Runtime,
    shapes: apt_scene::Shapes,
    /// The disc's trick-stat data for the screen (trick_stats.rs), Null when the manifest has none.
    pub(crate) stats: serde_json::Value,
}
impl Screen {
    pub(crate) fn load(root: &Path, file: &str, input: hud_runtime::Input) -> Result<Self, String> {
        let source: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join(file)).map_err(|e| format!("{file}: {e}"))?)
            .map_err(|e| format!("{file}: {e}"))?;
        let runtime = hud_runtime::Runtime::load(&source, input).map_err(|e| format!("{file}: {e}"))?;
        let shapes = serde::Deserialize::deserialize(&source["shapes"]).map_err(|e| format!("{file}: {e}"))?;
        Ok(Self { runtime, shapes, stats: source["trick_stats"].clone() })
    }
    pub(crate) fn draws(&self) -> Result<Vec<Draw>, String> {
        apt_scene::draw(&self.runtime.bindings.movie, &self.runtime.vm, &self.shapes)
    }
}

#[derive(Resource)]
pub(crate) struct Skate2Hud {
    canvas: Canvas,
    root: PathBuf,
    sfx: PathBuf,
    trick: Option<Screen>,
    /// The measurements behind the trick display's big-trick callouts.
    stats: (trick_stats::Sampler, trick_stats::BigTricks),
    district: Option<Screen>,
    minimap: Option<Minimap>,
    minimap_fade: MinimapFade,
    /// The key the screens were loaded for ([`reset_key`]).
    loaded: Option<(u64, bool, bool)>,
    /// The district last seen; None until the first sighting after a load, so spawning shows no card.
    district_seen: Option<Option<String>>,
}
impl Skate2Hud {
    /// The Skate 2 trick display is up, so the Skate 3 one stands aside.
    pub(crate) fn active(&self) -> bool {
        self.trick.is_some()
    }
}

pub(crate) struct Skate2HudPlugin;
impl Plugin for Skate2HudPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Skate2Waypoint>()
            .init_resource::<Skate2RaceMap>()
            .add_message::<crate::skate_audio::FrontEndSoundRequest>()
            .init_resource::<crate::skate_audio::FrontEndSoundsVoicedElsewhere>()
            .add_systems(PostStartup, setup.after(crate::graphics_menu::PresentationSetup))
            .add_systems(
                FixedUpdate,
                advance.after(crate::app::SimulationSet::Physics).run_if(crate::graphics_menu::gameplay_active),
            )
            .add_systems(Update, (resize_target, reset, render, play_sounds).chain().after(crate::app::FrameSet::Physics));
    }
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
    let root = config.asset_root.join("private/hud-skate2");
    let canvas = Canvas::new(&mut commands, &mut images, &mut composites, &window, root.clone(), LAYER, -7);
    commands.spawn((
        MaterialNode(canvas.composite.clone()),
        crate::apt_canvas::CompositeOf(canvas.camera()),
        UiTargetCamera(output),
        GlobalZIndex(1),
        Pickable::IGNORE,
        Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() },
    ));
    let sfx = config.asset_root.join(SFX);
    commands.insert_resource(Skate2Hud {
        canvas,
        root,
        sfx,
        trick: None,
        stats: Default::default(),
        district: None,
        minimap: None,
        minimap_fade: MinimapFade::default(),
        loaded: None,
        district_seen: None,
    });
}

fn resize_target(
    hud: Option<ResMut<Skate2Hud>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut images: ResMut<Assets<Image>>,
    mut composites: ResMut<Assets<HudComposite>>,
) {
    if let Some(mut hud) = hud {
        hud.canvas.resize(&window, &mut images, &mut composites);
    }
}

/// What the screens are loaded for: the map's generation, the toggle, and whether Skate 2 physics has the skater.
/// Not native Skate 2 (user 10-20): a switch between Skate 3 and Skate 2 physics, either way, reloads the screens as a
/// new map does, so a trick the other physics named (mid-trick at the switch) does not stay on the trick display
/// (review I1; `scoring_hud::reset` does the same for Skate 3's). Not tested in play.
fn reset_key(generation: u64, want: bool, skate2: Option<&crate::skate2_physics::Skate2Physics>) -> (u64, bool, bool) {
    (generation, want, skate2.is_some_and(crate::skate2_physics::Skate2Physics::drives_local))
}

/// (Re)load the screens when the map or the toggle changes: fresh movies on every map, as the Skate 3 HUD does.
fn reset(
    hud: Option<ResMut<Skate2Hud>>,
    map: Res<CurrentMap>,
    menu: Option<Res<crate::graphics_menu::Menu>>,
    skater: Res<SkaterRuntime>,
    skate2: Option<Res<crate::skate2_physics::Skate2Physics>>,
) {
    let Some(mut hud) = hud else { return };
    let want = hud_game(menu.as_deref(), &map) == 2;
    let key = reset_key(map.generation, want, skate2.as_deref());
    if hud.loaded == Some(key) {
        return;
    }
    hud.loaded = Some(key);
    hud.district_seen = None;
    (hud.trick, hud.district, hud.minimap) = (None, None, None);
    hud.stats = Default::default();
    hud.minimap_fade = MinimapFade::default();
    if !want {
        return;
    }
    let root = hud.root.clone();
    match Screen::load(&root, "runtime/trickdisplay.json", skater.scoring.hud_input()) {
        Ok(screen) => hud.trick = Some(screen),
        Err(error) => {
            warn!("Skate 2 HUD unavailable, keeping Skate 3's ({error}). Prepare it with skate2port/old_hud.py");
            return;
        }
    }
    // The title card and minimap are extras: without them the trick display still shows.
    hud.district = Screen::load(&root, "runtime/central.json", skater.scoring.hud_input())
        .inspect_err(|error| warn!("Skate 2 district card unavailable ({error})"))
        .ok();
    if map.path.as_deref().and_then(Path::file_stem).is_some_and(|s| s == "New_San_Vanelona") {
        hud.minimap = std::fs::read(root.join("runtime/minimap.json"))
            .map_err(|e| e.to_string())
            .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|e| e.to_string()))
            .inspect_err(|error| warn!("Skate 2 minimap unavailable ({error})"))
            .ok();
    }
    info!("Skate 2 HUD loaded from {} (district card {}, minimap {})", root.display(), hud.district.is_some(), hud.minimap.is_some());
}

fn advance(
    hud: Option<ResMut<Skate2Hud>>,
    skater: Res<SkaterRuntime>,
    district: Res<crate::retail_render::District>,
    time: Res<Time>,
) {
    let Some(mut hud) = hud else { return };
    let hud = &mut *hud;
    if let Some(trick) = &mut hud.trick {
        let s = &skater.scoring;
        // A measurement past its type's Big or Huge threshold is a callout (the score module's scan, 0x8233e3a8,
        // into AddBigTrick); the manager's update asks the screen to redraw them (HUD call 4, ContextTrickDone).
        let (sampler, callouts) = &mut hud.stats;
        for (stat, value) in sampler.sample(&trick_stats::Tap::of(s), time.delta_secs()) {
            if let Some((kind, achievement)) = trick_stats::big_trick(&trick.stats, stat, value) {
                callouts.add(kind, achievement, value);
            }
        }
        let redraw = callouts.tick((time.delta_secs() * 1000.).round() as i32);
        let mut input = s.hud_input();
        input.context_tricks = callouts.data();
        let result = trick.runtime.update_skate2(input, s.new_trick, s.modified_trick, s.close_tricks);
        if let Err(error) = result.and_then(|()| if redraw { trick.runtime.call("ContextTrickDone") } else { Ok(()) }) {
            error!("Skate 2 trick display stopped, back to Skate 3's: {error}");
            hud.trick = None;
        }
    }
    let Some(card) = &mut hud.district else { return };
    let result = (|| -> Result<(), String> {
        match &hud.district_seen {
            Some(seen) if *seen == district.0 => {}
            Some(_) => {
                if let Some(label) = &district.0 {
                    let title = card.runtime.bindings.movie.text_assets.localize(label);
                    card.runtime.show_district(&title)?;
                }
                hud.district_seen = Some(district.0.clone());
            }
            None => hud.district_seen = Some(district.0.clone()),
        }
        card.runtime.advance()
    })();
    if let Err(error) = result {
        error!("Skate 2 district card stopped: {error}");
        hud.district = None;
    }
}

/// The screens' own sounds (trickdisplay2's multiplier steps) and, while the HUD is up, the session marker's in
/// Skate 2's voice (player_audio leaves those ids to us then), and the phone list's opening.
fn play_sounds(
    mut commands: Commands,
    hud: Option<ResMut<Skate2Hud>>,
    mut requests: MessageReader<crate::skate_audio::FrontEndSoundRequest>,
    mut pcm: ResMut<Assets<crate::skate_audio::StreamPcm>>,
    mut elsewhere: ResMut<crate::skate_audio::FrontEndSoundsVoicedElsewhere>,
    menu: Option<Res<crate::graphics_menu::Menu>>,
) {
    // Skate 2's "phonelist" HUD component (input handler 0x8258B500) plays `cellphone_activate` as it opens
    // (closed -> open, then SetState); closing is silent. Its panel here is the session marker's, and the marker posts
    // Skate 3's `cellphone_activate` on that edge (session_marker): voiced here in Skate 2's, as the other marker sounds.
    // It opens only if its check (0x8258B770) passes on the press: no mission flag up, the game mode (0x82DF3750) is 0, 3,
    // 4 or 5, or it is 1 or 7 and the skater's state (+0x2118) isn't 3..=10. ponytail: the skater part is the marker's own state check
    // (grounded on the board or standing); the mission flags and game modes have no port counterpart mapped.
    let Some(mut hud) = hud else {
        requests.clear();
        return;
    };
    let hud = &mut *hud;
    let active = hud.active();
    if active != !elsewhere.0.is_empty() {
        elsewhere.0 = if active { MARKER_SOUNDS.iter().map(|(id, _)| *id).collect() } else { Vec::new() };
    }
    let markers: Vec<&str> = requests.read().filter_map(|r| marker_sound(r.0)).filter(|_| active).collect();
    let screens = [&mut hud.trick, &mut hud.district].into_iter().flatten();
    let keys: Vec<String> =
        screens.flat_map(|s| s.runtime.bindings.sounds.drain(..)).chain(markers.into_iter().map(String::from)).collect();
    for key in keys {
        crate::apt_screen::play_sfx(&mut commands, &mut pcm, &hud.sfx, &key, crate::music::one_shot(menu.as_deref(), crate::music::SFX));
    }
}

#[allow(clippy::too_many_arguments)]
fn render(
    mut commands: Commands,
    hud: Option<ResMut<Skate2Hud>>,
    menu: Option<Res<crate::graphics_menu::Menu>>,
    customiser: Option<Res<crate::customiser::Customiser>>,
    (skater, time, roster, matched, race): (
        Res<SkaterRuntime>,
        Res<Time>,
        Option<Res<crate::online_roster::OnlineRoster>>,
        Option<Res<crate::match_core::Match>>,
        Res<Skate2RaceMap>,
    ),
    remotes: Query<(&crate::multiplayer::appearance::RemoteCharacter, &GlobalTransform)>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<HudMaterial>>,
    mut cameras: Query<&mut Camera>,
    waypoint: Res<Skate2Waypoint>,
    replay: Res<crate::replay::Replay>,
    cutscene: Option<Res<crate::challenge::cutscene::Cutscene>>,
    hall_of_meat: Option<Res<crate::skate2_hall_of_meat::HallOfMeat>>,
) {
    let Some(mut hud) = hud else { return };
    let hud = &mut *hud;
    // Free roam's bail panel hides the minimap while it is up and restores it after (0x82650028 / 0x8264fe40).
    let bail_panel = hall_of_meat.is_some_and(|h| matches!(h.free, Some(Some(_))));
    // Retail hides the HUD (the minimap with it) under cutscenes, the highlight reel and its photos.
    let hidden = customiser.is_some_and(|c| c.open)
        || menu.is_some_and(|m| !m.hud_visible())
        || replay.scripted
        || cutscene.is_some_and(|c| c.active());
    let matched = matched.as_deref();
    // A match's mode hides the trick display (HUD2.Show(TrickDisplayHUD, false): S.K.A.T.E.'s setup_skate).
    let trick_hidden = matched.is_some_and(|m| {
        m.trick_display_hidden && m.phase != crate::match_core::Phase::Idle || matches!(m.phase, crate::match_core::Phase::Intro { .. })
    });
    let match_hidden = matched.is_some_and(match_hides_minimap);
    let mut draws = Vec::new();
    if !hidden {
        let trick = hud.trick.as_ref().filter(|_| !trick_hidden);
        for screen in [trick, hud.district.as_ref()].into_iter().flatten() {
            match screen.draws() {
                Ok(d) => draws.extend(d),
                Err(error) => error!("Skate 2 HUD geometry: {error}"),
            }
        }
    }
    // The minimap follows the skater (not the camera) even while hidden, so its heading is settled when it shows.
    if let Some(minimap) = &mut hud.minimap {
        let (at, facing) = crate::old_minimap::skater_pose(&skater);
        minimap.update(at, facing, time.delta_secs());
        // The bail panel's and a mode's hide play the outro. The whole-HUD hides (menus, cutscenes) cut it at once, and
        // the intro plays when they end.
        let fade = hud.minimap_fade.step(!hidden && !bail_panel && !match_hidden, time.delta_secs());
        if let (false, Some(alpha)) = (hidden, fade) {
            minimap.set_markers(waypoint.1.iter().chain(&race.0).cloned().collect());
            let mut map = minimap.draws(&crate::old_minimap::others(roster.as_deref(), remotes.iter()), waypoint.0);
            map.iter_mut().filter(|d| d.texture == "frame").for_each(|d| d.multiply[3] *= alpha);
            draws.extend(map);
        }
    }
    hud.canvas.set_active(!draws.is_empty(), &mut cameras);
    hud.canvas.draw(&draws, &mut commands, &mut images, &mut meshes, &mut materials);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// hudminimap's intro/outro envelope on the 60 Hz frames, the same at 144 fps as at 24 fps.
    #[test]
    fn skate2_minimap_fades_like_hudminimap() {
        for fps in [24., 60., 144.] {
            let mut fade = MinimapFade::default();
            let dt = 1. / fps;
            assert_eq!(fade.step(false, dt), None, "hidden before it is first shown");
            assert_eq!(fade.step(true, dt), Some(76. / 255.));
            let mut t = 0.;
            let mut last = None;
            while t < 0.3 {
                last = fade.step(true, dt);
                t += dt;
            }
            assert_eq!(last, Some(1.), "{fps} fps: the intro (10 frames) is over by 0.3 s");
            assert_eq!(fade.step(false, dt), Some(195. / 255.));
            let mut ticks = 0;
            while fade.step(false, dt).is_some() {
                ticks += 1;
                assert!(ticks < 1000);
            }
            let secs = (ticks + 1) as f32 * dt;
            assert!((secs - 8. / 60.).abs() <= dt, "{fps} fps: outro took {secs} s");
        }
    }

    /// Review I1: the Skate 2 trick display reloads on a physics switch either way, as on a new map.
    #[test]
    fn a_physics_switch_reloads_the_skate2_hud() {
        use crate::skate2_physics::Skate2Physics;
        use crate::skate2_physics::sim::{HostState, Link, Refused};
        let mut s2 = Skate2Physics::with(Box::new(|_: &Path, _: &skate2_physics_protocol::Boot| Err(Refused::Busy)), Box::new(std::time::Instant::now));
        let off = reset_key(4, true, Some(&s2));
        assert_eq!(off, reset_key(4, true, None), "Skate 3 physics: the same key as without the switch");
        s2.state = HostState::Driving(Link::from_channels(std::sync::mpsc::channel().0, std::sync::mpsc::channel().1));
        let on = reset_key(4, true, Some(&s2));
        assert_ne!(on, off, "Skate 2 physics takes the skater: the display reloads");
        assert_ne!(reset_key(5, true, Some(&s2)), on, "a new map still reloads it");
    }

    /// GAP-8 (online audit 10-28): Spot Battle's and Hall of Meat's reset hide the minimap (spot_battle.lua 365,
    /// hall_of_meat.lua 274) until challenge_cleanup's DeleteGroup (online: the results); the other modes keep it.
    #[test]
    fn spot_battle_and_hall_of_meat_hide_the_minimap_until_the_results() {
        use crate::match_core::{Match, Mode, Phase};
        let m = |mode, phase| Match { mode: Some(mode), phase, ..Default::default() };
        let running = || Phase::Running { started: 0., ends: None };
        assert!(match_hides_minimap(&m(Mode::SpotBattle, running())));
        assert!(match_hides_minimap(&m(Mode::HallOfMeat, Phase::Intro { since: 0. })));
        assert!(match_hides_minimap(&m(Mode::HallOfMeat, Phase::Countdown { ends: 3. })));
        assert!(!match_hides_minimap(&m(Mode::SpotBattle, Phase::Results { until: 8. })), "cleanup's DeleteGroup");
        assert!(!match_hides_minimap(&Match::default()), "free skate");
        for mode in [Mode::BestTrick, Mode::Jam, Mode::DeathRace, Mode::Skate, Mode::OneUp] {
            assert!(!match_hides_minimap(&m(mode, running())), "{mode:?}");
        }
    }

    #[test]
    fn only_the_skate2_maps_get_the_skate2_hud() {
        let map = |path: Option<&str>| CurrentMap { path: path.map(PathBuf::from), name: String::new(), spawn: [0.; 3], heading: 0., generation: 0 };
        assert!(skate2_map(&map(Some("maps/New_San_Vanelona.skate"))));
        assert!(skate2_map(&map(Some("maps/Skate_2_Maloof.skate"))));
        assert!(!skate2_map(&map(Some("maps/Skate_1_San_Vanelona.skate"))));
        assert!(!skate2_map(&map(Some("maps/DownTown.skate"))));
        assert!(!skate2_map(&map(None)));
    }

    /// Review Focus 5 (Task 9): x1 -> x1.5 -> x2 -> x3 -> x2 -> x1 inside a second. trickdisplay2's UpdateMultiplier asks
    /// for one sound a step; the ones Skate 2 has a sample for play (x1.5's record has none: silent in retail), from files
    /// with no flat-topped (clipped) run, at their record's level (sfx/gains.json). SFX at 0 is one_shot's Linear(0).
    #[test]
    #[ignore = "requires the prepared Skate 2 HUD (SKATE2_HUD_ROOT) and tutorial screens (SKATE2_SCREENS)"]
    fn multiplier_steps_play_one_unclipped_sound_each() {
        use crate::apt_vm::Value;
        let input = |multiplier: f32| hud_runtime::Input {
            sequence_score: 500,
            line_score: 12_345,
            sequence_timer: 3,
            line_time: 3.5,
            line_capacity: 10_000.,
            line_points: 1_250.,
            line_thresholds: [50., 450., 800.],
            multiplier,
            clean: true,
            sketchy: false,
            stance: [false, true, false, false],
            trick_name: "ID_TRICK_KICKFLIP".into(),
            trick_metrics: [Value::Text("ID_TRICK_KICKFLIP".into()), Value::Bool(false), Value::Bool(false), Value::Bool(false), Value::Bool(false)],
            context_tricks: Vec::new(),
        };
        let root = PathBuf::from(std::env::var("SKATE2_HUD_ROOT").unwrap());
        let sfx = PathBuf::from(std::env::var("SKATE2_SCREENS").unwrap()).join("sfx");
        let mut screen = Screen::load(&root, "runtime/trickdisplay.json", input(1.)).unwrap();
        screen.runtime.update_skate2(input(1.), true, false, false).unwrap();
        screen.runtime.bindings.sounds.clear();
        let mut played = Vec::new();
        for multiplier in [1.5, 2., 3., 2., 1.] {
            for _ in 0..10 {
                screen.runtime.update_skate2(input(multiplier), false, false, false).unwrap();
            }
            let keys: Vec<String> = screen.runtime.bindings.sounds.drain(..).collect();
            let audible: Vec<String> = keys.iter().filter(|k| sfx.join(format!("{k}.wav")).is_file()).cloned().collect();
            assert!(audible.len() <= 1, "x{multiplier}: {keys:?}");
            played.extend(audible);
        }
        assert_eq!(played, ["multiplyer_2", "multiplyer_3", "multiplyer_2"]);
        for key in ["multiplyer_2", "multiplyer_3"] {
            // s2_fe_sounds writes a 44-byte header, then interleaved stereo i16.
            let wav = std::fs::read(sfx.join(format!("{key}.wav"))).unwrap();
            let pcm: Vec<i16> = wav[44..].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
            for channel in 0..2 {
                let full = |frame: usize| pcm[frame * 2 + channel].unsigned_abs() >= 32767;
                let flat = (1..pcm.len() / 2).filter(|&frame| full(frame - 1) && full(frame)).count();
                assert_eq!(flat, 0, "{key}: channel {channel} is clipped");
            }
        }
        // multiplyer_3's mix ran past full scale (dumpster-sound.md B: 376 clipped samples): playback gets back what the
        // export took, above its record's 0.85, but never past full scale with SFX at 10 (review I1).
        let gain = crate::apt_screen::fe_gain(&sfx, "multiplyer_3");
        assert!(gain > 0.85 && gain <= 1.0, "multiplyer_3 plays at x{gain}");
    }
}
