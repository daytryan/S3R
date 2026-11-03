//! Windows device transport. Raw signed axes/trigger bytes reach the TU3
//! converter without Bevy/gilrs deadzones or normalized-axis reconstruction.
use skate_core::input::xbox::XboxState;

pub(crate) struct DevicePacket {
    pub number: u32,
    pub state: XboxState,
    pub subtype: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DeviceError {
    Disconnected,
    State(u32),
    Capabilities(u32),
}

/// Device identity is metadata; raw input is still sampled every host frame.
/// Refresh periodically as well as after errors, so hot swaps cannot leave a
/// subtype cached indefinitely even if Windows never exposes a disconnect.
#[derive(Default)]
pub(crate) struct CapabilityCache {
    value: Option<(u8, std::time::Instant)>,
}
impl CapabilityCache {
    pub(crate) fn invalidate(&mut self) {
        self.value = None;
    }
    fn get(
        &mut self,
        now: std::time::Instant,
        read: impl FnOnce() -> Result<u8, DeviceError>,
    ) -> Result<u8, DeviceError> {
        if let Some((subtype, expires)) = self.value {
            if now < expires {
                return Ok(subtype);
            }
        }
        self.value = None;
        let subtype = read()?;
        self.value = Some((subtype, now + std::time::Duration::from_secs(1)));
        Ok(subtype)
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::mem::MaybeUninit;

    // ABI from the installed Windows SDK Xinput.h. No OS-owned pointers are
    // retained and only successful calls permit reading output storage.
    #[repr(C)]
    struct Gamepad {
        buttons: u16,
        left_trigger: u8,
        right_trigger: u8,
        left_x: i16,
        left_y: i16,
        right_x: i16,
        right_y: i16,
    }
    #[repr(C)]
    struct State {
        number: u32,
        gamepad: Gamepad,
    }
    #[repr(C)]
    struct Vibration {
        left: u16,
        right: u16,
    }
    #[repr(C)]
    struct Capabilities {
        device_type: u8,
        subtype: u8,
        flags: u16,
        gamepad: Gamepad,
        vibration: Vibration,
    }
    const _: () = assert!(size_of::<Gamepad>() == 12);
    const _: () = assert!(size_of::<State>() == 16);
    const _: () = assert!(size_of::<Capabilities>() == 20);

    #[link(name = "xinput")]
    unsafe extern "system" {
        fn XInputGetState(index: u32, state: *mut State) -> u32;
        fn XInputGetCapabilities(index: u32, flags: u32, capabilities: *mut Capabilities) -> u32;
        fn XInputSetState(index: u32, vibration: *mut Vibration) -> u32;
    }

    /// Motor speeds 0..1: left = the low-frequency (heavy) motor, right = the high-frequency one.
    pub(super) fn vibrate(index: u32, [low, high]: [f32; 2]) {
        let speed = |v: f32| (v.clamp(0.0, 1.0) * 65535.0) as u16;
        let mut vibration = Vibration { left: speed(low), right: speed(high) };
        // SAFETY: a valid XINPUT_VIBRATION for the call's duration.
        unsafe { XInputSetState(index, &mut vibration) };
    }

    pub(super) fn poll(
        index: u32,
        cache: &mut CapabilityCache,
    ) -> Result<DevicePacket, DeviceError> {
        let mut state = MaybeUninit::<State>::uninit();
        // SAFETY: properly aligned writable storage with the SDK's exact C ABI.
        let result = unsafe { XInputGetState(index, state.as_mut_ptr()) };
        if result != 0 {
            cache.invalidate();
        }
        if result == 1167 {
            return Err(DeviceError::Disconnected);
        }
        if result != 0 {
            return Err(DeviceError::State(result));
        }
        let subtype = cache.get(std::time::Instant::now(), || {
            let mut capabilities = MaybeUninit::<Capabilities>::uninit();
            // SAFETY: writable storage with the SDK ABI; read only on success.
            let result = unsafe { XInputGetCapabilities(index, 1, capabilities.as_mut_ptr()) };
            if result != 0 {
                return Err(DeviceError::Capabilities(result));
            }
            Ok(unsafe { capabilities.assume_init() }.subtype)
        })?;
        // SAFETY: successful XInputGetState initialized the complete structure.
        let state = unsafe { state.assume_init() };
        Ok(DevicePacket {
            number: state.number,
            state: XboxState {
                buttons: state.gamepad.buttons,
                triggers: [state.gamepad.left_trigger, state.gamepad.right_trigger],
                left: [state.gamepad.left_x, state.gamepad.left_y],
                right: [state.gamepad.right_x, state.gamepad.right_y],
            },
            subtype,
        })
    }
}

/// Settings > Audio > Voice Chat's Push to Talk Button: its XInput bit (0: none, and while Voice Chat is Off); set by
/// graphics_menu::publish.
pub(crate) static PTT_PAD: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);

/// Slots that held the Push to Talk Button since the voice plugin last looked (a bit each; voice.rs takes them each
/// frame, so a pad no longer polled can't hold it down).
pub(crate) static PTT_PAD_HELD: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// The Push to Talk Button only talks (user 10-26): `button` taken out of the state the game sees, and whether it was
/// held.
fn talk_only(button: u16, packet: Result<DevicePacket, DeviceError>) -> (Result<DevicePacket, DeviceError>, bool) {
    let held = button != 0 && packet.as_ref().is_ok_and(|p| p.state.buttons & button != 0);
    let packet = packet.map(|mut p| {
        p.state.buttons &= !button;
        p
    });
    (packet, held)
}

/// When each slot may be queried again after it reported no controller.
// ponytail: Microsoft's XInputGetState docs warn that querying empty slots every
// frame is slow and suggest checking for new controllers every few seconds. A
// newly connected pad is picked up within one second; connected pads still poll every frame.
static EMPTY_UNTIL: std::sync::Mutex<[Option<std::time::Instant>; 4]> = std::sync::Mutex::new([None; 4]);

/// A pad's state as the game sees it: the Push to Talk Button taken out ([`talk_only`]), its hold kept in
/// [`PTT_PAD_HELD`].
pub(crate) fn poll_cached(index: usize, cache: &mut CapabilityCache) -> Result<DevicePacket, DeviceError> {
    let (packet, held) = talk_only(PTT_PAD.load(std::sync::atomic::Ordering::Relaxed), poll_device(index, cache));
    if held {
        PTT_PAD_HELD.fetch_or(1 << index, std::sync::atomic::Ordering::Relaxed);
    }
    packet
}
fn poll_device(
    index: usize,
    cache: &mut CapabilityCache,
) -> Result<DevicePacket, DeviceError> {
    assert!(index < 4);
    if let Some(scripted) = super::script::pad(index) {
        return scripted;
    }
    if let Some(hero) = crate::hero_clip::pad(index) {
        return hero.map(|(number, state)| DevicePacket { number, state, subtype: 1 }).ok_or(DeviceError::Disconnected);
    }
    // Unit tests never open real devices.
    if !cfg!(test) {
        START.call_once(|| {
            #[cfg(windows)]
            super::hid::start();
            super::generic_pads::start();
        });
    }
    let now = std::time::Instant::now();
    let empty = || EMPTY_UNTIL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let probe = !empty()[index].is_some_and(|until| now < until);
    #[cfg(windows)]
    let result = if probe { windows::poll(index as u32, cache) } else { Err(DeviceError::Disconnected) };
    #[cfg(not(windows))]
    let result = { let _ = cache; Err(DeviceError::Disconnected) };
    if probe {
        empty()[index] = matches!(result, Err(DeviceError::Disconnected)).then(|| now + std::time::Duration::from_secs(1));
        registry().set_xinput(index, !matches!(result, Err(DeviceError::Disconnected)));
    }
    // A PlayStation, Switch or other pad fills a slot XInput leaves empty.
    if matches!(result, Err(DeviceError::Disconnected)) {
        if let Some(pad) = registry().pads.iter().find(|p| p.slot == Some(index)) {
            return Ok(DevicePacket { number: pad.number, state: pad.state, subtype: 1 });
        }
    }
    result
}

static START: std::sync::Once = std::sync::Once::new();

/// The pad menus read (Create-a-Skater, the customiser, the Graphic Creator): the retail active pad
/// (`ActivePad`) when one is chosen, else the first connected pad.
pub(crate) fn active_packet(active: Option<u8>, caches: &mut [CapabilityCache; 4]) -> Option<DevicePacket> {
    pick(active, |i| poll_cached(i, &mut caches[i]))
}

fn pick(active: Option<u8>, mut poll: impl FnMut(usize) -> Result<DevicePacket, DeviceError>) -> Option<DevicePacket> {
    match active.map(usize::from).filter(|&p| p < 4) {
        Some(pad) => poll(pad).ok(),
        None => (0..4).find_map(|i| poll(i).ok()),
    }
}

/// What kind of pad is in a slot (the button glyphs follow it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PadKind {
    Xbox,
    PlayStation,
    Nintendo,
    Generic,
}

struct ExternalPad {
    id: u64,
    kind: PadKind,
    state: XboxState,
    number: u32,
    slot: Option<usize>,
    /// [low, high] motor speeds the rumble mixer asked for.
    rumble: [f32; 2],
}

/// Pads the HID driver and the gilrs thread publish. XInput keeps its own user indices; the
/// others fill the free slots from the top, so they rarely move when an Xbox pad is plugged in.
struct Registry {
    xinput: [bool; 4],
    pads: Vec<ExternalPad>,
    next_id: u64,
    /// (from, to) slot moves of external pads an Xbox pad evicted, for `ActivePad` to follow.
    moves: Vec<(usize, usize)>,
}

static REGISTRY: std::sync::Mutex<Registry> =
    std::sync::Mutex::new(Registry { xinput: [false; 4], pads: Vec::new(), next_id: 0, moves: Vec::new() });

fn registry() -> std::sync::MutexGuard<'static, Registry> {
    REGISTRY.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Registry {
    fn assign(&mut self) {
        for i in 0..self.pads.len() {
            if self.pads[i].slot.is_none() {
                self.pads[i].slot = (0..4)
                    .rev()
                    .find(|&s| !self.xinput[s] && self.pads.iter().all(|p| p.slot != Some(s)));
            }
        }
    }
    fn set_xinput(&mut self, slot: usize, present: bool) {
        if self.xinput[slot] == present {
            return;
        }
        self.xinput[slot] = present;
        let mut evicted = None;
        if present {
            for pad in self.pads.iter_mut().filter(|p| p.slot == Some(slot)) {
                pad.slot = None;
                evicted = Some(pad.id);
            }
        }
        self.assign();
        if let Some(to) = evicted.and_then(|id| self.pads.iter().find(|p| p.id == id)?.slot) {
            self.moves.push((slot, to));
        }
    }
}

pub(crate) fn external_connect(kind: PadKind) -> u64 {
    let mut r = registry();
    r.next_id += 1;
    let id = r.next_id;
    let neutral = XboxState { buttons: 0, triggers: [0; 2], left: [0; 2], right: [0; 2] };
    r.pads.push(ExternalPad { id, kind, state: neutral, number: 0, slot: None, rumble: [0.0; 2] });
    r.assign();
    id
}

pub(crate) fn external_update(id: u64, state: XboxState) {
    if let Some(pad) = registry().pads.iter_mut().find(|p| p.id == id) {
        pad.state = state;
        pad.number = pad.number.wrapping_add(1);
    }
}

pub(crate) fn external_disconnect(id: u64) {
    let mut r = registry();
    r.pads.retain(|p| p.id != id);
    r.assign();
}

/// An external pad's registration: dropping it (a thread ending, or panicking) disconnects the pad.
pub(crate) struct External(pub u64);
impl External {
    pub(crate) fn connect(kind: PadKind) -> Self {
        Self(external_connect(kind))
    }
}
impl Drop for External {
    fn drop(&mut self) {
        external_disconnect(self.0);
    }
}

/// Slot moves since the last call (see `Registry::moves`).
pub(crate) fn take_moves() -> Vec<(usize, usize)> {
    std::mem::take(&mut registry().moves)
}

/// The [low, high] motor speeds wanted for an external pad.
pub(crate) fn external_rumble(id: u64) -> [f32; 2] {
    registry().pads.iter().find(|p| p.id == id).map_or([0.0; 2], |p| p.rumble)
}

pub(crate) fn pad_kind(slot: usize) -> Option<PadKind> {
    let r = registry();
    if r.xinput.get(slot).copied().unwrap_or(false) {
        return Some(PadKind::Xbox);
    }
    r.pads.iter().find(|p| p.slot == Some(slot)).map(|p| p.kind)
}

/// Drive a slot's motors: [low (left, heavy), high (right, light)], 0..1.
pub(crate) fn set_rumble(slot: usize, motors: [f32; 2]) {
    let mut r = registry();
    if r.xinput.get(slot).copied().unwrap_or(false) {
        drop(r);
        #[cfg(windows)]
        windows::vibrate(slot as u32, motors);
        return;
    }
    if let Some(pad) = r.pads.iter_mut().find(|p| p.slot == Some(slot)) {
        pad.rumble = motors;
    }
}

/// Every pad's motors off (on exit). The HID threads send their zero report within one read
/// timeout, so the caller waits that long.
pub(crate) fn stop_all_rumble() {
    for slot in 0..4 {
        set_rumble(slot, [0.0; 2]);
    }
    for pad in &mut registry().pads {
        pad.rumble = [0.0; 2];
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    /// User 10-26: a bound Push to Talk Button only talks: gone from the state the game sees, reported held; unbound,
    /// nothing is taken.
    #[test]
    fn the_bound_pad_button_only_talks() {
        let pad = |buttons| Ok(DevicePacket { number: 0, state: XboxState { buttons, triggers: [0; 2], left: [0; 2], right: [0; 2] }, subtype: 1 });
        let (packet, held) = talk_only(0x0100, pad(0x0100 | 0x1000));
        assert_eq!((packet.unwrap().state.buttons, held), (0x1000, true), "LB gone, A kept");
        assert!(!talk_only(0x0100, pad(0x1000)).1, "not held");
        assert!(!talk_only(0x0100, Err(DeviceError::Disconnected)).1, "no pad");
        let (packet, held) = talk_only(0, pad(0x0100 | 0x1000));
        assert_eq!((packet.unwrap().state.buttons, held), (0x1100, false), "unbound: nothing taken");
    }
    #[test]
    fn capability_cache_refreshes_and_never_caches_errors() {
        let start = std::time::Instant::now();
        let mut cache = CapabilityCache::default();
        assert_eq!(cache.get(start, || Ok(1)), Ok(1));
        assert_eq!(
            cache.get(start + std::time::Duration::from_millis(999), || panic!(
                "redundant capability query"
            )),
            Ok(1)
        );
        assert_eq!(
            cache.get(start + std::time::Duration::from_secs(1), || Ok(2)),
            Ok(2)
        );
        cache.invalidate();
        assert_eq!(
            cache.get(start, || Err(DeviceError::Capabilities(5))),
            Err(DeviceError::Capabilities(5))
        );
        assert_eq!(cache.get(start, || Ok(3)), Ok(3));
        cache.invalidate();
        assert_eq!(cache.get(start, || Ok(4)), Ok(4));
    }

    #[test]
    fn external_pads_fill_free_slots_and_yield_to_xinput() {
        let neutral = XboxState { buttons: 0, triggers: [0; 2], left: [0; 2], right: [0; 2] };
        let pad = |id| ExternalPad { id, kind: PadKind::PlayStation, state: neutral, number: 0, slot: None, rumble: [0.0; 2] };
        let mut r = Registry { xinput: [true, false, false, false], pads: vec![pad(1), pad(2)], next_id: 2, moves: Vec::new() };
        r.assign();
        assert_eq!(r.pads.iter().map(|p| p.slot).collect::<Vec<_>>(), [Some(3), Some(2)]);
        // An Xbox pad appearing at index 2 takes it; that pad moves to the last free slot.
        r.set_xinput(2, true);
        assert_eq!(r.pads.iter().map(|p| p.slot).collect::<Vec<_>>(), [Some(3), Some(1)]);
        // ...and the move is recorded, so an ActivePad on slot 2 follows it to 1.
        assert_eq!(r.moves, [(2, 1)]);
        // Five pads: the fifth waits for a slot.
        r.pads.push(pad(3));
        r.assign();
        assert_eq!(r.pads[2].slot, None);
        r.set_xinput(0, false);
        assert_eq!(r.pads[2].slot, Some(0));
    }

    #[test]
    fn menus_follow_the_active_pad_not_the_first_one() {
        // An idle XInput pad in slot 0 and the active DS4 (holding A) in slot 3.
        let poll = |i: usize| match i {
            0 => Ok(DevicePacket { number: 1, state: XboxState { buttons: 0, triggers: [0; 2], left: [0; 2], right: [0; 2] }, subtype: 1 }),
            3 => Ok(DevicePacket { number: 1, state: XboxState { buttons: 0x1000, triggers: [0; 2], left: [0; 2], right: [0; 2] }, subtype: 1 }),
            _ => Err(DeviceError::Disconnected),
        };
        assert_eq!(pick(Some(3), poll).unwrap().state.buttons, 0x1000);
        // No active pad yet: the first connected one.
        assert_eq!(pick(None, poll).unwrap().state.buttons, 0);
        // The active pad unplugged: nothing, not another pad.
        assert!(pick(Some(2), poll).is_none());
    }
}
