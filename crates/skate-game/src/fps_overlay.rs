//! Presentation-frame FPS, independent of fixed physics and HUD refresh cadence.
use bevy::prelude::*;

pub(crate) struct FpsOverlayPlugin;

#[derive(Component)]
struct FpsText;
/// The box, hidden while Settings > Video > FPS Counter is off.
#[derive(Component)]
struct FpsOverlay;

#[derive(Default)]
struct FrameSample {
    frames: u32,
    seconds: f64,
}

impl Plugin for FpsOverlayPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn).add_systems(Update, (update, show));
    }
}

fn spawn(mut commands: Commands) {
    // Use the existing gameplay camera; no independent overlay render loop.
    commands
        .spawn((
            Name::new("FPS overlay"),
            FpsOverlay,
            Node {
                position_type: PositionType::Absolute,
                top: px(12),
                right: px(12),
                padding: UiRect::axes(px(10), px(6)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.7)),
            GlobalZIndex(100),
        ))
        .with_children(|parent| {
            parent.spawn((
                FpsText,
                Text::new("FPS: --"),
                TextFont {
                    font_size: 20.0,
                    ..default()
                },
                TextColor(Color::WHITE),
            ));
        });
}

fn update(
    time: Res<Time<Real>>,
    mut sample: Local<FrameSample>,
    mut labels: Query<&mut Text, With<FpsText>>,
) {
    let seconds = time.delta_secs_f64();
    if seconds <= 0.0 {
        return;
    }
    // One sample for EVERY presentation frame, including long/stalled frames.
    // Real time is unaffected by slow motion or the fixed simulation timestep.
    sample.frames += 1;
    sample.seconds += seconds;
    if sample.seconds >= 0.5 {
        let fps = f64::from(sample.frames) / sample.seconds;
        for mut text in &mut labels {
            text.0 = format!("FPS: {fps:.0}");
        }
        *sample = FrameSample::default();
    }
}

/// Not retail (user 10-26): Settings > Video > FPS Counter.
fn show(menu: Option<Res<crate::graphics_menu::Menu>>, mut boxes: Query<&mut Node, With<FpsOverlay>>) {
    let display = if menu.is_none_or(|m| m.fps_counter()) { Display::Flex } else { Display::None };
    for mut node in &mut boxes {
        if node.display != display {
            node.display = display;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FPS Counter Off hides the box; On shows it again.
    #[test]
    fn the_fps_counter_setting_hides_the_box() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).insert_resource(crate::graphics_menu::test_menu()).add_plugins(FpsOverlayPlugin);
        app.update();
        let shown = |app: &mut App| {
            let mut boxes = app.world_mut().query_filtered::<&Node, With<FpsOverlay>>();
            boxes.single(app.world()).unwrap().display
        };
        assert_eq!(shown(&mut app), Display::Flex);
        let setting = crate::graphics_menu::Setting::FpsCounter;
        app.world_mut().resource_mut::<crate::graphics_menu::Menu>().choose(setting, 0);
        app.update();
        assert_eq!(shown(&mut app), Display::None);
        app.world_mut().resource_mut::<crate::graphics_menu::Menu>().choose(setting, 1);
        app.update();
        assert_eq!(shown(&mut app), Display::Flex);
    }
}
