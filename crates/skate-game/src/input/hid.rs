//! Our own HID driver for the pads XInput can't see: DualShock 4, DualSense (Edge) and the Switch
//! Pro Controller, over USB and Bluetooth, with rumble. Report layouts follow the public
//! documentation of these devices (Linux `hid-playstation` / `hid-nintendo`, SDL's HIDAPI
//! drivers, dekuNukem's Switch reverse-engineering notes), written fresh here. Input is converted
//! to XInput-style raw values so the retail deadzone (`xbox::convert`) applies unchanged.
use skate_core::input::xbox::XboxState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Model {
    Ds4,
    DualSense,
    SwitchPro,
}

pub(crate) fn model(vendor: u16, product: u16) -> Option<Model> {
    match (vendor, product) {
        (0x054C, 0x05C4 | 0x09CC | 0x0BA0) => Some(Model::Ds4),
        (0x054C, 0x0CE6 | 0x0DF2) => Some(Model::DualSense),
        (0x057E, 0x2009) => Some(Model::SwitchPro),
        _ => None,
    }
}

const UP: u16 = 0x0001;
const DOWN: u16 = 0x0002;
const LEFT: u16 = 0x0004;
const RIGHT: u16 = 0x0008;
const START: u16 = 0x0010;
const BACK: u16 = 0x0020;
const L3: u16 = 0x0040;
const R3: u16 = 0x0080;
const LB: u16 = 0x0100;
const RB: u16 = 0x0200;
const A: u16 = 0x1000;
const B: u16 = 0x2000;
const X: u16 = 0x4000;
const Y: u16 = 0x8000;

fn bits(byte: u8, map: &[(u8, u16)]) -> u16 {
    map.iter().filter(|(bit, _)| byte & bit != 0).fold(0, |all, (_, button)| all | button)
}

/// PlayStation sticks: 0 = left/up, 255 = right/down. XInput's Y is up-positive.
fn sony_x(v: u8) -> i16 {
    (i32::from(v) * 257 - 32768) as i16
}
fn sony_y(v: u8) -> i16 {
    (32767 - i32::from(v) * 257) as i16
}

/// The shared PlayStation button bytes: hat + face, shoulders/Share/Options/sticks, PS/touchpad.
/// Share (Create) is Back and Options is Start, like the PS3 Select/Start they replace; a touchpad
/// click is Back too.
fn sony(sticks: &[u8], face: u8, shoulders: u8, system: u8, triggers: [u8; 2]) -> XboxState {
    const HAT: [u16; 8] = [UP, UP | RIGHT, RIGHT, DOWN | RIGHT, DOWN, DOWN | LEFT, LEFT, UP | LEFT];
    let hat = HAT.get(usize::from(face & 0x0F)).copied().unwrap_or(0);
    let buttons = hat
        | bits(face, &[(0x10, X), (0x20, A), (0x40, B), (0x80, Y)])
        | bits(shoulders, &[(0x01, LB), (0x02, RB), (0x10, BACK), (0x20, START), (0x40, L3), (0x80, R3)])
        | bits(system, &[(0x02, BACK)]);
    XboxState {
        buttons,
        triggers,
        left: [sony_x(sticks[0]), sony_y(sticks[1])],
        right: [sony_x(sticks[2]), sony_y(sticks[3])],
    }
}

/// DS4 layout (and the DualSense's Bluetooth "simple" report): sticks, face, shoulders, system,
/// L2, R2 from `at`.
fn ds4_layout(r: &[u8], at: usize) -> Option<XboxState> {
    let d = r.get(at..at + 9)?;
    Some(sony(&d[0..4], d[4], d[5], d[6], [d[7], d[8]]))
}

/// Switch Pro stick calibration (factory SPI 0x603D): per stick [x, y] centre and the spans above
/// and below it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StickCal {
    pub center: [u16; 2],
    pub above: [u16; 2],
    pub below: [u16; 2],
}
pub(crate) const DEFAULT_CAL: [StickCal; 2] =
    [StickCal { center: [2048; 2], above: [1600; 2], below: [1600; 2] }; 2];

fn twelve(b: &[u8]) -> [u16; 2] {
    [u16::from(b[0]) | (u16::from(b[1] & 0x0F) << 8), (u16::from(b[1]) >> 4) | (u16::from(b[2]) << 4)]
}

/// The 18 factory calibration bytes at SPI 0x603D: left stick (above, centre, below), then the
/// right stick (centre, below, above). Unprogrammed flash (0xFFF) keeps the defaults.
pub(crate) fn switch_calibration(d: &[u8]) -> [StickCal; 2] {
    if d.len() < 18 {
        return DEFAULT_CAL;
    }
    let l = [twelve(&d[0..3]), twelve(&d[3..6]), twelve(&d[6..9])];
    let r = [twelve(&d[9..12]), twelve(&d[12..15]), twelve(&d[15..18])];
    let cal = [StickCal { above: l[0], center: l[1], below: l[2] }, StickCal { center: r[0], below: r[1], above: r[2] }];
    let sane = cal.iter().all(|c| {
        [c.center, c.above, c.below].iter().flatten().all(|&v| v != 0 && v != 0x0FFF)
    });
    if sane { cal } else { DEFAULT_CAL }
}

fn switch_axis(raw: u16, cal: &StickCal, axis: usize) -> i16 {
    let d = f32::from(raw) - f32::from(cal.center[axis]);
    let span = f32::from(if d >= 0.0 { cal.above[axis] } else { cal.below[axis] }).max(1.0);
    ((d / span).clamp(-1.0, 1.0) * 32767.0) as i16
}

/// A Switch Pro full input report (0x30, or a 0x21 subcommand reply, which carries the same
/// state). Buttons map by position, so the bottom face button is A as on an Xbox pad.
fn switch_state(r: &[u8], cal: &[StickCal; 2]) -> Option<XboxState> {
    if !matches!(r.first(), Some(0x30 | 0x21)) || r.len() < 12 {
        return None;
    }
    let (right, shared, left) = (r[3], r[4], r[5]);
    let buttons = bits(right, &[(0x01, X), (0x02, Y), (0x04, A), (0x08, B), (0x40, RB)])
        | bits(shared, &[(0x01, BACK), (0x02, START), (0x04, R3), (0x08, L3)])
        | bits(left, &[(0x01, DOWN), (0x02, UP), (0x04, RIGHT), (0x08, LEFT), (0x40, LB)]);
    let stick = |at: usize, cal: &StickCal| {
        let [x, y] = twelve(&r[at..at + 3]);
        [switch_axis(x, cal, 0), switch_axis(y, cal, 1)]
    };
    Some(XboxState {
        buttons,
        triggers: [if left & 0x80 != 0 { 255 } else { 0 }, if right & 0x80 != 0 { 255 } else { 0 }],
        left: stick(6, &cal[0]),
        right: stick(9, &cal[1]),
    })
}

/// One input report, as read from the device (report id first).
pub(crate) fn parse(model: Model, bluetooth: bool, r: &[u8], cal: &[StickCal; 2]) -> Option<XboxState> {
    match (model, *r.first()?) {
        (Model::Ds4, 0x01) => ds4_layout(r, 1),
        (Model::Ds4, 0x11) => ds4_layout(r, 3),
        (Model::DualSense, 0x01) if bluetooth => ds4_layout(r, 1),
        (Model::DualSense, id @ (0x01 | 0x31)) => {
            let d = r.get(usize::from(id == 0x31) + 1..)?.get(..10)?;
            Some(sony(&d[0..4], d[7], d[8], d[9], [d[4], d[5]]))
        }
        (Model::SwitchPro, _) => switch_state(r, cal),
        _ => None,
    }
}

/// CRC-32 (IEEE, reflected), as the PlayStation Bluetooth reports carry it.
pub(crate) fn crc32(bytes: impl IntoIterator<Item = u8>) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// Bluetooth output reports end in the CRC of 0xA2 (the HID output header) + the report.
fn seal(mut r: Vec<u8>) -> Vec<u8> {
    let n = r.len() - 4;
    let crc = crc32(std::iter::once(0xA2).chain(r[..n].iter().copied()));
    r[n..].copy_from_slice(&crc.to_le_bytes());
    r
}

/// DualShock 4 rumble (motor flag only, so the light bar is left alone).
pub(crate) fn ds4_rumble(bluetooth: bool, [low, high]: [u8; 2]) -> Vec<u8> {
    if bluetooth {
        let mut r = vec![0; 78];
        (r[0], r[1], r[3], r[6], r[7]) = (0x11, 0xC0, 0x01, high, low);
        seal(r)
    } else {
        let mut r = vec![0; 32];
        (r[0], r[1], r[4], r[5]) = (0x05, 0x01, high, low);
        r
    }
}

/// The DS4 USB wireless adapter (0BA0) keeps reporting with no pad paired: byte 31 bit 2.
pub(crate) fn dongle_empty(product: u16, r: &[u8]) -> bool {
    product == 0x0BA0 && r.first() == Some(&0x01) && r.len() >= 32 && r[31] & 0x04 != 0
}

/// Whether a DualSense takes "compatible vibration v2": always the Edge, else firmware update
/// version (feature report 0x20, bytes 44..46) 2.21 (0x0215) or newer, as Linux hid-playstation.
pub(crate) fn dualsense_v2(product: u16, firmware: Option<&[u8]>) -> bool {
    product == 0x0DF2
        || firmware.and_then(|f| f.get(44..46)).is_some_and(|v| u16::from_le_bytes([v[0], v[1]]) >= 0x0215)
}

/// DualSense rumble in its "compatible vibration" mode: flag0 vibration + haptics select (v1), or
/// haptics select + valid_flag2 (common +38) vibration v2 on newer firmware. Bluetooth carries a
/// 4-bit sequence number and the 0x10 tag before the common block.
pub(crate) fn dualsense_rumble(bluetooth: bool, sequence: u8, [low, high]: [u8; 2], v2: bool) -> Vec<u8> {
    let (mut r, common) = if bluetooth { (vec![0; 78], 3) } else { (vec![0; 48], 1) };
    r[0] = if bluetooth { 0x31 } else { 0x02 };
    if bluetooth {
        (r[1], r[2]) = ((sequence & 0x0F) << 4, 0x10);
    }
    (r[common], r[common + 2], r[common + 3]) = (if v2 { 0x02 } else { 0x03 }, high, low);
    if v2 {
        r[common + 38] = 0x04;
    }
    if bluetooth { seal(r) } else { r }
}

/// HD rumble amplitude code for 0..1 (dekuNukem's piecewise log table; 1.0 = 100).
fn switch_amplitude(a: f32) -> u16 {
    let a = a.clamp(0.0, 1.0);
    let code = if a > 0.23 {
        (a * 8.7).log2() * 32.0
    } else if a > 0.12 {
        (a * 17.0).log2() * 16.0
    } else {
        a / 0.12 * (0.12f32 * 17.0).log2() * 16.0
    };
    code.round() as u16
}

/// One actuator's 4 HD rumble bytes: the high band at 320 Hz carries `high`, the low band at
/// 160 Hz carries `low`, like the two motors of an Xbox pad.
pub(crate) fn switch_rumble_bytes(low: f32, high: f32) -> [u8; 4] {
    const HF: u16 = 0x0100; // (round(log2(320/10)*32) - 0x60) * 4
    const LF: u8 = 0x40; // round(log2(160/10)*32) - 0x40
    let hf_amp = switch_amplitude(high) * 2;
    let code = switch_amplitude(low);
    let lf_amp = code / 2 + 64 + if code & 1 != 0 { 0x8000 } else { 0 };
    [(HF & 0xFF) as u8, (hf_amp + (HF >> 8)) as u8, LF + (lf_amp >> 8) as u8, (lf_amp & 0xFF) as u8]
}

/// A rumble-only output report (0x10) for both actuators.
pub(crate) fn switch_rumble(counter: u8, low: f32, high: f32) -> Vec<u8> {
    let b = switch_rumble_bytes(low, high);
    [&[0x10, counter & 0x0F][..], &b, &b].concat()
}

/// A subcommand output report (0x01) with neutral rumble.
pub(crate) fn switch_subcommand(counter: u8, command: u8, args: &[u8]) -> Vec<u8> {
    let neutral = switch_rumble_bytes(0.0, 0.0);
    [&[0x01, counter & 0x0F][..], &neutral, &neutral, &[command], args].concat()
}

#[cfg(windows)]
pub(super) use os::start;

#[cfg(windows)]
mod os {
    use super::*;
    use crate::input::platform::{self, PadKind};
    use std::sync::{Mutex, PoisonError};
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::*;
    use windows_sys::Win32::Devices::HumanInterfaceDevice::*;
    use windows_sys::Win32::Foundation::*;
    use windows_sys::Win32::Storage::FileSystem::*;
    use windows_sys::Win32::System::IO::*;
    use windows_sys::Win32::System::Threading::CreateEventW;

    /// Device paths a pad thread has open.
    static OPEN: Mutex<Vec<Vec<u16>>> = Mutex::new(Vec::new());

    pub(in crate::input) fn start() {
        let _ = std::thread::Builder::new().name("hid-pads".into()).spawn(|| loop {
            for (path, model, product) in enumerate() {
                let mut open = OPEN.lock().unwrap_or_else(PoisonError::into_inner);
                if open.contains(&path) {
                    continue;
                }
                open.push(path.clone());
                drop(open);
                let _ = std::thread::Builder::new().name("hid-pad".into()).spawn(move || {
                    // Released however the thread ends, a panic included.
                    let open = Open(path);
                    run(&open.0, model, product);
                });
            }
            // ponytail: re-enumerating every 2 s is the hot-plug detection; a device-change
            // notification would need a window.
            std::thread::sleep(Duration::from_secs(2));
        });
    }

    struct Open(Vec<u16>);
    impl Drop for Open {
        fn drop(&mut self) {
            OPEN.lock().unwrap_or_else(PoisonError::into_inner).retain(|p| *p != self.0);
        }
    }

    struct Device {
        feature_len: usize,
        handle: HANDLE,
        event: HANDLE,
        input_len: usize,
        output_len: usize,
    }

    impl Drop for Device {
        fn drop(&mut self) {
            // SAFETY: both handles are owned by this Device and no I/O is pending.
            unsafe {
                CloseHandle(self.handle);
                CloseHandle(self.event);
            }
        }
    }

    /// Open a HID path; `None` unless it's a gamepad/joystick collection.
    fn open(path: &[u16], access: u32, flags: u32) -> Option<(HANDLE, HIDD_ATTRIBUTES, HIDP_CAPS)> {
        // SAFETY: `path` is NUL-terminated; out-structures are plain data the calls fill.
        unsafe {
            let handle = CreateFileW(
                path.as_ptr(),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                flags,
                std::ptr::null_mut(),
            );
            if handle == INVALID_HANDLE_VALUE {
                return None;
            }
            let mut attributes = HIDD_ATTRIBUTES { Size: size_of::<HIDD_ATTRIBUTES>() as u32, ..Default::default() };
            let mut caps = HIDP_CAPS::default();
            let mut data: PHIDP_PREPARSED_DATA = 0;
            let ok = HidD_GetAttributes(handle, &mut attributes) && HidD_GetPreparsedData(handle, &mut data) && {
                let status = HidP_GetCaps(data, &mut caps);
                HidD_FreePreparsedData(data);
                status == HIDP_STATUS_SUCCESS
            };
            if !ok || caps.UsagePage != 1 || !matches!(caps.Usage, 4 | 5) {
                CloseHandle(handle);
                return None;
            }
            Some((handle, attributes, caps))
        }
    }

    /// Every present HID gamepad this driver knows, as (NUL-terminated path, model, product id).
    fn enumerate() -> Vec<(Vec<u16>, Model, u16)> {
        let mut found = Vec::new();
        // SAFETY: SetupAPI calls with correctly sized structures; the detail buffer is u32-aligned
        // and as large as the API asked for.
        unsafe {
            let mut guid = std::mem::zeroed();
            HidD_GetHidGuid(&mut guid);
            let set = SetupDiGetClassDevsW(&guid, std::ptr::null(), std::ptr::null_mut(), DIGCF_PRESENT | DIGCF_DEVICEINTERFACE);
            if set == INVALID_HANDLE_VALUE as HDEVINFO {
                return found;
            }
            let mut index = 0;
            loop {
                let mut interface = SP_DEVICE_INTERFACE_DATA { cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32, ..Default::default() };
                if SetupDiEnumDeviceInterfaces(set, std::ptr::null(), &guid, index, &mut interface) == 0 {
                    break;
                }
                index += 1;
                let mut size = 0u32;
                SetupDiGetDeviceInterfaceDetailW(set, &interface, std::ptr::null_mut(), 0, &mut size, std::ptr::null_mut());
                if size < 8 {
                    continue;
                }
                let mut buffer = vec![0u32; (size as usize).div_ceil(4)];
                let detail = buffer.as_mut_ptr().cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
                (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
                if SetupDiGetDeviceInterfaceDetailW(set, &interface, detail, size, std::ptr::null_mut(), std::ptr::null_mut()) == 0 {
                    continue;
                }
                let text = std::ptr::addr_of!((*detail).DevicePath).cast::<u16>();
                let chars = (size as usize - 4) / 2;
                let path: Vec<u16> = (0..chars).map(|i| *text.add(i)).take_while(|&c| c != 0).chain([0]).collect();
                // Only Sony and Nintendo devices get opened (USB `vid_054c`, Bluetooth `vid&0002054c`).
                let lower = String::from_utf16_lossy(&path).to_ascii_lowercase();
                let ours = ["054c", "057e"].iter().any(|v| lower.contains(&format!("vid_{v}")) || lower.contains(&format!("vid&0002{v}")));
                if !ours || OPEN.lock().unwrap_or_else(PoisonError::into_inner).contains(&path) {
                    continue;
                }
                if let Some((handle, attributes, _)) = open(&path, 0, 0) {
                    CloseHandle(handle);
                    if let Some(model) = model(attributes.VendorID, attributes.ProductID) {
                        found.push((path, model, attributes.ProductID));
                    }
                }
            }
            SetupDiDestroyDeviceInfoList(set);
        }
        found
    }

    impl Device {
        fn open(path: &[u16]) -> Option<Self> {
            let (handle, _, caps) = open(path, GENERIC_READ | GENERIC_WRITE, FILE_FLAG_OVERLAPPED)
                .or_else(|| open(path, GENERIC_READ, FILE_FLAG_OVERLAPPED))?;
            // SAFETY: a manual-reset, unnamed event.
            let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
            if event.is_null() {
                // SAFETY: the handle was just opened.
                unsafe { CloseHandle(handle) };
                return None;
            }
            Some(Self {
                handle,
                event,
                feature_len: usize::from(caps.FeatureReportByteLength),
                input_len: usize::from(caps.InputReportByteLength).max(64),
                output_len: usize::from(caps.OutputReportByteLength),
            })
        }

        /// One overlapped transfer: `Some(bytes)`, `Some(0)` on timeout, `None` when the device
        /// is gone.
        fn transfer(&self, buffer: &mut [u8], write: bool, timeout_ms: u32) -> Option<usize> {
            // SAFETY: `buffer` and `overlapped` outlive the I/O: it completes, or is cancelled and
            // waited for, before returning.
            unsafe {
                let mut overlapped: OVERLAPPED = std::mem::zeroed();
                overlapped.hEvent = self.event;
                let len = buffer.len() as u32;
                let started = if write {
                    WriteFile(self.handle, buffer.as_ptr(), len, std::ptr::null_mut(), &mut overlapped)
                } else {
                    ReadFile(self.handle, buffer.as_mut_ptr(), len, std::ptr::null_mut(), &mut overlapped)
                };
                if started == 0 && GetLastError() != ERROR_IO_PENDING {
                    return None;
                }
                let mut done = 0u32;
                if GetOverlappedResultEx(self.handle, &overlapped, &mut done, timeout_ms, 0) != 0 {
                    return Some(done as usize);
                }
                let error = GetLastError();
                CancelIoEx(self.handle, &overlapped);
                GetOverlappedResult(self.handle, &overlapped, &mut done, 1);
                (error == WAIT_TIMEOUT).then_some(0)
            }
        }

        fn read<'a>(&self, buffer: &'a mut [u8], timeout_ms: u32) -> Option<&'a [u8]> {
            let n = self.transfer(buffer, false, timeout_ms)?;
            Some(&buffer[..n])
        }

        /// A feature report by id (`None` if the device refuses it).
        fn feature(&self, id: u8) -> Option<Vec<u8>> {
            let mut buffer = vec![0u8; self.feature_len.max(64)];
            buffer[0] = id;
            // SAFETY: a writable buffer of the length passed.
            unsafe { HidD_GetFeature(self.handle, buffer.as_mut_ptr().cast(), buffer.len() as u32) }.then_some(buffer)
        }

        /// Windows wants every output report padded to the device's longest one.
        fn write(&self, report: &[u8]) -> bool {
            let mut padded = report.to_vec();
            padded.resize(self.output_len.max(report.len()), 0);
            self.transfer(&mut padded, true, 100).is_some_and(|n| n > 0)
        }
    }

    /// The Switch Pro's start-up: the USB handshake (handshake, 3 Mbit, handshake, no USB
    /// timeout), then full 0x30 reports, vibration on, player light 1, and the factory stick
    /// calibration from SPI 0x603D.
    fn switch_init(device: &Device, bluetooth: bool, counter: &mut u8) -> [StickCal; 2] {
        let mut buffer = vec![0u8; device.input_len];
        if !bluetooth {
            for step in [0x02, 0x03, 0x02, 0x04] {
                device.write(&[0x80, step]);
                for _ in 0..5 {
                    if device.read(&mut buffer, 100).is_none_or(|r| r.first() == Some(&0x81)) {
                        break;
                    }
                }
            }
        }
        let mut subcommand = |command: u8, args: &[u8]| -> Option<Vec<u8>> {
            *counter = counter.wrapping_add(1);
            device.write(&switch_subcommand(*counter, command, args));
            for _ in 0..10 {
                let reply = device.read(&mut buffer, 100)?;
                if reply.first() == Some(&0x21) && reply.get(14) == Some(&command) {
                    return Some(reply.to_vec());
                }
            }
            None
        };
        subcommand(0x03, &[0x30]);
        subcommand(0x48, &[0x01]);
        subcommand(0x30, &[0x01]);
        subcommand(0x10, &[0x3D, 0x60, 0x00, 0x00, 18])
            .and_then(|reply| reply.get(20..38).map(switch_calibration))
            .unwrap_or(DEFAULT_CAL)
    }

    fn run(path: &[u16], model: Model, product: u16) {
        let lower = String::from_utf16_lossy(path).to_ascii_lowercase();
        // The Bluetooth HID service class GUID is in every Bluetooth HID device path.
        let bluetooth = lower.contains("00001124-0000-1000-8000-00805f9b34fb");
        let Some(device) = Device::open(path) else {
            return;
        };
        let mut counter = 0u8;
        let cal = if model == Model::SwitchPro { switch_init(&device, bluetooth, &mut counter) } else { DEFAULT_CAL };
        let kind = if model == Model::SwitchPro { PadKind::Nintendo } else { PadKind::PlayStation };
        let v2 = model == Model::DualSense && dualsense_v2(product, device.feature(0x20).as_deref());
        // Registered on the first real report; dropping it (unplug, an emptied adapter, a panic)
        // frees the slot.
        let mut pad: Option<platform::External> = None;
        let mut buffer = vec![0u8; device.input_len];
        // The motors start off, so nothing is written until they turn on: a first Bluetooth write
        // switches a PlayStation pad to its enhanced reports for good.
        let mut sent = Some([0u8; 2]);
        let mut last_send = Instant::now();
        let mut last_report = Instant::now();
        let mut silent = false;
        loop {
            let Some(report) = device.read(&mut buffer, 50) else { break };
            if dongle_empty(product, report) {
                pad = None;
            } else if let Some(state) = parse(model, bluetooth, report, &cal) {
                let id = pad.get_or_insert_with(|| platform::External::connect(kind)).0;
                platform::external_update(id, state);
                (last_report, silent) = (Instant::now(), false);
            }
            let Some(id) = pad.as_ref().map(|p| p.0) else { continue };
            // A Bluetooth pad can stop streaming without the read failing: let go of its buttons.
            if !silent && last_report.elapsed() > Duration::from_secs(1) {
                platform::external_update(id, XboxState { buttons: 0, triggers: [0; 2], left: [0; 2], right: [0; 2] });
                silent = true;
            }
            let motors = platform::external_rumble(id);
            let bytes = motors.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
            let since = last_send.elapsed();
            // Switch HD rumble fades out unless it is refreshed; over Bluetooth it takes one write
            // per 30 ms at most.
            let refresh = model == Model::SwitchPro && bytes != [0, 0] && since > Duration::from_millis(50);
            let paced = !(model == Model::SwitchPro && bluetooth && since < Duration::from_millis(30));
            if (sent != Some(bytes) || refresh) && paced {
                counter = counter.wrapping_add(1);
                let report = match model {
                    Model::Ds4 => ds4_rumble(bluetooth, bytes),
                    Model::DualSense => dualsense_rumble(bluetooth, counter, bytes, v2),
                    Model::SwitchPro => switch_rumble(counter, motors[0], motors[1]),
                };
                device.write(&report);
                sent = Some(bytes);
                last_send = Instant::now();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skate_core::input::xbox;

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(crc32(*b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn ds4_usb_and_bluetooth_reports_decode_alike() {
        // Sticks hard right / up, cross + dpad east, L1 + Options, PS, L2 half, R2 full.
        let common = [255, 0, 128, 255, 0x22, 0x21, 0x01, 128, 255];
        let usb = [&[0x01][..], &common].concat();
        let bt = [&[0x11, 0xC0, 0x00][..], &common].concat();
        let simple_bt = usb.clone();
        let expect = XboxState {
            buttons: RIGHT | A | LB | START,
            triggers: [128, 255],
            left: [32767, 32767],
            right: [128, -32768],
        };
        for report in [&usb, &bt, &simple_bt] {
            assert_eq!(parse(Model::Ds4, false, report, &DEFAULT_CAL), Some(expect));
        }
        assert_eq!(parse(Model::Ds4, false, &[0x05, 0, 0], &DEFAULT_CAL), None);
    }

    #[test]
    fn dualsense_usb_bluetooth_and_simple_reports_decode() {
        // Sticks centre-ish, L2 10, R2 20, seq, hat none + triangle + square, R1 + Create + R3, touchpad.
        let common = [128, 128, 0, 255, 10, 20, 7, 0x98, 0x92, 0x02];
        let usb = [&[0x01][..], &common].concat();
        let bt = [&[0x31, 0x00][..], &common].concat();
        let expect = XboxState {
            buttons: Y | X | RB | BACK | R3,
            triggers: [10, 20],
            left: [128, -129],
            right: [-32768, -32768],
        };
        assert_eq!(parse(Model::DualSense, false, &usb, &DEFAULT_CAL), Some(expect));
        assert_eq!(parse(Model::DualSense, true, &bt, &DEFAULT_CAL), Some(expect));
        // Bluetooth before the full mode: the DS4-style 0x01 report.
        let simple = [0x01, 128, 128, 0, 255, 0x98, 0x92, 0x02, 10, 20];
        assert_eq!(parse(Model::DualSense, true, &simple, &DEFAULT_CAL), Some(expect));
    }

    #[test]
    fn playstation_rumble_reports_and_crcs() {
        let usb = ds4_rumble(false, [200, 100]);
        assert_eq!((usb.len(), usb[0], usb[1], usb[4], usb[5]), (32, 0x05, 0x01, 100, 200));
        let bt = ds4_rumble(true, [200, 100]);
        assert_eq!((bt.len(), bt[0], bt[1], bt[3], bt[6], bt[7]), (78, 0x11, 0xC0, 0x01, 100, 200));
        let crc = crc32(std::iter::once(0xA2).chain(bt[..74].iter().copied()));
        assert_eq!(bt[74..], crc.to_le_bytes());

        let usb = dualsense_rumble(false, 3, [200, 100], false);
        assert_eq!((usb.len(), usb[0], usb[1], usb[3], usb[4], usb[39]), (48, 0x02, 0x03, 100, 200, 0));
        let bt = dualsense_rumble(true, 3, [200, 100], false);
        assert_eq!((bt.len(), bt[0], bt[1], bt[2], bt[3], bt[5], bt[6]), (78, 0x31, 0x30, 0x10, 0x03, 100, 200));
        let crc = crc32(std::iter::once(0xA2).chain(bt[..74].iter().copied()));
        assert_eq!(bt[74..], crc.to_le_bytes());
        // Any changed byte changes the CRC.
        assert_ne!(dualsense_rumble(true, 3, [201, 100], false)[74..], bt[74..]);
        // Vibration v2: haptics select alone in flag0, valid_flag2 (common +38) = 0x04.
        let usb = dualsense_rumble(false, 0, [200, 100], true);
        assert_eq!((usb[1], usb[3], usb[4], usb[39]), (0x02, 100, 200, 0x04));
        let bt = dualsense_rumble(true, 0, [200, 100], true);
        assert_eq!((bt[3], bt[5], bt[6], bt[41]), (0x02, 100, 200, 0x04));
        assert_eq!(bt[74..], crc32(std::iter::once(0xA2).chain(bt[..74].iter().copied())).to_le_bytes());
    }

    #[test]
    fn dualsense_v2_and_the_empty_ds4_adapter() {
        let mut firmware = vec![0u8; 64];
        firmware[44..46].copy_from_slice(&0x0215u16.to_le_bytes());
        assert!(dualsense_v2(0x0DF2, None) && dualsense_v2(0x0CE6, Some(&firmware)));
        firmware[44..46].copy_from_slice(&0x0214u16.to_le_bytes());
        assert!(!dualsense_v2(0x0CE6, Some(&firmware)) && !dualsense_v2(0x0CE6, None));
        let mut empty = vec![0u8; 64];
        (empty[0], empty[31]) = (0x01, 0x04);
        assert!(dongle_empty(0x0BA0, &empty) && !dongle_empty(0x09CC, &empty));
        empty[31] = 0;
        assert!(!dongle_empty(0x0BA0, &empty));
    }

    #[test]
    fn switch_hd_rumble_encoding() {
        assert_eq!(switch_rumble_bytes(0.0, 0.0), [0x00, 0x01, 0x40, 0x40]);
        // Full: hf amp 0xC8 (+1 from the 320 Hz code), lf amp 0x72.
        assert_eq!(switch_rumble_bytes(1.0, 1.0), [0x00, 0xC9, 0x40, 0x72]);
        // 0.5: code round(log2(4.35)*32) = 68 -> hf 136, lf 34 + 64 = 98.
        assert_eq!(switch_rumble_bytes(0.5, 0.5), [0x00, 0x89, 0x40, 0x62]);
        // An odd low code sets the 0x8000 bit (byte 2 + 0x80): 0.32 -> round(log2(2.784)*32) = 47,
        // lf amp 23 + 64 = 0x57.
        assert_eq!(switch_rumble_bytes(0.32, 0.0), [0x00, 0x01, 0xC0, 0x57]);
        let r = switch_rumble(17, 1.0, 0.0);
        assert_eq!(r, [0x10, 0x01, 0x00, 0x01, 0x40, 0x72, 0x00, 0x01, 0x40, 0x72]);
        let s = switch_subcommand(2, 0x48, &[0x01]);
        assert_eq!(s, [0x01, 0x02, 0x00, 0x01, 0x40, 0x40, 0x00, 0x01, 0x40, 0x40, 0x48, 0x01]);
    }

    #[test]
    fn switch_pro_full_report_maps_by_position_with_calibration() {
        let cal = switch_calibration(&[
            // left: above (1500, 1400), centre (2000, 2100), below (1300, 1200)
            0xDC, 0x85, 0x57, 0xD0, 0x47, 0x83, 0x14, 0x05, 0x4B,
            // right: centre (2048, 2048), below (1600, 1600), above (1600, 1600)
            0x00, 0x08, 0x80, 0x40, 0x06, 0x64, 0x40, 0x06, 0x64,
        ]);
        assert_eq!(cal[0], StickCal { center: [2000, 2100], above: [1500, 1400], below: [1300, 1200] });
        assert_eq!(cal[1], DEFAULT_CAL[1]);
        let mut r = [0u8; 49];
        r[0] = 0x30;
        r[3] = 0x04 | 0x08 | 0x80; // B (bottom) + A (right) + ZR
        r[4] = 0x02 | 0x01; // plus + minus
        r[5] = 0x02 | 0x40; // up + L
        // Left stick x 3500 (full right), y 2100 (centre); right stick x 448 (full left), y 2048.
        r[6..9].copy_from_slice(&[0xAC, 0x4D, 0x83]);
        r[9..12].copy_from_slice(&[0xC0, 0x01, 0x80]);
        let state = parse(Model::SwitchPro, false, &r, &cal).unwrap();
        assert_eq!(state.buttons, A | B | START | BACK | UP | LB);
        assert_eq!(state.triggers, [0, 255]);
        assert_eq!(state.left, [32767, 0]);
        assert_eq!(state.right, [-32767, 0]);
        // Unprogrammed flash keeps the defaults.
        assert_eq!(switch_calibration(&[0xFF; 18]), DEFAULT_CAL);
    }

    #[test]
    fn converted_pads_get_the_retail_deadzone_like_xinput() {
        // A PlayStation stick resting slightly off centre reads as zero after the retail radial
        // deadzone, exactly as an XInput pad with the same raw values would.
        let rest = parse(Model::Ds4, false, &[0x01, 140, 120, 128, 128, 0x08, 0, 0, 0, 0], &DEFAULT_CAL).unwrap();
        let xinput = XboxState { buttons: 0, triggers: [0; 2], left: [sony_x(140), sony_y(120)], right: [128, -129] };
        assert_eq!(xbox::convert(&rest, 0), xbox::convert(&xinput, 0));
        let values = xbox::convert(&rest, 0);
        let full = parse(Model::Ds4, false, &[0x01, 255, 128, 128, 128, 0x08, 0, 0, 0, 0], &DEFAULT_CAL).unwrap();
        assert_ne!(xbox::convert(&full, 0), values);
    }
}
