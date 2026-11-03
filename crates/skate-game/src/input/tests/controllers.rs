use super::*;
use crate::input::platform::{DeviceError, DevicePacket};
use skate_core::input::xbox::XboxState;

impl ControllerInput {
    /// Feed test packets through the production conversion, cache, history and Pad.
    /// Only the operating-system read is supplied by the test.
    pub(crate) fn sample_raw_for_test(&mut self, state: XboxState) {
        self.collect([
            Ok(DevicePacket {
                number: self.publications as u32 + 1,
                state,
                subtype: 1,
            }),
            Err(DeviceError::Disconnected),
            Err(DeviceError::Disconnected),
            Err(DeviceError::Disconnected),
        ]);
        assert!(self.publish_actions());
    }
}

fn packet(number: u32, buttons: u16, left: [i16; 2]) -> Result<DevicePacket, DeviceError> {
    Ok(DevicePacket {
        number,
        state: XboxState {
            buttons,
            triggers: [0; 2],
            left,
            right: [0; 2],
        },
        subtype: 1,
    })
}

#[test]
fn raw_packets_reach_stock_actions_without_losing_device_identity() {
    let mut input = ControllerInput::default();
    input.collect([
        packet(1, 0x1000, [32767, 0]),
        packet(2, 0x2000, [-32768, 0]),
        packet(3, 0x4000, [0, 0]),
        packet(4, 0x8000, [0, 0]),
    ]);
    assert!(input.publish_actions());
    assert!((input.mapped_actions[0][0] - 1.0).abs() < 0.000001);
    assert!((input.mapped_actions[1][0] + 1.0).abs() < 0.000001);
    assert_eq!(input.mapped_actions[0][16], 1.0); // GP_AFace
    assert_eq!(input.mapped_actions[1][17], 1.0); // GP_BFace
    assert_eq!(input.mapped_actions[2][14], 1.0); // GP_XFace
    assert_eq!(input.mapped_actions[3][15], 1.0); // GP_YFace
    assert_eq!(input.packet_numbers, [Some(1), Some(2), Some(3), Some(4)]);
}

/// Back (pad slot 5) is in none of the 18 stock gameplay expressions (82697740) nor the session marker's
/// LB + D-pad: the frame before the replay editor opens on it (replay/hud.rs reads the pad one poll late) does nothing.
#[test]
fn back_has_no_gameplay_binding() {
    let mut input = ControllerInput::default();
    input.collect(std::array::from_fn(|_| packet(1, 0x0020, [0, 0])));
    assert!(input.publish_actions());
    assert_eq!(input.mapped_actions[0], [0.0; 18]);
    assert_eq!(input.tick_input().actions().values(), &[0.0; 18]);
    assert_eq!(input.session_marker_actions(), (false, false, false));
}

/// User 10-24: B leaves spectating on its press only, once (match_core::spectate_back), not while held.
#[test]
fn b_is_pressed_once_on_its_edge() {
    let mut input = ControllerInput::default();
    let mut n = 0;
    let mut poll = |buttons: u16, polls: u32| {
        let mut presses = 0;
        for _ in 0..polls {
            n += 1;
            input.collect(std::array::from_fn(|_| packet(n, buttons, [0, 0])));
            input.publish_actions();
            presses += u32::from(input.b_pressed());
        }
        presses
    };
    assert_eq!(poll(0x0000, 4), 0);
    assert_eq!(poll(0x2000, 30), 1, "B held for half a second: one press");
    assert_eq!(poll(0x1000, 30), 0, "A is not B");
}

#[test]
fn lb_with_dpad_right_or_left_skips_or_restarts_the_song() {
    let mut input = ControllerInput::default();
    let mut n = 0;
    // One publication per poll; Pad ignores an edge for 3 polls after the last (82966F30).
    let mut poll = |buttons: u16, polls: u32| {
        for _ in 0..polls {
            n += 1;
            input.collect(std::array::from_fn(|_| packet(n, buttons, [0, 0])));
            input.publish_actions();
        }
        input.track_actions()
    };
    assert_eq!(poll(0x0100, 4), (false, false), "LB alone");
    assert_eq!(poll(0x0108, 1), (true, false), "LB + D-pad Right");
    assert_eq!(poll(0x0108, 23), (false, false), "held: no repeat for 24 polls");
    assert_eq!(poll(0x0108, 1), (true, false), "AptDRight = DPadR.repeat: the first repeat");
    assert_eq!(poll(0x0108, 11), (false, false));
    assert_eq!(poll(0x0108, 1), (true, false), "then every 12 polls");
    poll(0x0100, 4);
    assert_eq!(poll(0x0104, 1), (false, true), "LB + D-pad Left");
    poll(0x0000, 4);
    assert_eq!(poll(0x0008, 1), (false, false), "D-pad Right without LB");
}

#[test]
fn unchanged_packet_numbers_do_not_suppress_publication_and_empty_drain_does_not_advance() {
    let mut input = ControllerInput::default();
    for _ in 0..4 {
        input.collect(std::array::from_fn(|_| packet(7, 0x1000, [0; 2])));
        assert!(input.publish_actions());
    }
    assert_eq!(input.publications, 4);
    assert_eq!(input.consumed_batches, 4);
    assert_ne!(input.pads[0].records()[12][1] & 0x01000000, 0);
    let prior = input.pads.clone();
    assert!(!input.publish_actions());
    assert_eq!(input.pads, prior);
}

#[test]
fn disconnect_publishes_zero_count_and_reconnect_reinitializes_pad_records() {
    let mut input = ControllerInput::default();
    input.collect(std::array::from_fn(|_| packet(1, 0x1000, [32767, 0])));
    input.publish_actions();
    input.collect(std::array::from_fn(|_| Err(DeviceError::Disconnected)));
    input.publish_actions();
    assert_eq!(input.mapped_actions, [[0.0; 18]; 4]);
    assert!(input.pads.iter().all(|pad| pad.count() == 0));
    input.collect(std::array::from_fn(|_| packet(1, 0, [0; 2])));
    input.publish_actions();
    assert_eq!(input.mapped_actions, [[0.0; 18]; 4]);
    assert_eq!(input.pads[0].records()[12], [0, 0, 0, 1]);
}

#[test]
fn alternate_guitar_subtype_uses_verified_conversion_mask_and_errors_stay_visible() {
    let mut input = ControllerInput::default();
    let special = DevicePacket {
        number: 1,
        state: XboxState {
            buttons: 0,
            triggers: [255; 2],
            left: [0; 2],
            right: [-32768, 32767],
        },
        subtype: 7,
    };
    input.collect([
        Ok(special),
        Err(DeviceError::Capabilities(5)),
        Err(DeviceError::Disconnected),
        Err(DeviceError::Disconnected),
    ]);
    input.publish_actions();
    assert_eq!(input.mapped_actions[0][6..8], [0.0; 2]);
    assert_eq!(input.mapped_actions[0][3..5], [0.0; 2]);
    assert_eq!(
        input.status[1],
        ControllerStatus::Unavailable(DeviceError::Capabilities(5))
    );
    assert_eq!(input.mapped_actions[1], [0.0; 18]);
}

#[test]
fn tick_input_is_the_single_fixed_tick_snapshot() {
    let mut input = ControllerInput::default();
    input.collect(std::array::from_fn(|_| packet(1, 0, [19661, 0])));
    assert!(input.publish_actions());

    let first = input.tick_input();
    assert_eq!(first.tick(), 1);
    assert!(first.controller_available());
    let mut first_actions = first.actions();
    let first_turn = first_actions.value(64);

    // A second consumer reads the same published tick without touching the
    // platform cache or remapping the controller.
    let second = input.tick_input();
    let mut second_actions = second.actions();
    assert_eq!(second.tick(), first.tick());
    assert_eq!(second_actions.value(64), first_turn);
}

#[test]
fn unavailable_controllers_publish_zero_gameplay_actions() {
    let mut input = ControllerInput::default();
    input.collect(std::array::from_fn(|_| packet(1, 0x1000, [32767, 0])));
    assert!(input.publish_actions());
    assert!(input.tick_input().controller_available());

    input.collect(std::array::from_fn(|_| Err(DeviceError::Disconnected)));
    assert!(input.publish_actions());
    let snapshot = input.tick_input();
    assert!(!snapshot.controller_available());
    assert_eq!(*snapshot.actions().values(), [0.0; 18]);
}

#[test]
fn the_retail_active_pad_alone_drives_gameplay() {
    let mut input = ControllerInput::default();
    // Pad 0 holds A, pad 2 pushes the stick; before the title chooses, the first pad drives.
    input.collect([packet(1, 0x1000, [0, 0]), Err(DeviceError::Disconnected), packet(1, 0, [32767, 0]), Err(DeviceError::Disconnected)]);
    assert_eq!(input.device(), Some(0));
    assert_eq!(input.raw_input().buttons, 0x1000);
    // Pad 2 released Start on the title: only it drives now.
    input.active_pad = Some(2);
    assert_eq!(input.device(), Some(2));
    assert_eq!(input.raw_input().buttons, 0);
    assert!(input.raw_input().left[0] > 0.99);
    // Unplugged, the active pad gives no input rather than falling back to another pad.
    input.collect([packet(2, 0x1000, [0, 0]), Err(DeviceError::Disconnected), Err(DeviceError::Disconnected), Err(DeviceError::Disconnected)]);
    assert_eq!(input.device(), None);
    assert!(input.publish_actions());
    assert!(!input.tick_input().controller_available());
}

#[test]
fn start_on_another_pad_adopts_it_once_the_active_pad_is_pulled() {
    let mut input = ControllerInput::default();
    input.active_pad = Some(1);
    // Pad 1 connected: nothing to adopt, even with Start held on pad 0.
    input.collect([packet(1, 0x0010, [0, 0]), packet(1, 0, [0, 0]), Err(DeviceError::Disconnected), Err(DeviceError::Disconnected)]);
    assert_eq!(input.adopt(), None);
    // Pad 1 pulled: pad 3 idle adopts nothing, pad 3 pressing Start is adopted.
    input.collect([Err(DeviceError::Disconnected), Err(DeviceError::Disconnected), Err(DeviceError::Disconnected), packet(2, 0, [0, 0])]);
    assert_eq!(input.adopt(), None);
    input.collect([Err(DeviceError::Disconnected), Err(DeviceError::Disconnected), Err(DeviceError::Disconnected), packet(3, 0x0010, [0, 0])]);
    assert_eq!(input.adopt(), Some(3));
    // No active pad chosen yet: not a pull, nothing to adopt.
    input.active_pad = None;
    assert_eq!(input.adopt(), None);
}

/// Item 32: while the chat's typing bar is open the skater gets no actions; they come back when it closes.
#[test]
fn the_open_chat_bar_holds_the_skater() {
    let mut input = ControllerInput::default();
    input.collect(std::array::from_fn(|_| packet(1, 0x1000, [32767, 0])));
    assert!(input.publish_actions());
    assert_ne!(*input.tick_input().actions().values(), [0.0; 18]);
    input.chat = true;
    assert_eq!(*input.tick_input().actions().values(), [0.0; 18], "typing");
    assert!(input.tick_input().controller_available(), "the pad is still there");
    input.chat = false;
    assert_ne!(*input.tick_input().actions().values(), [0.0; 18]);
}

/// Bug 10-17 #44: held for a match intro / countdown, only the emotes (D-pad) and LB / RB, which block them, go through.
#[test]
fn a_match_hold_lets_only_the_emotes_through() {
    let mut input = ControllerInput::default();
    let (dpad_up, rb, a) = (0x0001, 0x0200, 0x1000);
    input.collect(std::array::from_fn(|_| packet(1, dpad_up | rb | a, [32767, 0])));
    assert!(input.publish_actions());
    let free = *input.tick_input().actions().values();
    input.match_hold = true;
    assert_eq!(*input.tick_input().actions().values(), [0.0; 18], "held, no gestures");
    input.match_gestures = true;
    let held = *input.tick_input().actions().values();
    for i in 0..18 {
        let gesture = matches!(i, 8..=13);
        assert_eq!(held[i], if gesture { free[i] } else { 0.0 }, "action {}", 64 + i);
    }
    let on = |i: usize| held[i] != 0.0;
    assert_eq!((8..=13).filter(|&i| on(i)).count(), 2, "D-pad up and RB come through: {held:?}");
}
