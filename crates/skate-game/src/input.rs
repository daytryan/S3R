//! Platform input adapter; no animation or physics state mutation here.
use crate::app::SimulationSet;
use bevy::prelude::*;

mod controllers;
pub(crate) mod gesture_catalog;
pub(crate) mod gesture_input;
pub(crate) mod gesture_mapping;
mod gesture_mapping_data;
mod generic_pads;
#[cfg(windows)]
mod hid;
pub(crate) mod platform;
mod script;
pub(crate) use controllers::{ControllerInput, ControllerStatus, RawInput};
use skate_core::input::tick::TickInput;

/// The retail active pad ("input+28"): the pad that released Start on the title screen drives
/// everything after it (the keyboard always works). `None` = no pad chosen yet, where gameplay
/// takes the first connected pad. The boot/front-end flow writes it; gameplay, rumble and the
/// button glyphs read it.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ActivePad(pub Option<u8>);

#[derive(Resource, Clone, Copy, Debug)]
pub(crate) struct PublishedTickInput(pub TickInput);

impl Default for PublishedTickInput {
    fn default() -> Self {
        Self(TickInput::new(
            0,
            skate_core::input::gameplay_map::GameplayActions::from_values([0.0; 18]),
            false,
        ))
    }
}

pub(crate) struct InputPlugin;
impl Plugin for InputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ControllerInput>()
            .init_resource::<PublishedTickInput>()
            .init_resource::<ActivePad>()
            .add_systems(
                PreUpdate,
                poll_controllers.run_if(crate::graphics_menu::gameplay_active),
            )
            .add_systems(Update, (pad_glyphs, hide_idle_cursor))
            .add_systems(Last, stop_rumble_on_exit)
            .add_systems(FixedUpdate, publish_actions.in_set(SimulationSet::Input));
    }
}

/// The button glyphs follow the active pad: a PlayStation pad's set for a PlayStation pad, the movies' own art
/// otherwise (a Switch Pro maps by position, so it keeps the Xbox art). Not retail (user 10-16, feature #12): the
/// modern PlayStation set built into the game (button_icons) in place of the install's PS3 art (`pad-glyphs`).
fn pad_glyphs(input: Res<ControllerInput>, mut active: ResMut<ActivePad>) {
    use crate::apt_scene::{PS_ACTIVE, PS_GLYPHS, Texture};
    // An Xbox pad took the active pad's slot: the active pad moved, so ActivePad follows it.
    for (from, to) in platform::take_moves() {
        if active.0 == Some(from as u8) {
            active.0 = Some(to as u8);
        }
    }
    let slot = active.0.map(usize::from).or_else(|| input.device());
    let playstation = slot.and_then(platform::pad_kind) == Some(platform::PadKind::PlayStation);
    if playstation && PS_GLYPHS.get().is_none() {
        use crate::button_icons::{PS_NAMES, PS_PREFIX};
        // apt_canvas::texture_bytes serves these paths from button_icons before it reads a file.
        let texture = |name: &str| Texture { width: 40, height: 30, rgba: format!("{PS_PREFIX}{name}.rgba"), frame: None };
        let _ = PS_GLYPHS.set(PS_NAMES.iter().map(|&n| (n.to_owned(), texture(n))).collect());
    }
    PS_ACTIVE.store(playstation, std::sync::atomic::Ordering::Relaxed);
}

/// No pad keeps buzzing after the game closes.
fn stop_rumble_on_exit(mut exit: MessageReader<AppExit>) {
    if exit.read().next().is_some() {
        platform::stop_all_rumble();
        // The HID pad threads send their zero report within one 50 ms read.
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// Seconds the mouse rests in a focused window before its cursor hides.
const CURSOR_IDLE: f32 = 2.0;

/// Not retail (user 10-14: the cursor stayed over gameplay). In the focused window the cursor hides once the mouse
/// has rested CURSOR_IDLE seconds; any motion, button or wheel shows it again, and an unfocused window always shows it.
/// ponytail: idle-based, not menu-aware (the menus are pad-driven); make it menu-aware if a mouse menu needs the
/// cursor while the mouse rests.
fn hide_idle_cursor(
    mut windows: Query<(&Window, &mut bevy::window::CursorOptions)>,
    motion: Option<Res<bevy::input::mouse::AccumulatedMouseMotion>>,
    wheel: Option<Res<bevy::input::mouse::AccumulatedMouseScroll>>,
    buttons: Option<Res<ButtonInput<MouseButton>>>,
    time: Res<Time<Real>>,
    mut idle: Local<f32>,
) {
    let used = motion.is_some_and(|m| m.delta != Vec2::ZERO)
        || wheel.is_some_and(|w| w.delta != Vec2::ZERO)
        || buttons.is_some_and(|b| b.get_pressed().next().is_some());
    *idle = if used { 0. } else { *idle + time.delta_secs() };
    for (window, mut cursor) in &mut windows {
        let shown = !window.focused || *idle < CURSOR_IDLE;
        if cursor.visible != shown {
            cursor.visible = shown;
        }
    }
}

pub(crate) fn poll_controllers(
    mut input: ResMut<ControllerInput>,
    config: Res<crate::config::Config>,
    net: Option<Res<crate::multiplayer::Multiplayer>>,
    windows: Query<&Window>,
    mut active_pad: Option<ResMut<ActivePad>>,
    keys: Option<Res<ButtonInput<KeyCode>>>,
    mut capabilities: Local<[platform::CapabilityCache; 4]>,
) {
    input.active_pad = active_pad.as_ref().and_then(|pad| pad.0);
    let previous = input.status;
    let focused = windows.iter().any(|w| w.focused);
    let active = net.is_some_and(|n| n.active());
    input.collect(std::array::from_fn(|slot| {
        if active
            && ((!focused && config.multiplayer.controller.is_none())
                || config
                    .multiplayer
                    .controller
                    .is_some_and(|selected| selected as usize != slot))
        {
            capabilities[slot].invalidate();
            Err(platform::DeviceError::Disconnected)
        } else {
            platform::poll_cached(slot, &mut capabilities[slot])
        }
    }));
    // The active pad pulled: Start on another pad adopts it; Enter falls back to the keyboard and any pad.
    if let Some(active) = active_pad.as_mut() {
        let gone = input.active_pad.is_some() && input.device().is_none();
        let adopted = input.adopt().map(Some).or_else(|| (gone && keys.is_some_and(|k| k.just_pressed(KeyCode::Enter))).then_some(None));
        if let Some(pad) = adopted {
            info!("Active controller: {pad:?}");
            active.0 = pad;
            input.active_pad = pad;
        }
    }
    for (index, (&before, &after)) in previous.iter().zip(&input.status).enumerate() {
        if before != after {
            match after {
                ControllerStatus::Ready => {
                    info!("Controller {index}: {:?} pad ready", platform::pad_kind(index).unwrap_or(platform::PadKind::Xbox))
                }
                ControllerStatus::Unavailable(platform::DeviceError::Disconnected) => {
                    info!("Controller {index}: disconnected");
                }
                _ => warn!("Controller {index}: {after:?}"),
            }
        }
    }
}

fn publish_actions(
    mut input: ResMut<ControllerInput>,
    mut published: ResMut<PublishedTickInput>,
    menu: Option<Res<crate::graphics_menu::Menu>>,
) {
    if !crate::graphics_menu::gameplay_active(menu) {
        input.discard_gameplay();
    }
    input.publish_actions();
    published.0 = input.tick_input();
    // perf 10-18: the tick that sees the Start menu's closing press released is still neutral; the next one acts.
    if input.menu_release && input.raw_input().buttons == 0 {
        input.menu_release = false;
    }
}

#[cfg(test)]
pub(crate) mod manual_replay;
