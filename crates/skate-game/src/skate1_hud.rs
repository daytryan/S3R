//! Skate 1's own HUD (Settings > Gameplay > HUD Style: on its San Vanelona map, or everywhere): its trick
//! display and, on its map, minimap. Prepared by skate2port/old_hud1.py into private/hud-skate1. While it is up the Skate 3 scoring HUD
//! stands aside; a missing or failing screen falls back to it.
use crate::{
    apt_canvas::Canvas,
    config::Config,
    map_transition::CurrentMap,
    old_minimap::Minimap,
    physics::SkaterRuntime,
    scoring_hud::{HudComposite, HudMaterial},
    skate2_hud::Screen,
    trick_stats,
};
use bevy::{prelude::*, window::PrimaryWindow};
use std::path::{Path, PathBuf};

const LAYER: usize = 36; // one above the Skate 2 HUD's 35
// ponytail: reload budget per map; raise it if a known-benign error keeps recurring.
const MAX_RELOADS: u32 = 20;
const ORDER: isize = -8; // below the Skate 2 HUD's -7

pub(crate) fn skate1_map(map: &CurrentMap) -> bool {
    map.path.as_deref().and_then(Path::file_stem).is_some_and(|s| s == "Skate_1_San_Vanelona")
}

/// What the tutorial's gate lets draw: (trick display, minimap).
fn shown(gate: Option<(bool, bool)>) -> (bool, bool) {
    gate.unwrap_or((true, true))
}

#[derive(Resource)]
pub(crate) struct Skate1Hud {
    canvas: Canvas,
    root: PathBuf,
    trick: Option<Screen>,
    /// The measurements behind the trick display's personal-best callout, and the profile's bests.
    stats: (trick_stats::Sampler, trick_stats::Bests),
    /// Where the profile's bests are kept: settings/skate1-personal-bests.json, beside its skater.
    bests: PathBuf,
    minimap: Option<Minimap>,
    /// Map generation and toggle the screens were loaded for.
    loaded: Option<(u64, bool)>,
    /// Errors recovered from by reloading the trick display this map.
    failures: u32,
    /// The skater bailed and the trick display is hidden until they are back up.
    bailed: bool,
    /// The Skate 1 tutorial's (trick display, minimap) gate (skate1_screens: hudbase SetState 2/3/4 through
    /// 0x82471a98 / 0x82471cb0); None: both draw as before.
    pub(crate) gate: Option<(bool, bool)>,
    /// The Skate 1 tutorial's offered challenges (icon, world x, z) for the minimap; None: its static icons.
    pub(crate) challenges: Option<Vec<(String, [f32; 2])>>,
}
pub(crate) struct Skate1HudPlugin;
impl Plugin for Skate1HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostStartup, setup.after(crate::graphics_menu::PresentationSetup))
            .add_systems(
                FixedUpdate,
                advance.after(crate::app::SimulationSet::Physics).run_if(crate::graphics_menu::gameplay_active),
            )
            .add_systems(Update, (resize_target, reset, render).chain().after(crate::app::FrameSet::Physics));
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
    let root = config.asset_root.join("private/hud-skate1");
    let canvas = Canvas::new(&mut commands, &mut images, &mut composites, &window, root.clone(), LAYER, ORDER);
    commands.spawn((
        MaterialNode(canvas.composite.clone()),
        crate::apt_canvas::CompositeOf(canvas.camera()),
        UiTargetCamera(output),
        GlobalZIndex(1),
        Pickable::IGNORE,
        Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() },
    ));
    let bests = crate::saves::settings_dir(&config.asset_root).join("skate1-personal-bests.json");  // Not retail (user 10-13)
    commands.insert_resource(Skate1Hud {
        canvas,
        root,
        trick: None,
        stats: Default::default(),
        bests,
        minimap: None,
        loaded: None,
        failures: 0,
        bailed: false,
        gate: None,
        challenges: None,
    });
}

fn resize_target(
    hud: Option<ResMut<Skate1Hud>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut images: ResMut<Assets<Image>>,
    mut composites: ResMut<Assets<HudComposite>>,
) {
    if let Some(mut hud) = hud {
        hud.canvas.resize(&window, &mut images, &mut composites);
    }
}

/// (Re)load the screens when the map or the toggle changes: fresh movies on every map, as the Skate 3 HUD does.
fn reset(hud: Option<ResMut<Skate1Hud>>, map: Res<CurrentMap>, menu: Option<Res<crate::graphics_menu::Menu>>, skater: Res<SkaterRuntime>) {
    let Some(mut hud) = hud else { return };
    let want = crate::skate2_hud::hud_game(menu.as_deref(), &map) == 1;
    if hud.loaded == Some((map.generation, want)) {
        return;
    }
    if hud.loaded.is_some_and(|(generation, _)| generation != map.generation) {
        hud.failures = 0;
    }
    hud.loaded = Some((map.generation, want));
    (hud.trick, hud.minimap) = (None, None);
    if !want {
        return;
    }
    let root = hud.root.clone();
    match Screen::load(&root, "runtime/trickdisplay.json", skater.scoring.hud_input()) {
        Ok(screen) => {
            hud.stats = (Default::default(), trick_stats::Bests::load(&screen.stats, &hud.bests));
            hud.trick = Some(screen);
        }
        Err(error) => {
            warn!("Skate 1 HUD unavailable, keeping the existing one ({error}). Prepare it with skate2port/old_hud1.py");
            return;
        }
    }
    // The minimap is an extra: without it the trick display still shows. It is Skate 1's map only.
    if !skate1_map(&map) {
        return;
    }
    hud.minimap = std::fs::read(root.join("runtime/minimap.json"))
        .map_err(|e| e.to_string())
        .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|e| e.to_string()))
        .inspect_err(|error| warn!("Skate 1 minimap unavailable ({error})"))
        .ok();
    info!("Skate 1 HUD loaded from {} (minimap {})", root.display(), hud.minimap.is_some());
}

fn advance(hud: Option<ResMut<Skate1Hud>>, skater: Res<SkaterRuntime>, time: Res<Time>) {
    let Some(mut hud) = hud else { return };
    let hud = &mut *hud;
    if let Some(trick) = &mut hud.trick {
        let s = &skater.scoring;
        // Each measurement goes to its best (submit and pend), then the frame's commit hands the new ones to the
        // screen (HUD call 16, UpdatePersonalBests).
        let (sampler, bests) = &mut hud.stats;
        for (stat, value) in sampler.sample(&trick_stats::Tap::of(s), time.delta_secs()) {
            if let Some(index) = trick_stats::best_index(stat) {
                bests.submit(index, value);
            }
        }
        let new_bests = bests.commit(crate::hud_runtime::imperial());
        let mut input = s.hud_input();
        input.trick_name = input.trick_name.to_uppercase(); // Skate 1 shows trick names in capitals
        // Skate 1's UpdateStance acts only while GetCurrentTrickStance()[2] is true; otherwise its switch/fakie
        // splat never leaves the design-time frame that shows it. The first update (load had false) plays its
        // outro, hiding it until the skater rides switch or fakie.
        input.stance[2] = true;
        // Skate 1 has one timer for the line bar and the multiplier; here that is the combo timer (its levels are
        // the bar's x1.5/x2/x3 thresholds), not Skate 3's separate line timer.
        input.line_points = s.session.combo.timer.points;
        input.line_capacity = s.data.combo_capacity;
        // A bail sends Skate 1's screen to its "bail" label, which hides it; Skate 1 shows it again once the
        // skater is back up.
        let back_up = hud.bailed && s.riding() && !s.close_tricks;
        let result = if back_up { trick.runtime.show() } else { Ok(()) }
            .and_then(|()| trick.runtime.update(input, s.new_trick, s.modified_trick, s.close_tricks))
            .and_then(|()| if s.sequence_landed { trick.runtime.landed() } else { Ok(()) })
            .and_then(|()| {
                if new_bests.is_empty() {
                    return Ok(());
                }
                trick.runtime.bindings.personal_bests.extend(new_bests);
                trick.runtime.call("UpdatePersonalBests")
            });
        hud.bailed = (hud.bailed && !back_up) || s.close_tricks;
        if let Err(error) = result {
            hud.recover(&format!("script: {error}"));
        }
    }
}

impl Skate1Hud {
    /// A script or geometry error: reload the trick display fresh next frame (the Skate 3 one covers a frame),
    /// giving up on this map after a few, so a hard fault cannot reload forever.
    fn recover(&mut self, error: &str) {
        self.failures += 1;
        self.trick = None;
        if self.failures <= MAX_RELOADS {
            error!("Skate 1 trick display failed ({error}); reloading it ({}/{MAX_RELOADS})", self.failures);
            self.loaded = None;
        } else {
            error!("Skate 1 trick display failed ({error}); back to the Skate 3 one for this map");
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render(
    mut commands: Commands,
    hud: Option<ResMut<Skate1Hud>>,
    menu: Option<Res<crate::graphics_menu::Menu>>,
    customiser: Option<Res<crate::customiser::Customiser>>,
    cutscene: Option<Res<crate::challenge::cutscene::Cutscene>>,
    (skater, time, roster): (Res<SkaterRuntime>, Res<Time>, Option<Res<crate::online_roster::OnlineRoster>>),
    online: Option<Res<crate::skate1_online::Skate1Online>>,
    remotes: Query<(&crate::multiplayer::appearance::RemoteCharacter, &GlobalTransform)>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<HudMaterial>>,
    mut cameras: Query<&mut Camera>,
) {
    let Some(mut hud) = hud else { return };
    let hud = &mut *hud;
    // overnight-1015 C4: hidden under cutscenes (FE NIS state 2), skate2_hud's rule. INFERRED, not traced in s1.pe.
    let hidden = customiser.is_some_and(|c| c.open)
        || menu.is_some_and(|m| !m.hud_visible())
        || cutscene.is_some_and(|c| c.active());
    let (trick, map) = shown(hud.gate);
    let mut draws = Vec::new();
    if !hidden && trick {
        match hud.trick.as_ref().map(Screen::draws) {
            Some(Ok(d)) => draws.extend(d),
            Some(Err(error)) => hud.recover(&format!("geometry: {error}")),
            None => {}
        }
    }
    // The minimap follows the skater (not the camera) even while hidden, so its heading and zoom are settled.
    if let Some(minimap) = &mut hud.minimap {
        let (at, facing) = crate::old_minimap::skater_pose(&skater);
        minimap.update(at, facing, time.delta_secs());
        // An online race's gates ahead, as mini_icon_racegate markers.
        minimap.set_gates(online.as_deref().map(crate::skate1_online::gate_marks).unwrap_or_default());
        minimap.set_challenges(hud.challenges.clone());
        if !hidden && map {
            draws.extend(minimap.draws(&crate::old_minimap::others(roster.as_deref(), remotes.iter()), None));
        }
    }
    hud.canvas.set_active(!draws.is_empty(), &mut cameras);
    hud.canvas.draw(&draws, &mut commands, &mut images, &mut meshes, &mut materials);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// P5: the tutorial's gate hides the trick display and / or the minimap; without one both draw as before.
    #[test]
    fn the_tutorial_gate_hides_what_it_says_and_nothing_without_it() {
        assert_eq!(shown(None), (true, true));
        assert_eq!(shown(Some((false, true))), (false, true));
        assert_eq!(shown(Some((true, false))), (true, false));
    }

    fn map(path: Option<&str>) -> CurrentMap {
        CurrentMap { path: path.map(PathBuf::from), name: String::new(), spawn: [0.; 3], heading: 0., generation: 0 }
    }

    fn blank_input() -> crate::hud_runtime::Input {
        crate::hud_runtime::Input {
            sequence_score: 0,
            line_score: 0,
            sequence_timer: 0,
            line_time: 0.,
            line_capacity: 1.,
            line_points: 0.,
            line_thresholds: [0.; 3],
            multiplier: 1.,
            clean: true,
            sketchy: false,
            stance: [false; 4],
            trick_name: String::new(),
            trick_metrics: std::array::from_fn(|_| crate::apt_vm::Value::Undefined),
            context_tricks: Vec::new(),
        }
    }

    #[test]
    fn each_map_shows_its_own_game_hud_without_a_setting() {
        use crate::skate2_hud::hud_game;
        assert_eq!(hud_game(None, &map(Some("maps/Skate_1_San_Vanelona.skate"))), 1);
        assert_eq!(hud_game(None, &map(Some("maps/New_San_Vanelona.skate"))), 2);
        assert_eq!(hud_game(None, &map(Some("maps/DownTown.skate"))), 3);
        assert_eq!(hud_game(None, &map(None)), 3);
    }

    #[test]
    fn a_missing_hud_folder_is_an_error_not_a_panic() {
        let dir = std::env::temp_dir().join("skate1-hud-missing");
        let error = crate::skate2_hud::Screen::load(&dir, "runtime/trickdisplay.json", blank_input()).err();
        assert!(error.is_some_and(|e| e.contains("trickdisplay.json")));
    }

    /// old_hud1.py's minimap (SKATE1_HUD_ROOT = private/hud-skate1) loads, and at Slappy's the disc shows street
    /// tiles, icons, the needle, the arrow and a district plate, every page of them on disk.
    #[test]
    #[ignore = "requires the prepared Skate 1 HUD"]
    fn skate1_minimap_loads_and_shows_icons() {
        let root = PathBuf::from(std::env::var("SKATE1_HUD_ROOT").unwrap());
        let mut minimap: Minimap = serde_json::from_slice(&std::fs::read(root.join("runtime/minimap.json")).unwrap()).unwrap();
        minimap.update(Vec3::new(1418., 0., 1046.), Vec2::NEG_Y, 0.);
        let draws = minimap.draws(&[], None);
        let has = |part: &str| draws.iter().any(|d| d.texture.contains(part));
        assert!(has("/tiles/") && has("mini_icon_chal_") && has("mini_icon_north") && has("mini_icon_arrow") && has("fonts/"));
        for d in &draws {
            assert!(root.join(&d.texture).is_file(), "{}", d.texture);
        }
    }

    #[test]
    fn only_the_skate1_map_gets_the_skate1_hud() {
        assert!(skate1_map(&map(Some("maps/Skate_1_San_Vanelona.skate"))));
        assert!(!skate1_map(&map(Some("maps/New_San_Vanelona.skate"))));
        assert!(!skate1_map(&map(Some("maps/Skate_2_Maloof.skate"))));
        assert!(!skate1_map(&map(Some("maps/DownTown.skate"))));
        assert!(!skate1_map(&map(None)));
    }
}
