//! ScoreModule integration. The simulation owns recognition and accounting;
//! the APT movie only consumes the resulting publication.
use crate::{apt_vm::Value, hud_runtime};
use skate_core::{
    animation::output::attributes::AttributeName,
    physics::{filtered_state::FilteredCategory, skate2::float as ppc},
    scoring::{
        air::{self, ContextBlock, ContextRun, GroundQuery, SpinTracker},
        carrier::{Carrier, delay_ticks},
        catalog,
        conversions::LINKS,
        session::Session,
    },
};
use skate_data::{collections::Collections, scoring::ScoringData};

/// The Skate 2 port's tricks in this scorer (Task 17).
#[path = "scoring_runtime_port.rs"]
mod port;
/// TU3's own ScoreModule beside this one, frame by frame (test-only).
#[cfg(test)]
#[path = "scoring_oracle.rs"]
mod oracle;

/// `air_horizontal_distance`. The five air metrics are consecutive from here.
const AIR_METRIC_BASE: usize = 129;

/// The localisation ids `sub_825E51A0` carries as string literals at 8220E238..8220E494.
///
/// They live in the executable, not in the vault -- searching the owner's collections for
/// `ID_TRICK_AIR*` finds nothing -- so a port has to hold them as literals too. The HUD's
/// own language table does resolve them: `Air`, `FS`, `BS`, `Frontflip`, `Backflip`.
///
/// `r27` = 8220E474 in 825E51A0; every literal below is an `addi rX,r27,-n` from it.
mod trick_tokens {
    /// 8220E3AC. The bare fallback when no record has been announced.
    pub const AIR: &str = "ID_TRICK_AIR";
    /// 8220E254 / its BS sibling.
    pub const FS_SPIN: &str = "ID_TRICK_AIR_FS_SPIN";
    pub const BS_SPIN: &str = "ID_TRICK_AIR_BS_SPIN";
    /// 8220E474 and 8220E454, appended by the sign of the flip direction.
    pub const FRONTFLIP: &str = "ID_TRICK_AIR_METRICS_FRONTFLIP";
    pub const BACKFLIP: &str = "ID_TRICK_AIR_METRICS_BACKFLIP";
    /// 8220E270 / 8220E290 (an unnamed spin) and their copies at 8220E3C0 / 8220E3E0 (a
    /// named one): one half-turn out of fakie.
    pub const FS_HALFCAB: &str = "ID_TRICK_AUTHENTIC_FS_HALFCAB";
    pub const BS_HALFCAB: &str = "ID_TRICK_AUTHENTIC_BS_HALFCAB";
    /// 8220E2B4 / 8220E2D0 and 8220E404 / 8220E420: two half-turns or more.
    pub const FS_CAB: &str = "ID_TRICK_AUTHENTIC_FS_CAB";
    pub const BS_CAB: &str = "ID_TRICK_AUTHENTIC_BS_CAB";
    /// 8220E238: scorable 252 (`superman`) named while it carries a flip.
    pub const MIRACLE_WHIP: &str = "ID_TRICK_GRAB_MIRACLE_WHIP";
}

/// `superman`, the scorable 825E51A0 renames to the Miracle Whip (`cmpwi cr6,r8,252`).
const MIRACLE_WHIP_BASE: usize = 252;

/// Whether a scorable's name may carry a spin, `r23` in `825E51A0`.
///
/// ```text
/// bl    0x82da22a8   ; the authored record of the named scorable
/// lwz   r10,392(r1)  ; desc+120 = its TrickType
/// lbz   r9,396(r1)   ; desc+124 = valid
/// lwz   r8,284(r1)   ; desc+12  = its scorable id
/// r23 = (TrickType == 4 && (!valid || id != 60)) || TrickType in {1,2,3,7,10,12,13}
/// ```
///
/// The admitted set is Air, Flip, FingerFlip, Grab, NoComply, HippyJump, Boneless and
/// Footplant. It deliberately excludes Grind(5), **Handplant(6)**, **Manual(8)**, Slide(9)
/// and Revert(11) -- which is why retail never writes "Manual 180". The one grab refused is
/// scorable 60, `coffin`. A resolved record is always valid here.
fn decorates_spin(trick_type: u32, id: usize) -> bool {
    trick_type == 4 && id != 60 || matches!(trick_type, 1 | 2 | 3 | 7 | 10 | 12 | 13)
}

/// 82DA4EE8: a nollie trick -- retail class (`+16`, the port's `score_type`) Air or Flip, and
/// an identifier (`+20`, via 82DA4E98) whose first byte is `'n'` (`cmplwi cr6,r11,110`).
fn is_nollie(id: usize) -> bool {
    catalog::IDENTIFIERS
        .get(id)
        .is_some_and(|&(identifier, _, class)| matches!(class, 1 | 2) && identifier.starts_with('n'))
}

/// The record `M+8` names, as 82DA22A8 resolves it for 825E51A0.
#[derive(Clone, Copy)]
struct Named<'a> {
    id: usize,
    /// desc+0, the record's label.
    label: &'a str,
    /// desc+120.
    trick_type: u32,
    /// desc+112 (record +152): the linked scorable and its label. In the owner's vault the only
    /// scorable-id field that links each `n_*` flip to its regular flip (and `nollie` to
    /// `ollie`) is `Hash_2E90BC04042A0B5A`, which skate-data loads as `variant`; that this is
    /// record +152 is inferred from the data, not traced.
    link: Option<(usize, &'a str)>,
}

/// What 825E51A0 leaves behind: the name it formats into `M+28` and the stance bytes it
/// rewrites (`M+20` switch, `M+158` fakie), plus `M+169`, set when the nollie swap fired.
#[derive(Debug, PartialEq)]
struct Composed {
    name: String,
    switch: bool,
    fakie: bool,
    swapped: bool,
}

/// Compose the displayed trick name the way `sub_825E51A0` does.
///
/// The result is one `#`, then space-separated localisation ids and a bare degree count,
/// which [`crate::apt_text`] resolves token by token. Retail localises each piece first and
/// formats with `"#%s %d "` (8220E43C) or `"#%s "` (8220E444), then `strcat`s the flip
/// suffix on; the port keeps the ids and drops the trailing space.
///
/// ```text
/// M+8 == -1:  M+12 == 0 ? "ID_TRICK_AIR"
///             : M+186 ? (|M+12| == 1 ? HALFCAB : CAB), FS when M+12 > 0     ; r22 = 1
///             : FS/BS spin by switch ^ (M+12 < 0) ^ M+187
/// M+8 named:  id == 252 && M+16 != 0 ? MIRACLE_WHIP, no flip suffix (r21 = 0) : label
///             0x825E54EC: nollie(id) && link != -1:
///                 switch && !fakie -> name = link's label, switch = 0, fakie = 1, M+169 = 1
///                 fakie && !switch -> name = link's label, switch = 1, fakie = 0, M+169 = 1
///             0x825E5644: r23 && M+186 && !nollie(M+169 ? link : id) && M+12 != 0:
///                 name += " " + (|M+12| == 1 ? HALFCAB : CAB)                ; r22 = 1
/// 0x825E5700: M+12 != 0 && r23: r22 ? (fakie = 0, degrees only when |M+12| >= 2) : degrees
/// 0x825E57A8: r21 && M+16 < 0 ? BACKFLIP : r21 && M+16 > 0 ? FRONTFLIP
/// ```
fn compose_trick_name(
    named: Option<Named>,
    spin_turns: i32,
    flip_direction: i32,
    mut switch: bool,
    mut fakie: bool,
    regular: bool,
    cab: bool,
) -> Composed {
    let cab_token = || match (spin_turns.unsigned_abs() == 1, spin_turns > 0) {
        (true, true) => trick_tokens::FS_HALFCAB,
        (true, false) => trick_tokens::BS_HALFCAB,
        (false, true) => trick_tokens::FS_CAB,
        (false, false) => trick_tokens::BS_CAB,
    };
    let (mut flip_suffix, mut cab_used, mut swapped) = (true, false, false);
    let (mut name, decorate) = match named {
        None if spin_turns == 0 => (trick_tokens::AIR.to_owned(), false),
        None if cab => {
            cab_used = true;
            (cab_token().to_owned(), true)
        }
        // 0x825E53C4: a spin's side is the parity of switch, the spin's own sign and M+187.
        None if switch ^ (spin_turns < 0) ^ regular => (trick_tokens::BS_SPIN.to_owned(), true),
        None => (trick_tokens::FS_SPIN.to_owned(), true),
        Some(n) => {
            let decorate = decorates_spin(n.trick_type, n.id);
            let mut name = if n.id == MIRACLE_WHIP_BASE && flip_direction != 0 {
                flip_suffix = false;
                trick_tokens::MIRACLE_WHIP
            } else {
                n.label
            }
            .to_owned();
            let mut id = n.id;
            if let (true, Some((link, label))) = (is_nollie(n.id), n.link) {
                if switch != fakie {
                    (switch, fakie, swapped) = (fakie, switch, true);
                    name = label.to_owned();
                    id = link;
                }
            }
            if decorate && cab && spin_turns != 0 && !is_nollie(id) {
                name.push(' ');
                name.push_str(cab_token());
                cab_used = true;
            }
            (name, decorate)
        }
    };
    name.insert(0, '#');
    if spin_turns != 0 && decorate {
        if cab_used {
            fakie = false;
        }
        if !cab_used || spin_turns.unsigned_abs() >= 2 {
            name.push(' ');
            name.push_str(&(spin_turns.unsigned_abs() * 180).to_string());
        }
    }
    if flip_suffix && flip_direction != 0 {
        name.push(' ');
        name.push_str(if flip_direction < 0 { trick_tokens::BACKFLIP } else { trick_tokens::FRONTFLIP });
    }
    Composed { name, switch, fakie, swapped }
}

/// The trick display module's stance bytes, which `Tricks.GetCurrentTrickStance` (825C2700)
/// hands the HUD as `[M+20, M+158, M+168, 825E4D00]`.
///
/// 825E51A0 rewrites `M+20`/`M+158` when it composes; 825E4D68 puts the trick module's own
/// values back the next frame and marks the stance dirty (`M+160`); 825E4F40 then fires the
/// stance event (825D3B38(5)) three frames later -- or at once after a nollie swap -- but only
/// while `M+168` is set, and a swap clears `M+168` for 140 frames. The HUD only reads these on
/// an event, so `hud` is what it last sampled.
#[derive(Default)]
struct StanceBytes {
    switch: bool,  // M+20
    fakie: bool,   // M+158
    dirty: bool,   // M+160
    shown: bool,   // M+168
    swapped: bool, // M+169
    wait: u32,     // M+172
    frames: u32,   // M+180
    /// M+186: the trick started out of fakie (0x825E5084..50A4), latched per named trick and
    /// kept through unnamed spins.
    cab: bool,
    hud: [bool; 4],
}

impl StanceBytes {
    /// State 1 of 825E4F40, a new trick: `M+168` armed (dirtying the stance if it was not),
    /// and `M+186` = fakie && !`M+20` -- the switch byte *before* this frame's 825E4D68.
    fn start(&mut self, fakie: bool) {
        self.dirty |= !self.shown;
        (self.shown, self.wait, self.swapped) = (true, 0, false);
        self.cab = fakie && !self.switch;
    }
    /// 825E4D68, then 0x825E50F0: `M+168` re-arms after 140 frames clear.
    fn sync(&mut self, switch: bool, fakie: bool) {
        if (self.switch, self.fakie) != (switch, fakie) {
            (self.switch, self.fakie, self.dirty) = (switch, fakie, true);
        }
        if !self.shown {
            self.wait += 1;
            if self.wait >= 140 {
                (self.wait, self.shown, self.dirty) = (0, true, true);
            }
        }
    }
    /// 825E51A0 ran: its stance bytes, and what the name event (825D3B38(1/2)) lets the HUD read.
    fn composed(&mut self, c: &Composed, nollie: bool) {
        (self.switch, self.fakie) = (c.switch, c.fakie);
        if c.swapped {
            (self.swapped, self.dirty) = (true, true);
        }
        self.hud = [self.switch, self.fakie, self.shown, nollie];
    }
    /// 0x825E5130..5194.
    fn settle(&mut self, nollie: bool) {
        if !self.dirty {
            return;
        }
        self.frames += 1;
        if self.frames > 2 || self.swapped {
            (self.dirty, self.frames) = (false, 0);
            if self.shown {
                self.hud = [self.switch, self.fakie, self.shown, nollie];
            }
            if self.swapped {
                (self.swapped, self.shown) = (false, false);
            }
        }
    }
}

/// The curves `82DA88E8` evaluates, in the order `82DA89A0` drives the runs. The runs
/// themselves (`82DA7B50`) and the ground query feeding them (`82DA7A38`) are
/// [`skate_core::scoring::air`], checked bit for bit against TU3 by its `air_oracle` tests.
///
/// `addi r10,r11,1440` / `1280` / `1360` / `1200` against the collector tuning block.
const CONTEXT_CURVES: [u16; 4] = [0x5a0, 0x500, 0x550, 0x4b0];

/// `air_metric`, the scorable `82DA8550` banks the gap/context total under.
const CONTEXT_METRIC: usize = 237;

fn v4(v: [f32; 3]) -> [f32; 4] {
    [v[0], v[1], v[2], 0.0]
}

/// A transform with only the rows 82DAC9D0 reads: up (row 1) and forward (row 2).
fn frame_rows(up: [f32; 3], forward: [f32; 3]) -> air::Matrix {
    [[0.0; 4], v4(up), v4(forward), [0.0; 4]]
}

/// `Handplant_CLASS`, index 6 of the `*_CLASS` list at 82084A40.
///
/// This is the metadata table's **+16** field, which [`catalog::IDENTIFIERS`] carries as its
/// third tuple element -- the one the port calls `score_type`. The +12 field it calls `class`
/// is retail's score/display type. The two names are the wrong way round, which is how the
/// Handplant and Offboard collectors came to be wired to each other's filters.
const HANDPLANT_CLASS: usize = 6;

/// `SKATE_SCORING_TRACE=1` prints what every air and every publication actually scored.
///
/// A HUD total cannot say *which* contributor is missing, so this exists to make a
/// playtest answer that instead of a guess doing it.
fn trace_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("SKATE_SCORING_TRACE").is_some())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Collector {
    None,
    Ground,
    Air,
    Grind,
    Offboard,
    Handplant,
}
pub(crate) struct Frame {
    pub tick: u32,
    pub dt: f32,
    pub category: FilteredCategory,
    pub state: u32,
    pub descriptor: Option<AttributeName>,
    pub grind_id: i32,
    /// Collector `+104`: the MotionGraph score packet's flags, which 82DAC498 copies with the packet each frame
    /// (82596008). Not the ScoreModule input's +2788, whose bits arrive as `multiplier_enabled`, `offline` and
    /// `manuals_connect`.
    pub flags: u32,
    pub position: [f32; 3],
    /// `[PhysOut+36]+0`, the filtered centre-of-mass velocity: what the air's Enter (82DA8078) and the
    /// offboard start (82DABE98) test the take-off speed on, against [`Frame::reckoning_up`].
    pub com_velocity: [f32; 3],
    /// Motion+0xA0: the effective forward turned to face the deck's travel, the manual metric's axis (82DB0588).
    pub forward: [f32; 3],
    /// PhysOut_Skeleton +368 rows 1 and 2 -- the rider skeleton root's own up and forward.
    ///
    /// This, and not the deck, is what retail's spin accumulator measures. 82DA8BE0 feeds
    /// accumulator A `[[owner+20]+368]` on every one of its four branches, and A's angle is
    /// the only one that ever reaches the banked total; the board-derived accumulator B is
    /// held at zero for as long as a named trick is carried.
    pub rider_up: [f32; 3],
    pub rider_forward: [f32; 3],
    /// `[PhysOut+0]` rows 1 and 2, the Motion block's effective deck up and forward: 82DA8BE0's
    /// accumulator B (`+896`) takes its reference from these. Row 1 is `[PhysOut+0]+80`, whose y
    /// 82DAB2A8 tests `0.65 >`: the deck tilted past ~49 degrees (82C02A80 copies 82C01BF8's
    /// effective transform to +64..+127).
    pub board_up: [f32; 3],
    pub board_forward: [f32; 3],
    /// `[PhysOut+36]+96`, the reckoning up 82DA8BE0 builds B's frame on (82D2CDC0).
    pub reckoning_up: [f32; 3],
    /// 82DA33E0's `X+0` (0x82DA349C..34E8): Ground+80 in physical category 100, Grinds+32 in category 400, else
    /// the constant up at 0x82139A20. The air's Reset copies it to `+144`, the height-gain axis.
    pub takeoff_normal: [f32; 3],
    /// `[[PhysOut+24]+0]`, the wheel contact count 82DAB2A8 tests `< 3`.
    pub wheel_count: u32,
    /// `[[PhysOut+32]+268]`, the pump acceleration 82DAB2A8 tests `> 0.1`.
    pub pump_acceleration: f32,
    pub switch: bool,
    /// The profile's natural stance is regular: M+187 = IsRegular (natural_stance == 0). 825E4DC8 stores
    /// `vfunc[+128]() == 0`, and slot +128 (82B97168) reads natural_stance.
    pub regular: bool,
    pub fakie: bool,
    pub nollie: bool,
    pub body_flip: bool,
    /// Air 445. Selects the flip's direction; see [`Runtime::flip_direction`].
    pub body_flip_side: bool,
    /// PhysOut.Ground +192: the hips line test's ray origin, the hips part's world
    /// position. `82DB6EC0` publishes it unconditionally.
    pub hips_position: [f32; 3],
    /// PhysOut.Ground +208, and `None` when Ground +320 says the test missed. The hips
    /// test's hit position, republished only while `player+1524` is set.
    pub hips_ground: Option<[f32; 3]>,
    /// PhysOut.Ground +260: the packed surface tag the hips test hit.
    pub hips_surface: u32,
    pub suspend_air: bool,
    pub landing: skate_core::animation::landing_quality::Output,
    pub teleported: bool,
    pub reverting: bool,
    /// PhysOut -> State byte 70: the ground collector's manual gate (82DAA8E0) and the revert
    /// slot's trigger (82DAAC10). 82DB6EC0 fills it from `[base+1888]+56`, the byte
    /// CalcSuggestedState 82D8ADE8 sets at 0x82D8B19C when RevertGround (102) falls back to
    /// ground (100) and clears on every other call -- the port's
    /// `selector.revert_exited_normally`. It is **not** State+66, the revert's lifetime.
    pub manual_block_70: bool,
    /// State +0x45, the selector's teleport request: 82DA3DD8 answers 3 (the bail index) while it is set.
    pub request_teleport: bool,
    /// Grinds +0x146: a grind with it set gets no collector (82DA3DD8 answers -1).
    pub tipslide_326: bool,
    /// Ground +0x138, the half turns the offboard collector's spin reads (82DABFB0).
    pub half_turns_312: i32,
    /// State +0x4E: the wipeout system asks for a runout (82DB9188, published by 82DB6EC0).
    pub runout_78: bool,
    /// Landing +0xA2: processed flags 2484 bit 31 (82DB7B60), the animation flags' 0x02000000 (82BDC680).
    pub flags_2484_31: bool,
    /// Off-board +0x143: the board is being retrieved (82D76D20: flags 2476 bit 0x400, or the controller's state 4).
    pub retrieving_board_323: bool,
}
pub(crate) struct Runtime {
    pub data: ScoringData,
    pub session: Session,
    collector: Collector,
    carriers: [Option<Carrier>; 4],
    held: [f32; 4],
    distance: [f32; 4],
    metric_rewards: [f32; 4],
    metric_started: [bool; 4],
    /// Air `+128`: the take-off position (Reset 82DA7DA0 copies `[PhysOut+0]+144`).
    start: [f32; 3],
    previous: [f32; 3],
    /// Air `+144`: [`Frame::takeoff_normal`] at take-off, the height-gain axis.
    takeoff_normal: [f32; 3],
    /// 82DA8BE0's two rotation accumulators, latch and banked degrees.
    spin: SpinTracker,
    /// 82DA8BE0's signed total this frame, in degrees.
    spin_total: f32,
    /// Air `+788`, the peak height (`fsel` maximum, seeded with the take-off height `+784`).
    peak: f32,
    /// Air `+808`: grab time, summed over the whole air (only Reset clears it).
    grab_time: f32,
    air_factor: f32,
    air_repetition: f32,
    air_repetition_set: bool,
    grab_chain: u32,
    air_metrics: [f32; 5],
    /// 82DA8BE0's +2344: the air's rotation quantised to whole half-turns and signed by
    /// its direction. The reward uses only its magnitude; the sign is for the trick name.
    pub spin_turns: i32,
    /// 82DA8EB8's +2348: `0` until a body flip is credited, then `+1` or `-1` for its
    /// side. Retail writes it only while an announced grab carrier is current, so an
    /// ungrabbed flip earns no flip reward -- but it is still *named* as a flip.
    pub flip_direction: i32,
    /// The gap/context collector's four run accumulators, at collector +192/+240/+288/+336.
    context_runs: [ContextRun; 4],
    /// Collector `+160`: the whole 82DA7A38 query the air's Reset (82DA7DA0) takes with no
    /// reference. Every later query measures its drop against the lower of this origin and
    /// the present one, and its `+25` (byte `+185`, the hips over a tag-10 surface at
    /// take-off) keeps run 0 shut for the whole air.
    takeoff_query: GroundQuery,
    /// Output `+13648`: the ten context slots 82DA7888 fills from the proximity lists
    /// (`+2764..+2776`, built by 82761460) and the vault entries at `+2688`. The physics frame
    /// writes slot 6 (the props under the probe) before every `advance`; the other slots have
    /// no engine source (`physics::frame::score`).
    pub(crate) context_slots: ContextBlock,
    /// Input `+2788` bit 0x00800000, which 82761460 sets only offline (82761518). 82DA8550
    /// pays slot 4 (another skater within 3 m) only while it is set.
    pub(crate) offline: bool,
    /// Ground collector `+945`, which 82DA3C68 copies every frame from input +2788 bit 0x40000000: 8275F430's copy
    /// of the player record's ManualsAreConnectors byte (`[0x83067068] + id * 0xA8 + 0x74`). 824FA9A0 sets it and a
    /// challenge's end (8256CAB0) sets it again; a challenge whose GlobalData has SetManualAndWalkAsConnector false
    /// clears it (8256C840) except inside its spot volumes (8256CB60), as does Lua's SetManualsAreConnectors(false)
    /// (82844474). Set, fewer than three wheels keep a sequence open (82DAB2A8): a manual connects. Written by
    /// `online_scoring` (the challenge's flag, the modes whose scripts clear it) and `skate1_challenge` (the missions'
    /// call). Clear, the manual and coffin slots hold only on a name post in some frames ([`Self::clear_only`]).
    /// shortcut: the spot volumes it is set inside and the leave-a-spot-in-a-manual sequence reset (8256CC60) are not
    /// ported. Upgrade with those sources.
    pub(crate) manuals_connect: bool,
    /// Input +0x74..+0x81 (8275F5F0 copies `[[TheSimulation+0xDC]+0x1A0]+0xC`): which `Trick.eTrickType` scores, by
    /// value (None 0, Air 1, Flip 2, FingerFlip 3, Grab 4, Grind 5, Handplant 6, NoComply 7, Manual 8, Slide 9,
    /// HippyJump 10, Revert 11, Boneless 12, Footplant 13; the registration at 8283C348). Lua's ChallengeManager
    /// DisableAllTrickTypes / EnableTrickType / ResetTrickTypeScoring (82844490 / 828444E8 / 82844558) write it.
    pub(crate) trick_types: [bool; 14],
    /// ScoreModule +0x38: the collector index 82DA3DD8 picked (0 ground, 1 air, 2 grind, 3/4/-1 the Other collector,
    /// 5 offboard, 6 handplant). The module's reset (82DA3070) leaves 4.
    index: i32,
    /// ScoreModule +0x83: set by the first frame on the board (82DA3E44); until then 82DA3DD8 answers 4.
    started: bool,
    /// ScoreModule +0x80: raised for the one frame the index becomes 3 (82DA4010); 82DA37B0's bail.
    bail: bool,
    /// ScoreModule +0x7C: frames left of the runout window (82DA3444).
    runout_frames: u32,
    /// The scorable the display last recorded (82DA5F98 into `L+0x80`), until 82DA4838 clears the display: what the
    /// air's Enter (82DA80C4..819C) reads as the trick before it.
    last_ended: Option<usize>,
    /// Air `+976`: the slots at take-off (82DA8078's memcpy).
    context_takeoff: ContextBlock,
    /// Air `+1656`: every slot entry seen during the air (82DA8A70's per-frame 82DA14A0).
    context_seen: ContextBlock,
    /// Score::GlobalMetrics X+0x18 / +0x244 / +0x248: slot 6 of the last eight frames, which
    /// 82DA7888 pushes every frame (the index wraps at 8 and sets the full flag) and 82DA8078
    /// merges into the take-off slot 6.
    context_ring: [air::ContextSlot; 8],
    context_ring_next: usize,
    context_ring_full: bool,
    /// Handplant collector `+320`/`+324`/`+336`: the peak clearance (82DAB7D0), its curve
    /// value (banked as scorable 253 at a completed exit, 82DAB6A8) and the frame count the
    /// carrier's accrual is timed by (82DABA10).
    handplant_peak: f32,
    handplant_height: f32,
    handplant_frames: u32,
    /// `L+128` (`L = [holder+4]`): tricks 82DA5F98 has recorded since 82DA4838 last cleared
    /// the display, which it does on every frame a sequence is neither open nor opening.
    display_tricks: u32,
    /// Ground collector `+928`: frames since its Reset (82DAA4C0 counts after its slots).
    ground_frames: u32,
    /// Ground collector `+116`: this frame's sum of the four slot returns (82DAA4C0).
    ground_live: f32,
    /// Holder `+0x30`: this frame's live part of the sequence total. 82DA3534 zeroes it every frame and the active
    /// collector adds to it; 82DA4238 shows it with the pending buckets and the accumulated reward.
    live: f32,
    /// Ground collector `+944`: the revert slot's State70 edge latch (82DAAC10).
    revert_latch: bool,
    /// Ground collector `+936`: the revert side, 1 (bit 0x20000000) or 2 (0x10000000).
    revert_side: u32,
    /// 82DA93D8's one-shot latch at +777: the class-3 bonus has already been paid for
    /// this air, and cannot be paid again however many flip tricks follow.
    flip_bonus_paid: bool,
    /// Offboard collector `+0x14C`: the held hippyjump's spin value so far (82DABFB0).
    offboard_spin: f32,
    /// Offboard collector `+0x141`: its contact bonus is paid, as the air's +777.
    offboard_paid: bool,
    /// 82DA8EB8's +2396 latch, which is also scoring output byte 14651: a body flip
    /// happened during this air, whatever the carrier was.
    ///
    /// This is *not* what names a flip -- that is [`Self::flip_direction`], by way of the
    /// output block's +12316. Byte 14651 only fires one HUD event: `82775328` reads it at
    /// 0x82775560 and calls the backend's vtable+24 with `r4 = 17`. Nothing consumes it
    /// here yet, because that event is not ported.
    pub flip_seen: bool,
    landing_countdown: u32,
    /// ScoreHolder+0x1C4C: a landing was registered for the pending sequence (82DA3310).
    landed: bool,
    idle_ticks: u32,
    /// ScoreModule+112. 82DA33E0 counts the consecutive frames the line timer's near-one
    /// hold actually took effect while the *ground* collector is the active one, and caps
    /// the hold at `[collector 0x5fc] * 60` frames. Any frame that does not hold resets it.
    ground_hold_ticks: u32,
    collector_ticks: u32,
    /// The ground collector's own counter at +932: frames the manual slot has been
    /// suppressed by State+70, which readmits it only while the count is <= 6.
    manual_block_ticks: u32,
    /// Bit 0x80000000 of `[holder+4]+1832`: the active collector's previous answer to "does
    /// the sequence continue?" (82DA37B0 stores it at 0x82DA3970). It is the `open` argument
    /// every collector's continues slot receives, the line timer's hold gate (82DA33E0,
    /// 0x82DA35D8) and the manual counter's reset gate (82DAA8E0, 0x82DAA9A0).
    sequence_active: bool,
    /// The running sequence's score; the banked one on the tick it lands.
    pub sequence_score: f32,
    /// The composed name the display shows, rebuilt each tick by [`compose_trick_name`].
    trick_name: String,
    /// `M+8`: the scorable the display names, before spin, flip, swap and Cab are applied.
    base_trick: Option<usize>,
    /// `M+164`: the id 825E4F40's state 1 last named (999, here `None`, while the module idles
    /// in state 0). State 1 (0x825E5034..5060) raises the new-entry byte `M+157` only when
    /// `B+20` differs from it and `B+241` is clear; otherwise 825E51A0 (0x825E57F0..5810) fires
    /// event 2, CurrentTrickModified, and renames the current entry in place.
    named_trick: Option<usize>,
    /// The trick module's stance at the announcement, `[switch, fakie, nollie]` (`X+40 == 1`,
    /// `X+44`; what 825E4D68 copies to `M+20`/`M+158` every frame).
    stance: [bool; 4],
    /// The display module's stance bytes and what the HUD last read of them.
    stance_bytes: StanceBytes,
    clean: bool,
    sketchy: bool,
    pub new_trick: bool,
    pub modified_trick: bool,
    pub close_tricks: bool,
    /// A sequence was banked this tick without a bail (Skate 1's HUD shows the score and clears its trick
    /// names 1.5 s later).
    pub sequence_landed: bool,
    /// Scorables announced this tick, by identifier (the tutorial ports' trick objectives).
    pub announced: Vec<&'static str>,
    /// Scorables that ended this tick outside the air collector (manuals, grinds, powerslides, reverts; not
    /// conversions), by identifier: Skate 1's per-trick cMsgTrickComplete (skate1_challenge::observe).
    pub completed: Vec<&'static str>,
    /// The spin and flip the *display* holds, `B+52`/`B+56` by way of `M+12`/`M+16`.
    ///
    /// These are not the collector's live values. The scoring output block is blanked at
    /// the top of every frame by 82DE53F0 and only the **active** collector's vtable slot 9
    /// republishes its own fields; 82DA9A18, the Air collector's, is the only one that ever
    /// writes the spin and flip. So while a handplant, a manual or a grind is the active
    /// collector these simply are not refreshed -- which is why a handplant cannot inherit
    /// the previous air's flip and a manual cannot inherit its spin.
    ///
    /// 827743A0 clears them at the end of a trick, via `[B]+92` = 825019B8.
    published_spin_turns: i32,
    published_flip_direction: i32,
    /// The multiplier this runtime last reported, `52(r31)` in 82666BC0.
    published_multiplier: f32,
    /// Set for the one tick on which the published sequence multiplier changed, to the value
    /// it changed to. 82666BC0 polls `140(r30)` against its own cached copy and reacts to any
    /// change in either direction; the ScoreModule itself plays nothing.
    pub multiplier_changed: Option<f32>,
    /// ScoreModule +130, which 82DA3C68 copies every frame from the input's bit 0x04000000 (82761548, see
    /// `online_scoring`). Clear: 82DA37B0 publishes at x1 and credits neither the combo nor the line timer, and
    /// 82DA4238 shows the live sequence total at x1. Set unless a challenge's data turns it off.
    pub multiplier_enabled: bool,
    /// The Skate 2 line meter's capacity (`Tricks::line_max_points`) once a Skate 2 physics frame has fed this
    /// scorer (`port`); `hud_input` prefers it to the data's.
    port_line_capacity: Option<f32>,
    /// The Skate 2 trick display's own score (`Tricks::sequence_score`, before the multiplier) once a Skate 2 physics
    /// frame has fed this scorer; `hud_input` prefers it to `sequence_score`, which keeps the line's total.
    port_sequence_score: Option<f32>,
}
impl Runtime {
    pub fn load(data: &Collections) -> Result<Self, String> {
        Ok(Self::from_data(ScoringData::load(data)?))
    }
    /// A fresh scorer on the same data (Party Play's handover: the next player starts with no line, sequence or
    /// trick state); the challenge's settings (offline, multiplier) carry over.
    pub fn fresh(&self) -> Self {
        Self {
            offline: self.offline,
            manuals_connect: self.manuals_connect,
            multiplier_enabled: self.multiplier_enabled,
            trick_types: self.trick_types,
            ..Self::from_data(self.data.clone())
        }
    }
    /// 82DA3070, the module's reset, which every teleport runs (82DB8998 -> 82DF2848 -> the scoring component's slot 2,
    /// 82DEF808): the holder (82DA5C78, which keeps only the completed lines at +0x18), every collector, both timers,
    /// the not-yet-started latch, index 4.
    fn reset_module(&mut self) {
        // The frame's context slots are this frame's input (82DA7888 refills them after the reset).
        let (completed, slots) = (self.session.holder.snapshot.completed_lines, self.context_slots);
        *self = self.fresh();
        self.session.holder.snapshot.completed_lines = completed;
        self.context_slots = slots;
    }
    /// 82DA3DD8: the collector index for this frame. `class` is 82DA3D58's: the score packet's scorable class (table
    /// +0xC), looked up with no flag or trick-type gate.
    fn select(&mut self, f: &Frame, class: Option<u32>) -> i32 {
        if f.request_teleport {
            return 3;
        }
        let category = f.category as i32;
        if !self.started && (category == 1 && f.wheel_count > 0 || category == 3 || category == 6) {
            self.started = true;
        }
        if !self.started && category != 4 {
            return 4;
        }
        if f.state == 600 && matches!(category, 1 | 2) {
            return if self.trick_types[6] { 6 } else { -1 };
        }
        // An air whose scorable is class 0 stays on the ground collector; a ground frame under a class-3 one stays air.
        let category = match category {
            2 if self.index == 0 && class == Some(0) => 1,
            1 if self.index == 1 && class == Some(3) => 2,
            c => c,
        };
        match category {
            1 => 0,
            2 if self.trick_types[1] => 1,
            3 if self.trick_types[5] && !f.tipslide_326 => 2,
            4 => 3,
            5 => 4,
            6 | 7 => 5,
            _ => -1,
        }
    }
    fn from_data(data: ScoringData) -> Self {
        Self {
            data,
            session: Session::default(),
            collector: Collector::None,
            carriers: std::array::from_fn(|_| None),
            held: [0.; 4],
            distance: [0.; 4],
            metric_rewards: [0.; 4],
            metric_started: [false; 4],
            start: [0.; 3],
            previous: [0.; 3],
            takeoff_normal: [0.; 3],
            spin: SpinTracker::new(frame_rows([0., 1., 0.], [0., 0., 1.]), frame_rows([0., 1., 0.], [0., 0., 1.])),
            spin_total: 0.,
            peak: 0.,
            grab_time: 0.,
            air_factor: 1.,
            air_repetition: 1.,
            air_repetition_set: false,
            grab_chain: 0,
            air_metrics: [0.; 5],
            spin_turns: 0,
            flip_direction: 0,
            context_runs: [ContextRun::default(); 4],
            takeoff_query: GroundQuery::default(),
            context_slots: ContextBlock::default(),
            offline: true,
            manuals_connect: true,
            trick_types: [true; 14],
            index: 4,
            started: false,
            bail: false,
            runout_frames: 0,
            last_ended: None,
            context_takeoff: ContextBlock::default(),
            context_seen: ContextBlock::default(),
            context_ring: Default::default(),
            context_ring_next: 0,
            context_ring_full: false,
            handplant_peak: 0.,
            handplant_height: 0.,
            handplant_frames: 0,
            display_tricks: 0,
            ground_frames: 0,
            ground_live: 0.,
            live: 0.,
            revert_latch: false,
            revert_side: 0,
            flip_bonus_paid: false,
            offboard_spin: 0.,
            offboard_paid: false,
            flip_seen: false,
            landing_countdown: 0,
            landed: false,
            idle_ticks: 0,
            ground_hold_ticks: 0,
            collector_ticks: 0,
            manual_block_ticks: 0,
            sequence_active: false,
            sequence_score: 0.,
            trick_name: String::new(),
            base_trick: None,
            named_trick: None,
            stance: [false; 4],
            stance_bytes: StanceBytes::default(),
            clean: false,
            sketchy: false,
            new_trick: false,
            modified_trick: false,
            close_tricks: false,
            sequence_landed: false,
            announced: Vec::new(),
            completed: Vec::new(),
            published_spin_turns: 0,
            published_flip_direction: 0,
            published_multiplier: 1.,
            multiplier_changed: None,
            multiplier_enabled: true,
            port_line_capacity: None,
            port_sequence_score: None,
        }
    }
    /// A collector is live: the skater is on the board (or walking), not wiped out.
    pub fn riding(&self) -> bool {
        self.collector != Collector::None
    }
    /// The air collector is live: the skater is airborne on the board (S.K.A.T.E.'s attempt watcher).
    pub(crate) fn in_air(&self) -> bool {
        self.collector == Collector::Air
    }
    /// The grind collector is live (Skate 2's Grind_Length contests measure the grind).
    pub(crate) fn grinding(&self) -> bool {
        self.collector == Collector::Grind
    }
    /// B60 0x392B / 0x392C: the Ground (`0x82DAB5E8`) or Air (`0x82DA9A18`) collector published this frame.
    pub(crate) fn ground_or_air(&self) -> bool {
        matches!(self.collector, Collector::Ground | Collector::Air)
    }
    pub fn hud_input(&self) -> hud_runtime::Input {
        hud_runtime::Input {
            sequence_score: self.port_sequence_score.unwrap_or(self.sequence_score) as i32,
            line_score: self.session.holder.snapshot.line as i32,
            sequence_timer: (self.session.line.points / self.data.line_drain) as i32,
            line_time: self.session.line.points / self.data.line_drain,
            line_capacity: self.port_line_capacity.unwrap_or(self.data.line_capacity),
            line_points: self.session.line.points,
            line_thresholds: self.data.combo_levels.map(|(points, _)| points),
            multiplier: self.session.combo.multiplier,
            clean: self.clean,
            sketchy: self.sketchy,
            stance: self.stance_bytes.hud,
            trick_name: self.trick_name.clone(),
            // Tricks.GetCurrentTrickMetrics, sub_825C27C0: the composed name, then the
            // module's switch (M+20), fakie (M+158), "clear the display" (M+156) and
            // "this is a new trick rather than a modification" (M+157) bytes.
            trick_metrics: [
                Value::Text(self.trick_name.clone()),
                Value::Bool(self.stance_bytes.hud[0]),
                Value::Bool(self.stance_bytes.hud[1]),
                Value::Bool(self.close_tricks),
                Value::Bool(self.new_trick),
            ],
            context_tricks: Vec::new(),
        }
    }

    /// Uses the same landing-quality thresholds as the retail scoring/HUD path without requiring
    /// audio to infer clean or sketchy from animation names.
    pub(crate) fn classify_landing_audio(
        &self,
        landing: skate_core::animation::landing_quality::Output,
    ) -> (bool, bool) {
        if !landing.landing_data_167 {
            return (false, false);
        }
        let clean = landing.landing_type_96 == 0;
        let sketchy = landing.landing_type_96 == 1
            && landing.sideways_speed_84 > self.data.sketchy_side_speed
            && landing.spin_92.abs() > 0.5;
        (clean, sketchy)
    }
    fn penalty(&self, id: usize) -> f32 {
        let Some(d) = self.data.by_id(id) else {
            return 1.;
        };
        if d.metadata.repetition_applies() {
            self.data.repetition.evaluate(
                self.session
                    .holder
                    .repetition_count(d.metadata)
                    .unwrap_or(0) as f32,
            )
        } else {
            1.
        }
    }
    fn finish(&mut self, slot: usize, complete: bool, keep_metric: bool) {
        if let Some(mut carrier) = self.carriers[slot].take() {
            if complete {
                // 82DA5F98 records every ended trick on the display (`L+128`) unless 7261.
                if carrier.scorable.valid() && !self.session.holder.suppressed() {
                    self.display_tricks = self.display_tricks.saturating_add(1);
                    self.last_ended = Some(carrier.scorable.id);
                }
                if carrier.complete(self.data.unannounced_factor)
                    && self.collector != Collector::Air
                    && let Some(d) = self.data.by_id(carrier.scorable.id)
                {
                    self.completed.push(d.identifier);
                }
                if self.collector == Collector::Air {
                    if trace_enabled() {
                        eprintln!(
                            "SCORE_TRICK air slot={slot} id={} class={} type={} points={} \
                             factor={:.3} reward={:.1} announced={} bonus_paid={}",
                            carrier.scorable.id,
                            carrier.scorable.class,
                            carrier.scorable.score_type,
                            carrier.points,
                            carrier.factor,
                            carrier.reward,
                            carrier.announced,
                            self.flip_bonus_paid,
                        );
                    }
                    self.session
                        .holder
                        .end_trick(carrier.scorable, carrier.reward);
                } else {
                    let metric = self.collector == Collector::Grind
                        || self.collector == Collector::Ground && slot < 3;
                    if !(keep_metric && self.collector == Collector::Ground) {
                        let reward = if keep_metric {
                            0.
                        } else if metric {
                            if carrier.scorable.score_type == 9 {
                                carrier.reward.max(self.metric_rewards[slot])
                            } else {
                                self.metric_rewards[slot].max(0.)
                            }
                        } else {
                            carrier.reward
                        };
                        if trace_enabled() {
                            eprintln!(
                                "SCORE_TRICK slot={slot} id={} class={} type={} points={} \
                                 factor={:.3} carrier={:.1} metric={:.1} credited={:.1} \
                                 rep={:?} announced={} held={:.2} dist={:.2}",
                                carrier.scorable.id,
                                carrier.scorable.class,
                                carrier.scorable.score_type,
                                carrier.points,
                                carrier.factor,
                                carrier.reward,
                                self.metric_rewards[slot],
                                reward,
                                self.session.holder.repetition_count(carrier.scorable),
                                carrier.announced,
                                self.held[slot],
                                self.distance[slot],
                            );
                        }
                        self.session.holder.credit_trick(carrier.scorable, reward);
                    }
                }
            }
        }
        if !keep_metric {
            self.metric_started[slot] = false;
            self.held[slot] = 0.;
            self.distance[slot] = 0.;
            self.metric_rewards[slot] = 0.;
        }
    }
    fn carrier(&mut self, slot: usize, id: Option<usize>, f: &Frame) -> Result<(), String> {
        if self.carriers[slot].as_ref().map(|c| c.scorable.id) != id {
            let conversion = id.filter(|new| {
                self.collector == Collector::Air
                    && self.carriers[slot].is_some()
                    && LINKS[*new].0 >= 0
            });
            let chained_grab = self.collector == Collector::Air
                && id
                    .and_then(|id| self.data.by_id(id))
                    .is_some_and(|d| d.metadata.class == 2)
                && self.carriers[slot]
                    .as_ref()
                    .is_some_and(|c| c.scorable.class == 2 && !c.announced);
            let previous_tick = self.carriers[slot].as_ref().map(|c| c.start_tick);
            let keep_metric = id.is_some()
                && (self.collector == Collector::Grind
                    || self.collector == Collector::Ground && slot == 0);
            if conversion.is_none() {
                if chained_grab {
                    self.carriers[slot] = None;
                    self.grab_chain += 1;
                } else {
                    self.finish(slot, true, keep_metric);
                    self.grab_chain = 0;
                }
            }
            if let Some(id) = id {
                let d = self
                    .data
                    .by_id(id)
                    .ok_or_else(|| format!("Missing native scorable {id}"))?;
                let factor = self.penalty(id)
                    * match self.collector {
                        Collector::Air => self.air_factor,
                        // 82DABE98: a hippyjump leaving slower than 4 (16 > the air's |.|^2) starts at 0.2.
                        Collector::Offboard if Self::slow(f, f32::from_bits(0x4180_0000)) => f32::from_bits(0x3E4C_CCCD),
                        _ => 1.,
                    };
                let mut carrier = Carrier::new(
                    d.metadata,
                    d.points,
                    factor,
                    self.data.announcement.evaluate(d.points as f32),
                    if chained_grab {
                        previous_tick.unwrap_or(f.tick)
                    } else {
                        f.tick
                    },
                    delay_ticks(
                        d.completion_delay,
                        if self.grab_chain > 1 {
                            self.data.collector.scalar(0x698)
                        } else {
                            0.
                        },
                    ),
                    f.switch,
                    f.fakie,
                );
                if let Some(old) = self.carriers[slot].as_mut() {
                    old.convert_to(&mut carrier, self.data.unannounced_factor);
                    self.modified_trick = true;
                }
                if conversion.is_some() {
                    // 82DA6320 lists the replacement at holder+0x9B0 (output +0xD80); 82775328
                    // names it with `B+241` set, so state 1 records it but never as a new entry.
                    self.base_trick = Some(d.metadata.id);
                    self.named_trick = Some(d.metadata.id);
                }
                self.carriers[slot] = Some(carrier);
                self.idle_ticks = 0;
            }
        }
        let penalty = self.carriers[slot]
            .as_ref()
            .map(|c| self.penalty(c.scorable.id))
            .unwrap_or(1.);
        if let Some(c) = self.carriers[slot].as_mut() {
            let announced_before = c.announced;
            if c.announce(f.tick, self.data.unannounced_factor) {
                let d = self
                    .data
                    .by_id(c.scorable.id)
                    .ok_or("Missing announced scorable")?;
                // 825E51A0 resolves the *named* scorable's record (TrickType, label, link)
                // itself, so only the id is kept.
                self.base_trick = Some(d.metadata.id);
                self.stance = [f.switch, f.fakie, f.nollie, false];
                self.stance_bytes.start(f.fakie);
                // 82DA5DE0 lists every announcement (holder+0x38 -> output +0xE0), 82775328 puts
                // its id in `B+20` and sets state 1 (825012E8), and 825E4F40 compares it with
                // `M+164`: the same scorable announced again in one sequence -- a footplant
                // planted again mid-spin, a regrab, a manual re-entered -- renames the current
                // entry (its spin keeps counting) instead of adding another one.
                if self.named_trick == Some(d.metadata.id) {
                    self.modified_trick = true;
                } else {
                    self.new_trick = true;
                }
                self.named_trick = Some(d.metadata.id);
                self.announced.push(d.identifier);
                if self.collector == Collector::Air && !self.air_repetition_set {
                    self.air_repetition = penalty;
                    self.air_repetition_set = true;
                }
            }
            let distance_metric = self.collector == Collector::Grind
                || self.collector == Collector::Ground && slot < 3;
            let was_active = self.metric_started[slot];
            self.metric_started[slot] = true;
            // 82DAA8E0 updates the manual's metric (82DB0588) before it starts its carrier and before 82DA5EF0 announces
            // it, so the manual earns only from the frame after its announcement (oracle 10-26). The other slots
            // announce first.
            let announced = if self.collector == Collector::Ground && slot == 1 { announced_before } else { c.announced };
            if announced && !(self.collector == Collector::Air && f.suspend_air) {
                // 82DB0588 starts a distance collector at time/distance zero.
                // Every active frame still updates its reference position.
                if !distance_metric || was_active {
                    self.held[slot] += f.dt;
                }
                let previous_distance = self.distance[slot];
                let displacement =
                    std::array::from_fn::<_, 3, _>(|i| f.position[i] - self.previous[i]);
                let delta = if self.collector == Collector::Ground && slot == 1 {
                    displacement
                        .iter()
                        .zip(f.forward)
                        .map(|(x, y)| x * y)
                        .sum::<f32>()
                } else {
                    (displacement.iter().map(|x| x * x).sum::<f32>()).sqrt()
                };
                if was_active && delta > f32::from_bits(0x3ba3d70a) {
                    self.distance[slot] += delta;
                }
                match self.collector {
                    // 82DA93D8 (0x82DA97AC): `+808` is the air's grab time, not the carrier's,
                    // and the reward is one fused multiply-add onto carrier +176.
                    Collector::Air if c.scorable.class == 2 => {
                        self.grab_time = ppc::fadds(self.grab_time, f.dt);
                        let rate = ppc::fmuls(self.data.collector.curve(0x460, self.grab_time), f.dt);
                        c.reward = ppc::fmadds(rate, c.announcement_threshold, c.reward);
                    }
                    // 82DABA10 (0x82DABBF0): timed by the collector's frame count at 1/60 s
                    // (0x820849C8), `((curve * [+168]) * k)` added to the reward.
                    Collector::Handplant => {
                        let k = f32::from_bits(0x3C88_8889);
                        let time = ppc::fmuls(self.handplant_frames as f32, k);
                        let step = ppc::fmuls(
                            ppc::fmuls(self.data.collector.curve(0x140, time), c.announcement_threshold),
                            k,
                        );
                        c.reward = ppc::fadds(c.reward, step);
                    }
                    _ => {}
                }
                let curve = match (self.collector, c.scorable.score_type) {
                    (Collector::Ground, 9) => Some((0x000, 0x050)),
                    (Collector::Ground, 8) => Some((0x0f0, 0x0a0)),
                    (Collector::Ground, _) if slot == 2 => Some((0x230, 0x1e0)),
                    (Collector::Grind, _) => Some((0x2d0, 0x280)),
                    _ => None,
                };
                if let Some((distance, time)) = curve {
                    let delta = (self.data.collector.curve(distance, self.distance[slot])
                        - self.data.collector.curve(distance, previous_distance))
                    .max(0.);
                    self.metric_rewards[slot] = (self
                        .data
                        .collector
                        .curve(time, self.held[slot])
                        .mul_add(f.dt, delta))
                    .mul_add(c.announcement_threshold, self.metric_rewards[slot]);
                }
            }
        }
        Ok(())
    }
    /// 82DA7A38 for this frame, against the take-off snapshot's origin when `reference`.
    fn ground_query(f: &Frame, reference: Option<[f32; 4]>) -> GroundQuery {
        air::ground_query(v4(f.hips_position), f.hips_ground.map(v4), f.hips_surface, reference)
    }

    /// The gap/context collector, `82DA89A0` driving four `82DA7B50` run accumulators.
    ///
    /// This is retail's "cleared a big gap" score. Each frame takes a fresh 82DA7A38 query
    /// against the take-off snapshot (`+160`) and drives one run on each of:
    ///
    /// ```text
    /// run 0: out+25 && !byte[+185]   ; over a tag-10 surface, and not already at take-off
    /// run 1: out+26                  ; over a class-8 surface
    /// run 2: out+20 > [0x680]        ; dropped more than 3 m below take-off
    /// run 3: out+20 > [0x67C]        ; more than 6 m
    /// ```
    ///
    /// every one with the bank flag set, so a run that ends mid-air is added to its total.
    /// It is skipped while `+2398` (Air 452, `suspend_air`) is set.
    fn advance_context_runs(&mut self, f: &Frame) {
        let query = Self::ground_query(f, Some(self.takeoff_query.origin));
        let gates = [
            query.tag_10 && !self.takeoff_query.tag_10,
            query.class_8,
            query.drop > self.data.collector.scalar(0x680),
            query.drop > self.data.collector.scalar(0x67c),
        ];
        for (run, active) in self.context_runs.iter_mut().zip(gates) {
            run.advance(query.origin, active, true);
        }
    }

    /// 82DA8550's landing close (0x82DA87D0): run 0 banks only when the landing query is not
    /// over a tag-10 surface, runs 1-3 always bank. A cancelled exit (0x82DA8844) drops all
    /// four runs unbanked.
    fn close_context_runs(&mut self, f: &Frame, complete: bool) {
        let query = Self::ground_query(f, Some(self.takeoff_query.origin));
        for (i, run) in self.context_runs.iter_mut().enumerate() {
            let bank = complete && (i != 0 || !query.tag_10);
            run.advance(query.origin, false, bank);
        }
    }

    /// `82DA88E8`: the four runs through their curves, summed in order. Banked as scorable
    /// 237 at the landing, and shown live in the sequence total meanwhile.
    fn context_reward(&self) -> f32 {
        self.context_runs
            .iter()
            .zip(CONTEXT_CURVES)
            .map(|(run, curve)| self.data.collector.curve(curve, run.total()))
            .reduce(ppc::fadds)
            .unwrap_or(0.)
    }

    /// 82DA93D8's one-shot class-3 bonus (0x82DA9828..0x82DA9884):
    ///
    /// ```text
    /// carrier present (+124) && announced (+188 = 772) && !2398  ; the accrual gate
    /// class (+12) == 3 && !777 && 82DAC780 sets output 14648
    /// reward = fmadds([0x600] * points, [+172], reward) ; 777 = 1
    /// ```
    ///
    /// [`air::contact_between`] is 82DAC780 (oracle-tested): the hips' contact lies between
    /// the deck probe's origin and the hips. Retail's deck origin is Ground+144, the
    /// board+7968 line test's start (82C07788: row 3 of `[[board+24]+8]+576`); this host has
    /// no deck probe, so the deck body's position stands in for it.
    fn pay_flip_bonus(&mut self, f: &Frame) {
        if self.flip_bonus_paid || f.suspend_air {
            return;
        }
        let Some(carrier) = self.carriers[0].as_ref() else {
            return;
        };
        if !carrier.announced || carrier.scorable.class != 3 {
            return;
        }
        if !air::contact_between(v4(f.position), v4(f.hips_position), f.hips_ground.map(v4)) {
            return;
        }
        let bonus = ppc::fmuls(self.data.collector.scalar(0x600), carrier.points as f32);
        if let Some(carrier) = self.carriers[0].as_mut() {
            carrier.reward = ppc::fmadds(bonus, carrier.factor, carrier.reward);
        }
        self.flip_bonus_paid = true;
    }

    /// The filtered centre-of-mass velocity's part off the reckoning up is under the speed whose square this takes
    /// (82DA81A0, 82DABE98): `k = (B.A)/(A.A)`, `B - A*k`, `speed^2 > |.|^2` (vmsum3fp, fdivs, vmulfp, vsubfp,
    /// vcmpgtfp).
    fn slow(f: &Frame, speed_squared: f32) -> bool {
        let (b, a) = (v4(f.com_velocity), v4(f.reckoning_up));
        let k = ppc::fdivs(ppc::vmsum3fp(b, a)[0], ppc::vmsum3fp(a, a)[0]);
        let off_axis = ppc::vsubfp(b, ppc::vmulfp(a, [k; 4]));
        speed_squared > ppc::vmsum3fp(off_axis, off_axis)[0]
    }

    /// 82DABFB0 after its carrier: a held hippyjump earns its spin (curve 0x370 of |half turns * 180|, halved; each
    /// rise times the carrier's factor) and, once, `0x600 * points` times the factor when the hips' contact lies
    /// between the deck and the hips (82DAC780, as [`Self::pay_flip_bonus`]).
    fn advance_offboard(&mut self, f: &Frame, continuing: bool) {
        let Some(c) = self.carriers[0].as_mut() else { return };
        if continuing {
            let degrees = ppc::fmuls(f.half_turns_312 as f32, f32::from_bits(0x4334_0000)).abs();
            let spin = ppc::fmuls(self.data.collector.curve(0x370, degrees), 0.5);
            let rise = ppc::fsubs(spin, self.offboard_spin);
            self.offboard_spin = spin;
            if rise > 0. {
                c.reward = ppc::fmadds(c.factor, rise, c.reward);
            }
        } else {
            self.offboard_spin = 0.;
        }
        // 82DAC0FC: a held hippyjump's reward is live after its spin, before this frame's bonus.
        let shown = continuing.then_some(c.reward);
        if !self.offboard_paid && air::contact_between(v4(f.position), v4(f.hips_position), f.hips_ground.map(v4)) {
            let bonus = ppc::fmuls(self.data.collector.scalar(0x600), c.points as f32);
            c.reward = ppc::fmadds(bonus, c.factor, c.reward);
            self.offboard_paid = true;
        }
        if let Some(reward) = shown {
            self.add_live(reward);
        }
    }

    /// 82DA8550 (complete): the landing context multiplier and flat bonuses, then the air
    /// metrics and the gap total, all through EndAirTrick 82DA6260.
    fn bank_air(&mut self, f: &Frame) {
        // The slots seen during the air, less those already there at take-off and those
        // still there on the landing frame (82DA1570 twice).
        let mut fresh = self.context_seen;
        air::remove(&mut fresh, &self.context_takeoff);
        air::remove(&mut fresh, &self.context_slots);
        let present = std::array::from_fn(|k| fresh[k].count != 0);
        let (multiplier, bonuses) = air::landing_context(
            present,
            self.offline,
            self.air_factor,
            self.air_repetition,
            |at| self.data.collector.scalar(at),
        );
        // 82DA8550 banks these by bare id (`li r4,129` / `bl 0x82da6260`).
        for (i, reward) in self.air_metrics.into_iter().enumerate() {
            if let Some(metric) = catalog::metadata(AIR_METRIC_BASE + i) {
                self.session.holder.end_trick(metric, ppc::fmuls(reward, multiplier));
            }
        }
        let gap = catalog::metadata(CONTEXT_METRIC);
        if let Some(metric) = gap {
            for bonus in bonuses {
                self.session.holder.end_trick(metric, bonus);
            }
        }
        self.close_context_runs(f, true);
        let total = self.context_reward();
        if let Some(metric) = gap {
            self.session.holder.end_trick(metric, total);
        }
    }

    /// 82DA8A70 while `+2398` (Air 452) is clear: the slot union, then 82DA8FF8 (distance,
    /// peak, gain), 82DA8BE0 (spin), 82DA8EB8 (flip) and 82DA89A0 (gap runs), in that order.
    fn advance_air(&mut self, f: &Frame) {
        air::union(&mut self.context_seen, &self.context_slots);
        let (factor, repetition) = (self.air_factor, self.air_repetition);
        // 82DA8FF8: `(curve * [+2376]) * [+2380]`. The position is `[PhysOut+0]+144`, whose
        // producer is not traced (82C02A80 never writes it); the deck position stands in.
        let now = v4(f.position);
        let distance = air::horizontal_distance(v4(self.start), now);
        self.peak = ppc::fsel(ppc::fsubs(now[1], self.peak), now[1], self.peak);
        let height = ppc::fsubs(self.peak, self.start[1]);
        let gain = ppc::vmsum3fp(ppc::vsubfp(now, v4(self.start)), v4(self.takeoff_normal))[0];
        for (i, (at, x)) in [(0x3c0, distance), (0x410, height), (0x320, gain)].into_iter().enumerate() {
            let value = self.data.collector.curve(at, x);
            self.air_metrics[i] = ppc::fmuls(ppc::fmuls(value, repetition), factor);
        }
        // 82DA8BE0 (oracle-tested as `SpinTracker`): carried = byte 708 and an id in range.
        let carried = self.carriers[0].as_ref().is_some_and(|c| c.scorable.valid());
        let degrees = self.spin.update(
            carried,
            &frame_rows(f.rider_up, f.rider_forward),
            &frame_rows(f.board_up, f.board_forward),
            v4(f.reckoning_up),
        );
        self.spin_total = degrees;
        self.spin_turns = air::half_turns(degrees, self.data.collector.scalar(0x63c));
        let spun = (self.spin_turns.wrapping_mul(180) as f32).abs();
        let value = self.data.collector.curve(0x370, spun);
        self.air_metrics[3] = ppc::fmuls(ppc::fmuls(value, factor), repetition);
        // 82DA8EB8. The +2396 latch, and with it scoring output byte 14651, is set on the
        // first flipping frame whatever the current carrier is. +2348, and so the reward,
        // waits for an announced grab carrier: `subfic`/`subfe`/`rlwinm 0,30,30`/`addi -1`
        // lands on -1 for side == 0 and +1 for side != 0.
        if f.body_flip {
            self.flip_seen = true;
            if self.carriers[0].as_ref().is_some_and(|c| c.announced && c.scorable.class == 2) {
                self.flip_direction = if f.body_flip_side { 1 } else { -1 };
            }
        }
        // Recomputed from the latched +2348 every frame: `(([2380] * [2376]) * [0x640]) * |d|`.
        self.air_metrics[4] = ppc::fmuls(
            ppc::fmuls(ppc::fmuls(factor, repetition), self.data.collector.scalar(0x640)),
            self.flip_direction.unsigned_abs() as f32,
        );
        self.advance_context_runs(f);
        // Live, as 82DA8FF8 / 82DA8BE0 / 82DA8EB8 / 82DA8B44 add it: distance, height, spin, flip, then the gap runs.
        // The gain (+0x93C) is banked at the landing but never shown live.
        for value in [self.air_metrics[0], self.air_metrics[1], self.air_metrics[3], self.air_metrics[4], self.context_reward()] {
            self.add_live(value);
        }
    }

    /// 82DA31F8's factor a pending sequence banks at: switch or fakie alone, on a grind, and the landing's clean or
    /// sketchy bonus (one at most; 0x624's manual term is 1.0 in the stock data).
    fn landing_factor(&self, f: &Frame) -> f32 {
        let mut factor = 1.;
        if f.switch && !f.fakie {
            factor *= self.data.collector.scalar(0x61c);
        }
        if f.fakie && !f.switch {
            factor *= self.data.collector.scalar(0x62c);
        }
        if self.collector == Collector::Grind {
            factor *= self.data.collector.scalar(0x628);
        }
        if self.clean {
            factor *= self.data.collector.scalar(0x630);
        }
        if self.sketchy {
            factor *= self.data.collector.scalar(0x620);
        }
        factor
    }

    /// Adds to the live part (holder +0x30), unless `+0x1C5D` holds this frame's record back.
    fn add_live(&mut self, value: f32) {
        if !self.session.holder.suppressed() {
            self.live = ppc::fadds(self.live, value);
        }
    }

    /// A slot's carrier reward, live once it is valid and announced (82DA9888, 82DABC4C).
    fn add_announced_live(&mut self, slot: usize) {
        if let Some(reward) = self.carriers[slot].as_ref().filter(|c| c.scorable.valid() && c.announced).map(|c| c.reward)
        {
            self.add_live(reward);
        }
    }

    /// 82DAAA44 (manual) / 82DAAF80 (coffin): with manuals not connecting (+945 clear), a slot whose carrier is live
    /// (present, in range, announced) and whose metric runs, with tricks on the display (`L+0x80`), nothing live from
    /// the slots before it this frame (`+116` under 0.0001) and no revert latched (+944), holds only on a frame
    /// carrying the trick bit 0x01000000.
    fn clear_only(&self, slot: usize, live_before: f32, f: &Frame) -> bool {
        !self.manuals_connect
            && self.metric_started[slot]
            && self.carriers[slot].as_ref().is_some_and(|c| c.scorable.valid() && c.announced)
            && self.display_tricks > 0
            && live_before < f32::from_bits(0x38D1_B717)
            && !self.revert_latch
            && f.flags & 0x0100_0000 == 0
    }

    /// Whether a sequence is open, i.e. whether the collectors are still building one.
    pub(crate) fn sequence_open(&self) -> bool {
        self.sequence_active
    }

    /// The SequenceScore a challenge's sequencecomplete gets: the reward cut toward zero (TU3 82DA65B8, Skate 2
    /// 82BB191C: `fctidz`), not rounded.
    pub(crate) fn lua_sequence_score(&self) -> i64 {
        self.sequence_score as i64
    }

    /// The composed name the display is showing, as [`compose_trick_name`] last built it.
    #[allow(dead_code)] // a read-only tap for other systems
    pub(crate) fn displayed_name(&self) -> &str {
        &self.trick_name
    }

    /// Read-only taps (scoring parity 10-04): 82DA8BE0's signed rotation total in degrees
    /// this air, its banked part (`+2384`) and the half-turn count (`+2344`).
    #[allow(dead_code)] // a read-only tap for other systems
    pub(crate) fn spin_degrees(&self) -> (f32, f32, i32) {
        (self.spin_total, self.spin.banked_degrees, self.spin_turns)
    }

    /// The flip a body flip earned (`+2348`: -1, 0, +1) and whether one happened (`+2396`).
    #[allow(dead_code)] // a read-only tap for other systems
    pub(crate) fn body_flip(&self) -> (i32, bool) {
        (self.flip_direction, self.flip_seen)
    }

    /// Sequence flags: open (bit 0x80000000 of `L+1832`), banked this tick, closed by a
    /// bail this tick, and the display's trick count (`L+128`).
    #[allow(dead_code)] // a read-only tap for other systems
    pub(crate) fn sequence_flags(&self) -> (bool, bool, bool, u32) {
        (self.sequence_active, self.sequence_landed, self.close_tricks, self.display_tricks)
    }

    /// The scorables counted in the current sequence and how often (ScoreHolder `+4428`).
    #[allow(dead_code)] // a read-only tap for other systems
    pub(crate) fn sequence_tricks(&self) -> Vec<(usize, i8)> {
        self.session.holder.sequence_tricks().collect()
    }

    /// The active collector's answer to "does the sequence continue?", vtable slot 5, which
    /// `82DA37B0` calls every frame and publishes on a no. The six collectors are named in
    /// `.rdata` at `0x823280F4` and their vtables run from `0x8232812C` in 40-byte strides,
    /// each with `GetName` at slot 6; the Air table at `0x823281A4` is confirmed by slot 3
    /// being its Enter `82DA8078` and slot 9 its publisher `82DA9A18`.
    ///
    /// ```text
    /// Other      8274CA90   li r3,0            ; never
    /// Grind      8281DD70   li r3,1            ; always
    /// Handplant  8281DD70   li r3,1            ; always
    /// Air        82DA98C8   r4 || [this+2352] > 5 || ([this+104] & 0x01000000)
    /// Offboard   82DAC1F8   [this+252] && [this+140] in (-1,332) && [this+316]
    /// Ground     82DAB2A8   a long OR; see below
    /// ```
    ///
    /// `82DAB2A8`, with `open` (`r4`) the previous answer, returns true on any of:
    ///
    /// ```text
    /// [+116] > 0                            ; the four slots' live returns this frame
    /// byte [+944]                           ; the revert slot's State70 latch
    /// open && flags & 0x30000000            ; a revert bit
    /// flags & 0x01000000                    ; the scoring-trick pulse
    /// slot +528 present && announced (716)  ; the powerslide carrier
    /// slot +128 present                     ; the ollie/nollie carrier
    /// r7: open && wheels < 3 && (945 ? [+932] <= 6 : [L+128] <= 0)
    /// r9: open && [+928] < (wheels < 3 && !945 && [L+128] > 0 ? 8 : 3)
    /// open && Ground+268 > 0.1              ; pumping
    /// open && 0.65 > Motion+80.y            ; deck tilted (Motion+80 = effective up row)
    /// ```
    ///
    /// `+945` is [`Self::manuals_connect`] (input +2788 bit 0x40000000), set unless a challenge clears it: a
    /// kickflip landed into a manual stays one sequence while the manual's reward is still zero. Ported as
    /// [`air::ground_continues`], against 82DAB2A8 itself in `air_oracle`.
    fn sequence_continues(&self, f: &Frame, open: bool) -> bool {
        let pulse = f.flags & 0x0100_0000 != 0;
        let present_announced = |slot: usize| {
            self.carriers[slot].as_ref().map(|c| c.scorable.valid() && c.announced)
        };
        match self.collector {
            Collector::None => false,
            Collector::Grind | Collector::Handplant => true,
            Collector::Air => open || self.collector_ticks > 5 || pulse,
            Collector::Offboard => self.offboard_holds(),
            Collector::Ground => air::ground_continues(&air::GroundHold {
                open,
                live: self.ground_live,
                revert_latch: self.revert_latch,
                flags: f.flags,
                slide: present_announced(2) == Some(true),
                ollie: self.carriers[0].as_ref().is_some_and(|c| c.scorable.valid()),
                wheels: f.wheel_count as i32,
                frames: self.ground_frames,
                manual_blocked: self.manual_block_ticks,
                manuals_connect: self.manuals_connect,
                tricks: self.display_tricks as i32,
                pump: f.pump_acceleration,
                deck_up_y: f.board_up[1],
            }),
        }
    }

    /// The Offboard collector's test (82DAC1F8, and again in its timer scales 82DAC248/2B0): its carrier
    /// (`+252` present, `+140` id in range, `+316` announced) is held.
    fn offboard_holds(&self) -> bool {
        self.carriers[0].as_ref().is_some_and(|c| c.scorable.valid() && c.announced)
    }

    /// Identifiers of the scorables being held right now (a manual while it lasts).
    pub fn holding(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.carriers.iter().flatten().filter_map(|c| self.data.by_id(c.scorable.id)).map(|d| d.identifier)
    }

    /// Read-only taps for the Skate 1/2 trick stats (trick_stats.rs): each carrier slot's scorable id and class, with
    /// the time it has been held and the distance it has covered.
    pub(crate) fn carrier_metrics(&self) -> [Option<(usize, u32, f32, f32)>; 4] {
        std::array::from_fn(|i| self.carriers[i].as_ref().map(|c| (c.scorable.id, c.scorable.class, self.held[i], self.distance[i])))
    }

    /// The air's take-off position and peak height (kept until the next take-off), this frame's position and the
    /// four gap runs' totals.
    pub(crate) fn air_measure(&self) -> ([f32; 3], f32, [f32; 3], [f32; 4]) {
        (self.start, self.peak, self.previous, self.context_runs.map(|run| run.total()))
    }

    /// The record the display names (`M+8`, [`Self::base_trick`]) with its link, for [`compose_trick_name`].
    fn named(&self) -> Option<Named<'_>> {
        self.base_trick.and_then(|id| self.data.by_id(id)).map(|d| Named {
            id: d.metadata.id,
            label: &d.label,
            trick_type: d.trick_type,
            link: usize::try_from(d.variant)
                .ok()
                .and_then(|link| self.data.by_id(link))
                .map(|l| (l.metadata.id, l.label.as_str())),
        })
    }

    pub fn advance(&mut self, f: Frame) -> Result<(), String> {
        // A teleport (82DB8998) resets the whole module before this frame's update runs.
        if f.teleported {
            self.reset_module();
        }
        // 82DA3430..3480: a runout request holds `+0x7C` at 30 frames, else it counts down; it or landing +0xA2
        // raises output +0x3941, which loses the sequence like a bail (82DA37B0 / 82DA6408) and settles the line
        // (82DA3B38).
        self.runout_frames = if f.runout_78 { 30 } else { self.runout_frames.saturating_sub(1) };
        let line_lost = f.flags_2484_31 || self.runout_frames > 0;
        // 82DA3534..3538, every frame: the holder's last reward (+0x24) holds a payout only on the frame it is
        // published (oracle 10-25: retail's reads 0 the frame after), and the live part (+0x30) starts again.
        self.session.holder.snapshot.last_reward = 0.;
        self.live = 0.;
        self.new_trick = false;
        self.modified_trick = false;
        // 82DA3070 raises the display's reset (output +0x3935), which closes it as a bail does.
        self.close_tricks = f.teleported;
        self.sequence_landed = false;
        self.announced.clear();
        self.completed.clear();
        // 82DA7888's tail: this frame's slot 6 into the ring, before any collector runs.
        self.context_ring[self.context_ring_next] = self.context_slots[6];
        self.context_ring_next += 1;
        if self.context_ring_next == 8 {
            self.context_ring_next = 0;
            self.context_ring_full = true;
        }
        let named = f.descriptor.and_then(|name| self.data.by_name(name));
        // 82DAC498, the collector's own copy of the packet's scorable: resolved only while its flags hold 0x01000000
        // or 0x02000000, dropped for a trick type the challenge switched off (the record's TrickType, +0x70), and
        // dropped under the grab bit for 67/68/69.
        let descriptor = named
            .filter(|d| f.flags & 0x0300_0000 != 0)
            .filter(|d| self.trick_types.get(d.trick_type as usize).copied().unwrap_or(false))
            .filter(|d| !(f.flags & 0x02000000 != 0 && [67, 68, 69].contains(&d.metadata.id)))
            .map(|d| (d.metadata.id, d.metadata.class, d.metadata.score_type));
        let index = self.select(&f, named.map(|d| d.metadata.class));
        let next = match index {
            0 => Collector::Ground,
            1 => Collector::Air,
            2 => Collector::Grind,
            5 => Collector::Offboard,
            6 => Collector::Handplant,
            _ => Collector::None,
        };
        // 82DA4010: the bail is the frame the index becomes 3.
        self.bail = index != self.index && index == 3;
        if trace_enabled() && (next != self.collector || descriptor.is_some()) {
            eprintln!(
                "SCORE_STATE tick={} cat={:?} state={} collector={:?}->{:?} descriptor={:?} \
                 grind={} flags={:08x} switch={} fakie={} flip={}",
                f.tick,
                f.category,
                f.state,
                self.collector,
                next,
                descriptor,
                f.grind_id,
                f.flags,
                f.switch,
                f.fakie,
                f.body_flip,
            );
        }
        if index != self.index {
            // 82DA4010 exits the old collector with r4 = 0 for the new indices 3 (a bail), 4 (not started, teleporting)
            // and 5 (the Offboard collector, module+0x18): stepping off drops what it held (the Ground exit's
            // 82DA60F0). Any other index, the Other collector's -1 included, completes it.
            let complete = !matches!(index, 3 | 4 | 5);
            // 82DA9B08 (complete) adds the grind's distance (its metric +0x14) to holder +0x12A8, the sequence's grind
            // total that its publication copies to output +0xDC, before the metric ends.
            if complete && self.collector == Collector::Grind {
                let s = &mut self.session.holder.snapshot;
                s.grind_reward = ppc::fadds(self.distance[0], s.grind_reward);
            }
            // The Ground exit (82DAA2D8) ends its slots powerslide, manual, revert, coffin.
            let order = if self.collector == Collector::Ground { [0, 1, 3, 2] } else { [0, 1, 2, 3] };
            for slot in order {
                self.finish(slot, complete, false);
            }
            match self.collector {
                Collector::Air => {
                    if complete {
                        self.bank_air(&f);
                        if trace_enabled() {
                            let m = self.air_metrics;
                            eprintln!(
                                "SCORE_AIR spin_deg={:.0} turns={} flip={} dist={:.0} peak={:.0} \
                                 gain={:.0} spin={:.0} flipmetric={:.0} gaps=[{:.1},{:.1},{:.1},{:.1}] \
                                 gap_reward={:.0} factor={:.3} rep={:.3}",
                                self.spin.banked_degrees,
                                self.spin_turns,
                                self.flip_direction,
                                m[0],
                                m[1],
                                m[2],
                                m[3],
                                m[4],
                                self.context_runs[0].total(),
                                self.context_runs[1].total(),
                                self.context_runs[2].total(),
                                self.context_runs[3].total(),
                                self.context_reward(),
                                self.air_factor,
                                self.air_repetition,
                            );
                        }
                    } else {
                        self.close_context_runs(&f, false);
                    }
                    self.session.holder.finish_collector();
                    self.landing_countdown = 2;
                }
                // 82DAB6A8: a completed handplant banks its height through 82DA6198 as
                // scorable 253 (`li r4,253` / `lfs f0,324(r31)`), straight into +32.
                Collector::Handplant if complete => {
                    if let Some(metric) = catalog::metadata(253) {
                        self.session.holder.credit_trick(metric, self.handplant_height);
                    }
                }
                _ => {}
            }
            self.collector = next;
            self.index = index;
            self.grab_chain = 0;
            self.collector_ticks = 0;
            self.manual_block_ticks = 0;
            match next {
                // The ground Reset 82DAA188 (0x82DAA2C0..D0) clears +928..+944.
                Collector::Ground => {
                    self.ground_frames = 0;
                    self.revert_latch = false;
                    self.revert_side = 0;
                }
                // The handplant Reset 82DAB640 clears +320..+336.
                Collector::Handplant => {
                    self.handplant_peak = 0.;
                    self.handplant_height = 0.;
                    self.handplant_frames = 0;
                }
                // 82DA4174: entering Offboard settles a pending sequence at once, at x1.0, and empties it first
                // unless a landing was registered for it (0x1C4C, which 82DA3310 sets and 82DA4D38 clears).
                // shortcut: the 0x3941 branch (82DA6408 instead) is not ported; its writer is not traced.
                // Its Reset 82DABCB8 clears +0x128..+0x150.
                Collector::Offboard => {
                    (self.offboard_spin, self.offboard_paid) = (0., false);
                    if self.session.holder.has_pending_sequence() {
                        if !self.landed {
                            self.session.holder.cancel_pending();
                        }
                        self.session.holder.reward_sequence(1.);
                        self.landing_countdown = 0;
                    }
                }
                _ => {}
            }
            // Everything below belongs to the *air* collector and is cleared by its own
            // Enter, 82DA8078, and Reset, 82DA7DA0 -- not by leaving it. Clearing on every
            // transition wiped the spin and the flip on the landing tick, before the
            // publication that had to name them, so a 540 published as its bare label.
            if next == Collector::Air {
                self.start = f.position;
                self.peak = f.position[1];
                self.takeoff_normal = f.takeoff_normal;
                self.grab_time = 0.;
                self.spin = SpinTracker::new(
                    frame_rows(f.rider_up, f.rider_forward),
                    frame_rows(f.board_up, f.board_forward),
                );
                self.air_metrics = [0.; 5];
                self.spin_total = 0.;
                self.spin_turns = 0;
                self.flip_direction = 0;
                self.context_runs = [ContextRun::default(); 4];
                self.takeoff_query = Self::ground_query(&f, None);
                // 82DA8078 copies output +13648 to +976, then inserts every entry of the ring's
                // filled slots into the take-off slot 6 (+1384, 82DA14A0): an object under the
                // probe in the eight frames up to take-off never pays.
                self.context_takeoff = self.context_slots;
                let filled = if self.context_ring_full { 8 } else { self.context_ring_next };
                for slot in &self.context_ring[..filled] {
                    for &(key, value) in &slot.entries[..slot.count as usize] {
                        self.context_takeoff[6].insert(key, value);
                    }
                }
                self.context_seen = ContextBlock::default();
                self.flip_bonus_paid = false;
                self.flip_seen = false;
                self.air_repetition = 1.;
                self.air_repetition_set = false;
                self.air_factor = 1.;
                self.session.holder.reward_sequence(1.);
                self.landing_countdown = 0;
                // 82DA80C4..819C: the trick before this air is the display's last record (this frame's list
                // `L+0x3B8`, else the ring `L+0x80`), by its fixed table type: a manual or a grind earns 0x68C /
                // 0x690 however long the skater rolled since, until the display resets.
                let previous_type = self.last_ended.and_then(catalog::metadata).map(|m| m.score_type);
                if previous_type == Some(8) {
                    self.air_factor *= self.data.collector.scalar(0x68c);
                }
                if previous_type == Some(5) {
                    self.air_factor *= self.data.collector.scalar(0x690);
                }
                // 0x82DA81A0..826C: slow under 0x66C. Up a quarter pipe the reckoning up is the ramp's normal, so a
                // take-off up its face is not slow; the world-horizontal speed this used to test made every vert air slow.
                let speed = self.data.collector.scalar(0x66c);
                if Self::slow(&f, ppc::fmuls(speed, speed)) {
                    self.air_factor *= self.data.collector.scalar(0x688);
                }
                if f.switch && !f.fakie {
                    self.air_factor *= self.data.collector.scalar(0x684);
                }
                if f.fakie && !f.switch {
                    self.air_factor *= self.data.collector.scalar(0x694);
                }
            }
        }
        let mut ids = [None; 4];
        if !(self.collector == Collector::Air && f.suspend_air) {
            self.collector_ticks = self.collector_ticks.saturating_add(1);
        }
        // 82DA8A70 runs the air's own measures before 82DA93D8 updates its carrier, so the
        // spin (`[708]`/`[596]`) and the flip (`[772]`) see the carrier as the previous frame
        // left it.
        if self.collector == Collector::Air && !f.suspend_air {
            self.advance_air(&f);
        }
        let mut offboard_continuing = false;
        match self.collector {
            // 82DAA4C0 runs the four slots in this order, each after `+116` = the sum of the returns before it:
            // 82DAA5E8 powerslide (slot 0), 82DAAC10 revert (3), 82DAAEC8 coffin (2), 82DAA8E0 manual (1).
            Collector::Ground => {
                // 82DAA5E8: a powerslide bit, its type on (input +125).
                let powerslide = if !self.trick_types[9] {
                    None
                } else if f.flags & 0x80000000 != 0 {
                    Some(0)
                } else if f.flags & 0x40000000 != 0 {
                    Some(1)
                } else {
                    None
                };
                // Its return (0x82DAA828..A834) is its metric while it slides, never under 0. On the frame it ends it
                // is what its exit credits, max(metric, carrier +0xB0), taken before 82DB0588 zeroes the metric
                // (0x82DAA704..A718), so the slot total (+116) still pays that frame.
                let ending = self.carriers[0].clone().filter(|_| powerslide.is_none()).map(|mut c| {
                    c.complete(self.data.unannounced_factor);
                    c.reward.max(self.metric_rewards[0])
                });
                self.carrier(0, powerslide, &f)?;
                let mut live = ppc::fadds(0., ending.unwrap_or(self.metric_rewards[0]).max(0.));
                if self.metric_rewards[0] > 0. {
                    self.add_live(self.metric_rewards[0]);
                }
                // 82DAAC10, the revert slot. The side (+936) latches from the revert bits; the carrier starts on the
                // frame State70 rises (`fsrevert` 4 for side 1, `bsrevert` 5 otherwise) and ends on the frame it
                // falls, crediting its reward through 82DA6198 (the slot's return) and clearing the side.
                // `[input+127]` (the Revert type) gates the trigger.
                if f.flags & 0x20000000 != 0 {
                    self.revert_side = 1;
                } else if f.flags & 0x10000000 != 0 {
                    self.revert_side = 2;
                }
                let trigger = f.manual_block_70 && self.trick_types[11];
                let mut revert_return = 0.;
                let revert = match (trigger, self.revert_latch) {
                    (true, false) => Some(if self.revert_side == 1 { 4 } else { 5 }),
                    (true, true) => self.carriers[3].as_ref().map(|c| c.scorable.id),
                    (false, latched) => {
                        if latched {
                            revert_return = self.carriers[3].as_ref().map_or(0., |c| c.reward);
                            self.revert_side = 0;
                        }
                        None
                    }
                };
                self.revert_latch = trigger;
                self.carrier(3, revert, &f)?;
                live = ppc::fadds(live, revert_return);
                // 82DAAEC8: the coffin (60) under the grab bit, its type on (input +120, the descriptor's filter).
                let coffin = descriptor
                    .filter(|d| d.0 == 60 && f.flags & 0x02000000 != 0)
                    .map(|d| d.0)
                    .filter(|_| !self.clear_only(2, live, &f));
                self.carrier(2, coffin, &f)?;
                // Its return is its metric (+0x438) as 82DB0588 left it; only a positive one is live (82DAB274).
                live = ppc::fadds(live, self.metric_rewards[2]);
                if self.metric_rewards[2] > 0. {
                    self.add_live(self.metric_rewards[2]);
                }
                // 82DAA8E0, the ground collector's manual slot:
                //
                // ```text
                // r10 = [[[this+4]+28]+70]       ; PhysOut -> State, byte 70
                // r4  = (bit4 || bit5 of [this+104]) && [input+124]
                // if (r4 && r10) { [this+932]++ ; r4 = 0 }
                // else if (!bit31([L+1832])) [this+932] = 0   ; 0x82DAA9A0: rlwinm 1,31,31
                // r4 &= ([this+932] <= 6)
                // ```
                //
                // The gate is State+**70**, not State+66 (the revert's whole lifetime), and
                // the reset reads the sequence-open bit 0x80000000. `[input+124]` is the Manual byte of the
                // challenge's trick-type mask ([`Self::trick_types`]); then 0x82DAAA44..AAB4's gate
                // ([`Self::clear_only`]).
                let mut manual = if !self.trick_types[8] {
                    None
                } else if f.flags & 0x08000000 != 0 {
                    Some(2)
                } else if f.flags & 0x04000000 != 0 {
                    Some(3)
                } else {
                    None
                };
                if manual.is_some() && f.manual_block_70 {
                    self.manual_block_ticks = self.manual_block_ticks.saturating_add(1);
                    manual = None;
                } else if !self.sequence_active {
                    self.manual_block_ticks = 0;
                }
                if self.manual_block_ticks > 6 || self.clear_only(1, live, &f) {
                    manual = None;
                }
                if manual.is_some()
                    && let Some(c) = &self.carriers[1]
                {
                    manual = Some(c.scorable.id);
                }
                self.carrier(1, manual, &f)?;
                // Its return is its metric (+0x408), live when positive (82DAAAEC).
                self.ground_live = ppc::fadds(live, self.metric_rewards[1]);
                if self.metric_rewards[1] > 0. {
                    self.add_live(self.metric_rewards[1]);
                }
            }
            Collector::Air => ids[0] = descriptor.filter(|d| d.2 != 5).map(|d| d.0),
            // 82DA9D68, the Grind collector: the physics grind id, unless the descriptor is a
            // different type-5 scorable whose record sets layout +148 (`lbz r11,148(r11)`) --
            // the 50 grind grabs (`T_GrindGrabs`/`T_BoardGrabs`' `ScoringTrick`), which then
            // take the carrier.
            Collector::Grind => {
                ids[0] = descriptor
                    .filter(|d| d.2 == 5 && self.data.by_id(d.0).is_some_and(|r| r.grind_grab))
                    .map(|d| d.0)
                    .or((f.grind_id >= 0).then_some(f.grind_id as usize))
            }
            // 82DABFB0, the Offboard collector: the hippyjump (`cmpwi cr6,r4,234`; the descriptor already needs its
            // type on, input +126) or, once held, physical state 503. Losing both cancels it (82DA60F0): only
            // the exit to another collector credits it.
            Collector::Offboard => {
                let held = self.carriers[0].as_ref().map(|c| c.scorable.id);
                ids[0] = descriptor.filter(|d| d.0 == 234).map(|d| d.0).or(held.filter(|_| f.state == 503));
                if held.is_some() && ids[0].is_none() {
                    self.finish(0, false, false);
                    self.offboard_paid = false;
                }
                offboard_continuing = held.is_some() && ids[0].is_some();
            }
            // 82DABA10, the Handplant collector: `addi r9,r27,16` / `cmpwi cr6,r6,6`,
            // i.e. any scorable whose class is Handplant_CLASS. `[+336]` counts the frames
            // with one (0x82DABB1C), before the accrual reads it.
            Collector::Handplant => {
                ids[0] = descriptor.filter(|d| d.2 == HANDPLANT_CLASS).map(|d| d.0);
                if ids[0].is_some() {
                    self.handplant_frames = self.handplant_frames.wrapping_add(1);
                }
            }
            Collector::None => {}
        }
        // The ground slots ran above, in their own order.
        if self.collector != Collector::Ground {
            for (slot, id) in ids.into_iter().enumerate() {
                self.carrier(slot, id, &f)?;
            }
        }
        match self.collector {
            Collector::Air => {
                self.pay_flip_bonus(&f);
                // 82DA9888: the carrier's reward is live once valid and announced, unless the air is suspended.
                if !f.suspend_air {
                    self.add_announced_live(0);
                }
            }
            Collector::Offboard => self.advance_offboard(&f, offboard_continuing),
            // 82DA9FD8: the grind's metric is live whatever its sign.
            Collector::Grind => self.add_live(self.metric_rewards[0]),
            // 82DAB7D0: the peak clearance of the hips probe and the deck probe, and its
            // curve (0x190). Not ported: the deck probe (Ground+144/+160/+319, the
            // board+7968 line test), which this host does not run.
            Collector::Handplant => {
                // 82DABC4C: the carrier's reward is live (valid and announced), then 82DAB8E0 adds the height (+0x144).
                self.add_announced_live(0);
                let hips = f.hips_ground.map(|contact| (v4(f.hips_position), v4(contact)));
                self.handplant_peak = air::handplant_peak(self.handplant_peak, hips, None);
                self.handplant_height = self.data.collector.curve(0x190, self.handplant_peak);
                self.add_live(self.handplant_height);
            }
            // 82DAA4C0: after the slots (`+116` above), `+928` counts the frame.
            Collector::Ground => {
                self.ground_frames = self.ground_frames.wrapping_add(1);
            }
            _ => {}
        }
        let active = self.carriers.iter().any(Option::is_some) || self.collector == Collector::Air;
        self.idle_ticks = if active {
            0
        } else {
            self.idle_ticks.saturating_add(1)
        };
        // 82DA4D38 clears 0x1C4C with every bank, so it only ever describes the pending sequence.
        self.landed &= self.session.holder.has_pending_sequence();
        if self.session.holder.has_pending_sequence() {
            if f.landing.landing_data_167 {
                self.landed = true;
                self.clean = f.landing.landing_type_96 == 0;
                self.sketchy = f.landing.landing_type_96 == 1
                    && f.landing.sideways_speed_84 > self.data.sketchy_side_speed
                    && f.landing.spin_92.abs() > 0.5;
            }
            if self.landing_countdown == 0 {
                self.session.holder.reward_sequence(self.landing_factor(&f));
            } else {
                self.landing_countdown -= 1;
            }
        }
        // The two timers' drain scales are the active collector's vtable +0x1C / +0x20, which 82DA33E0 calls
        // every frame whatever the collector holds: Air 82DA9900/9910, Grind 82DA9FF8/A008 and Handplant
        // 82DABC78/88 return their scalars outright; Ground 82DAB528/558 only while this frame's slot returns
        // (+116) are positive; Offboard 82DAC248/2B0 on its slot-5 test (else 0x5f8/0x5f4); Other 82481C70 1.0.
        let scales = match self.collector {
            Collector::Air => Some((0x668, 0x664)),
            Collector::Grind => Some((0x618, 0x614)),
            Collector::Handplant => Some((0x608, 0x604)),
            Collector::Ground => (self.ground_live > 0.).then_some((0x610, 0x60c)),
            Collector::Offboard if self.offboard_holds() => Some((0x610, 0x60c)),
            Collector::Offboard => Some((0x5f8, 0x5f4)),
            Collector::None => None,
        };
        let (line_scale, combo_scale) =
            scales.map_or((1., 1.), |(line, combo)| (self.data.collector.scalar(line), self.data.collector.scalar(combo)));
        // 82DA33E0 calls 82DA4C28 twice, and the two calls do *not* share a hold flag.
        //
        // ```text
        // r29 = ([module+4] == [module+16])   ; the active collector is the ground one
        // r5  = bit31 of [holder+4]+1832     ; the sequence was open last frame
        // if (!r5)          r30 = 0           ; no hold at all
        // else if (!r29)    r30 = 1           ; any other collector holds without a bound
        // else              r30 = [module+112] < (int)([module+64]+1532 * 60.0)
        // bl 0x82da4c28     ; r3 = module+40, the line timer, r5 = r30
        // [module+112] = (returned hold && r29) ? [module+112] + 1 : 0
        // li r5,0
        // bl 0x82da4c28     ; r3 = module+44, the combo timer -- never held
        // ```
        //
        // The collector at module+16 is the ground one: 82DA3C68 writes its byte+945 every
        // frame, and 82DAA8E0, which reads that same byte's object, is what tests the
        // nose/tail manual bits 0x08000000/0x04000000. Its authored bound is
        // `0x5fc` = 5.0 seconds times the 60.0 at 0x8303745C, so 300 frames.
        //
        // Passing "a collector is active" to both calls, as this did, held the line just
        // above one point for as long as *any* ground carrier existed -- a manual, a
        // powerslide, a revert -- so the line never expired and the score and trick name
        // stayed on screen indefinitely. It also held the multiplier timer, which retail
        // never does.
        //
        // `r5` is bit 0x80000000 of `[holder+4]+1832` (`rlwinm r5,r6,1,31,31` at 0x82DA35D8):
        // the active collector's *previous* answer to "does the sequence continue?", which
        // 82DA37B0 stores at 0x82DA3970. It was read as bit 30 (0x40000000, the "published"
        // stamp 82DA48B8 sets) until this was traced; `sequence_active` is that bit, so the
        // hold exists exactly while a sequence is open and lapses the frame after it closes.
        let grounded = self.collector == Collector::Ground;
        let hold_limit = (self.data.collector.scalar(0x5fc) * 60.) as u32;
        let open = self.sequence_active;
        let hold_line = open && (!grounded || self.ground_hold_ticks < hold_limit);
        let held = self
            .session
            .line
            .advance(f.dt, self.data.line_drain, line_scale, hold_line);
        self.ground_hold_ticks = if held && grounded {
            self.ground_hold_ticks.saturating_add(1)
        } else {
            0
        };
        self.session
            .combo
            .timer
            .advance(f.dt, self.data.combo_drain, combo_scale, false);
        // 82DA37B0: a bail is the index's one-frame 3 (`module+0x80`), not any frame without a collector -- the Other
        // collector's 4 and -1 (not started, teleporting, a switched-off trick type) end a sequence as a landing does.
        // A bail empties the pending buckets (0x82DA384C..38A0, then 82DA6468 at x1, which clears the pending flag)
        // and ends the sequence at the bail factor; any other frame asks the *active collector* whether the sequence
        // continues (vtable slot 5, `r4` = the previous answer) and publishes on the falling edge (82DA48B8, then
        // 82DA31F8 + 82DA6538). A sequence opens only through that answer -- a new carrier does not open one by itself.
        // Output +0x3941 takes the same path at 82DA382C (82DA6408 for the pending buckets): only its HUD bytes differ.
        let bailout = self.bail;
        let lost = bailout || line_lost;
        if lost && self.session.holder.has_pending_sequence() {
            self.session.holder.cancel_pending();
            self.session.holder.reward_sequence(1.);
            self.landed = false;
            // 82DA6468 clears the landing countdown (+0x1C58) with the pending flag.
            self.landing_countdown = 0;
        }
        let continues = !lost && self.sequence_continues(&f, open);
        // Oracle 10-25: the falling edge publishes whatever the landing countdown says (an air ending in a tipslide
        // publishes that frame); 82DA3994 banks a still-pending sequence first through 82DA31F8, at its landing
        // factor, and 82DA6468 clears the countdown.
        if open && (lost || !continues) {
            self.sequence_score =
                self.session
                    .publish_sequence(&self.data.session_rules(), self.landing_factor(&f), lost, self.multiplier_enabled);
            self.landing_countdown = 0;
            // 827743A0 runs every frame and, on the frame a trick finishes without a bail
            // (O[14626] set, O[14640]/O[14630] clear), calls [B]+92 = 825019B8, which zeroes
            // M+12 and M+16 and drops the module back to state 0. That is what stops the
            // next trick inheriting this one's spin and flip.
            if !bailout {
                self.published_spin_turns = 0;
                self.published_flip_direction = 0;
            }
            // Either way the module returns to state 0 (825019B8 here; a bail goes through
            // 825013A0's state 3), whose 0x825E4FD4 sets `M+164` = 999: the next sequence's
            // first trick is a new entry even when it repeats the last one.
            self.named_trick = None;
            if trace_enabled() {
                let s = &self.session.holder.snapshot;
                eprintln!(
                    "SCORE_PUBLISH reward={:.0} mult={:.2} line={:.0} lifetime={:.0} \
                     bail={bailout} name={:?}",
                    self.sequence_score,
                    self.session.combo.multiplier,
                    s.line,
                    s.completed_lines,
                    self.trick_name,
                );
            }
            self.sequence_active = false;
            // 82775328 -> 82774E88 closes only for ScoreModule reset/bail
            // output 14630 (82DA4010/82DA4238), not a banked landing.
            self.close_tricks |= bailout;
            self.sequence_landed = !lost;
            // 82DA48B8 publishes the display and clears it (82DA4838): the next air has no trick before it.
            self.display_tricks = 0;
            self.last_ended = None;
        } else {
            // ponytail: a falling edge the port's two-frame landing countdown (holder+7256)
            // defers keeps the bit set until it publishes; how retail orders the two is not
            // traced.
            self.sequence_active = continues || open;
            // 82DA4838 on a frame that neither had nor has a sequence: the display's trick
            // record (`L+128`) and stamps are cleared.
            if !open && !continues {
                self.display_tricks = 0;
                self.last_ended = None;
            }
        }
        if self.sequence_active {
            let s = &self.session.holder.snapshot;
            // 82DA4238: (((+0x30 + +0x2C) + +0x28) + +0x20), times [module+44]+32 when `lbz r11,130(r31)`, else 1.0.
            let total = ppc::fadds(ppc::fadds(ppc::fadds(self.live, s.fingerflip_pending), s.general_pending), s.accumulated);
            self.sequence_score =
                ppc::fmuls(if self.multiplier_enabled { self.session.combo.multiplier } else { 1. }, total);
        }
        if self.session.line.expired || f.teleported || bailout {
            self.sequence_score = 0.;
        }
        // 82DA3B38: the bail, output +0x3941 and the board's retrieval (off-board +0x143) settle the line too.
        self.session.settle_line(f.teleported || lost || f.retrieving_board_323, self.sequence_active);
        self.previous = f.position;
        // The multiplier poll, after settlement so a line that drops it back to x1 is seen as
        // the change it is. The cached copy is updated whether or not anything reacted, which
        // is 82666BC0's unconditional `stfs f31,52(r31)`.
        let multiplier = self.session.combo.multiplier;
        self.multiplier_changed = (multiplier != self.published_multiplier).then_some(multiplier);
        self.published_multiplier = multiplier;
        // The name is rebuilt rather than assigned once, because the spin and the flip that
        // go into it keep accumulating after the record that named the trick was announced.
        // 82666BC0's counterpart recomposes whenever its dirty flag is set, for the same
        // reason. A closed display drops the base label so the next air starts clean.
        if self.close_tricks {
            self.base_trick = None;
        }
        // Only the Air collector's slot 9 (82DA9A18) ever publishes the spin and the flip,
        // and only while it is the active collector. Every other collector leaves them
        // untouched, so they must not be refreshed from the air's live values here.
        let (previous_spin, previous_flip) =
            (self.published_spin_turns, self.published_flip_direction);
        if self.collector == Collector::Air {
            self.published_spin_turns = self.spin_turns;
            self.published_flip_direction = self.flip_direction;
        }
        // 825E4F40 compares M+12 and M+16 against the module's cached copies and sets the
        // dirty byte 159 only when one of those two differs; 825E51A0 then recomposes and
        // re-fires the display event. Firing on any change to the whole composed string
        // also caught switch-parity flips, which re-triggered the display for no reason.
        if self.published_spin_turns != previous_spin
            || self.published_flip_direction != previous_flip
        {
            self.modified_trick = true;
        }
        // 825E4F40's tail: 825E4D68 resyncs the stance bytes, 825E51A0 composes on a dirty
        // frame (byte 159: a new trick or a changed spin/flip), then the stance event settles.
        // The name is a pure function of its inputs, so it is rebuilt every tick.
        self.stance_bytes.sync(self.stance[0], self.stance[1]);
        let named = self.named();
        let composed = compose_trick_name(
            named,
            self.published_spin_turns,
            self.published_flip_direction,
            self.stance[0],
            self.stance[1],
            // M+187 = IsRegular (natural_stance == 0).
            f.regular,
            self.stance_bytes.cab,
        );
        // 825E4D00, the HUD's fourth stance slot: the named record is a nollie trick and the
        // module is mid-trick (`M+0 != 0`). ponytail: `M+0`'s 2 -> 3 -> 0 end-of-trick
        // transition is driven from outside the module and not traced; the port takes "a
        // trick is named" (cleared on close) for it.
        let nollie = self.base_trick.is_some_and(is_nollie);
        if self.new_trick || self.modified_trick {
            self.stance_bytes.composed(&composed, nollie);
        }
        self.stance_bytes.settle(nollie);
        self.trick_name = composed.name;
        // Every trace above is edge-triggered, and a line timer pinned just above one
        // point by the near-one hold looks exactly like a healthy line unless it is
        // sampled on every tick. 82DA33E0 bounds that hold with the counter at
        // module+112; this is how we see whether it is running at all.
        if trace_enabled() {
            eprintln!(
                "SCORE_TICK tick={} col={:?} flags={:08x} rev={} mblock={} active={active} idle={} land={} hold={hold_line} groundhold={} line={:.2} line_s={:.2} combo={:.2} mult={:.2} seq={} bail={bailout} close={} name={:?}",
                f.tick,
                self.collector,
                f.flags,
                f.reverting,
                f.manual_block_70,
                self.idle_ticks,
                self.landing_countdown,
                self.ground_hold_ticks,
                self.session.line.points,
                self.session.line.points / self.data.line_drain,
                self.session.combo.timer.points,
                self.session.combo.multiplier,
                self.sequence_active,
                self.close_tricks,
                self.trick_name,
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod trick_name_tests {
    use super::{Composed, Named, StanceBytes, compose_trick_name, decorates_spin, trick_tokens};

    fn name(named: Option<Named>, turns: i32, flip: i32, switch: bool, regular: bool) -> String {
        compose_trick_name(named, turns, flip, switch, false, regular, false).name
    }
    fn record(id: usize, label: &str, trick_type: u32) -> Named<'_> {
        Named { id, label, trick_type, link: None }
    }
    const KICKFLIP: Named = Named { id: 96, label: "ID_TRICK_FLIP_KICKFLIP", trick_type: 2, link: None };
    /// `n_kickflip` (117), linked through `Hash_2E90BC04042A0B5A` to `kickflip` (96).
    const N_KICKFLIP: Named = Named {
        id: 117,
        label: "ID_TRICK_FLIP_NOLLIE_KICKFLIP",
        trick_type: 2,
        link: Some((96, "ID_TRICK_FLIP_KICKFLIP")),
    };

    /// The user-visible symptom this fixes: a body flip was displayed as a bare air,
    /// because nothing ever appended the flip token 82DA8EB8's +2348 selects.
    #[test]
    fn a_body_flip_is_named_and_takes_its_side_from_the_flip_direction() {
        assert_eq!(name(None, 0, 1, false, false), format!("#{} {}", trick_tokens::AIR, trick_tokens::FRONTFLIP));
        assert_eq!(name(None, 0, -1, false, false), format!("#{} {}", trick_tokens::AIR, trick_tokens::BACKFLIP));
        // A flip with no grab to record its side leaves +2348 at zero, and retail appends
        // nothing at all rather than guessing a side.
        assert_eq!(name(None, 0, 0, false, false), format!("#{}", trick_tokens::AIR));
    }

    /// 82DA8BE0 keeps signed half-turns; the display prints whole degrees.
    #[test]
    fn a_spin_prints_degrees_and_picks_its_side_by_parity() {
        assert_eq!(name(Some(KICKFLIP), 2, 0, false, false), "#ID_TRICK_FLIP_KICKFLIP 360");
        assert_eq!(name(Some(KICKFLIP), -3, 0, false, false), "#ID_TRICK_FLIP_KICKFLIP 540");
        // Unnamed spins fall back to the FS/BS tokens, chosen by switch ^ sign ^ regular.
        for (turns, switch, regular, expected) in [
            (1, false, false, trick_tokens::FS_SPIN),
            (-1, false, false, trick_tokens::BS_SPIN),
            (1, true, false, trick_tokens::BS_SPIN),
            (1, false, true, trick_tokens::BS_SPIN),
            (-1, true, true, trick_tokens::BS_SPIN),
        ] {
            assert_eq!(
                name(None, turns, 0, switch, regular),
                format!("#{expected} 180"),
                "turns {turns} switch {switch} regular {regular}"
            );
        }
    }

    /// 825E51A0 refuses the spin decoration for a trick class that may not carry one, so
    /// a manual (TrickType 8) never reads "Manual 180" and a handplant (6) never gains a
    /// spin from the air before it. The one grab refused is `coffin` (60).
    #[test]
    fn only_the_admitted_trick_classes_carry_a_spin() {
        for admitted in [1, 2, 3, 4, 7, 10, 12, 13] {
            assert!(decorates_spin(admitted, 61), "TrickType {admitted}");
        }
        for refused in [0, 5, 6, 8, 9, 11, 14] {
            assert!(!decorates_spin(refused, 61), "TrickType {refused}");
        }
        assert!(!decorates_spin(4, 60) && decorates_spin(2, 60));
        assert_eq!(name(Some(record(2, "ID_TRICK_GROUND_TRICK_MANUAL", 8)), 1, 0, false, false), "#ID_TRICK_GROUND_TRICK_MANUAL");
        // The flip suffix is *not* behind this gate -- 0x825E57A8 tests only the sign.
        assert_eq!(name(Some(record(149, "ID_TRICK_BS_HANDPLANT", 6)), 2, 0, false, false), "#ID_TRICK_BS_HANDPLANT");
    }

    /// Everything after the single leading `#` has to be tokens the text layer can look
    /// up, so the name must never carry a second `#` or a double space.
    #[test]
    fn the_composed_name_is_one_hash_and_resolvable_tokens() {
        let name = name(Some(KICKFLIP), 2, 1, true, false);
        assert_eq!(name.matches('#').count(), 1);
        assert!(!name.contains("  "));
        assert_eq!(
            name.trim_start_matches('#').split(' ').collect::<Vec<_>>(),
            ["ID_TRICK_FLIP_KICKFLIP", "360", trick_tokens::FRONTFLIP]
        );
    }

    /// 0x825E5644/5700: out of fakie (`M+186`) a named spin gains the Cab token, FS by the
    /// spin's sign; a Half-Cab prints no degrees, a Cab still does, and both clear `M+158`.
    /// An unnamed spin is the Cab token alone (0x825E5350).
    #[test]
    fn a_spin_out_of_fakie_is_a_cab() {
        let cab = |named, turns| compose_trick_name(named, turns, 0, false, true, false, true);
        assert_eq!(
            cab(Some(KICKFLIP), 1),
            Composed { name: "#ID_TRICK_FLIP_KICKFLIP ID_TRICK_AUTHENTIC_FS_HALFCAB".into(), switch: false, fakie: false, swapped: false }
        );
        assert_eq!(cab(Some(KICKFLIP), -2).name, "#ID_TRICK_FLIP_KICKFLIP ID_TRICK_AUTHENTIC_BS_CAB 360");
        assert_eq!(cab(None, 1).name, "#ID_TRICK_AUTHENTIC_FS_HALFCAB");
        assert_eq!(cab(None, -3).name, "#ID_TRICK_AUTHENTIC_BS_CAB 540");
        // No spin, no Cab, and fakie stays on the HUD.
        assert_eq!(cab(Some(KICKFLIP), 0), Composed { name: "#ID_TRICK_FLIP_KICKFLIP".into(), switch: false, fakie: true, swapped: false });
        // A class that may not carry a spin takes no Cab either.
        assert_eq!(cab(Some(record(2, "ID_TRICK_GROUND_TRICK_MANUAL", 8)), 1).name, "#ID_TRICK_GROUND_TRICK_MANUAL");
    }

    /// 0x825E54EC: a nollie trick ridden switch reads as the fakie version of its linked
    /// trick, and ridden fakie as the switch one; `M+169` records the swap.
    #[test]
    fn a_nollie_trick_swaps_to_its_linked_trick_and_stance() {
        let compose = |switch, fakie| compose_trick_name(Some(N_KICKFLIP), 0, 0, switch, fakie, false, false);
        assert_eq!(compose(true, false), Composed { name: "#ID_TRICK_FLIP_KICKFLIP".into(), switch: false, fakie: true, swapped: true });
        assert_eq!(compose(false, true), Composed { name: "#ID_TRICK_FLIP_KICKFLIP".into(), switch: true, fakie: false, swapped: true });
        for (switch, fakie) in [(false, false), (true, true)] {
            assert_eq!(compose(switch, fakie).name, "#ID_TRICK_FLIP_NOLLIE_KICKFLIP");
            assert!(!compose(switch, fakie).swapped);
        }
        // `M+186` latched from fakie and then swapped: the Cab test reads the *linked* id,
        // which is not a nollie trick, so the swapped name takes the Cab.
        let c = compose_trick_name(Some(N_KICKFLIP), 1, 0, false, true, false, true);
        assert_eq!(c.name, "#ID_TRICK_FLIP_KICKFLIP ID_TRICK_AUTHENTIC_FS_HALFCAB");
        // Unswapped, the nollie trick itself refuses it.
        let c = compose_trick_name(Some(N_KICKFLIP), 1, 0, false, false, false, true);
        assert_eq!(c.name, "#ID_TRICK_FLIP_NOLLIE_KICKFLIP 180");
    }

    /// 0x825E5288: `superman` (252) with a flip is the Miracle Whip, and the flip suffix is
    /// dropped (r21 = 0); without one it keeps its own label.
    #[test]
    fn a_flipped_superman_is_a_miracle_whip() {
        let superman = record(252, "ID_TRICK_GRAB_SUPERMAN", 4);
        assert_eq!(name(Some(superman), 0, 1, false, false), "#ID_TRICK_GRAB_MIRACLE_WHIP");
        assert_eq!(name(Some(superman), 2, -1, false, false), "#ID_TRICK_GRAB_MIRACLE_WHIP 360");
        assert_eq!(name(Some(superman), 0, 0, false, false), "#ID_TRICK_GRAB_SUPERMAN");
    }

    /// 825E4F40's stance bytes: a swap fires the stance event at once with `M+168` set, then
    /// clears `M+168`, so the next frame's resync stays silent until it re-arms 140 frames on.
    #[test]
    fn a_nollie_swap_holds_the_hud_stance_for_140_frames() {
        let mut s = StanceBytes::default();
        s.start(false);
        s.sync(true, false);
        let c = compose_trick_name(Some(N_KICKFLIP), 0, 0, true, false, false, false);
        s.composed(&c, true);
        s.settle(true);
        assert_eq!(s.hud, [false, true, true, true], "the swapped stance is what the HUD reads");
        assert!(!s.shown);
        for frame in 1..140 {
            s.sync(true, false);
            s.settle(true);
            assert_eq!(s.hud, [false, true, true, true], "frame {frame}");
        }
        // Frame 140 re-arms M+168; the dirty stance settles three frames later.
        s.sync(true, false);
        s.settle(true);
        s.sync(true, false);
        s.settle(true);
        assert_eq!(s.hud, [false, true, true, true]);
        s.sync(true, false);
        s.settle(true);
        assert_eq!(s.hud, [true, false, true, true]);
        // Out of fakie into a new trick, the Cab latch reads the switch byte before resync.
        s.start(true);
        assert!(!s.cab, "M+20 was still set");
        s.sync(false, true);
        s.start(true);
        assert!(s.cab);
    }
}

#[cfg(test)]
mod spin_tests {
    //! 82DA8BE0's arithmetic is checked bit for bit against TU3 by skate-core's `air_oracle`;
    //! these only pin the behaviour the trick name depends on. Retail's own 8296EC98 reads
    //! about 0.0198 degrees between two identical frames, so a held matrix drifts by that
    //! much a frame; the `[0x63C]` offset (80 degrees, `OFFSET` here) absorbs it.
    use super::frame_rows;
    use skate_core::scoring::air::{SpinTracker, half_turns};

    const UP: [f32; 3] = [0., 1., 0.];
    const OFFSET: f32 = 80.;
    const RECKONING_UP: [f32; 4] = [0., 1., 0., 0.];

    fn yaw(degrees: f32) -> [f32; 3] {
        let radians = degrees.to_radians();
        [radians.sin(), 0., radians.cos()]
    }

    fn spin(carried: bool, rider_step: f32, board_step: f32) -> f32 {
        let mut spin = SpinTracker::new(frame_rows(UP, yaw(0.)), frame_rows(UP, yaw(0.)));
        let mut degrees = 0.;
        for step in 1..=72 {
            let rider = frame_rows(UP, yaw(rider_step * step as f32));
            let board = frame_rows(UP, yaw(board_step * step as f32));
            degrees = spin.update(carried, &rider, &board, RECKONING_UP);
        }
        degrees
    }

    /// With no trick carried, B -- the board on the reckoning frame -- is what is measured:
    /// a full turn in 5-degree steps is two half-turns, whichever way.
    #[test]
    fn an_uncarried_full_turn_is_two_half_turns() {
        for sign in [1.0_f32, -1.0] {
            let degrees = spin(false, sign * 5., sign * 5.);
            assert_eq!(half_turns(degrees, OFFSET).abs(), 2, "{degrees}");
        }
    }

    /// While a trick is carried, B is re-referenced every frame and A, the rider, is banked:
    /// a 360 shove-it under a square rider is no spin, a rider turning over a still board is.
    #[test]
    fn a_carried_trick_measures_the_rider_not_the_board() {
        let shove_it = spin(true, 0., 5.);
        assert!(shove_it.abs() < 2., "{shove_it}");
        assert_eq!(half_turns(shove_it, OFFSET), 0);
        let degrees = spin(true, -5., 0.);
        assert_eq!(half_turns(degrees, OFFSET).abs(), 2, "{degrees}");
    }

    /// 82DAC9D0 scores a jump of more than a quarter turn in one frame as nothing.
    #[test]
    fn a_discontinuity_scores_nothing() {
        let board = frame_rows(UP, yaw(0.));
        let mut spin = SpinTracker::new(frame_rows(UP, yaw(0.)), board);
        let degrees = spin.update(true, &frame_rows(UP, yaw(120.)), &board, RECKONING_UP);
        assert_eq!(degrees, 0.);
    }
}

#[cfg(test)]
mod ground_tests {
    //! The ground collector's revert slot (82DAAC10), its keep-going answer (82DAB2A8) and the
    //! line hold it drives (bit 31 of `[holder+4]+1832`). Needs the owned collections.
    use super::*;

    fn runtime() -> Runtime {
        let root = std::path::PathBuf::from(std::env::var_os("SKATE3_ASSET_ROOT").unwrap());
        let data = skate_data::collections::Collections::load(&root).unwrap();
        Runtime::load(&data).unwrap()
    }

    fn ground(tick: u32, flags: u32, state70: bool) -> Frame {
        Frame {
            tick,
            dt: 1. / 60.,
            category: FilteredCategory::Ground,
            state: 100,
            descriptor: None,
            grind_id: -1,
            flags,
            position: [0.; 3],
            com_velocity: [0., 0., 5.],
            forward: [0., 0., 1.],
            rider_up: [0., 1., 0.],
            rider_forward: [0., 0., 1.],
            board_up: [0., 1., 0.],
            board_forward: [0., 0., 1.],
            reckoning_up: [0., 1., 0.],
            takeoff_normal: [0., 1., 0.],
            wheel_count: 4,
            pump_acceleration: 0.,
            switch: false,
            regular: false,
            fakie: false,
            nollie: false,
            body_flip: false,
            body_flip_side: false,
            hips_position: [0., 1., 0.],
            hips_ground: None,
            hips_surface: 0,
            suspend_air: false,
            landing: Default::default(),
            teleported: false,
            reverting: false,
            manual_block_70: state70,
            request_teleport: false,
            tipslide_326: false,
            half_turns_312: 0,
            runout_78: false,
            flags_2484_31: false,
            retrieving_board_323: false,
        }
    }

    /// 82DAAC10: the side latches from 0x20000000/0x10000000, the carrier (4 `fsrevert`, 5
    /// `bsrevert`) starts on State70's rising edge and is credited through 82DA6198 on its
    /// falling edge -- once, with the carrier's reward.
    #[test]
    #[ignore = "requires owned collections (SKATE3_ASSET_ROOT)"]
    fn the_revert_slot_scores_on_state70s_falling_edge() {
        for (side_flag, id) in [(0x2000_0000, 4), (0x1000_0000, 5)] {
            let mut r = runtime();
            r.advance(ground(0, side_flag, false)).unwrap();
            for tick in 1..20 {
                r.advance(ground(tick, 0, true)).unwrap();
                assert_eq!(r.carriers[3].as_ref().map(|c| c.scorable.id), Some(id));
            }
            assert!(r.session.holder.sequence_tricks().next().is_none());
            let reward = r.carriers[3].as_ref().unwrap().reward;
            let before = r.session.holder.snapshot.accumulated;
            r.advance(ground(20, 0, false)).unwrap();
            println!(
                "revert id {id}: reward {reward} accumulated {} sequence {:?} published {}",
                r.session.holder.snapshot.accumulated,
                r.session.holder.sequence_tricks().collect::<Vec<_>>(),
                r.sequence_score,
            );
            assert!(r.carriers[3].is_none());
            assert!(reward > 0.);
            // 82DA6198 counted it in the sequence history and added the reward to +32. (The
            // repetition count is not checked: with an empty line timer and no carrier left,
            // `settle_line` banks the line and clears repetition on this same frame.)
            assert_eq!(r.session.holder.sequence_tricks().collect::<Vec<_>>(), [(id, 1)]);
            assert_eq!(r.session.holder.snapshot.accumulated, before + reward);
        }
    }

    /// 82DAB2A8 on the ground with nothing carried: only the open-sequence terms can keep it
    /// going, and a closed sequence never reopens on its own.
    #[test]
    #[ignore = "requires owned collections (SKATE3_ASSET_ROOT)"]
    fn ground_keep_going_terms_follow_82dab2a8() {
        let mut r = runtime();
        r.advance(ground(0, 0, false)).unwrap();
        r.ground_frames = 3;
        let f = ground(1, 0, false);
        assert!(!r.sequence_continues(&f, false));
        assert!(!r.sequence_continues(&f, true));
        // pumping: Ground+268 > 0.1 (0x3DCCCCCD), open only
        let pump = Frame { pump_acceleration: 0.2, ..ground(1, 0, false) };
        assert!(r.sequence_continues(&pump, true) && !r.sequence_continues(&pump, false));
        // deck tilted (a ramp, a quarter pipe): 0.65 (0x3F266666) > Motion+80.y, the effective up, open only;
        // flat ground (up.y 1) never holds the line open
        let tilted = Frame { board_up: [0., 0.5, 0.866], ..ground(1, 0, false) };
        assert!(r.sequence_continues(&tilted, true) && !r.sequence_continues(&tilted, false));
        assert!(!r.sequence_continues(&ground(1, 0, false), true), "flat ground closes the line");
        // r7: fewer than three wheels with no recorded tricks
        let wheels = Frame { wheel_count: 2, ..ground(1, 0, false) };
        assert!(r.sequence_continues(&wheels, true) && !r.sequence_continues(&wheels, false));
        // r9: the first frames on the ground (+928 < 3) while open
        r.ground_frames = 2;
        assert!(r.sequence_continues(&f, true) && !r.sequence_continues(&f, false));
        // the pulse bit and the revert latch hold regardless
        let pulse = ground(1, 0x0100_0000, false);
        r.ground_frames = 3;
        assert!(r.sequence_continues(&pulse, false));
        r.revert_latch = true;
        assert!(r.sequence_continues(&f, false));
    }

    /// 82DAB2A8 with +945 set (input +2788 bit 0x40000000, ManualsAreConnectors, set unless a challenge clears it):
    /// fewer than three wheels keep an open sequence going however many tricks it holds -- a manual connects --
    /// until the manual slot has been blocked by State70 for more than six frames (+932).
    #[test]
    #[ignore = "requires owned collections (SKATE3_ASSET_ROOT)"]
    fn a_manual_keeps_a_sequence_with_tricks_open() {
        let mut r = runtime();
        r.advance(ground(0, 0, false)).unwrap();
        r.display_tricks = 1;
        r.ground_frames = 20;
        let two = Frame { wheel_count: 2, ..ground(1, 0, false) };
        assert!(r.sequence_continues(&two, true), "a manual connects");
        assert!(!r.sequence_continues(&two, false), "but opens nothing");
        assert!(!r.sequence_continues(&ground(1, 0, false), true), "four wheels close it");
        r.manual_block_ticks = 7;
        assert!(!r.sequence_continues(&two, true), "blocked for seven frames");
        // +945 clear (a challenge's SetManualAndWalkAsConnector false): only the first eight frames on the ground.
        r.manual_block_ticks = 0;
        r.manuals_connect = false;
        assert!(!r.sequence_continues(&two, true), "clear: the manual does not connect");
        r.ground_frames = 7;
        assert!(r.sequence_continues(&two, true), "clear: the first eight frames do");
    }

    /// The line timer holds exactly while bit 31 is set: a revert opens the sequence, and it
    /// stays open until the ground collector's answer falls and the sequence publishes.
    #[test]
    #[ignore = "requires owned collections (SKATE3_ASSET_ROOT)"]
    fn the_line_hold_is_the_sequence_open_bit() {
        let mut r = runtime();
        r.advance(ground(0, 0, false)).unwrap();
        r.advance(ground(1, 0, false)).unwrap();
        assert!(!r.sequence_active, "an idle ground never opens a sequence");
        r.advance(ground(2, 0x2000_0000, true)).unwrap();
        assert!(r.sequence_active, "the revert latch opens it");
        r.advance(ground(3, 0, false)).unwrap();
        let mut tick = 4;
        while r.sequence_active {
            assert!(tick < 20, "the sequence never closed");
            r.advance(ground(tick, 0, false)).unwrap();
            tick += 1;
        }
        assert!(r.sequence_landed, "closing publishes the sequence on the same frame");
    }

    /// 82DA33E0 takes both timers' drain scales from the active collector (vtable +0x1C/+0x20). The Ground one,
    /// 82DAB528/558, slows them only while its slots pay this frame (+116 > 0): a held carrier that pays nothing,
    /// like the revert while State70 is set, drains at full speed, where "any carrier held" slowed it to 0.2.
    #[test]
    #[ignore = "requires owned collections (SKATE3_ASSET_ROOT)"]
    fn the_ground_timers_slow_only_while_the_slots_pay() {
        let mut r = runtime();
        r.advance(ground(0, 0x2000_0000, false)).unwrap();
        let mut unpaid = 0;
        for tick in 1..20 {
            r.session.line.points = 100.;
            r.advance(ground(tick, 0, true)).unwrap();
            assert!(r.carriers[3].is_some(), "the revert is held");
            let scale = if r.ground_live > 0. { r.data.collector.scalar(0x610) } else { 1. };
            let mut expected = skate_core::scoring::timer::PointTimer { points: 100., expired: false };
            expected.advance(1. / 60., r.data.line_drain, scale, false);
            assert_eq!(r.session.line.points, expected.points, "tick {tick}: live {}", r.ground_live);
            unpaid += usize::from(r.ground_live <= 0.);
        }
        assert!(unpaid > 0, "the revert paid on every frame, so this proves nothing");
    }

    /// Slot 6 online (user 10-24, Spot Battle): an air over a prop -- under the hips probe only mid-air -- pays the
    /// five air metrics x [0x65C] (1.15) and [0x660] (100) * factor * repetition * 1.15 as scorable 237 (82DA8550),
    /// where an air over nothing pays neither. The same prop under the probe within the eight frames before take-off
    /// (82DA7888's ring, merged by 82DA8078) never pays; nine frames before, it does.
    #[test]
    #[ignore = "requires owned collections (SKATE3_ASSET_ROOT)"]
    fn an_online_air_over_a_prop_pays_slot_6() {
        let prop = air::ContextObject {
            matrix: [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [0., 0., 0., 1.]],
            min: [-1., 0., -1., 0.],
            max: [1., 1., 1., 0.],
            id: 1,
            entity: 0,
        };
        // Take-off on tick 20, landing on tick 60.
        let base = runtime();
        let run = |over: &dyn Fn(u32) -> bool| {
            let mut r = base.fresh();
            r.offline = false;
            for tick in 0..=60 {
                let hit = if over(tick) { 1. } else { 0. };
                let probe = Some(([0., 3., 0., 1.], [0., hit, 0., 1.]));
                r.context_slots[6] = air::object_slot(air::near_objects([0., 0., 0., 1.], [prop]), probe);
                let a = (tick as f32 - 20.) / 40.;
                let frame = if (20..60).contains(&tick) {
                    Frame {
                        category: FilteredCategory::Air,
                        state: 200,
                        position: [0., 6. * a * (1. - a), tick as f32 * 0.2],
                        ..ground(tick, 0, false)
                    }
                } else {
                    Frame { position: [0., 0., tick as f32 * 0.2], ..ground(tick, 0, false) }
                };
                r.advance(frame).unwrap();
            }
            (r.session.holder.snapshot.general_pending, r.air_metrics, r.air_factor, r.air_repetition)
        };
        let (plain, metrics, factor, repetition) = run(&|_| false);
        let (over, ..) = run(&|tick| (30..40).contains(&tick));
        let (multiplier, bonus) = (base.data.collector.scalar(0x65C), base.data.collector.scalar(0x660));
        assert_eq!((multiplier, bonus), (f32::from_bits(0x3F93_3333), 100.), "x1.15 and +100");
        assert!(metrics.iter().sum::<f32>() > 0., "the air earned metrics: {metrics:?}");
        let paid = ppc::fmuls(ppc::fmuls(ppc::fmuls(bonus, factor), repetition), multiplier);
        let extra: f32 = metrics.iter().map(|&m| ppc::fmuls(m, multiplier) - m).sum::<f32>() + paid;
        println!("plain {plain} over {over} metrics {metrics:?} extra {extra}");
        assert!((over - plain - extra).abs() < 1e-3 * over.abs().max(1.), "over {over} plain {plain} extra {extra}");
        let (ring, ..) = run(&|tick| (14..18).contains(&tick) || (30..40).contains(&tick));
        assert_eq!(ring, plain, "under it within eight frames of take-off");
        let (early, ..) = run(&|tick| (8..12).contains(&tick) || (30..40).contains(&tick));
        assert_eq!(early, over, "nine or more frames before take-off");
    }

    /// Spot Battle parity (user 10-24): retail's Lua gets 52 for a 52.6 reward; rounding gave 53.
    #[test]
    #[ignore = "requires owned collections (SKATE3_ASSET_ROOT)"]
    fn the_lua_sequence_score_is_cut_toward_zero() {
        let mut r = runtime();
        r.sequence_score = 52.6;
        assert_eq!(r.lua_sequence_score(), 52);
    }

    /// Playtest run12 (Manuals 101): Skate 1's trickcomplete for a manual comes when it ends. It is announced while
    /// held, and the old announce-time post failed the lesson's "stay in the manual" substate, which fails on any trick.
    #[test]
    #[ignore = "requires owned collections (SKATE3_ASSET_ROOT)"]
    fn a_manual_completes_when_it_ends_not_when_announced() {
        let mut r = runtime();
        r.advance(ground(0, 0, false)).unwrap();
        let mut announced = false;
        for tick in 1..60 {
            r.advance(ground(tick, 0x0800_0000, false)).unwrap();
            announced |= r.announced.contains(&"nosemanual");
            assert!(r.completed.is_empty(), "held at tick {tick}");
        }
        assert!(announced, "announced while held");
        r.advance(ground(60, 0, false)).unwrap();
        assert_eq!(r.completed, ["nosemanual"], "completed as it ends");
        r.advance(ground(61, 0, false)).unwrap();
        assert!(r.completed.is_empty(), "for that tick only");
    }

    fn named(name: &str) -> Option<AttributeName> {
        Some(skate_core::animation::skeleton_input::name::encode(name.as_bytes()))
    }

    /// 82DA9D68: a grind-grab descriptor (layout +148 set) takes the grind carrier from the
    /// physics grind id; any other descriptor leaves the physics id in place.
    #[test]
    #[ignore = "requires owned collections (SKATE3_ASSET_ROOT)"]
    fn a_grind_grab_descriptor_takes_the_grind_carrier() {
        let grind = |tick, descriptor| Frame {
            category: FilteredCategory::Grind,
            state: 300,
            grind_id: 23,
            descriptor,
            ..ground(tick, 0x0100_0000, false)
        };
        let mut r = runtime();
        r.advance(grind(0, named("fs_50_50"))).unwrap();
        assert_eq!(r.carriers[0].as_ref().map(|c| c.scorable.id), Some(23));
        r.advance(grind(1, named("fs_50_50_fsgrab"))).unwrap();
        assert_eq!(r.carriers[0].as_ref().map(|c| c.scorable.id), Some(193));
        r.advance(grind(2, named("kickflip"))).unwrap();
        assert_eq!(r.carriers[0].as_ref().map(|c| c.scorable.id), Some(23));
    }

    /// The grab bit 0x02000000: on the ground only the coffin (60) fills slot 2 (82DAAF1C), and
    /// the gnd_* grabs (67..69) never become a descriptor at all (82DAC5D0).
    #[test]
    #[ignore = "requires owned collections (SKATE3_ASSET_ROOT)"]
    fn the_grab_bit_admits_the_ground_coffin_and_drops_ground_grabs() {
        let mut r = runtime();
        r.advance(Frame { descriptor: named("Coffin"), ..ground(0, 0x0200_0000, false) }).unwrap();
        assert_eq!(r.carriers[2].as_ref().map(|c| c.scorable.id), Some(60));
        let mut r = runtime();
        let air = |tick| Frame {
            category: FilteredCategory::Air,
            state: 200,
            descriptor: named("gnd_fsgrab"),
            ..ground(tick, 0x0200_0000, false)
        };
        r.advance(air(0)).unwrap();
        r.advance(air(1)).unwrap();
        assert!(r.carriers.iter().flatten().all(|c| c.scorable.id != 69));
    }

    /// 825E4F40 state 1 against `M+164`: an air footplant planted again in the same air is
    /// announced again, but renames the entry (event 2) instead of adding a second one.
    #[test]
    #[ignore = "requires owned collections (SKATE3_ASSET_ROOT)"]
    fn the_same_trick_announced_again_renames_its_entry() {
        // The plant's ScoringTrick sets the packet's 0x01000000 while it plays (82DAC498 reads the name only then).
        let air = |tick, descriptor| Frame {
            category: FilteredCategory::Air,
            state: 601,
            descriptor,
            ..ground(tick, 0x0100_0000, false)
        };
        let plant = |r: &mut Runtime, tick: &mut u32| loop {
            r.advance(air(*tick, named("fsgrab_fs_footplant"))).unwrap();
            *tick += 1;
            if !r.announced.is_empty() {
                break (r.new_trick, r.modified_trick);
            }
            assert!(*tick < 600, "never announced");
        };
        // The module scores nothing before its first frame on the board (82DA3DD8's +0x83).
        let (mut r, mut tick) = (runtime(), 1);
        r.advance(ground(0, 0, false)).unwrap();
        assert_eq!(plant(&mut r, &mut tick), (true, false), "the first plant is a new entry");
        r.advance(air(tick, None)).unwrap();
        tick += 1;
        assert_eq!(plant(&mut r, &mut tick), (false, true), "the second renames it");
    }
}
