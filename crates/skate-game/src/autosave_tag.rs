//! The autosave icon: the busy tag (controls/autosave) spinning in the corner while a save is written, as
//! popup 23 promises ("When you see this icon, the title is saving content"). Saves anywhere call [`saving`].
use crate::{
    apt_canvas::{Canvas, CompositeOf, movie_textures},
    apt_vm::{Value, Vm},
    config::Config,
    core_menu_runtime::drain,
    map_transition::MapTransition,
    scoring_hud::{HudComposite, HudMaterial},
    tip_loader::LoadingHold,
    tip_loader_runtime::Wheel,
};
use bevy::{prelude::*, window::PrimaryWindow};
use std::sync::atomic::{AtomicU32, Ordering};

const LAYER: usize = 54;
const FRAME: f32 = 1. / 60.;
/// The save tag's minimum time up (hide path 0x825223F0: timer +2768 against [0x82063B08] = 3.0).
const HOLD: f32 = 3.0;

static SAVES: AtomicU32 = AtomicU32::new(0);
/// A save was just written: show the autosave icon.
pub(crate) fn saving() {
    SAVES.fetch_add(1, Ordering::Relaxed);
}

/// When the tag is up: [`HOLD`] seconds after the latest save.
#[derive(Default)]
struct Pulse {
    seen: u32,
    left: f32,
}
impl Pulse {
    fn tick(&mut self, saves: u32, dt: f32) -> bool {
        if saves != self.seen {
            self.seen = saves;
            self.left = HOLD;
        } else {
            self.left = (self.left - dt).max(0.);
        }
        self.left > 0.
    }
}

#[derive(Resource)]
struct Tag {
    vm: Vm,
    wheel: Wheel,
    canvas: Canvas,
    pulse: Pulse,
    on: bool,
    accumulated: f32,
}

pub(crate) struct AutosaveTagPlugin;
impl Plugin for AutosaveTagPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostStartup, setup.after(crate::graphics_menu::PresentationSetup)).add_systems(Update, animate);
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
    let root = std::env::var_os("SKATE_TIP_LOADER_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| config.asset_root.join("private/tip-loader"));
    let mut canvas = Canvas::new(&mut commands, &mut images, &mut composites, &window, root.clone(), LAYER, -5);
    let result = (|| -> Result<(Vm, Wheel), String> {
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("runtime/autosave.json")).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        if json["format"] != "skate3-busy-tag" || json["version"] != 1 {
            return Err("Unsupported busy tag manifest".into());
        }
        for (path, size) in movie_textures(&json)? {
            canvas.preload(&mut images, &path, size)?;
        }
        load(&json)
    })();
    let (vm, wheel) = match result {
        Ok(value) => value,
        Err(error) => return warn!("Autosave icon unavailable ({error})"),
    };
    commands.spawn((
        UiTargetCamera(output),
        GlobalZIndex(26),
        Pickable::IGNORE,
        CompositeOf(canvas.camera()),
        MaterialNode(canvas.composite.clone()),
        Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() },
    ));
    commands.insert_resource(Tag { vm, wheel, canvas, pulse: Pulse::default(), on: false, accumulated: 0. });
}

#[allow(clippy::too_many_arguments)]
fn animate(
    mut commands: Commands,
    tag: Option<ResMut<Tag>>,
    transition: Res<MapTransition>,
    hold: Res<LoadingHold>,
    window: Single<&Window, With<PrimaryWindow>>,
    real: Res<Time<Real>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<HudMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut composites: ResMut<Assets<HudComposite>>,
    mut cameras: Query<&mut Camera>,
) {
    let Some(mut tag) = tag else { return };
    let tag = &mut *tag;
    // The loading screen draws the same wheel in the same corner.
    let on = tag.pulse.tick(SAVES.load(Ordering::Relaxed), real.delta_secs()) && !transition.busy() && !hold.0;
    let result = (|| -> Result<(), String> {
        if on != tag.on {
            tag.on = on;
            show(&mut tag.vm, &mut tag.wheel, on)?;
        }
        if !on {
            return Ok(());
        }
        tag.accumulated = (tag.accumulated + real.delta_secs()).min(4. * FRAME);
        while tag.accumulated >= FRAME {
            tag.accumulated -= FRAME;
            step(&mut tag.vm, &mut tag.wheel)?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        error!("Autosave icon stopped: {error}");
        commands.remove_resource::<Tag>();
        tag.canvas.set_active(false, &mut cameras);
        return;
    }
    tag.canvas.set_active(on, &mut cameras);
    tag.canvas.resize(&window, &mut images, &mut composites);
    let draws = if on { crate::apt_scene::draw(&tag.wheel.movie, &tag.vm, &tag.wheel.shapes).unwrap_or_default() } else { Vec::new() };
    tag.canvas.draw(&draws, &mut commands, &mut images, &mut meshes, &mut materials);
}

/// The busy tag, hidden: Wheel::load starts the loading screen's wheel spinning.
fn load(json: &serde_json::Value) -> Result<(Vm, Wheel), String> {
    let (mut vm, mut wheel) = Wheel::load(json)?;
    vm.begin_update();
    vm.call_method(vm.global, "Load_HideTag", Vec::new(), &mut wheel)?;
    drain(&mut vm, &mut wheel)?;
    Ok((vm, wheel))
}
/// _global.Save_EnableTag(false) (the corner style, not the centred one) / Save_HideTag.
fn show(vm: &mut Vm, wheel: &mut Wheel, on: bool) -> Result<(), String> {
    vm.begin_update();
    let (name, args) = if on { ("Save_EnableTag", vec![Value::Bool(false)]) } else { ("Save_HideTag", Vec::new()) };
    vm.call_method(vm.global, name, args, wheel)?;
    drain(vm, wheel)
}
/// One authored frame.
fn step(vm: &mut Vm, wheel: &mut Wheel) -> Result<(), String> {
    vm.begin_update();
    wheel.movie.advance(vm)?;
    drain(vm, wheel)?;
    let roots: Vec<usize> = wheel.movie.instances.keys().copied().chain(wheel.movie.classes.values().copied()).collect();
    vm.collect(roots)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_save_shows_the_icon_for_the_hold_and_a_later_save_restarts_it() {
        let mut pulse = Pulse::default();
        assert!(!pulse.tick(0, 0.016), "nothing saved yet");
        assert!(pulse.tick(1, 0.016));
        assert!(pulse.tick(1, HOLD - 0.1));
        assert!(pulse.tick(2, 0.05), "a second save restarts the hold");
        assert!(pulse.tick(2, HOLD - 0.1));
        assert!(!pulse.tick(2, 0.2), "then it hides");
    }

    #[test]
    #[ignore = "requires the prepared loading screen (SKATE_TIP_LOADER_ROOT)"]
    fn the_save_tag_spins_in_the_corner_and_hides() {
        let root = std::path::PathBuf::from(std::env::var("SKATE_TIP_LOADER_ROOT").unwrap());
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("runtime/autosave.json")).unwrap()).unwrap();
        let (mut vm, mut wheel) = load(&json).unwrap();
        let draws = |vm: &Vm, wheel: &Wheel| crate::apt_scene::draw(&wheel.movie, vm, &wheel.shapes).unwrap();
        assert!(draws(&vm, &wheel).is_empty(), "hidden until a save");
        show(&mut vm, &mut wheel, true).unwrap();
        (0..20).for_each(|_| step(&mut vm, &mut wheel).unwrap());
        let shown = draws(&vm, &wheel);
        assert!(!shown.is_empty(), "the save wheel draws");
        let right = shown.iter().flat_map(|d| d.vertices.iter().map(|v| v.position[0])).fold(f32::MIN, f32::max);
        assert!(right > 1000., "in the bottom-right corner, not centred (right edge {right})");
        show(&mut vm, &mut wheel, false).unwrap();
        step(&mut vm, &mut wheel).unwrap();
        assert!(draws(&vm, &wheel).is_empty(), "Save_HideTag hides it");
    }
}
