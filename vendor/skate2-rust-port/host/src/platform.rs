//! The host services the port's session asks for, as the spike harness gave them (`s2spike/src/main.rs` 51-120):
//! a pad on controller 0 that plays what the game sends, Juice with no network, and a world stream with no renderer.
//! Not tested in play.

use skate2_core::{
    JuiceBootPlatform, JuiceConnection, JuiceConsoleName, JuiceEndpoint, PadCapabilitiesResult, PadPlatform,
    PadStateResult, PadVibration, XboxPadSample, world_stream_runtime::WorldStreamEffects,
};
use skate2_physics_protocol::Pad;

/// XInput's `ERROR_DEVICE_NOT_CONNECTED`.
const NOT_CONNECTED: u32 = 1167;

/// Controller 0 plays the last sample the game sent; slots 1-3 report "not connected".
pub struct HostPad {
    sample: XboxPadSample,
}

impl HostPad {
    /// Holds `pad` as the sample the next tick reads.
    pub fn set(&mut self, pad: Pad) {
        self.sample = sample(pad);
    }
}

impl Default for HostPad {
    /// A neutral pad: no button, triggers and sticks at rest.
    fn default() -> Self {
        Self { sample: sample(Pad::default()) }
    }
}

/// The protocol's pad is the port's `XboxPadSample`, field for field.
fn sample(pad: Pad) -> XboxPadSample {
    XboxPadSample {
        buttons: pad.buttons,
        left_trigger: pad.left_trigger,
        right_trigger: pad.right_trigger,
        left_x: pad.left[0],
        left_y: pad.left[1],
        right_x: pad.right[0],
        right_y: pad.right[1],
    }
}

impl PadPlatform for HostPad {
    type Error = String;

    fn get_state(&mut self, index: u32) -> Result<PadStateResult, Self::Error> {
        Ok(if index == 0 {
            PadStateResult { status: 0, state: Some((7, self.sample)) }
        } else {
            PadStateResult { status: NOT_CONNECTED, state: None }
        })
    }

    fn get_capabilities(&mut self, index: u32, flags: u32) -> Result<PadCapabilitiesResult, Self::Error> {
        if flags != 1 {
            return Err(format!("unsupported capability flags {flags}"));
        }
        // The synthetic controller-0 capabilities the port's PC app reports for a demo sample.
        Ok(if index == 0 {
            PadCapabilitiesResult { status: 0, device_type: Some(1), subtype: Some(0), flags: Some(0) }
        } else {
            PadCapabilitiesResult { status: NOT_CONNECTED, device_type: None, subtype: None, flags: None }
        })
    }

    fn set_state(&mut self, index: u32, _vibration: PadVibration) -> Result<u32, Self::Error> {
        Ok(if index == 0 { 0 } else { NOT_CONNECTED })
    }
}

/// Juice (EA's debug tuning link) with no network: it never opens a socket.
pub struct NoJuice;

impl JuiceBootPlatform for NoJuice {
    type Error = String;
    type Connection = ();

    fn initialize_network(&mut self) -> Result<bool, Self::Error> {
        Ok(false)
    }

    fn debug_address(&mut self) -> Result<[u8; 4], Self::Error> {
        Err("host: no debug identity".into())
    }

    fn console_name(&mut self) -> Result<JuiceConsoleName, Self::Error> {
        Err("host: no debug identity".into())
    }

    fn connect(&mut self, _endpoint: &JuiceEndpoint) -> Result<JuiceConnection<()>, Self::Error> {
        Ok(JuiceConnection::Failed)
    }

    fn sleep_milliseconds(&mut self, _milliseconds: u32) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// The world stream's AddHighCollection and FinalizeWorldBounds, which feed the renderer in the port's app. The host
/// has no renderer, so both do nothing (the spike only counted them).
pub struct HeadlessEffects;

impl WorldStreamEffects for HeadlessEffects {
    fn add_high_collection(&mut self, _id: u64, _x: u32, _z: u32) {}

    fn finalize_world_bounds(&mut self) {}
}
