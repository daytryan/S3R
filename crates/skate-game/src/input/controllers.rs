//! Persistent platform-input owner. Device collection runs on host frames;
//! this does not choose the skater simulation clock or advance Derived input.
use super::platform::{DeviceError, DevicePacket};
use bevy::prelude::Resource;
use skate_core::input::{
    controller::ActionMap,
    gameplay_map::GameplayActions,
    history::{DEVICE_SLOTS, HistoryRecord, PadHistory},
    pad::Pad,
    tick::TickInput,
    xbox,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ControllerStatus {
    Unpolled,
    Ready,
    Unavailable(DeviceError),
}

#[derive(Resource)]
pub(crate) struct ControllerInput {
    raw: [RawInput; DEVICE_SLOTS],
    cache: [[HistoryRecord; DEVICE_SLOTS]; 2],
    active: usize,
    history: PadHistory,
    pads: [Pad; DEVICE_SLOTS],
    pub status: [ControllerStatus; DEVICE_SLOTS],
    pub packet_numbers: [Option<u32>; DEVICE_SLOTS],
    pub mapped_actions: [[f32; 18]; DEVICE_SLOTS],
    pub publications: u64,
    pub consumed_batches: u64,
    /// The Skate 2 tutorial's cMsgPinPlayer: the skater gets no gameplay actions; the pad still reads (session markers).
    pub pinned: bool,
    /// match_core: retail pins everyone on the start through a match's intro and countdown.
    pub match_hold: bool,
    /// Bug 10-17 #44 / #45: under `match_hold`, the emotes (D-pad gestures) still go through: a mode's intro and
    /// countdown, and spectating. Not while a menu owns the pad (Results, Party Play's screens).
    pub match_gestures: bool,
    /// online_chat's typing bar is open: the skater gets no gameplay actions.
    pub chat: bool,
    /// Not retail (user 10-16): Edit Skater is open online (the world runs on): the skater gets no gameplay actions.
    pub cas: bool,
    /// perf 10-18: the Start menu gave the world back on its closing press (core_menu::early_resume): the skater gets
    /// no gameplay actions until the pad's buttons are all up, so that press never reaches gameplay.
    pub menu_release: bool,
    /// `ActivePad`, mirrored each poll: only this pad drives gameplay once the title chose it.
    pub active_pad: Option<u8>,
    tick: u64,
}

impl Default for ControllerInput {
    fn default() -> Self {
        Self {
            raw: [RawInput::default(); DEVICE_SLOTS],
            cache: [[HistoryRecord::new(&[]); DEVICE_SLOTS]; 2],
            active: 0,
            history: PadHistory::new(),
            pads: std::array::from_fn(|_| Pad::new()),
            status: [ControllerStatus::Unpolled; DEVICE_SLOTS],
            packet_numbers: [None; DEVICE_SLOTS],
            mapped_actions: [[0.0; 18]; DEVICE_SLOTS],
            publications: 0,
            consumed_batches: 0,
            pinned: false,
            match_hold: false,
            match_gestures: false,
            cas: false,
            menu_release: false,
            chat: false,
            active_pad: None,
            tick: 0,
        }
    }
}

impl ControllerInput {
    /// The pad gameplay reads: the retail active pad (none while it is unplugged), or before one
    /// is chosen the first connected pad.
    /// Retail's ID_XBOX_CONTROLLER_PULLED recovery: once the active pad is gone, a Start press on
    /// another connected pad makes it the active pad.
    // ponytail: retail also raises an ID_COMMON_WARNING_TITLE / ID_XBOX_CONTROLLER_PULLED popup and pauses
    // (0x825E01A8, pause reason 19, cleared by 0x825E0340); its trigger wasn't traced, so no popup here.
    pub(crate) fn adopt(&self) -> Option<u8> {
        let active = usize::from(self.active_pad?);
        if self.status.get(active) == Some(&ControllerStatus::Ready) {
            return None;
        }
        (0..DEVICE_SLOTS)
            .find(|&i| self.status[i] == ControllerStatus::Ready && self.raw[i].buttons & 0x0010 != 0)
            .map(|i| i as u8)
    }
    pub(crate) fn device(&self) -> Option<usize> {
        match self.active_pad {
            Some(pad) => Some(usize::from(pad)).filter(|&p| self.status.get(p) == Some(&ControllerStatus::Ready)),
            None => self.status.iter().position(|s| *s == ControllerStatus::Ready),
        }
    }
    /// Original input.cfg: LB.held && DPadD.pressed / LB.held && DPadU.held.
    /// Use the same debounced native Pad publication as ordinary gameplay.
    pub(crate) fn session_marker_actions(&self) -> (bool, bool, bool) {
        let Some(device) = self.device() else {
            return (false, false, false);
        };
        let pad = &self.pads[device];
        if pad.count() == 0 {
            return (false, false, false);
        }
        let flags = |i: usize| pad.records().get(i).map_or(0, |r| r[1]);
        let modifier = flags(8) & 0xff00 != 0;
        (
            modifier,
            modifier && flags(1) & 0xff00_0000 != 0,
            modifier && flags(0) & 0xff00 != 0,
        )
    }
    /// The session marker panel's song shortcuts (S3 phonelist 826682B0, codes 0x4000 / 0x8000), as (next, restart).
    /// TU3 825EC350 maps the Apt keys to HUD codes: 19 AptDRight -> 0x4000 (next_song, Next Track: ID_HINTS_TEXT_17
    /// "L_Bumper + DPad_Right"), 18 AptDLeft -> 0x8000 (restart_song); LB press / release (9 / 53) -> 4 / 8 open and
    /// close the panel. input.cfg: AptDRight = DPadR.repeat, AptDLeft = DPadL.repeat, so a held D-pad repeats: the
    /// record's low byte, set on the press, 24 polls later and every 12 after (82966F30). The left stick (AptRight,
    /// key 15) maps to 0: only the D-pad.
    pub(crate) fn track_actions(&self) -> (bool, bool) {
        let Some(device) = self.device() else {
            return (false, false);
        };
        let pad = &self.pads[device];
        let flags = |i: usize| pad.records().get(i).map_or(0, |r| r[1]);
        let modifier = pad.count() != 0 && flags(8) & 0xff00 != 0;
        (modifier && flags(3) & 0xff != 0, modifier && flags(2) & 0xff != 0)
    }
    /// B (pad slot 13) pressed on this publication: the press bit 82966F30 sets on the edge only.
    pub(crate) fn b_pressed(&self) -> bool {
        self.device().is_some_and(|d| self.pads[d].count() > 13 && self.pads[d].records()[13][1] & 0xff00_0000 != 0)
    }
    pub(crate) fn raw_input(&self) -> RawInput {
        self.device().map_or(RawInput::default(), |i| self.raw[i])
    }
    /// The skater gets no gameplay actions: the same four gates `tick_input` applies. The Skate 2 physics switch sends
    /// the port a neutral pad while any is set (Task 15), so a tutorial pin, the chat bar, Edit Skater online or the
    /// menu's closing press never reaches Skate 2's sim either.
    pub(crate) fn gated(&self) -> bool {
        self.pinned || self.chat || self.cas || self.menu_release
    }

    // Replay controls must never queue a flick or a button press for gameplay.
    pub(crate) fn discard_gameplay(&mut self) {
        self.history = PadHistory::new();
        self.pads = std::array::from_fn(|_| Pad::new());
        self.mapped_actions = [[0.0; 18]; DEVICE_SLOTS];
    }
    pub(crate) fn player_actions(&self) -> GameplayActions {
        GameplayActions::from_pad(&self.pads[self.device().unwrap_or(0)])
    }

    pub(crate) fn tick_input(&self) -> TickInput {
        /// The D-pad gestures (actions 74..77, packed bits 27..24) and LB and RB (actions 72/73, bits 29/28), which
        /// suppress them (trick_intentions); every other action neutral.
        fn gestures_only(actions: &GameplayActions) -> GameplayActions {
            let all = actions.values();
            GameplayActions::from_values(std::array::from_fn(|i| if matches!(i, 8..=13) { all[i] } else { 0.0 }))
        }
        let device = self.device();
        let controller_available = device.is_some();
        let pad = device.filter(|_| !self.pinned && !self.chat && !self.cas && !self.menu_release).map(|device| GameplayActions::from_pad(&self.pads[device]));
        let actions = match pad {
            Some(actions) if !self.match_hold => actions,
            Some(actions) if self.match_gestures => gestures_only(&actions),
            _ => GameplayActions::from_values([0.0; 18]),
        };
        TickInput::new(self.tick, actions, controller_available)
    }
    /// TU3 8296D288/8296D0D0: write the inactive cache, publish one four-device
    /// batch. Even an unchanged platform packet is sampled; packet-number
    /// deduplication would alter native Pad edge/repeat behavior.
    pub(super) fn collect(&mut self, samples: [Result<DevicePacket, DeviceError>; DEVICE_SLOTS]) {
        let next = self.active ^ 1;
        for (device, sample) in samples.into_iter().enumerate() {
            match sample {
                Ok(packet) => {
                    self.raw[device] = RawInput {
                        buttons: packet.state.buttons,
                        triggers: packet.state.triggers.map(|v| f32::from(v) / 255.0),
                        left: packet.state.left.map(|v| f32::from(v) / 32768.0),
                        right: packet.state.right.map(|v| f32::from(v) / 32768.0),
                    };
                    // 8296D480 sets byte13 only when capability SubType == 7.
                    let values = xbox::convert(&packet.state, u8::from(packet.subtype == 7));
                    self.cache[next][device] = HistoryRecord::new(&values);
                    self.packet_numbers[device] = Some(packet.number);
                    self.status[device] = ControllerStatus::Ready;
                }
                Err(error) => {
                    self.raw[device] = RawInput::default();
                    self.cache[next][device].clear_count();
                    self.packet_numbers[device] = None;
                    self.status[device] = ControllerStatus::Unavailable(error);
                }
            }
        }
        self.active = next;
        self.history.publish(&self.cache[self.active]);
        self.publications += 1;
    }

    /// TU3 82699230 drains first, then updates each Pad once. The game owns
    /// this snapshot for subsequent consumers; Derived timers require the
    /// actual actor timestep and state flags and are not driven by render dt.
    pub(super) fn publish_actions(&mut self) -> bool {
        self.tick = self.tick.wrapping_add(1);
        if !self.history.drain_to_latest(&mut self.pads) {
            return false;
        }
        for (device, pad) in self.pads.iter().enumerate() {
            let mut actions = GameplayActions::from_pad(pad);
            self.mapped_actions[device] = std::array::from_fn(|i| actions.value(64 + i as u32));
        }
        self.consumed_batches += 1;
        true
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) struct RawInput {
    pub buttons: u16,
    pub triggers: [f32; 2],
    pub left: [f32; 2],
    pub right: [f32; 2],
}

#[cfg(test)]
#[path = "tests/controllers.rs"]
mod tests;
