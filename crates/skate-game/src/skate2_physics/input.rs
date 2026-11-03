//! The engine's pad to the Skate 2 port's `Pad` (spec 3.3, Task 15). Not tested in play.
//!
//! The port reads raw XInput integers (`XboxPadSample`, `skate2-core/src/input/devices/input_pad.rs` 17-25), which are
//! the integers the engine's `XboxState` holds before `collect` turns them into `RawInput`'s floats
//! (`input/controllers.rs` 178-183). This undoes that division, so every value `collect` makes comes back exact.
//!
//! The D-pad reaches the port unchanged: alone it fires Skate 2's gestures (Task 8; slots Up, Down, Left, Right).
//! Three things never reach it, because they would open Skate 2's own menus over the engine's:
//! - Start and Back (`WITHHELD`);
//! - LB with any D-pad bit: the D-pad bits are cleared. That is Skate 2's session marker, and the engine's own
//!   session marker panel owns it;
//! - everything, while the engine gates its own gameplay (`ControllerInput::gated`).
//!
//! The pad comes from the engine's polled controller only; during play the engine has no keyboard-as-pad source, so a
//! keyboard player gives the port a neutral pad (keyboard-as-pad exists only in the menus).

use skate2_physics_protocol::Pad;

use crate::input::RawInput;

/// The XInput bits that are never sent to the port.
///
/// Step 1, read from the port and from Skate 2's own `data\config\input.cfg` (`Button0..15` are XInput bits 0..15;
/// `Start = Button4`, `Back = Button5`, `LBumper = Button8`, `Y = Button15`; `try_convert_xbox_pad`,
/// `input_pad.rs` 54 and 63, takes bit n of `XboxPadSample.buttons` for Button n):
/// - Start `0x0010`: `AptStart = Start.pressed` (cfg 265) is `PauseMenuInput.start` (state 134,
///   `selected_production_input_owner.rs` 430-454); `PauseMenu::update` opens the pause hub on it
///   (`ui/pause_menu.rs` 627), the facade calls that update (`selected_session_public_facade.rs` 2662), and the
///   hub's Map row opens the map (facade 2665-2668; the map has no other way in: `map_menu.open` is called only
///   there). Settings, Blog, Profile, Trick Guide, Replay and Edit Skater all open from the same hub (facade
///   2672-2757).
/// - Back `0x0020`: `AptSelect = Back.pressed` (cfg 266) is `PauseMenuInput.back` (state 135); the single-player
///   and online hub opens on it (`session/online/online_hub.rs` 115, called from facade 2138).
///
/// Nothing else is withheld as a bit:
/// - The phone is LB held and Y released, two bits together (`try_phone_options_input`, state 29 = cfg 98
///   `TopLeftShoulderHeld` and state 182 = cfg 313 `AptOptionReleased`; facade 2063, 2593-2599). It opens only
///   through a career (`self.career.as_mut().filter(..can_open_contacts)`), and the host never starts one (no `career`
///   in `host/src`; the free-skate starts set `career = None`, `selected_session_freeskate.rs` 129, 173, 224), so it
///   cannot open. LB and Y each
///   pass alone, as the engine's gameplay needs them.
/// - The session marker is LB held with D-pad Down pressed or D-pad Up held (cfg 138-139, states 52/53,
///   `selected_session_marker.rs` 335-342): that is the LB-with-D-pad rule in `pad`, not a bit.
/// - The D-pad alone is read by `try_hud_dpad_input` (states 150-153, facade 2065, 2773-2777). With no career it
///   only reaches `queue_prop_input`, which does nothing unless a prop stands in front of the skater
///   (`selected_session_props.rs` 11-17), and the port's world never runs (Amendment C). It also reaches the
///   port's debug events `DebugPrev/Next/ReplaceSkateboard` (cfg 149-153, D-pad Left/Right/Up pressed;
///   `game_input_recognizer.rs` 236-257), which become named action-graph intents only
///   (`action_graph_input_listener.rs` 203-217, 281-285) and which no code in the port reads. That is Skate 2's
///   native D-pad, and the gestures need it, so no D-pad bit is withheld (spec 3.3).
pub const WITHHELD: &[u16] = &[0x0010, 0x0020];

/// LB, the shoulder that makes a D-pad press Skate 2's session marker.
const LB: u16 = 0x0100;
/// The four D-pad bits: Up, Down, Left, Right.
const DPAD: u16 = 0x000F;

/// The engine's pad as the port's `Pad`. A gated pad (the skater gets no gameplay actions) is neutral.
pub(crate) fn pad(raw: RawInput, gated: bool) -> Pad {
    if gated {
        return Pad::default();
    }
    let mut buttons = raw.buttons & !WITHHELD.iter().fold(0u16, |mask, &bit| mask | bit);
    if buttons & LB != 0 && buttons & DPAD != 0 {
        // The engine's session marker (LB with the D-pad), not Skate 2's.
        buttons &= !DPAD;
    }
    Pad {
        buttons,
        left_trigger: trigger(raw.triggers[0]),
        right_trigger: trigger(raw.triggers[1]),
        left: [stick(raw.left[0]), stick(raw.left[1])],
        right: [stick(raw.right[0]), stick(raw.right[1])],
    }
}

/// `collect` made `f` as `v / 255.0`: this is `v` again.
fn trigger(f: f32) -> u8 {
    (f * 255.0).round().clamp(0.0, 255.0) as u8
}

/// `collect` made `f` as `v / 32768.0`: this is `v` again (exact: a power of two), clamped to an `i16`.
fn stick(f: f32) -> i16 {
    (f * 32768.0).round().clamp(-32768.0, 32767.0) as i16
}

#[cfg(test)]
mod tests {
    use skate_core::input::xbox::XboxState;

    use super::*;
    use crate::input::ControllerInput;

    /// `RawInput` as `collect` makes it: the state goes through the production path, and only the OS read is faked.
    fn collected(state: XboxState) -> RawInput {
        let mut input = ControllerInput::default();
        input.sample_raw_for_test(state);
        input.raw_input()
    }

    fn buttons(buttons: u16) -> RawInput {
        RawInput { buttons, ..RawInput::default() }
    }

    #[test]
    fn a_pad_round_trips_to_xinput_values() {
        let raw = collected(XboxState {
            buttons: 0x9001,
            triggers: [255, 1],
            left: [-32768, 32767],
            right: [0, -1],
        });
        assert_eq!(
            pad(raw, false),
            Pad { buttons: 0x9001, left_trigger: 255, right_trigger: 1, left: [-32768, 32767], right: [0, -1] }
        );
    }

    #[test]
    fn withheld_buttons_never_reach_the_host() {
        assert_eq!(pad(buttons(0x0010), false).buttons, 0, "Start");
        assert_eq!(pad(buttons(0x0020), false).buttons, 0, "Back");
        assert_eq!(pad(buttons(0x0100 | 0x0001), false).buttons, 0x0100, "LB with the D-pad keeps only LB");
        for bit in WITHHELD {
            assert_eq!(pad(buttons(*bit), false).buttons, 0, "withheld bit {bit:#06x}");
        }
        // Only the withheld bit goes: the buttons pressed with it stay.
        assert_eq!(pad(buttons(0x0030 | 0x1000), false).buttons, 0x1000);
        // LB with a D-pad pair clears both; LB with a face button keeps it (the phone needs a career).
        assert_eq!(pad(buttons(0x0100 | 0x0005), false).buttons, 0x0100);
        assert_eq!(pad(buttons(0x0100 | 0x8000), false).buttons, 0x8100);
    }

    #[test]
    fn the_dpad_alone_reaches_the_host() {
        for bits in [0x0001, 0x0002, 0x0004, 0x0008, 0x0200 | 0x0001] {
            assert_eq!(pad(buttons(bits), false).buttons, bits, "buttons {bits:#06x}");
        }
    }

    #[test]
    fn a_gated_pad_is_neutral() {
        let raw = collected(XboxState {
            buttons: 0x9001,
            triggers: [255, 1],
            left: [-32768, 32767],
            right: [5, -1],
        });
        assert_ne!(pad(raw, false), Pad::default(), "the same input is not neutral ungated");
        assert_eq!(pad(raw, true), Pad::default());
        // The engine's gates: the Skate 2 tutorial pin, the chat bar, Edit Skater online, the menu's closing press.
        let mut input = ControllerInput::default();
        assert!(!input.gated());
        input.pinned = true;
        assert!(input.gated(), "pinned");
        input.pinned = false;
        input.chat = true;
        assert!(input.gated(), "chat");
        input.chat = false;
        input.cas = true;
        assert!(input.gated(), "cas");
        input.cas = false;
        input.menu_release = true;
        assert!(input.gated(), "menu_release");
        input.menu_release = false;
        assert!(!input.gated());
    }
}
