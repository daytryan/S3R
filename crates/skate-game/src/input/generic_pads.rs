//! Every other controller (8BitDo, PowerA, Logitech, ...) through gilrs and its SDL mapping
//! database, on its own thread: Bevy's GilrsPlugin stays off because it stalled PreUpdate. No
//! gilrs filters, so the raw values reach the retail deadzone. On Windows, XInput-class pads
//! (gilrs reports them with a nil UUID) and the Sony/Nintendo pads the HID driver owns are skipped.
use super::platform::{self, PadKind};
use gilrs::{Axis, Button, GamepadId, GilrsBuilder};
use skate_core::input::xbox::XboxState;
use std::collections::HashMap;
use std::time::Duration;

pub(super) fn start() {
    let _ = std::thread::Builder::new().name("generic-pads".into()).spawn(|| {
        let Ok(mut gilrs) = GilrsBuilder::new().with_default_filters(false).build() else {
            return;
        };
        // Dropping an entry (gone, or the thread panicking) disconnects its pad.
        let mut ids: HashMap<GamepadId, platform::External> = HashMap::new();
        loop {
            if gilrs.next_event_blocking(Some(Duration::from_millis(8))).is_some() {
                while gilrs.next_event().is_some() {}
            }
            let mut seen = Vec::new();
            for (gamepad_id, pad) in gilrs.gamepads() {
                if owned_elsewhere(&pad) || !pad.is_connected() {
                    continue;
                }
                seen.push(gamepad_id);
                let id = ids.entry(gamepad_id).or_insert_with(|| platform::External::connect(PadKind::Generic)).0;
                platform::external_update(id, state(&pad));
            }
            ids.retain(|gamepad_id, _| seen.contains(gamepad_id));
        }
    });
}

/// XInput-class pads (nil UUID) and the exact models the HID driver handles.
#[cfg(windows)]
fn owned_elsewhere(pad: &gilrs::Gamepad) -> bool {
    pad.uuid() == [0; 16]
        || matches!((pad.vendor_id(), pad.product_id()), (Some(v), Some(p)) if super::hid::model(v, p).is_some())
}
#[cfg(not(windows))]
fn owned_elsewhere(_: &gilrs::Gamepad) -> bool {
    false
}

fn state(pad: &gilrs::Gamepad) -> XboxState {
    const BUTTONS: [(Button, u16); 14] = [
        (Button::DPadUp, 0x0001),
        (Button::DPadDown, 0x0002),
        (Button::DPadLeft, 0x0004),
        (Button::DPadRight, 0x0008),
        (Button::Start, 0x0010),
        (Button::Select, 0x0020),
        (Button::LeftThumb, 0x0040),
        (Button::RightThumb, 0x0080),
        (Button::LeftTrigger, 0x0100),
        (Button::RightTrigger, 0x0200),
        (Button::South, 0x1000),
        (Button::East, 0x2000),
        (Button::West, 0x4000),
        (Button::North, 0x8000),
    ];
    let buttons = BUTTONS.iter().filter(|(b, _)| pad.is_pressed(*b)).fold(0, |all, (_, bit)| all | bit);
    // gilrs axes are -1..1 with up positive, as XInput's.
    let axis = |a: Axis| (pad.value(a).clamp(-1.0, 1.0) * 32767.0) as i16;
    let trigger = |b: Button| (pad.button_data(b).map_or(0.0, |d| d.value()).clamp(0.0, 1.0) * 255.0) as u8;
    XboxState {
        buttons,
        triggers: [trigger(Button::LeftTrigger2), trigger(Button::RightTrigger2)],
        left: [axis(Axis::LeftStickX), axis(Axis::LeftStickY)],
        right: [axis(Axis::RightStickX), axis(Axis::RightStickY)],
    }
}
