//! The messages the game and the Skate 2 physics host swap once per 60 Hz tick over pipes. Not tested in play.
//!
//! One message on the wire is [`MAGIC`], a u32 payload length, then the payload: a one-byte tag for an enum and the
//! fields in declaration order. Values are little-endian, a `Vec` is a u32 count and its items, and an `Option` is a
//! 0 or 1 byte and then its value. Both sides build from this crate, so the layout is these definitions and the tag
//! numbers in the `Wire` impls of [`EngineMessage`] and [`HostMessage`]. Any change to either bumps
//! [`PROTOCOL_VERSION`].

use std::fmt;
use std::io::{self, Read, Write};

pub const MAGIC: [u8; 4] = *b"S2PH";
pub const PROTOCOL_VERSION: u32 = 4;
/// The largest payload a reader accepts; a frame is under 3 KB.
pub const MAX_MESSAGE: u32 = 1 << 20;
/// The host's period when nothing stretches it: the port runs at 60 Hz.
pub const TICK_SECONDS: f32 = 1.0 / 60.0;

/// The names of `Frame::bones`, in order: the first 32 of `tools/author_bike_poses.py` `BONES`, lowercased.
pub const PORT_BONES: [&str; 32] = [
    "trajectory",
    "hips",
    "spine",
    "spine1",
    "spine2",
    "spine3",
    "neck",
    "neck1",
    "head",
    "rightshoulder",
    "rightarm",
    "rightforearm",
    "righthand",
    "leftshoulder",
    "leftarm",
    "leftforearm",
    "lefthand",
    "rightupleg",
    "rightleg",
    "rightfoot",
    "righttoebase",
    "leftupleg",
    "leftleg",
    "leftfoot",
    "lefttoebase",
    "skateboard_root",
    "truck_front",
    "right_wheelfront",
    "left_wheelfront",
    "truck_back",
    "left_wheelback",
    "right_wheelback",
];

// ---- engine to host -------------------------------------------------------------------------------------------

/// The port's `XboxPadSample`: raw XInput integers, before any deadzone or Y inversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pad {
    pub buttons: u16,
    pub left_trigger: u8,
    pub right_trigger: u8,
    /// `[x, y]`.
    pub left: [i16; 2],
    /// `[x, y]`.
    pub right: [i16; 2],
}

/// Skate 2's camera options (its `ProfileSettings`, which the port applies to its camera every tick). The host boots
/// with no Skate 2 profile, so these are how the engine's own options reach the port's camera.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Profile {
    /// 0 is the Low camera and 1 the High one.
    pub camera_angle: i32,
    /// Inverts the right stick's x when the skater is off the board and looks around.
    pub invert_x: bool,
    /// The same for y.
    pub invert_y: bool,
    pub grind_transparency: bool,
}

impl Default for Profile {
    /// The port's own default profile: `ProfileSettings::default()` (`tSettings::Default` 0x825644d8) in
    /// `skate2-formats/src/skater_profile.rs` 306-329: `camera_angle` 0, both inverts and `grind_transparency` off.
    fn default() -> Self {
        Self { camera_angle: 0, invert_x: false, invert_y: false, grind_transparency: false }
    }
}

/// What the host needs to start a session.
#[derive(Debug, Clone, PartialEq)]
pub struct Boot {
    pub seed: Option<u64>,
    pub exact_delivery: Option<bool>,
    /// The game mode: 1, 3 or 4. Any other value (0 included) leaves the mode the port boots in.
    pub mode: u8,
    pub gestures: [u32; 4],
    pub ambient: bool,
    pub profile: Profile,
}

impl Default for Boot {
    /// Nothing asked for: clock-seeded, the port's own delivery and mode, the native gestures (sky, what, snap and
    /// thumbdown: `[0, 1, 2, 3]`, world-objects 8), no ambient life and the port's default profile.
    fn default() -> Self {
        Self {
            seed: None,
            exact_delivery: None,
            mode: 0,
            gestures: [0, 1, 2, 3],
            ambient: false,
            profile: Profile::default(),
        }
    }
}

/// Put the skater down at `position` (metres, the host's world) facing `yaw`, moving at `velocity`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Place {
    pub position: [f32; 3],
    pub yaw: f32,
    pub velocity: [f32; 3],
    pub end_line: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EngineMessage {
    Hello { version: u32 },
    Boot(Boot),
    Tick(Pad),
    Place(Place),
    SetGestures([u32; 4]),
    SetMode(u8),
    SetAmbient(bool),
    Shutdown,
    SetProfile(Profile),
}

// ---- host to engine -------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Camera {
    pub position: [f32; 3],
    pub forward: [f32; 3],
    pub right: [f32; 3],
    pub up: [f32; 3],
    pub clip_from_world: [f32; 16],
}

/// The tick's trick events and the line's running score. Trick ids are the port's `0..=268`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Tricks {
    pub begun: Vec<u16>,
    pub converted: Vec<u16>,
    /// `(trick id, points)`.
    pub ended: Vec<(u16, u32)>,
    pub active: Vec<u16>,
    pub grinding: bool,
    pub sequence_begun: bool,
    pub sequence_completed: bool,
    pub completed_successfully: bool,
    pub interrupted: bool,
    pub line_score: u32,
    /// Protocol 3 (user 10-22, the HUD): the port's pre-multiplied HUD sequence score (`hud_score`), trickdisplay2's
    /// score; `line_score` is its line total.
    pub sequence_score: u32,
    pub line_points: i32,
    pub line_max_points: i32,
    pub multiplier: f32,
    pub spins: i32,
    pub flips: i32,
    pub just_landed: bool,
    pub sketchy: bool,
}

/// The Hall of Meat bail state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Hom {
    pub just_wiped_out: bool,
    pub effect_enabled: bool,
    pub bail_in_progress: bool,
    pub bail_completed: bool,
    pub just_reactivated: bool,
    pub hud: HomHud,
}

/// The port's `HomHudData` (`skate2-core` `ui/hom_scoring.rs` 16-40): every field, in the port's order. The four
/// per-entity score arrays and the tweak score are named `car_scores`, `ped_scores`, `dmo_scores`, `skater_scores`
/// and `tweak` here (the port's `*_collision_score` and `body_tweak_score`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HomHud {
    pub dirty: bool,
    pub score: u32,
    /// Never written by the port.
    pub multiplier: u32,
    pub rotation: f32,
    pub rotation_score: f32,
    pub wipeout_speed_score: f32,
    pub max_air_drop_score: f32,
    pub max_air_time_score: f32,
    pub wipeout_duration_score: f32,
    pub wipeout_speed: f32,
    pub max_air_drop: f32,
    pub max_air_time: f32,
    pub wipeout_duration: f32,
    pub new_car_collisions: u32,
    pub new_ped_collisions: u32,
    pub new_dmo_collisions: u32,
    pub new_skater_collisions: u32,
    pub new_body_tweak: bool,
    pub car_scores: [f32; 10],
    pub ped_scores: [f32; 10],
    pub dmo_scores: [f32; 10],
    pub skater_scores: [f32; 10],
    pub tweak: f32,
}

/// The `SkaterAudioPhysOut` fields the engine's audio record maps.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SkaterAudio {
    pub centre_of_mass_velocity: [f32; 4],
    pub deck_velocity: [f32; 4],
    pub wheel_pos: [[f32; 4]; 4],
    pub skateboard_surface_speed: f32,
    pub time_in_known_air: f32,
    pub jump_height: f32,
    pub deck_slide_speed: f32,
    pub wheel_slide: f32,
    pub turn: f32,
    pub num_wheels_in_contact: i32,
    pub physics_state: i32,
    pub is_skateboard_airborne: bool,
    pub is_wiping_out: bool,
    pub is_wipeout_over: bool,
    pub is_manual: bool,
    pub is_foot_braking: bool,
    pub is_physically_pushing: bool,
    pub is_left_foot_planting: bool,
    pub is_right_foot_planting: bool,
    /// Protocol 3 (user 10-22, the sound): the board's up and nose axes (`effective_transform_y`, `deck_nose_dir`),
    /// world, W 0.
    pub deck_up: [f32; 4],
    pub deck_nose: [f32; 4],
    /// The board's angular speed about its own axes (`deck_local_xyz_angular_speed`).
    pub deck_local_angular_speed: [f32; 4],
    pub deck_tilt: f32,
    pub is_manual_braking: bool,
    pub is_generally_pushing: bool,
    pub is_generally_pushing_right_foot: bool,
    pub is_reverting: bool,
    pub is_left_foot_on_deck: bool,
    pub is_right_foot_on_deck: bool,
    pub is_front_truck_in_contact: bool,
    pub is_back_truck_in_contact: bool,
    pub is_deck_in_contact: bool,
    /// Protocol 4 (user 10-22, no walking, bail or wheel spin sounds): the skeleton's centre of mass, world, W 1.
    pub centre_of_mass: [f32; 4],
    /// The feet's velocities, world and in the deck's axes (`foot_left_vel`, `foot_l_vel_deck_space`, ...).
    pub foot_left_velocity: [f32; 4],
    pub foot_right_velocity: [f32; 4],
    pub foot_left_velocity_deck: [f32; 4],
    pub foot_right_velocity_deck: [f32; 4],
    /// `torso_angular_vel`, `right_foot_angular_vel`, `left_foot_angular_vel`.
    pub torso_angular_velocity: [f32; 4],
    pub right_foot_angular_velocity: [f32; 4],
    pub left_foot_angular_velocity: [f32; 4],
    pub average_limb_speed: [f32; 4],
    /// Off the board: each foot on the ground (`left_foot_down`, `right_foot_down`) and the board in the hand.
    pub left_foot_down: bool,
    pub right_foot_down: bool,
    pub is_holding_skateboard: bool,
    /// An air footplant (`is_foot_planting`).
    pub is_foot_planting: bool,
    /// `averaged_skeleton_collision_info`: each body region's contact force and slide speed, the groin and face
    /// contacts.
    pub body_contact_force: [f32; 8],
    pub body_slide_speed: [f32; 8],
    pub is_groin_contact: bool,
    pub is_face_contact: bool,
}

/// Everything one host tick publishes.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Frame {
    pub tick: u32,
    /// The real-time period of this tick that the port's sim-rate controller asks for (slow motion stretches it).
    pub sim_period_s: f32,
    /// The presentation's `local_to_world` (`root * hips`, the matrix the port packs its skin palette with): the skater
    /// root after the saved Hips transform, at the hips (host/src/frame.rs). It is not an animation-to-world matrix: the
    /// engine takes its skater's `animation_to_world` from `bones[0]`, the skater root (Task 16).
    pub root: [f32; 16],
    /// World matrices in `PORT_BONES` order.
    pub bones: [[f32; 16]; 32],
    pub camera: Camera,
    pub tricks: Tricks,
    pub hom: Hom,
    pub audio: SkaterAudio,
    /// The port's `diagnostic_state_ids`: the Action state, the Motion state and the PhysOut physics state.
    pub state: (u16, u16, i32),
    /// The port's filtered state category.
    pub category: u8,
    /// The board's position.
    pub deck: [f32; 3],
    pub speed: f32,
    /// The last placement's generation and whether it has settled.
    pub placed: Option<(u32, bool)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HostMessage {
    Ready { boot_ms: u32 },
    Frame(Box<Frame>),
    /// A disc file the host needed and did not find.
    Missing(String),
    Log(String),
    Failed(String),
}

// ---- encoding -------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    BadMagic,
    /// The input ended before the message did.
    Truncated,
    BadTag(u8),
    /// A payload length above [`MAX_MESSAGE`].
    TooLarge(u32),
    /// A pipe error, or a payload that is not well formed (bad UTF-8, bytes left over).
    Io(String),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadMagic => f.write_str("not a skate2-physics message (bad magic)"),
            Self::Truncated => f.write_str("message ended early"),
            Self::BadTag(tag) => write!(f, "unknown tag {tag:#04x}"),
            Self::TooLarge(len) => write!(f, "message of {len} bytes is over the {MAX_MESSAGE} byte limit"),
            Self::Io(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for ProtocolError {}

impl From<io::Error> for ProtocolError {
    fn from(e: io::Error) -> Self {
        match e.kind() {
            io::ErrorKind::UnexpectedEof => Self::Truncated,
            _ => Self::Io(e.to_string()),
        }
    }
}

/// A type with a byte layout. Every item encodes to at least one byte, which `Vec::take` relies on.
pub trait Wire: Sized {
    fn put(&self, out: &mut Vec<u8>);
    fn take(input: &mut &[u8]) -> Result<Self, ProtocolError>;
}

/// The magic, a u32 payload length, then the payload.
pub fn encode<M: Wire>(m: &M) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(HEADER + 64);
    bytes.extend_from_slice(&MAGIC);
    bytes.extend_from_slice(&[0; 4]);
    m.put(&mut bytes);
    let len = bytes.len() - HEADER;
    debug_assert!(len <= MAX_MESSAGE as usize, "a {len} byte message is over MAX_MESSAGE");
    bytes[MAGIC.len()..HEADER].copy_from_slice(&(len as u32).to_le_bytes());
    bytes
}

/// The first message in `bytes`; anything after it is ignored.
pub fn decode<M: Wire>(bytes: &[u8]) -> Result<M, ProtocolError> {
    let mut input = bytes;
    let header = take_bytes(&mut input, HEADER)?;
    let len = payload_len(header)?;
    decode_payload(take_bytes(&mut input, len)?)
}

pub fn write_message<W: Write, M: Wire>(w: &mut W, m: &M) -> io::Result<()> {
    w.write_all(&encode(m))?;
    w.flush()
}

/// Reads exactly one message. A pipe that closes before or inside a message is `Truncated`.
pub fn read_message<R: Read, M: Wire>(r: &mut R) -> Result<M, ProtocolError> {
    let mut header = [0; HEADER];
    r.read_exact(&mut header)?;
    let mut payload = vec![0; payload_len(&header)?];
    r.read_exact(&mut payload)?;
    decode_payload(&payload)
}

const HEADER: usize = MAGIC.len() + 4;

/// Checks the magic and the length of a message header, and returns the payload length.
fn payload_len(header: &[u8]) -> Result<usize, ProtocolError> {
    if header[..MAGIC.len()] != MAGIC {
        return Err(ProtocolError::BadMagic);
    }
    let mut len_bytes = [0; 4];
    len_bytes.copy_from_slice(&header[MAGIC.len()..HEADER]);
    let len = u32::from_le_bytes(len_bytes);
    if len > MAX_MESSAGE {
        return Err(ProtocolError::TooLarge(len));
    }
    Ok(len as usize)
}

fn decode_payload<M: Wire>(mut payload: &[u8]) -> Result<M, ProtocolError> {
    let m = M::take(&mut payload)?;
    if !payload.is_empty() {
        return Err(ProtocolError::Io(format!("{} bytes left after the message", payload.len())));
    }
    Ok(m)
}

fn take_bytes<'a>(input: &mut &'a [u8], n: usize) -> Result<&'a [u8], ProtocolError> {
    if input.len() < n {
        return Err(ProtocolError::Truncated);
    }
    let (head, tail) = input.split_at(n);
    *input = tail;
    Ok(head)
}

macro_rules! wire_number {
    ($($t:ty),*) => {$(
        impl Wire for $t {
            fn put(&self, out: &mut Vec<u8>) {
                out.extend_from_slice(&self.to_le_bytes());
            }
            fn take(input: &mut &[u8]) -> Result<Self, ProtocolError> {
                const SIZE: usize = std::mem::size_of::<$t>();
                let mut raw = [0; SIZE];
                raw.copy_from_slice(take_bytes(input, SIZE)?);
                Ok(<$t>::from_le_bytes(raw))
            }
        }
    )*};
}
wire_number!(u8, u16, u32, u64, i16, i32, f32);

/// A struct is its fields in the order listed, which must be all of them (the destructure and the literal both
/// fail to compile otherwise).
macro_rules! wire_struct {
    ($name:ident { $($field:ident),* $(,)? }) => {
        impl Wire for $name {
            fn put(&self, out: &mut Vec<u8>) {
                let Self { $($field),* } = self;
                $($field.put(out);)*
            }
            fn take(input: &mut &[u8]) -> Result<Self, ProtocolError> {
                Ok(Self { $($field: Wire::take(input)?),* })
            }
        }
    };
}
wire_struct!(Pad { buttons, left_trigger, right_trigger, left, right });
wire_struct!(Profile { camera_angle, invert_x, invert_y, grind_transparency });
wire_struct!(Boot { seed, exact_delivery, mode, gestures, ambient, profile });
wire_struct!(Place { position, yaw, velocity, end_line });
wire_struct!(Camera { position, forward, right, up, clip_from_world });
wire_struct!(Tricks {
    begun, converted, ended, active, grinding, sequence_begun, sequence_completed, completed_successfully, interrupted,
    line_score, sequence_score, line_points, line_max_points, multiplier, spins, flips, just_landed, sketchy
});
wire_struct!(Hom { just_wiped_out, effect_enabled, bail_in_progress, bail_completed, just_reactivated, hud });
wire_struct!(HomHud {
    dirty, score, multiplier, rotation, rotation_score, wipeout_speed_score, max_air_drop_score, max_air_time_score,
    wipeout_duration_score, wipeout_speed, max_air_drop, max_air_time, wipeout_duration, new_car_collisions,
    new_ped_collisions, new_dmo_collisions, new_skater_collisions, new_body_tweak, car_scores, ped_scores, dmo_scores,
    skater_scores, tweak
});
wire_struct!(SkaterAudio {
    centre_of_mass_velocity, deck_velocity, wheel_pos, skateboard_surface_speed, time_in_known_air, jump_height,
    deck_slide_speed, wheel_slide, turn, num_wheels_in_contact, physics_state, is_skateboard_airborne, is_wiping_out,
    is_wipeout_over, is_manual, is_foot_braking, is_physically_pushing, is_left_foot_planting, is_right_foot_planting,
    deck_up, deck_nose, deck_local_angular_speed, deck_tilt, is_manual_braking, is_generally_pushing,
    is_generally_pushing_right_foot, is_reverting, is_left_foot_on_deck, is_right_foot_on_deck, is_front_truck_in_contact,
    is_back_truck_in_contact, is_deck_in_contact, centre_of_mass, foot_left_velocity, foot_right_velocity,
    foot_left_velocity_deck, foot_right_velocity_deck, torso_angular_velocity, right_foot_angular_velocity,
    left_foot_angular_velocity, average_limb_speed, left_foot_down, right_foot_down, is_holding_skateboard,
    is_foot_planting, body_contact_force, body_slide_speed, is_groin_contact, is_face_contact
});
wire_struct!(Frame {
    tick, sim_period_s, root, bones, camera, tricks, hom, audio, state, category, deck, speed, placed
});

/// A bool is a 0 or 1 byte.
impl Wire for bool {
    fn put(&self, out: &mut Vec<u8>) {
        out.push(*self as u8);
    }
    fn take(input: &mut &[u8]) -> Result<Self, ProtocolError> {
        match u8::take(input)? {
            0 => Ok(false),
            1 => Ok(true),
            tag => Err(ProtocolError::BadTag(tag)),
        }
    }
}

impl<T: Wire> Wire for Option<T> {
    fn put(&self, out: &mut Vec<u8>) {
        match self {
            None => out.push(0),
            Some(value) => {
                out.push(1);
                value.put(out);
            }
        }
    }
    fn take(input: &mut &[u8]) -> Result<Self, ProtocolError> {
        match u8::take(input)? {
            0 => Ok(None),
            1 => Ok(Some(T::take(input)?)),
            tag => Err(ProtocolError::BadTag(tag)),
        }
    }
}

impl<T: Wire> Wire for Vec<T> {
    fn put(&self, out: &mut Vec<u8>) {
        (self.len() as u32).put(out);
        for item in self {
            item.put(out);
        }
    }
    fn take(input: &mut &[u8]) -> Result<Self, ProtocolError> {
        let count = u32::take(input)? as usize;
        // Each item is at least a byte, so a count above the bytes left is a lie, and this keeps it from reserving
        // gigabytes before the first item fails.
        if count > input.len() {
            return Err(ProtocolError::Truncated);
        }
        let mut items = Vec::with_capacity(count);
        for _ in 0..count {
            items.push(T::take(input)?);
        }
        Ok(items)
    }
}

/// A fixed array is its items with no count.
impl<T: Wire, const N: usize> Wire for [T; N] {
    fn put(&self, out: &mut Vec<u8>) {
        for item in self {
            item.put(out);
        }
    }
    fn take(input: &mut &[u8]) -> Result<Self, ProtocolError> {
        let mut items = Vec::with_capacity(N);
        for _ in 0..N {
            items.push(T::take(input)?);
        }
        // N items were pushed, so the conversion cannot fail.
        items.try_into().map_err(|_| ProtocolError::Truncated)
    }
}

impl<A: Wire, B: Wire> Wire for (A, B) {
    fn put(&self, out: &mut Vec<u8>) {
        self.0.put(out);
        self.1.put(out);
    }
    fn take(input: &mut &[u8]) -> Result<Self, ProtocolError> {
        Ok((A::take(input)?, B::take(input)?))
    }
}

impl<A: Wire, B: Wire, C: Wire> Wire for (A, B, C) {
    fn put(&self, out: &mut Vec<u8>) {
        self.0.put(out);
        self.1.put(out);
        self.2.put(out);
    }
    fn take(input: &mut &[u8]) -> Result<Self, ProtocolError> {
        Ok((A::take(input)?, B::take(input)?, C::take(input)?))
    }
}

impl<T: Wire> Wire for Box<T> {
    fn put(&self, out: &mut Vec<u8>) {
        (**self).put(out);
    }
    fn take(input: &mut &[u8]) -> Result<Self, ProtocolError> {
        T::take(input).map(Box::new)
    }
}

/// A u32 byte count and the UTF-8 bytes.
impl Wire for String {
    fn put(&self, out: &mut Vec<u8>) {
        (self.len() as u32).put(out);
        out.extend_from_slice(self.as_bytes());
    }
    fn take(input: &mut &[u8]) -> Result<Self, ProtocolError> {
        let len = u32::take(input)? as usize;
        let bytes = take_bytes(input, len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| ProtocolError::Io("text is not valid UTF-8".to_string()))
    }
}

impl Wire for EngineMessage {
    fn put(&self, out: &mut Vec<u8>) {
        match self {
            Self::Hello { version } => {
                out.push(0);
                version.put(out);
            }
            Self::Boot(boot) => {
                out.push(1);
                boot.put(out);
            }
            Self::Tick(pad) => {
                out.push(2);
                pad.put(out);
            }
            Self::Place(place) => {
                out.push(3);
                place.put(out);
            }
            Self::SetGestures(gestures) => {
                out.push(4);
                gestures.put(out);
            }
            Self::SetMode(mode) => {
                out.push(5);
                mode.put(out);
            }
            Self::SetAmbient(ambient) => {
                out.push(6);
                ambient.put(out);
            }
            Self::Shutdown => out.push(7),
            Self::SetProfile(profile) => {
                out.push(8);
                profile.put(out);
            }
        }
    }
    fn take(input: &mut &[u8]) -> Result<Self, ProtocolError> {
        Ok(match u8::take(input)? {
            0 => Self::Hello { version: u32::take(input)? },
            1 => Self::Boot(Boot::take(input)?),
            2 => Self::Tick(Pad::take(input)?),
            3 => Self::Place(Place::take(input)?),
            4 => Self::SetGestures(<[u32; 4]>::take(input)?),
            5 => Self::SetMode(u8::take(input)?),
            6 => Self::SetAmbient(bool::take(input)?),
            7 => Self::Shutdown,
            8 => Self::SetProfile(Profile::take(input)?),
            tag => return Err(ProtocolError::BadTag(tag)),
        })
    }
}

impl Wire for HostMessage {
    fn put(&self, out: &mut Vec<u8>) {
        match self {
            Self::Ready { boot_ms } => {
                out.push(0);
                boot_ms.put(out);
            }
            Self::Frame(frame) => {
                out.push(1);
                frame.put(out);
            }
            Self::Missing(entry) => {
                out.push(2);
                entry.put(out);
            }
            Self::Log(line) => {
                out.push(3);
                line.put(out);
            }
            Self::Failed(why) => {
                out.push(4);
                why.put(out);
            }
        }
    }
    fn take(input: &mut &[u8]) -> Result<Self, ProtocolError> {
        Ok(match u8::take(input)? {
            0 => Self::Ready { boot_ms: u32::take(input)? },
            1 => Self::Frame(Wire::take(input)?),
            2 => Self::Missing(String::take(input)?),
            3 => Self::Log(String::take(input)?),
            4 => Self::Failed(String::take(input)?),
            tag => return Err(ProtocolError::BadTag(tag)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Debug;

    /// `start, start + 1, ...`: every slot differs, so a swapped or dropped field cannot round-trip by luck.
    fn ramp<const N: usize>(start: f32) -> [f32; N] {
        std::array::from_fn(|i| start + i as f32)
    }

    fn frame() -> Frame {
        let mut bones = [[0.0; 16]; 32];
        for (i, bone) in bones.iter_mut().enumerate() {
            *bone = ramp(i as f32 * 100.0);
        }
        bones[31][14] = -61.34;
        Frame {
            tick: 4242,
            sim_period_s: 1.0 / 120.0,
            root: ramp(0.5),
            bones,
            camera: Camera {
                position: [1.0, 2.0, 3.0],
                forward: [0.0, 0.0, 1.0],
                right: [1.0, 0.0, 0.0],
                up: [0.0, 1.0, 0.0],
                clip_from_world: ramp(-8.0),
            },
            tricks: Tricks {
                begun: vec![1, 2, 3],
                converted: vec![4],
                ended: vec![(268, 1250)],
                active: vec![5, 6],
                grinding: true,
                sequence_begun: true,
                sequence_completed: true,
                completed_successfully: true,
                interrupted: true,
                line_score: 90_000,
                sequence_score: 36_000,
                line_points: -12,
                line_max_points: 3400,
                multiplier: 2.5,
                spins: -2,
                flips: 3,
                just_landed: true,
                sketchy: true,
            },
            hom: Hom {
                just_wiped_out: true,
                effect_enabled: true,
                bail_in_progress: true,
                bail_completed: true,
                just_reactivated: true,
                hud: {
                    let mut hud = HomHud {
                        dirty: true,
                        score: 12345,
                        multiplier: 3,
                        rotation: 10.0,
                        rotation_score: 5.0,
                        wipeout_speed_score: 1.0,
                        max_air_drop_score: 2.0,
                        max_air_time_score: 3.0,
                        wipeout_duration_score: 4.0,
                        wipeout_speed: 6.0,
                        max_air_drop: 7.0,
                        max_air_time: 8.0,
                        wipeout_duration: 9.0,
                        new_car_collisions: 2,
                        new_ped_collisions: 3,
                        new_dmo_collisions: 4,
                        new_skater_collisions: 5,
                        new_body_tweak: true,
                        car_scores: ramp(100.0),
                        ped_scores: ramp(200.0),
                        dmo_scores: ramp(300.0),
                        skater_scores: ramp(400.0),
                        tweak: 11.0,
                    };
                    hud.car_scores[9] = 500.0;
                    hud
                },
            },
            audio: SkaterAudio {
                centre_of_mass_velocity: ramp(1.0),
                deck_velocity: ramp(5.0),
                wheel_pos: [ramp(10.0), ramp(20.0), ramp(30.0), ramp(40.0)],
                skateboard_surface_speed: 5.5,
                time_in_known_air: 0.75,
                jump_height: 0.4,
                deck_slide_speed: 1.5,
                wheel_slide: 0.25,
                turn: -0.5,
                num_wheels_in_contact: 4,
                physics_state: -3,
                is_skateboard_airborne: true,
                is_wiping_out: true,
                is_wipeout_over: true,
                is_manual: true,
                is_foot_braking: true,
                is_physically_pushing: true,
                is_left_foot_planting: true,
                is_right_foot_planting: true,
                deck_up: ramp(50.0),
                deck_nose: ramp(60.0),
                deck_local_angular_speed: ramp(70.0),
                deck_tilt: 0.125,
                is_manual_braking: true,
                is_generally_pushing: true,
                is_generally_pushing_right_foot: true,
                is_reverting: true,
                is_left_foot_on_deck: true,
                is_right_foot_on_deck: true,
                is_front_truck_in_contact: true,
                is_back_truck_in_contact: true,
                is_deck_in_contact: true,
                centre_of_mass: ramp(80.0),
                foot_left_velocity: ramp(90.0),
                foot_right_velocity: ramp(100.0),
                foot_left_velocity_deck: ramp(110.0),
                foot_right_velocity_deck: ramp(120.0),
                torso_angular_velocity: ramp(130.0),
                right_foot_angular_velocity: ramp(140.0),
                left_foot_angular_velocity: ramp(150.0),
                average_limb_speed: ramp(160.0),
                left_foot_down: true,
                right_foot_down: true,
                is_holding_skateboard: true,
                is_foot_planting: true,
                body_contact_force: [0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875, 1.0],
                body_slide_speed: [8.0, 7.0, 6.0, 5.0, 4.0, 3.0, 2.0, 1.0],
                is_groin_contact: true,
                is_face_contact: true,
            },
            state: (7, 9, -11),
            category: 2,
            deck: [0.1, 0.2, 0.3],
            speed: 6.5,
            placed: Some((7, true)),
        }
    }

    fn round_trips<M: Wire + Clone + PartialEq + Debug>(m: M) {
        let bytes = encode(&m);
        assert_eq!(decode::<M>(&bytes), Ok(m.clone()));
        assert_eq!(read_message::<_, M>(&mut &bytes[..]), Ok(m));
    }

    #[test]
    fn every_message_round_trips() {
        let pad = Pad { buttons: 0x1234, left_trigger: 200, right_trigger: 17, left: [-32768, 32767], right: [-1, 1] };
        let engine = [
            EngineMessage::Hello { version: PROTOCOL_VERSION },
            EngineMessage::Boot(Boot {
                seed: Some(0xDEAD_BEEF_0123_4567),
                exact_delivery: Some(false),
                mode: 2,
                gestures: [3, 1, 4, 1],
                ambient: true,
                profile: Profile { camera_angle: 1, invert_x: true, invert_y: false, grind_transparency: true },
            }),
            EngineMessage::Tick(pad),
            EngineMessage::Place(Place {
                position: [10.0, -2.5, 300.25],
                yaw: 1.5,
                velocity: [0.0, 0.0, 4.0],
                end_line: true,
            }),
            EngineMessage::SetGestures([9, 8, 7, 6]),
            EngineMessage::SetMode(1),
            EngineMessage::SetAmbient(true),
            EngineMessage::Shutdown,
            EngineMessage::SetProfile(Profile {
                camera_angle: -1,
                invert_x: false,
                invert_y: true,
                grind_transparency: false,
            }),
        ];
        for m in engine {
            round_trips(m);
        }
        // The optionals' other arm.
        round_trips(EngineMessage::Boot(Boot { seed: None, exact_delivery: None, ..Boot::default() }));

        let host = [
            HostMessage::Ready { boot_ms: 1234 },
            HostMessage::Frame(Box::new(frame())),
            HostMessage::Missing("skate2/missing.rx3".to_string()),
            HostMessage::Log("a line with unicode: \u{e9}\u{4e16}".to_string()),
            HostMessage::Failed("the host stopped".to_string()),
        ];
        for m in host {
            round_trips(m);
        }
        round_trips(HostMessage::Frame(Box::new(Frame { placed: None, ..frame() })));
    }

    #[test]
    fn the_default_profile_is_the_ports() {
        // skate2-formats/src/skater_profile.rs 306-329 (`ProfileSettings::default`).
        let profile = Profile::default();
        assert_eq!(
            (profile.camera_angle, profile.invert_x, profile.invert_y, profile.grind_transparency),
            (0, false, false, false)
        );
        assert_eq!(Boot::default().profile, profile);
        assert_eq!(Boot::default().gestures, [0, 1, 2, 3]);
    }

    #[test]
    fn a_tick_is_21_bytes() {
        // 4 bytes of magic, 4 of length, 1 of tag and 12 of pad.
        assert_eq!(encode(&EngineMessage::Tick(Pad::default())).len(), 21);
    }

    #[test]
    fn bad_input_is_an_error() {
        fn with_header(magic: &[u8], len: u32, payload: &[u8]) -> Vec<u8> {
            let mut bytes = magic.to_vec();
            bytes.extend_from_slice(&len.to_le_bytes());
            bytes.extend_from_slice(payload);
            bytes
        }
        fn check(bytes: &[u8], expected: ProtocolError) {
            assert_eq!(decode::<EngineMessage>(bytes), Err(expected.clone()));
            assert_eq!(read_message::<_, EngineMessage>(&mut &bytes[..]), Err(expected));
        }

        check(&with_header(b"XXXX", 1, &[7]), ProtocolError::BadMagic);

        let whole = encode(&EngineMessage::Hello { version: 1 });
        check(&whole[..whole.len() - 1], ProtocolError::Truncated);
        // The header is complete but the payload ends inside the version.
        check(&with_header(&MAGIC, 3, &[0, 1, 0]), ProtocolError::Truncated);

        check(&with_header(&MAGIC, 1, &[0xEE]), ProtocolError::BadTag(0xEE));
        assert_eq!(decode::<HostMessage>(&with_header(&MAGIC, 1, &[0xEE])), Err(ProtocolError::BadTag(0xEE)));

        check(&with_header(&MAGIC, MAX_MESSAGE + 1, &[]), ProtocolError::TooLarge(1_048_577));
    }

    #[test]
    fn the_port_bones_are_our_first_32() {
        assert_eq!(PORT_BONES[0], "trajectory");
        assert_eq!(PORT_BONES[1], "hips");
        assert_eq!(PORT_BONES[25], "skateboard_root");
        assert_eq!(PORT_BONES[31], "right_wheelback");
    }
}
