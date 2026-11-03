//! Shared by the disc tests: a booted host, the spike's scripted pad and the archive-read log.

pub mod names;
pub mod scorables;

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

use skate2_formats::big::BigEntryKey;
use skate2_formats::mount::{ArchiveRead, SKATE2_ARCHIVES, observe_misses, observe_reads};
use skate2_physics_host::{Host, unclean_environment};
use skate2_physics_protocol::{Boot, Pad};

/// A host booted on the data folder that the variable `root_var` names.
pub fn host(root_var: &str) -> Host {
    host_with(root_var, &Boot::default())
}

/// A host booted with `boot` on the data folder that the variable `root_var` names.
pub fn host_with(root_var: &str, boot: &Boot) -> Host {
    // The game starts the host with a clean environment; the port reads `SKATE2_*` variables in this process too.
    let vars = std::env::vars_os().map(|(name, value)| {
        (name.to_string_lossy().into_owned(), value.to_string_lossy().into_owned())
    });
    if let Some(name) = unclean_environment(vars) {
        panic!("unset {name}: the host runs without SKATE2_* variables");
    }
    let root = std::env::var_os(root_var).unwrap_or_else(|| panic!("set {root_var} to a Skate 2 data folder"));
    Host::boot(Path::new(&root), boot).unwrap_or_else(|why| panic!("{root_var}: {why}"))
}

const A: u16 = 0x1000;
/// The right-stick patterns the spike read from the session (`controlled_right_pattern_points`), as its run printed
/// them (`runs/q3-min.out`; `{:?}` prints the shortest text that reads back to the same `f32`).
const OLLIE: [[f32; 2]; 2] = [[-0.245_714, 0.942_857], [0.371_429, -0.908_571]];
const KICKFLIP: [[f32; 2]; 2] = [[-0.028_571, 0.691_429], [0.908_571, -0.417_143]];
/// Ticks the antic point is held before a flick.
const HOLD: u64 = 2;

/// The spike's `Script`, variant 'a' (`s2spike/src/main.rs` 153-224), for its tick `tick` (counted from 0): push x4,
/// ollie at 6.0 s, kickflip at 8.0 s, two pushes, powerslide at 10.6 s, then coast into the wall.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn spike_script(tick: u32) -> Pad {
    let tick = u64::from(tick);
    let s = |sec: f64| (sec * 60.0) as u64;
    for at in [1.0, 1.9, 2.8, 3.7, 9.0, 9.7] {
        if (s(at)..s(at) + 12).contains(&tick) {
            return Pad { buttons: A, ..Pad::default() };
        }
    }
    if let Some(pad) = flick(OLLIE, s(6.0), tick) {
        return pad;
    }
    if let Some(pad) = flick(KICKFLIP, s(8.0), tick) {
        return pad;
    }
    if (s(10.6)..s(11.8)).contains(&tick) {
        return Pad { right: [i16::MAX, 0], ..Pad::default() };
    }
    Pad::default()
}

/// A flick trick like the port app's `--live-ollie-demo`: the antic point for `HOLD` ticks, one neutral tick, then
/// the flick point for two ticks.
fn flick(points: [[f32; 2]; 2], start: u64, tick: u64) -> Option<Pad> {
    let [antic, flick] = points;
    let flick_start = start + HOLD + 1;
    if (start..start + HOLD).contains(&tick) {
        Some(right_point(antic))
    } else if (flick_start..flick_start + 2).contains(&tick) {
        Some(right_point(flick))
    } else {
        None
    }
}

/// The spike's `Script`, variant 'c' with its left stick at (0.5, -0.85) (`runs/run3-down.out`), for its tick `tick`
/// (counted from 0): push x4, then the powerslide from 4.8 s to 6.0 s, then coast.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn slide_script(tick: u32) -> Pad {
    let tick = u64::from(tick);
    let s = |sec: f64| (sec * 60.0) as u64;
    if [1.0, 1.9, 2.8, 3.7].iter().any(|&at| (s(at)..s(at) + 12).contains(&tick)) {
        return Pad { buttons: A, ..Pad::default() };
    }
    if (s(4.8)..s(6.0)).contains(&tick) {
        return Pad { left: [stick(0.5), stick(-0.85)], ..Pad::default() };
    }
    Pad::default()
}

/// The spike's pushes: A held for 12 ticks.
#[must_use]
pub fn push() -> Pad {
    Pad { buttons: A, ..Pad::default() }
}

/// A stick axis as the spike's variant 'c' wrote it: `value` of full scale, `y` up.
#[allow(clippy::cast_possible_truncation)]
fn stick(value: f32) -> i16 {
    (value * 32767.0).round().clamp(-32768.0, 32767.0) as i16
}

/// The port app's `live_skating_client::demo_right_point`: inverts the source radial stick conversion. A pattern
/// point's `y` is down (the pad's `y` is up).
#[allow(clippy::cast_possible_truncation)]
pub fn right_point(point: [f32; 2]) -> Pad {
    let magnitude = point[1].mul_add(point[1], point[0] * point[0]).sqrt();
    let axis = |value: f32| -> i16 {
        if magnitude == 0.0 {
            return 0;
        }
        let raw_radius = 0.25 + magnitude / f32::from_bits(0x3fb6_db6e);
        ((value / magnitude * raw_radius * 32768.0).round()).clamp(-32768.0, 32767.0) as i16
    };
    Pad { right: [axis(point[0]), axis(-point[1])], ..Pad::default() }
}

/// The logs `record_reads` writes to. The port reads on its own threads too, so a read cannot be told apart by test
/// when tests run side by side: each read goes to every log opened in this process. Only the union is used.
static LOGS: Mutex<Vec<File>> = Mutex::new(Vec::new());

/// Appends `archive \t key \t entry` for every archive read from now on to `S2HOST_TEST_READS/<test>.tsv`, and
/// `!notfound \t path` for every path read that no mounted archive holds (see `log_miss`).
///
/// The archive is its disc path (`data\big\anim.big`). The key is the entry name for a named table and `0x%08x` for a
/// hashed one. The entry is the name the read reports, which for a hashed table is its hash again: the port's
/// observer gets only the table entry, not the path that was looked up.
pub fn record_reads(test: &str) {
    let dir = std::env::var_os("S2HOST_TEST_READS").unwrap_or_else(|| panic!("set S2HOST_TEST_READS to a folder"));
    let path = Path::new(&dir).join(format!("{test}.tsv"));
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .unwrap_or_else(|why| panic!("{}: {why}", path.display()));
    LOGS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(log);
    observe_reads(log_read);
    observe_misses(log_miss);
}

/// A path read that no mounted archive holds (`MountError::EntryNotFound`): `!notfound \t path`. The stage test counts
/// these lines.
fn log_miss(path: &str) {
    for log in LOGS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter_mut() {
        log.write_all(format!("!notfound\t{path}\n").as_bytes()).expect("write the read log");
    }
}

fn log_read(read: &ArchiveRead<'_>) {
    let file = read.archive.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let archive = SKATE2_ARCHIVES
        .iter()
        .map(|spec| spec.relative_path)
        .find(|relative| relative.rsplit('\\').next().is_some_and(|name| name.eq_ignore_ascii_case(&file)))
        .map_or_else(|| read.archive.display().to_string(), str::to_owned);
    let key = match &read.entry.key {
        BigEntryKey::Name(name) => name.clone(),
        BigEntryKey::Hash(hash) => format!("0x{hash:08x}"),
    };
    let line = format!("{archive}\t{key}\t{key}\n");
    for log in LOGS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter_mut() {
        log.write_all(line.as_bytes()).expect("write the read log");
    }
}
