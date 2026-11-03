//! Headless end-to-end check of the retail player-sound path: the exact worker setup
//! ([`super::prepare`]) and frame ([`super::sound::PlayerSound::frame`]) the game runs, fed
//! synthetic physics ticks, rendered block by block.
//!
//! `cargo test -p skate-game --bin skate3rust --locked -- --ignored headless --nocapture`
//! (assets from `SKATE_ASSETS`, defaulting to the owner's install). Optional `SKATE_HEADLESS_WAV`
//! writes the stereo output.

use crate::skate_audio::{PlayerAudioObservation, RetailAudioInputs};

const DEFAULT_ASSETS: &str = r"C:\s3\installations\70eda9dc4644496d81ae73af95ff4285\assets";

/// A grounded roll at `speed` m/s straight along +X on four wheels.
fn rolling(tick: u64, speed: f32) -> PlayerAudioObservation {
    let mut retail = RetailAudioInputs::default();
    retail.dt = 1.0 / 60.0;
    retail.ground_speed = speed;
    retail.com_velocity = [speed, 0.0, 0.0];
    retail.linear_velocity = [speed, 0.0, 0.0];
    retail.wheel_count = 4;
    retail.wheel_contacts = [true; 4];
    retail.truck_contacts = [true; 2];
    retail.state_category = 100;
    retail.state = 100;
    retail.motion_200 = 1.0;
    retail.deck_rows = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    retail.effective_deck_up = [0.0, 1.0, 0.0];
    retail.ground_normal = [0.0, 1.0, 0.0];
    retail.wheel_contact_normals = [[0.0, 1.0, 0.0]; 4];
    retail.wheel_audio_surfaces = [3; 4];
    retail.scorable_id = -1;
    retail.deck_forward = [1.0, 0.0, 0.0];
    // A follow camera three metres behind and above, looking along the roll.
    retail.camera = Some(([1.0, 0.0, 0.0], [-3.0, 1.5, 0.0]));
    PlayerAudioObservation {
        tick,
        state: 100,
        board_speed: speed,
        rider_speed: speed,
        grounded: true,
        grinding: false,
        grind_family: 0,
        grind_substate: 0,
        grind_audio_surface: 0,
        grind_impact_speed: 0.0,
        wiping_out: false,
        landed: false,
        landing_impact_speed: 0.0,
        landing_clean: false,
        landing_sketchy: false,
        landing_type: 0,
        landing_spin: 0.0,
        landing_sideways_speed: 0.0,
        footstep_strength: 0.0,
        footstep_bone: 0,
        foot_push_speed: 0.0,
        feet_supported: [false; 2],
        foot_surface: 0,
        contact_count: 4,
        powersliding: false,
        trick_id: None,
        trick_identifier: None,
        animation_name: None,
        riding_switch: false,
        riding_fakie: false,
        nollie: false,
        wheel_surface: 3,
        events: Vec::new(),
        frontend_sounds: Vec::new(),
        volumes: [1.0; 3],
        map: String::new(),
        skate2: false,
        district: None,
        retail,
        remote_records: Vec::new(),
        watched: None,
        traffic: None,
        dmos: Vec::new(),
        riders: Vec::new(),
    }
}

/// The voice-graph arena must plateau. `build_graph` takes a whole graph out of the arena in one
/// allocation and only the deferred teardown (`stop_player`) can give it back; when that free was
/// missing, every one-shot cost a graph for the rest of the session and a long playtest ended in
/// "authored audio guest heap exhausted" with the audio worker dead and the game still running.
///
///     cargo test -p skate-game --bin skate3rust -- --ignored the_voice_arena --nocapture
#[test]
#[ignore = "needs the owner's assets"]
fn the_voice_arena_plateaus_under_repeated_voices() {
    use skate_audio_core::authored::oneshot::{OneshotBus, OneshotVoice};
    let assets = std::env::var_os("SKATE_ASSETS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| DEFAULT_ASSETS.into());
    let super::Prepared { mut runtime, .. } =
        super::prepare(&assets).expect("prepare the retail player-sound worker");
    // Any installed sample will do; the graph is what is being measured, not the audio.
    let sample = runtime.any_installed_sample().expect("some decoded sample");
    let mut marks = Vec::new();
    for round in 0..40 {
        let mut handle = runtime
            .play_oneshot(&OneshotVoice {
                sample,
                gain: 0.5,
                pitch: 1.0,
                delay: 0.0,
                pan: 0.0,
                bus: OneshotBus::Default,
            })
            .expect("open a one-shot");
        // Run it to the end, rendering blocks as the worker does: the teardown is *deferred*
        // through the command ring, and the ring is only drained while blocks are being rendered.
        for _ in 0..600 {
            let live = runtime.tick_oneshot(&mut handle, 1.0 / 60.0).expect("tick");
            runtime.pump_once().expect("render one block");
            if !live {
                break;
            }
        }
        runtime.release_oneshot(&mut handle).expect("release");
        // A few more blocks so the stop command this release queued is drained too.
        for _ in 0..4 {
            runtime.pump_once().expect("render one block");
        }
        if round % 10 == 9 {
            marks.push(runtime.arena_usage());
        }
    }
    println!("arena (high-water bytes, live blocks) after each 10 voices: {marks:?}");
    let (first, last) = (marks[0], *marks.last().unwrap());
    // The high-water mark must stop moving: the graph each voice takes out of the arena is given
    // back by the deferred teardown and reused by the next voice.
    assert_eq!(
        first.0, last.0,
        "arena high-water grew from {} to {} bytes across 30 further voices",
        first.0, last.0
    );
    // Known residual, and much smaller: the 128-byte decoder `open` allocates per voice is keyed
    // by the voice's stream but freed by the graph teardown's child walk, so it survives when the
    // two do not match. It costs 128 bytes a voice against a 64 MB arena rather than a whole
    // graph, and the free list absorbs it, but it is not zero.
    let leaked = last.1 - first.1;
    println!("residual live blocks across 30 voices: {leaked}");
    assert!(
        leaked <= 30,
        "more than one block per voice survives teardown"
    );
}

#[test]
#[ignore = "needs the owner's assets; run explicitly"]
fn headless_rolling_through_the_retail_path() {
    use skate_audio_core::authored::{PCM_CHANNELS, PCM_FRAMES_PER_BLOCK};
    let assets = std::env::var_os("SKATE_ASSETS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| DEFAULT_ASSETS.into());
    let started = std::time::Instant::now();
    let super::Prepared {
        mut runtime,
        mut sound,
        ..
    } = super::prepare(&assets).expect("prepare the retail player-sound worker");
    eprintln!("prepared in {:.1} s", started.elapsed().as_secs_f32());

    // (seconds, speed): idle, slow, cruise, fast, stop.
    let script: [(u32, f32); 5] = [(2, 0.0), (3, 2.0), (3, 5.0), (3, 9.0), (2, 0.0)];
    let mut wav: Vec<f32> = Vec::new();
    let mut tick = 0u64;
    let mut blocks_owed = 0.0f64;
    let blocks_per_frame = 48_000.0 / f64::from(PCM_FRAMES_PER_BLOCK) / 60.0;
    let mut worst_ms = 0.0f64;
    for (seconds, speed) in script {
        let (mut peak, mut energy, mut samples, mut silent, mut blocks) =
            (0f32, 0f64, 0usize, 0, 0);
        for _ in 0..seconds * 60 {
            sound
                .frame(&mut runtime, &rolling(tick, speed))
                .expect("player-sound frame");
            tick += 1;
            blocks_owed += blocks_per_frame;
            while blocks_owed >= 1.0 {
                blocks_owed -= 1.0;
                let begin = std::time::Instant::now();
                let native = runtime.pump_once().expect("render one block");
                worst_ms = worst_ms.max(begin.elapsed().as_secs_f64() * 1e3);
                assert_eq!(
                    native.len(),
                    PCM_FRAMES_PER_BLOCK as usize * usize::from(PCM_CHANNELS)
                );
                let stereo = super::downmix(&native);
                let block_peak = stereo.iter().fold(0f32, |p, s| p.max(s.abs()));
                peak = peak.max(block_peak);
                energy += stereo
                    .iter()
                    .map(|s| f64::from(*s) * f64::from(*s))
                    .sum::<f64>();
                samples += stereo.len();
                // Retail fades rolling in over a few frames; judge continuity after 0.25 s.
                if block_peak < 1.0e-5 && blocks >= 47 {
                    silent += 1;
                }
                blocks += 1;
                wav.extend_from_slice(&stereo);
            }
        }
        let stats = runtime.stats();
        if let Some((ctrl, c)) = sound.controls("SkateBoard") {
            use super::components::Controls;
            let levels: Vec<u32> = (0..20).map(|id| c.level(id)).collect();
            let raw: Vec<u32> = (0..20).map(|id| c.raw(id)).collect();
            let inputs: Vec<u32> = (0..16)
                .map(|id| runtime.mixmap_get(ctrl, id).unwrap_or(0))
                .collect();
            eprintln!(
                "  SkateBoard {ctrl:#010x} levels {levels:?}
  raw {raw:?}
  inputs {inputs:?}"
            );
        }
        eprintln!(
            "speed {speed:>4.1} m/s for {seconds} s: peak {:>6.1} dBFS, rms {:>6.1} dBFS, silent blocks {silent}/{blocks}, voices opened {} live {}",
            20.0 * peak.max(1e-9).log10(),
            10.0 * (energy / samples.max(1) as f64).max(1e-18).log10(),
            stats.voices_opened,
            stats.live_voices,
        );
        if speed > 1.0 {
            assert_eq!(
                silent, 0,
                "rolling at {speed} m/s must never drop to silence"
            );
        }
    }
    eprintln!("worst block render {worst_ms:.2} ms (budget 5.33 ms)");
    if let Some(path) = std::env::var_os("SKATE_HEADLESS_WAV") {
        write_wav(std::path::Path::new(&path), &wav).expect("write wav");
        eprintln!("wrote {}", std::path::Path::new(&path).display());
    }
}

/// How loud a revert's skid is against the roll it interrupts. RevertGround (102) with State+66
/// set drives the skid counter to its +5/frame ramp; retail's capture (music off) puts a revert
/// at 3–6 m/s **+10.2 dB** over rolling at the same speed, and a powerslide +4.1 dB.
#[test]
#[ignore = "needs the owner's assets; run explicitly"]
fn headless_revert_skid_level() {
    use skate_audio_core::authored::PCM_FRAMES_PER_BLOCK;
    let assets = std::env::var_os("SKATE_ASSETS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| DEFAULT_ASSETS.into());
    let super::Prepared {
        mut runtime,
        mut sound,
        ..
    } = super::prepare(&assets).expect("prepare the retail player-sound worker");
    let blocks_per_frame = 48_000.0 / f64::from(PCM_FRAMES_PER_BLOCK) / 60.0;
    let (mut tick, mut owed) = (0u64, 0.0f64);
    // (label, frames, reverting)
    let script = [
        ("settle", 120, false),
        ("roll", 60, false),
        ("revert", 40, true),
        ("after", 60, false),
    ];
    for (label, frames, reverting) in script {
        let (mut energy, mut samples, mut peak) = (0f64, 0usize, 0f32);
        for _ in 0..frames {
            let mut o = rolling(tick, 5.0);
            // Deck X across the roll, so rolling has retail's slip word of 1 rather than 11.
            o.retail.deck_rows = [[0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]];
            if reverting {
                o.state = 102;
                o.retail.state = 102;
                o.retail.state_flags[66 - 52] = true;
            }
            sound.frame(&mut runtime, &o).expect("player-sound frame");
            tick += 1;
            owed += blocks_per_frame;
            while owed >= 1.0 {
                owed -= 1.0;
                let stereo = super::downmix(&runtime.pump_once().expect("render one block"));
                peak = stereo.iter().fold(peak, |p, s| p.max(s.abs()));
                energy += stereo.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>();
                samples += stereo.len();
            }
        }
        eprintln!(
            "{label:>6} at 5 m/s: rms {:>6.1} dBFS, peak {:>6.1} dBFS",
            10.0 * (energy / samples.max(1) as f64).max(1e-18).log10(),
            20.0 * peak.max(1e-9).log10(),
        );
    }
}

/// Airborne: no wheels down, in the air state, with a hop's air timing.
fn airborne(tick: u64, speed: f32, elapsed: f32, jump_height: f32) -> PlayerAudioObservation {
    let mut observation = rolling(tick, speed);
    observation.state = 201;
    observation.grounded = false;
    observation.contact_count = 0;
    observation.retail.state = 201;
    observation.retail.state_category = 200;
    observation.retail.wheel_count = 0;
    observation.retail.wheel_contacts = [false; 4];
    observation.retail.truck_contacts = [false; 2];
    observation.retail.in_known_air = true;
    observation.retail.air_time_in_state = elapsed;
    observation.retail.air_time_until_landing = (0.7 - elapsed).max(0.0);
    observation.retail.air_jump_height = jump_height;
    observation
}

/// A hop and its landing must make the contact one-shots (`sub_824B9CC8` pops, `sub_824BA630`
/// landing impact) audible: they are bank voices from `Skate_Collisions.bnk`, not authored patches.
#[test]
#[ignore = "needs the owner's assets; run explicitly"]
fn headless_landing_impact_is_audible() {
    use skate_audio_core::authored::{PCM_CHANNELS, PCM_FRAMES_PER_BLOCK};
    let assets = std::env::var_os("SKATE_ASSETS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| DEFAULT_ASSETS.into());
    let super::Prepared {
        mut runtime,
        mut sound,
        ..
    } = super::prepare(&assets).expect("prepare the retail player-sound worker");
    let mut tick = 0u64;
    let mut blocks_owed = 0.0f64;
    let blocks_per_frame = 48_000.0 / f64::from(PCM_FRAMES_PER_BLOCK) / 60.0;
    // Roll, hop for 0.7 s, then land on all four and roll on.
    let script: Vec<PlayerAudioObservation> = (0..60)
        .map(|i| rolling(i, 5.0))
        .chain((0..42).map(|i| airborne(60 + i, 5.0, i as f32 / 60.0, 1.2)))
        .chain((0..120).map(|i| rolling(102 + i, 5.0)))
        .collect();
    let mut peaks = Vec::new();
    let mut natives = Vec::new();
    let mut matrices = Vec::new();
    for observation in script {
        sound
            .frame(&mut runtime, &observation)
            .expect("player-sound frame");
        tick += 1;
        blocks_owed += blocks_per_frame;
        // Three meters, because they answer different questions. The native peak is what the
        // retail recomp's own output pass logs, so it is the one that compares with a capture;
        // the matrix peak is the console fold-down before the clamp, which says whether a landing
        // runs into full scale; the device peak is what is heard.
        let (mut frame_peak, mut frame_native, mut frame_matrix) = (0.0f32, 0.0f32, 0.0f32);
        while blocks_owed >= 1.0 {
            blocks_owed -= 1.0;
            let native = runtime.pump_once().expect("render one block");
            assert_eq!(
                native.len(),
                PCM_FRAMES_PER_BLOCK as usize * usize::from(PCM_CHANNELS)
            );
            for sample in &native {
                frame_native = frame_native.max(sample.abs());
            }
            for frame in native.chunks_exact(usize::from(PCM_CHANNELS)) {
                for (front, surround) in [(frame[0], frame[4]), (frame[1], frame[5])] {
                    frame_matrix = frame_matrix.max(super::fold_side(front, frame[2], surround).abs());
                }
            }
            frame_peak = super::downmix(&native)
                .iter()
                .fold(frame_peak, |peak, sample| peak.max(sample.abs()));
        }
        peaks.push((tick, frame_peak));
        natives.push(frame_native);
        matrices.push(frame_matrix);
    }
    // The airborne stretch is frames 61..102; the landing lands at 103.
    let air = peaks[70..100].iter().fold(0f32, |p, (_, v)| p.max(*v));
    let landing = peaks[102..130].iter().fold(0f32, |p, (_, v)| p.max(*v));
    let db = |v: f32| 20.0 * v.max(1e-9).log10();
    let rolling_native = natives[20..60].iter().fold(0f32, |p, v| p.max(*v));
    let landing_native = natives[102..130].iter().fold(0f32, |p, v| p.max(*v));
    let landing_matrix = matrices[102..130].iter().fold(0f32, |p, v| p.max(*v));
    // Retail, metered at its own output pass with its music off: rolling -22.9 dBFS, landing
    // +0.4 dBFS, i.e. +23.3 dB of impact over the bed and a landing that runs past unity.
    eprintln!(
        "native rolling {:.1} dBFS, native landing {:.1} dBFS (+{:.1} dB; retail +23.3), matrix landing {:.1} dBFS",
        db(rolling_native),
        db(landing_native),
        db(landing_native) - db(rolling_native),
        db(landing_matrix),
    );
    eprintln!(
        "air peak {:.1} dBFS, landing peak {:.1} dBFS",
        db(air),
        db(landing),
    );
    assert!(
        landing > air * 1.5,
        "the landing impact must be clearly louder than the airborne stretch: {landing} vs {air}"
    );
}

fn write_wav(path: &std::path::Path, stereo: &[f32]) -> std::io::Result<()> {
    use std::io::Write;
    let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
    let data = (stereo.len() * 2) as u32;
    out.write_all(b"RIFF")?;
    out.write_all(&(36 + data).to_le_bytes())?;
    out.write_all(b"WAVEfmt ")?;
    out.write_all(&16u32.to_le_bytes())?;
    out.write_all(&1u16.to_le_bytes())?;
    out.write_all(&2u16.to_le_bytes())?;
    out.write_all(&48_000u32.to_le_bytes())?;
    out.write_all(&(48_000u32 * 4).to_le_bytes())?;
    out.write_all(&4u16.to_le_bytes())?;
    out.write_all(&16u16.to_le_bytes())?;
    out.write_all(b"data")?;
    out.write_all(&data.to_le_bytes())?;
    for sample in stereo {
        out.write_all(&((sample.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())?;
    }
    out.flush()
}

/// Replay one skid's exact messages from a playtest trace through the headless runtime, one game
/// frame at a time, so a patch decision seen in game (which voices it opens) can be reproduced and
/// bisected. `SKATE_REPLAY_TRACE=<trace>`, `SKATE_REPLAY_FROM` / `_TO` = frames (the post at or
/// before `FROM` is used), `SKATE_REPLAY_ZERO=3,9` zeroes those update words and `SKATE_REPLAY_SET=7=3999` pins one (only within `SKATE_REPLAY_SET_FROM..=_TO` if given). Run with
/// `SKATE_AUDIO_TRACE` set to read the `OP` lines it produces.
#[test]
#[ignore = "needs the owner's assets and a playtest trace; run explicitly"]
fn headless_replay_skid_from_trace() {
    use skate_audio_core::authored::PCM_FRAMES_PER_BLOCK;
    let (Ok(path), Ok(from), Ok(to)) = (
        std::env::var("SKATE_REPLAY_TRACE"),
        std::env::var("SKATE_REPLAY_FROM"),
        std::env::var("SKATE_REPLAY_TO"),
    ) else {
        eprintln!("skipped: set SKATE_REPLAY_TRACE, SKATE_REPLAY_FROM and SKATE_REPLAY_TO");
        return;
    };
    let (from, to): (u64, u64) = (from.parse().unwrap(), to.parse().unwrap());
    let zero: Vec<usize> = std::env::var("SKATE_REPLAY_ZERO")
        .unwrap_or_default()
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    // `SKATE_REPLAY_SET=7=3999,10=46` pins update words to constants.
    let set: Vec<(usize, u32)> = std::env::var("SKATE_REPLAY_SET")
        .unwrap_or_default()
        .split(',')
        .filter_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            Some((k.trim().parse().ok()?, v.trim().parse().ok()?))
        })
        .collect();
    let words = |line: &str| -> Vec<u32> {
        line.split('|')
            .nth(1)
            .unwrap()
            .split_whitespace()
            .map(|w| u32::from_str_radix(w, 16).unwrap())
            .collect()
    };
    let (mut post, mut updates) = (None, Vec::new());
    let text = String::from_utf8_lossy(&std::fs::read(&path).expect("read trace")).into_owned();
    for line in text.lines() {
        let mut f = line.split_whitespace();
        let (Some(_), Some(frame), Some(tag)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        let Ok(frame) = frame.parse::<u64>() else {
            continue;
        };
        if !line.contains("Class_wheels_skid") || frame > to {
            continue;
        }
        if tag == "PO" && frame <= from {
            post = Some((frame, words(line)));
            updates.clear();
        } else if tag == "UP" && post.is_some() {
            updates.push((frame, words(line)));
        }
    }
    let (posted_at, post) = post.expect("a skid post at or before FROM");
    let assets = std::env::var_os("SKATE_ASSETS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| DEFAULT_ASSETS.into());
    let super::Prepared { mut runtime, .. } =
        super::prepare(&assets).expect("prepare the retail player-sound worker");
    let blocks_per_frame = 48_000.0 / f64::from(PCM_FRAMES_PER_BLOCK) / 60.0;
    let mut owed = 0.0f64;
    let mut pump = |runtime: &mut skate_audio_core::authored::AuthoredRuntime, owed: &mut f64| {
        *owed += blocks_per_frame;
        while *owed >= 1.0 {
            *owed -= 1.0;
            runtime.pump_once().expect("render one block");
        }
    };
    super::trace::frame(posted_at);
    let handle = runtime
        .post("Class_wheels_skid", &post[..18])
        .expect("post the skid");
    pump(&mut runtime, &mut owed);
    for (frame, mut w) in updates {
        for &i in &zero {
            w[i] = 0;
        }
        let (set_from, set_to) = (
            std::env::var("SKATE_REPLAY_SET_FROM")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            std::env::var("SKATE_REPLAY_SET_TO")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(u64::MAX),
        );
        if (set_from..=set_to).contains(&frame) {
            for &(i, v) in &set {
                w[i] = v;
            }
        }
        super::trace::frame(frame);
        runtime.redeliver(handle, &w[..18]).expect("redeliver");
        pump(&mut runtime, &mut owed);
    }
    eprintln!("replayed skid posted at {posted_at} through {to}");
}

/// Replay the four `Class_Seams` packets from a trace (ours, or a retail capture converted to
/// this format), one frame at a time, to see which seam voices the patch opens for them.
/// `SKATE_REPLAY_TRACE=<trace>` and `SKATE_REPLAY_TO=<frame>`; `SKATE_REPLAY_SET=13=3` pins words.
/// Run with `SKATE_AUDIO_TRACE` set to read the `OP` lines.
#[test]
#[ignore = "needs the owner's assets and a trace; run explicitly"]
fn headless_replay_seams_from_trace() {
    use skate_audio_core::authored::PCM_FRAMES_PER_BLOCK;
    let (Ok(path), Ok(to)) = (
        std::env::var("SKATE_REPLAY_TRACE"),
        std::env::var("SKATE_REPLAY_TO"),
    ) else {
        eprintln!("skipped: set SKATE_REPLAY_TRACE and SKATE_REPLAY_TO");
        return;
    };
    let to: u64 = to.parse().unwrap();
    let set: Vec<(usize, u32)> = std::env::var("SKATE_REPLAY_SET")
        .unwrap_or_default()
        .split(',')
        .filter_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            Some((k.trim().parse().ok()?, v.trim().parse().ok()?))
        })
        .collect();
    let words = |line: &str| -> Vec<u32> {
        line.split('|').nth(1).unwrap().split_whitespace()
            .map(|w| u32::from_str_radix(w, 16).unwrap()).collect()
    };
    let text = String::from_utf8_lossy(&std::fs::read(&path).expect("read trace")).into_owned();
    let assets = std::env::var_os("SKATE_ASSETS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| DEFAULT_ASSETS.into());
    let super::Prepared { mut runtime, .. } =
        super::prepare(&assets).expect("prepare the retail player-sound worker");
    let blocks_per_frame = 48_000.0 / f64::from(PCM_FRAMES_PER_BLOCK) / 60.0;
    let mut owed = 0.0f64;
    let extra_draws: u32 = std::env::var("SKATE_REPLAY_DRAWS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut dumped = 0;
    let mut handles: [Option<(u32, Vec<u32>)>; 4] = Default::default();
    let mut current = None;
    for line in text.lines() {
        let mut f = line.split_whitespace();
        let (Some(_), Some(frame), Some(tag)) = (f.next(), f.next(), f.next()) else { continue };
        let Ok(frame) = frame.parse::<u64>() else { continue };
        if !line.contains(" Class_Seams ") || frame > to {
            continue;
        }
        if current != Some(frame) {
            if current.is_some() {
                for _ in 0..extra_draws {
                    runtime.advance_counter_for_test();
                }
                owed += blocks_per_frame;
                while owed >= 1.0 {
                    owed -= 1.0;
                    if let Ok(dir) = std::env::var("SKATE_REPLAY_DUMP") {
                        let frame = current.unwrap();
                        if std::env::var("SKATE_REPLAY_DUMP_FRAME").ok().and_then(|v| v.parse().ok()) == Some(frame) {
                            let dir = std::path::Path::new(&dir).join(format!("block_{dumped}"));
                            runtime.dump_guest_for_test(&dir).expect("dump guest");
                            eprintln!("dumped guest before a block of frame {frame} to {}", dir.display());
                            dumped += 1;
                        }
                    }
                    runtime.pump_once().expect("render one block");
                }
            }
            current = Some(frame);
            super::trace::frame(frame);
        }
        let mut w = words(line);
        w.resize(20, 0);
        let wheel = (w[14] & 3) as usize;
        if tag == "PO" {
            let handle = runtime.post("Class_Seams", &w[..20]).expect("post");
            handles[wheel] = Some((handle, w));
        } else if tag == "UP" {
            let Some((handle, posted)) = &handles[wheel] else { continue };
            w[17..20].copy_from_slice(&posted[17..20]);
            for &(i, v) in &set {
                w[i] = v;
            }
            runtime.redeliver(*handle, &w[..20]).expect("redeliver");
        }
    }
    eprintln!("replayed seams through frame {to}");
}

/// What do the `SFXObj_Collision` controllers actually output in the live runtime?
///
/// `sub_824D20E8` scales every contact voice by the Collision controller output for its material
/// category (13,14,15,16,17,18,12,19,21,20 for categories 0..=9). A retail capture of a playtest
/// reads those outputs at 660 / 455 / 14669 / 39 / 38 (outputs 12 / 13 / 17 / 18 / 20), i.e. -34
/// to -59 dB except 17's -7 dB -- which is why a landing's two collision voices, played here at
/// full material level, drowned the class voice. `collision_output_probe` reads zero for all of
/// them, but it drives only the Contacts inputs; these controllers are fed by the global mix
/// controllers, so this asks the question inside the runtime the game actually builds.
#[test]
#[ignore = "needs the owner's assets; run explicitly"]
fn headless_collision_controller_outputs() {
    use skate_audio_core::authored::PCM_FRAMES_PER_BLOCK;
    let assets = std::env::var_os("SKATE_ASSETS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| DEFAULT_ASSETS.into());
    let super::Prepared {
        mut runtime,
        mut sound,
        ..
    } = super::prepare(&assets).expect("prepare the retail player-sound worker");
    let blocks_per_frame = 48_000.0 / f64::from(PCM_FRAMES_PER_BLOCK) / 60.0;
    let mut owed = 0.0f64;
    // Roll, hop, land: the landing is when these voices are opened.
    let script: Vec<PlayerAudioObservation> = (0..60)
        .map(|i| rolling(i, 5.0))
        .chain((0..42).map(|i| airborne(60 + i, 5.0, i as f32 / 60.0, 1.2)))
        .chain((0..60).map(|i| rolling(102 + i, 5.0)))
        .collect();
    for observation in &script {
        sound.frame(&mut runtime, observation).expect("frame");
        owed += blocks_per_frame;
        while owed >= 1.0 {
            owed -= 1.0;
            runtime.pump_once().expect("render one block");
        }
    }
    for instance in 0..3u32 {
        let key = 0x4003_0000 + 0x800 * instance;
        let Some(controller) = runtime.mixmap_controller(key) else {
            eprintln!("SFXObj_Collision #{instance}: no controller {key:#010x}");
            continue;
        };
        let levels: Vec<u32> = (12..=22)
            .map(|id| runtime.mixmap_level(controller, id))
            .collect();
        eprintln!("SFXObj_Collision #{instance} ({controller:#010x}) outputs 12..=22: {levels:?}");
    }
}

/// The landing ladder: does a landing's level follow retail's, all the way down to a 50 ms hop?
///
/// The owner's report is that *this port's* low ollies sound unlike retail's while its bigger
/// drops sound about right. Retail's own curve, extracted from
/// `.local/captures/retail-lowollie-20260920-205434.log` (35 landings; `OUT` block peaks in the
/// twelve frames after `Class_Treatment` word 7 returns to zero, binned by the air time that word
/// carried):
///
/// | air ms | n | p50 dBFS |
/// |---|---|---|
/// | 0–60 | 3 | −8.4 |
/// | 60–120 | 1 | −6.7 |
/// | 120–200 | 1 | −1.4 |
/// | 200–350 | 4 | −4.1 |
/// | 350–600 | 13 | −4.7 |
/// | 600–1100 | 9 | −1.2 |
/// | 1100+ | 4 | +6.8 |
///
/// Note the step at the bottom rather than a curve: below six frames of air the per-wheel
/// touchdown never latches (`sub_82772FD8`'s `frames > 5`, `audio_state.rs:392-415`), so the class
/// voice — which `ContactsOwner::landing` gates on `wheel_landed_464` — and the two collision
/// voices with it never play, and only the fixed impact and the ladder voice are left.
///
///     cargo test -p skate-game --bin skate3rust --locked -- --ignored landing_ladder --nocapture
#[test]
#[ignore = "needs the owner's assets; run explicitly"]
fn headless_landing_ladder_matches_retails_curve() {
    use skate_audio_core::authored::{PCM_CHANNELS, PCM_FRAMES_PER_BLOCK};
    let assets = std::env::var_os("SKATE_ASSETS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| DEFAULT_ASSETS.into());
    let db = |v: f32| 20.0 * v.max(1e-9).log10();
    println!("air ms |   min RMS | mean 200ms | mean 1.0 s |    spread   (retail 200 ms mean)");
    let mut rows = Vec::new();
    // Retail's own (air time, jump height) pairs, not a guess: `Class_Treatment` word 9 is
    // `jump_height × 166.667`, and holding it constant across the rungs mismeasures the heavy end,
    // where retail carries 1.5-2.2 m. These are the medians per band over the same 35 landings.
    for (frames, height) in [
        (3u64, 0.28f32),
        (6, 0.28),
        (9, 0.37),
        (15, 0.37),
        (27, 0.37),
        (48, 1.00),
        (72, 1.81),
    ] {
        let air_ms = (frames as f32 / 60.0 * 1000.0).round() as i32;
        // Band *means*, not p50s: retail's peaks scatter about 8 dB inside a single air-time band,
        // so a p50 over the three or four landings a band holds lands near its maximum.
        // Retail's own block RMS, same twelve-frame window, same binning.
        let retail = match air_ms {
            0..=119 => "-22.9",
            120..=499 => "-20.8",
            500..=999 => "-17.4",
            _ => "-12.8",
        };
        let mut repeats: Vec<f32> = Vec::new();
        let mut longs: Vec<f32> = Vec::new();
        {
            // **Six consecutive landings in one worker, not six fresh ones.** The Splice `Rand` is
            // seeded to 1 at construction (`contact_voices.rs:373`) and only advances when a one-shot
            // resolves, so a fresh worker always draws the *same* members, gains and delays — five
            // fresh workers measure one draw five times. Retail's generator is global, shared by ~100
            // call sites and running since boot, so every retail landing draws from a different point
            // in the sequence. Landing repeatedly inside one session is the closest this port gets to
            // that, and it is also what play actually does.
            let roll = 60u64;
            let super::Prepared {
                mut runtime,
                mut sound,
                ..
            } = super::prepare(&assets).expect("prepare the retail player-sound worker");
            let air_seconds = frames as f32 / 60.0;
            // `roll` varies the pre-roll length so each repeat advances the shared `Rand` differently.
            // Retail's generator is shared game-wide and cannot be reproduced, so a single draw is not
            // comparable to retail's mean: a landing's level depends on which members each group drew
            // and how their delays landed. Repeats give this side a spread to compare against retail's.
            // roll, hop, land, settle -- six times over.
            const CYCLES: usize = 6;
            let settle = 90u64;
            let mut script: Vec<PlayerAudioObservation> = Vec::new();
            let mut landing_frames = Vec::new();
            let mut tick = 0u64;
            for _ in 0..CYCLES {
                for _ in 0..roll {
                    script.push(rolling(tick, 5.0));
                    tick += 1;
                }
                for i in 0..frames {
                    let mut o = airborne(tick, 5.0, i as f32 / 60.0, height);
                    o.retail.air_time_until_landing = (air_seconds - i as f32 / 60.0).max(0.0);
                    script.push(o);
                    tick += 1;
                }
                landing_frames.push(script.len());
                for _ in 0..settle {
                    script.push(rolling(tick, 5.0));
                    tick += 1;
                }
            }
            let mut blocks_owed = 0.0f64;
            let blocks_per_frame = 48_000.0 / f64::from(PCM_FRAMES_PER_BLOCK) / 60.0;
            // RMS as well as peak, computed exactly as `trace::output` writes the `rms` field of an
            // `OUT` line, so it compares directly with the recomp capture's own. Retail's landing peak
            // scatters (25.8 dB inside one air band) while its RMS is cleanly monotonic
            // -22.9 / -20.8 / -17.4 / -12.8 dBFS, so RMS is the meter that answers "how heavy did that
            // landing sound".
            let (mut natives, mut matrices, mut devices) = (Vec::new(), Vec::new(), Vec::new());
            let mut rmss: Vec<f32> = Vec::new();
            for observation in script {
                sound
                    .frame(&mut runtime, &observation)
                    .expect("player-sound frame");
                blocks_owed += blocks_per_frame;
                let (mut native_peak, mut matrix_peak, mut device_peak) = (0.0f32, 0.0f32, 0.0f32);
                let mut frame_rms = 0.0f32;
                while blocks_owed >= 1.0 {
                    blocks_owed -= 1.0;
                    let native = runtime.pump_once().expect("render one block");
                    for sample in &native {
                        native_peak = native_peak.max(sample.abs());
                    }
                    for frame in native.chunks_exact(usize::from(PCM_CHANNELS)) {
                        for (front, surround) in [(frame[0], frame[4]), (frame[1], frame[5])] {
                            matrix_peak =
                                matrix_peak.max(super::fold_side(front, frame[2], surround).abs());
                        }
                    }
                    device_peak = super::downmix(&native)
                        .iter()
                        .fold(device_peak, |p, s| p.max(s.abs()));
                    let energy: f64 = native
                        .iter()
                        .filter(|s| s.is_finite())
                        .map(|s| f64::from(*s) * f64::from(*s))
                        .sum();
                    frame_rms = frame_rms.max((energy / native.len().max(1) as f64).sqrt() as f32);
                }
                rmss.push(frame_rms);
                natives.push(native_peak);
                matrices.push(matrix_peak);
                devices.push(device_peak);
            }
            // The same twelve-frame window the retail extraction used, once per landing.
            // Two windows. The 12-frame one is what the retail extraction used; the 60-frame one
            // exists because a class-2 container's kids are ~900 ms samples (record `+24` reads 890
            // and 966 ms) against class 1's ~500 ms, so a short window can under-read a heavier
            // landing whose energy is spread over a longer sample.
            let _ = (&matrices, &devices, &natives);
            for at in landing_frames {
                let short = rmss[at..(at + 12).min(rmss.len())]
                    .iter()
                    .fold(0f32, |p, s| p.max(*s));
                let long: f32 = {
                    let w = &rmss[at..(at + 60).min(rmss.len())];
                    (w.iter().map(|s| f64::from(*s) * f64::from(*s)).sum::<f64>()
                        / w.len().max(1) as f64)
                        .sqrt() as f32
                };
                repeats.push(db(short));
                longs.push(db(long));
            }
        }
        repeats.sort_by(f32::total_cmp);
        let mean = repeats.iter().sum::<f32>() / repeats.len() as f32;
        let long_mean = longs.iter().sum::<f32>() / longs.len().max(1) as f32;
        println!(
            "{air_ms:6} | {:9.1} | {:9.1} | {:9.1} | {:9.1}   ({retail})",
            repeats[0],
            mean,
            long_mean,
            repeats[repeats.len() - 1] - repeats[0],
        );
        rows.push((air_ms, mean));
    }
    // The shape is the claim, not any single value. Retail's RMS rises 10.1 dB from its lightest
    // band (-22.9) to its heaviest (-12.8) and never falls; this engine rises 2.6 dB and then
    // drops at class 2, because the only class step the authored data supplies is
    // `landing_send_scale`'s +1.4 dB and the class-2 container averages fewer members than the
    // class-1 one. See `docs/engine-defects.md` 12. The assertion below is deliberately the weak
    // form -- monotonicity -- so it starts passing the moment the missing term is recovered,
    // rather than encoding a number that would then need changing.
    let low = rows.first().expect("a rung").1;
    let high = rows.last().expect("a rung").1;
    let heaviest_two = rows[rows.len() - 2].1;
    assert!(
        high > low,
        "a 1.2 s drop must be louder than a 50 ms hop: {high:.1} vs {low:.1} dBFS RMS"
    );
    assert!(
        high >= heaviest_two,
        "the ladder must not fall at the top: 1.2 s reads {high:.1} dBFS RMS against          0.8 s at {heaviest_two:.1} -- the class-2 step is missing, see engine-defects 12"
    );
}

/// The rolling bed across speed **and** surface, against retail's measured table.
///
/// A playtest cannot answer this cleanly: `docs/player-audio-retail-drivers.md` §11 tabulates
/// retail's grain records against ground speed over a session that crossed many surfaces, and the
/// grain position is a Bezier of `speed / maxKmh` where `maxKmh` is *per surface* (55-74 km/h).
/// Comparing a playtest pinned to one surface against that average cannot separate "the Bezier is
/// wrong" from "we are on a different surface", and the test world's collision tags are quad-group
/// indices rather than materials so it cannot vary them either. Here both are controlled.
///
/// Retail, for reference (truck 0, A player): position 0.121 at 10 km/h, 0.407 at 20, 0.340 at 25,
/// 0.394 at 30, 0.515 at 35, 0.634 at 40; gain A 0.06-0.17 across that range.
///
///     cargo test -p skate-game --bin skate3rust --locked -- --ignored speed_surface --nocapture
#[test]
#[ignore = "needs the owner's assets; run explicitly"]
fn headless_rolling_speed_and_surface_sweep() {
    let assets = std::env::var_os("SKATE_ASSETS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| DEFAULT_ASSETS.into());
    // Retail surface tags seen on University, plus the concrete this port used to pin everything
    // to. The tag is the raw `surface_tag & 0x7f`; the audio material is `tag - 1`.
    const TAGS: [(u32, &str); 5] = [
        (3, "material 2"),
        (4, "material 3"),
        (17, "material 16"),
        (42, "material 41"),
        (54, "material 53"),
    ];
    // Retail's position for the A player at each speed bin (§11), for the columns below.
    let retail_position = |kmh: u32| match kmh {
        10 => Some(0.121),
        15 => Some(0.140),
        20 => Some(0.407),
        25 => Some(0.340),
        30 => Some(0.394),
        35 => Some(0.515),
        40 => Some(0.634),
        45 => Some(0.596),
        _ => None,
    };
    for (tag, label) in TAGS {
        let super::Prepared {
            mut runtime,
            mut sound,
            ..
        } = super::prepare(&assets).expect("prepare the retail player-sound worker");
        println!("\nsurface tag {tag} ({label})");
        println!(" km/h |   gain A |   gain B |    pos A |    pos B | retail pos A");
        // `SKATE_SWEEP_SPEEDS=60,70` overrides the ladder; the order matters when chasing a
        // fault, because it distinguishes a speed-dependent one from a cumulative one.
        let speeds: Vec<u32> = std::env::var("SKATE_SWEEP_SPEEDS")
            .ok()
            .map(|v| v.split(',').filter_map(|p| p.trim().parse().ok()).collect())
            .unwrap_or_else(|| vec![5, 10, 15, 20, 25, 30, 35, 40, 45, 50, 60, 70]);
        for kmh in speeds {
            let speed = kmh as f32 / 3.6;
            // Settle: the intensity slew and the MixMap envelopes both need a run-up, and the
            // grain position is read from the record the owner last wrote.
            // **Settle for five seconds, not 1.5.** The MixMap's outputs run through attack/hold/
            // release envelopes (`mixmap/eval.rs`, stage 4), so a short run reads them mid-ramp.
            // At 90 frames this sweep put the wind level at 694 where a three-second run gives 925
            // against retail's 975 -- a 2.9 dB error that is entirely settling, not engine
            // behaviour. Sampling only the last second keeps the reading on the plateau.
            let mut last = None;
            for tick in 0..300u64 {
                let mut observation = rolling(tick, speed);
                observation.retail.wheel_audio_surfaces = [tag; 4];
                observation.wheel_surface = tag;
                sound
                    .frame(&mut runtime, &observation)
                    .expect("player-sound frame");
                if tick >= 240 {
                    last = super::trace::last_grain_records(0).or(last);
                }
            }
            match last {
                Some((a, b)) => {
                    let reference = retail_position(kmh)
                        .map(|p| format!("{p:.3}"))
                        .unwrap_or_else(|| "  -- ".into());
                    println!(
                        "{kmh:5} | {:8.4} | {:8.4} | {:8.4} | {:8.4} | {reference}",
                        a.0, b.0, a.2, b.2
                    );
                }
                None => println!("{kmh:5} | (no grain record -- surface has no grain bed)"),
            }
        }
    }
}

/// The world ambience (SFXObj_Ambience as original code, `world.rs`): stood at the University
/// Library the painted bed must be picked, faded in over the fade-in time and audible, with the
/// player sound (idle, then rolling) still working beside it and the reverb sharing the game object.
///
///     cargo test -p skate-game --bin skate3rust --no-default-features -- --ignored headless_ambience --nocapture
#[test]
#[ignore = "needs the owner's assets; run explicitly"]
fn headless_ambience_at_the_university() {
    use skate_audio_core::authored::PCM_FRAMES_PER_BLOCK;
    let assets = std::env::var_os("SKATE_ASSETS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| DEFAULT_ASSETS.into());
    let super::Prepared {
        mut runtime,
        mut sound,
        ..
    } = super::prepare(&assets).expect("prepare the retail player-sound worker");
    let blocks_per_frame = 48_000.0 / f64::from(PCM_FRAMES_PER_BLOCK) / 60.0;
    let (mut tick, mut owed) = (0u64, 0.0f64);
    // (seconds, speed): idle 6 s (the bed fades in over ~3 s), roll 3 s.
    for (seconds, speed) in [(6u32, 0.0f32), (3, 4.0)] {
        for second in 0..seconds {
            let mut peak = 0f32;
            for _ in 0..60 {
                let mut observation = rolling(tick, speed);
                observation.map = "University".into();
                observation.retail.position = [281.5, 81.5, -447.8];
                sound.frame(&mut runtime, &observation).expect("player-sound frame");
                tick += 1;
                owed += blocks_per_frame;
                while owed >= 1.0 {
                    owed -= 1.0;
                    let stereo = super::downmix(&runtime.pump_once().expect("render one block"));
                    peak = stereo.iter().fold(peak, |p, s| p.max(s.abs()));
                }
            }
            eprintln!(
                "speed {speed} second {second}: peak {:.1} dBFS ambience {:?}",
                20.0 * peak.max(1e-9).log10(),
                runtime.ambience_status()
            );
            if speed == 0.0 && second == 5 {
                assert!(peak > 1.0e-4, "the ambience bed is silent at the Library");
            }
        }
    }
}

/// Leaving a district drops its beds' PCM and the next district's are decoded on the loader while its map loads
/// (`announce_map`, as the map transition calls it): the mix is bit-identical to keeping every bed and decoding on
/// the worker (`SKATE_AUDIO_KEEP_BEDS=1`, the old behaviour), University -> DownTown -> University, a bed playing.
///
///     cargo test -p skate-game --bin skate3rust --no-default-features -- --ignored headless_district_beds --nocapture
#[test]
#[ignore = "needs the owner's assets; run explicitly"]
fn headless_district_beds_drop_without_a_sound_change() {
    use skate_audio_core::authored::PCM_FRAMES_PER_BLOCK;
    let assets = std::env::var_os("SKATE_ASSETS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| DEFAULT_ASSETS.into());
    let blocks_per_frame = 48_000.0 / f64::from(PCM_FRAMES_PER_BLOCK) / 60.0;
    let university = [281.5, 81.5, -447.8];
    let run = |keep: bool| {
        // SAFETY: an ignored test, run alone; the worker reads the variable on this thread.
        unsafe { std::env::set_var("SKATE_AUDIO_KEEP_BEDS", if keep { "1" } else { "0" }) };
        // The worker's own path (the game side unless SKATE_AUDIO_GAME_SIDE=0), as the replay export drives it.
        let mut worker = super::OfflineRender::new(&assets, &|| false).expect("boot the audio worker");
        let (mut tick, mut owed, mut mix) = (0u64, 0.0f64, Vec::new());
        // (map, position, seconds, the map announced a second in).
        for (map, position, seconds, next) in [
            ("University", university, 6, Some("DownTown")),
            ("DownTown", [-64.0, 10.0, -320.0], 6, Some("University")),
            ("University", university, 4, None),
        ] {
            for frame in 0..seconds * 60 {
                if let (60, Some(next)) = (frame, next) {
                    super::announce_map(next);
                }
                let mut observation = rolling(tick, 0.0);
                observation.map = map.into();
                observation.retail.position = position;
                let started = std::time::Instant::now();
                worker.frame(&observation).expect("audio frame");
                if frame == 0 {
                    eprintln!("keep={keep} {map}: the map change's frame took {:.3}s", started.elapsed().as_secs_f32());
                }
                tick += 1;
                owed += blocks_per_frame;
                while owed >= 1.0 {
                    owed -= 1.0;
                    mix.extend(worker.block().expect("render one block"));
                }
            }
        }
        mix
    };
    let dropping = run(false);
    let keeping = run(true);
    unsafe { std::env::remove_var("SKATE_AUDIO_KEEP_BEDS") };
    assert_eq!(dropping.len(), keeping.len());
    assert!(dropping.iter().any(|s| s.abs() > 1.0e-4), "no bed played");
    let first = dropping.iter().zip(&keeping).position(|(a, b)| a.to_bits() != b.to_bits());
    assert_eq!(first, None, "first differing sample of {}", dropping.len());
}

/// The MixMap is the one stage of the game side the original code does not run: its tick
/// (`sub_8294BAE8`) is the runtime's port. With `SKATE_AUDIO_MIXMAP_SHADOW` every tick also runs
/// the original on a copy of the guest and compares every byte outside the stack. Here that runs
/// over a roll, a hop and landing, a revert and a stop at the University Library.
///
///     cargo test -p skate-game --bin skate3rust --no-default-features -- --ignored headless_mixmap_port --nocapture
#[test]
#[ignore = "needs the owner's assets; run explicitly"]
fn headless_mixmap_port_matches_the_original() {
    use skate_audio_core::authored::{AuthoredRuntime, PCM_FRAMES_PER_BLOCK};
    let assets = std::env::var_os("SKATE_ASSETS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| DEFAULT_ASSETS.into());
    // SAFETY: an explicitly run, ignored test; the game side below is the only reader.
    unsafe { std::env::set_var("SKATE_AUDIO_MIXMAP_SHADOW", "1") };
    let guest = skate_data::audio::catalog::load_guest_image(&assets.join("private/stock/audio-runtime-image"))
        .expect("audio runtime image");
    let mut runtime = AuthoredRuntime::new(guest, Vec::new(), Vec::new()).expect("runtime");
    let mut side = super::game_side::GameSide::build(&mut runtime, &assets).expect("boot the game side");
    let mut script: Vec<PlayerAudioObservation> = Vec::new();
    for i in 0..120 {
        script.push(rolling(i, 5.0));
    }
    for i in 0..30 {
        script.push(airborne(120 + i, 5.0, i as f32 / 60.0, 0.6));
    }
    for i in 0..60 {
        script.push(rolling(150 + i, 5.0));
    }
    for i in 0..40 {
        let mut o = rolling(210 + i, 5.0);
        o.state = 102;
        o.retail.state = 102;
        o.retail.state_flags[66 - 52] = true;
        script.push(o);
    }
    for i in 0..60 {
        script.push(rolling(250 + i, 0.0));
    }
    let blocks_per_frame = 48_000.0 / f64::from(PCM_FRAMES_PER_BLOCK) / 60.0;
    let mut owed = 0.0f64;
    // Per phase: the native peak, the folded stereo peak and the samples the fold held at full
    // scale, so the level the fold-down hands the device is on record next to the shadow result.
    let phases = [("roll", 120), ("air", 150), ("land", 210), ("revert", 250), ("stop", 310)];
    let mut meters = [(0f32, 0f32, 0u32); 5];
    let db = |v: f32| 20.0 * v.max(1e-9).log10();
    for (frame, mut observation) in script.into_iter().enumerate() {
        observation.map = "University".into();
        observation.retail.position = [281.5, 81.5, -447.8];
        side.frame(&mut runtime, &observation, false).expect("game-side frame");
        let phase = phases.iter().position(|p| frame < p.1).unwrap_or(4);
        owed += blocks_per_frame;
        while owed >= 1.0 {
            owed -= 1.0;
            let native = runtime.pump_once().expect("render one block");
            let meter = &mut meters[phase];
            meter.0 = native.iter().fold(meter.0, |p, s| p.max(s.abs()));
            for sample in super::downmix(&native) {
                meter.1 = meter.1.max(sample.abs());
                meter.2 += u32::from(sample.abs() >= 1.0);
            }
        }
    }
    for ((label, _), (native, stereo, clipped)) in phases.iter().zip(meters) {
        eprintln!(
            "{label:>6}: native peak {:6.1} dBFS, folded peak {:6.1} dBFS, {clipped} samples at full scale",
            db(native),
            db(stereo)
        );
    }
    let totals = skate_audio_core::game::mixmap_shadow_totals();
    eprintln!("MIXMAP_SHADOW {totals:?}");
    assert!(totals.ticks > 0, "the MixMap never ticked");
    assert!(totals.written > 0, "the port wrote nothing, so a match would prove nothing");
    assert_eq!(totals.failed, 0, "the original tick did not finish");
    assert_eq!(totals.diverged, 0, "the port left different memory from the original");
}

/// A Skate 2 cutscene holds `Time<Virtual>`, so `forward` repeats the last observation with
/// `paused` set: the skater must go quiet and the district's bed play on under the speech, as in
/// retail Skate 2; a pause menu still silences both. The game side as the worker runs it
/// (`run_game_side`), in Soho on New San Vanelona.
///
///     cargo test -p skate-game --bin skate3rust --no-default-features -- --ignored headless_skate2_cutscene --nocapture
#[test]
#[ignore = "needs the owner's assets; run explicitly"]
fn headless_skate2_cutscene_keeps_the_ambience() {
    use skate_audio_core::authored::{AuthoredRuntime, PCM_FRAMES_PER_BLOCK};
    let assets = std::env::var_os("SKATE_ASSETS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| DEFAULT_ASSETS.into());
    let guest = skate_data::audio::catalog::load_guest_image(&assets.join("private/stock/audio-runtime-image"))
        .expect("audio runtime image");
    let mut runtime = AuthoredRuntime::new(guest, Vec::new(), Vec::new()).expect("runtime");
    let mut side = super::game_side::GameSide::build(&mut runtime, &assets).expect("boot the game side");
    let blocks_per_frame = 48_000.0 / f64::from(PCM_FRAMES_PER_BLOCK) / 60.0;
    let (mut tick, mut owed) = (0u64, 0.0f64);
    // The RMS (dBFS) of the last second of `seconds` at `speed`; `hold`: None plays, else the
    // frozen frame `forward` sends with `Some(cutscene)`.
    let mut run = |seconds: u32, speed: f32, hold: Option<bool>, label: &str| {
        let mut last = 0.0;
        for second in 0..seconds {
            let (mut energy, mut samples) = (0f64, 0usize);
            for _ in 0..60 {
                let mut observation = rolling(tick, speed);
                observation.map = "New_San_Vanelona".into();
                observation.skate2 = true;
                observation.district = Some("ID_DISTRICT_SOHO".into());
                observation.retail.paused = hold.is_some();
                side.frame(&mut runtime, &observation, hold == Some(true)).expect("game-side frame");
                tick += 1;
                owed += blocks_per_frame;
                while owed >= 1.0 {
                    owed -= 1.0;
                    let stereo = super::downmix(&runtime.pump_once().expect("render one block"));
                    energy += stereo.iter().map(|s| f64::from(*s) * f64::from(*s)).sum::<f64>();
                    samples += stereo.len();
                }
            }
            last = 10.0 * (energy / samples.max(1) as f64).max(1e-18).log10();
            eprintln!("{label} second {second}: rms {last:.1} dBFS");
        }
        last
    };
    // The bed decodes on its loader thread (seconds with a cold PCM cache), then fades in.
    let bed = run(15, 0.0, None, "idle");
    let rolling = run(3, 4.0, None, "rolling");
    let cutscene = run(4, 4.0, Some(true), "cutscene");
    let menu = run(4, 4.0, Some(false), "pause menu");
    let back = run(4, 0.0, None, "idle again");
    eprintln!("bed {bed:.1} rolling {rolling:.1} cutscene {cutscene:.1} menu {menu:.1} back {back:.1} dBFS");
    assert!(bed > -60.0, "the Soho bed never came in ({bed:.1} dBFS)");
    assert!(cutscene > bed - 6.0, "a cutscene ducked the bed: {bed:.1} -> {cutscene:.1} dBFS");
    assert!(cutscene < rolling - 10.0, "a cutscene left the skater audible: {rolling:.1} -> {cutscene:.1} dBFS");
    assert!(menu < -90.0, "a pause menu no longer silences everything ({menu:.1} dBFS)");
    assert!(back > bed - 6.0, "the bed did not come back ({back:.1} dBFS)");
}

/// The boot's front-end frames (no map, a skater at rest, as `forward` sends them while the boot holds the
/// world): with the FE audio state at 10 ("bootflow", ESRB and Press Start) SFXObj_Ambience plays the title's
/// own bed (23_Press_start_screen); a movie's state 0 fades it out and 10 brings it back; `titlescreen_start`
/// clears the root's +1204, after which state 10 no longer picks it.
///
///     SKATE_ASSETS=<install>\assets cargo test -p skate-game --bin skate3rust --no-default-features -- --ignored headless_press_start_bed --nocapture
#[test]
#[ignore = "needs the owner's assets; run explicitly"]
fn headless_press_start_bed_follows_the_fe_state() {
    use skate_audio_core::authored::{AuthoredRuntime, PCM_FRAMES_PER_BLOCK};
    let assets = std::env::var_os("SKATE_ASSETS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| DEFAULT_ASSETS.into());
    let guest = skate_data::audio::catalog::load_guest_image(&assets.join("private/stock/audio-runtime-image"))
        .expect("audio runtime image");
    let mut runtime = AuthoredRuntime::new(guest, Vec::new(), Vec::new()).expect("runtime");
    let mut side = super::game_side::GameSide::build(&mut runtime, &assets).expect("boot the game side");
    let blocks_per_frame = 48_000.0 / f64::from(PCM_FRAMES_PER_BLOCK) / 60.0;
    let mut owed = 0.0f64;
    // The RMS (dBFS) of the last second of `seconds` at FE audio state `state`; `sounds` ride the first frame.
    let mut run = |seconds: u32, state: u32, sounds: Vec<u64>, label: &str| {
        side.fe_state = Some(state);
        let mut sounds = Some(sounds);
        let mut last = 0.0;
        for second in 0..seconds {
            let (mut energy, mut samples) = (0f64, 0usize);
            for _ in 0..60 {
                let observation = PlayerAudioObservation {
                    volumes: [1.0; 3],
                    frontend_sounds: sounds.take().unwrap_or_default(),
                    ..Default::default()
                };
                side.frame(&mut runtime, &observation, false).expect("game-side frame");
                owed += blocks_per_frame;
                while owed >= 1.0 {
                    owed -= 1.0;
                    let stereo = super::downmix(&runtime.pump_once().expect("render one block"));
                    energy += stereo.iter().map(|s| f64::from(*s) * f64::from(*s)).sum::<f64>();
                    samples += stereo.len();
                }
            }
            last = 10.0 * (energy / samples.max(1) as f64).max(1e-18).log10();
            eprintln!("{label} second {second}: rms {last:.1} dBFS");
        }
        last
    };
    const TITLESCREEN_START: u64 = 0x2E77_2A5D_9676_7A5F;
    let esrb = run(4, 10, Vec::new(), "esrb");
    let movie = run(3, 0, Vec::new(), "movie");
    let title = run(3, 10, Vec::new(), "title");
    let accepted = run(4, 10, vec![TITLESCREEN_START], "accepted");
    eprintln!("esrb {esrb:.1} movie {movie:.1} title {title:.1} accepted {accepted:.1} dBFS");
    assert!(esrb > -60.0, "the Press Start bed never came in ({esrb:.1} dBFS)");
    assert!(movie < esrb - 20.0, "state 0 left the bed playing: {esrb:.1} -> {movie:.1} dBFS");
    assert!(title > esrb - 3.0, "the bed did not come back on the title ({title:.1} dBFS)");
    assert!(accepted < title - 20.0, "titlescreen_start left the bed playing: {title:.1} -> {accepted:.1} dBFS");
}

/// Audio Settings' SFX Volume reaches the mixer: the game side's SFXObj_Master reads it from the
/// settings object every frame, so a lower slider must lower the rolling sound.
#[test]
#[ignore = "needs the owner's assets; run explicitly"]
fn headless_sfx_volume_lowers_the_game_side() {
    use skate_audio_core::authored::{AuthoredRuntime, PCM_FRAMES_PER_BLOCK};
    let assets = std::env::var_os("SKATE_ASSETS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| DEFAULT_ASSETS.into());
    let guest = skate_data::audio::catalog::load_guest_image(&assets.join("private/stock/audio-runtime-image"))
        .expect("audio runtime image");
    let mut runtime = AuthoredRuntime::new(guest, Vec::new(), Vec::new()).expect("runtime");
    let mut side = super::game_side::GameSide::build(&mut runtime, &assets).expect("boot the game side");
    let blocks_per_frame = 48_000.0 / f64::from(PCM_FRAMES_PER_BLOCK) / 60.0;
    let (mut tick, mut owed) = (0u64, 0.0f64);
    // The RMS (dBFS) of the last of `seconds` rolling at 4 m/s with SFX at `sfx`.
    let mut run = |seconds: u32, sfx: f32| {
        let mut last = 0.0;
        for _ in 0..seconds {
            let (mut energy, mut samples) = (0f64, 0usize);
            for _ in 0..60 {
                let mut observation = rolling(tick, 4.0);
                observation.volumes = [sfx, 1.0, 1.0];
                side.frame(&mut runtime, &observation, false).expect("game-side frame");
                tick += 1;
                owed += blocks_per_frame;
                while owed >= 1.0 {
                    owed -= 1.0;
                    let stereo = super::downmix(&runtime.pump_once().expect("render one block"));
                    energy += stereo.iter().map(|s| f64::from(*s) * f64::from(*s)).sum::<f64>();
                    samples += stereo.len();
                }
            }
            last = 10.0 * (energy / samples.max(1) as f64).max(1e-18).log10();
        }
        last
    };
    let full = run(4, 1.0);
    let low = run(3, 0.3);
    let off = run(3, 0.0);
    eprintln!("full {full:.1} low {low:.1} off {off:.1} dBFS");
    assert!(full > -60.0, "no rolling sound at full volume ({full:.1} dBFS)");
    assert!(low < full - 3.0, "SFX 3/10 did not lower the roll: {full:.1} -> {low:.1} dBFS");
    assert!(off < low - 3.0, "SFX 0 is not quieter than 3/10: {low:.1} -> {off:.1} dBFS");
}

/// A long session through the game side as the worker runs it, watching the voice-graph heap.
/// Long playtests ended in "authored audio guest heap exhausted" (audio-errors.log, five sessions
/// on 10-02/10-03); the high-water mark must plateau once every voice type has played.
///
///     SKATE_HEAP_MINUTES=10 cargo test -p skate-game --bin skate3rust --no-default-features -- --ignored headless_graph_heap --nocapture
#[test]
#[ignore = "needs the owner's assets; run explicitly"]
fn headless_graph_heap_plateaus_over_a_long_session() {
    use skate_audio_core::authored::{AuthoredRuntime, PCM_FRAMES_PER_BLOCK};
    let assets = std::env::var_os("SKATE_ASSETS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| DEFAULT_ASSETS.into());
    let minutes: u64 = std::env::var("SKATE_HEAP_MINUTES").ok().and_then(|m| m.parse().ok()).unwrap_or(10);
    let guest = skate_data::audio::catalog::load_guest_image(&assets.join("private/stock/audio-runtime-image"))
        .expect("audio runtime image");
    let mut runtime = AuthoredRuntime::new(guest, Vec::new(), Vec::new()).expect("runtime");
    let mut side = super::game_side::GameSide::build(&mut runtime, &assets).expect("boot the game side");
    let blocks_per_frame = 48_000.0 / f64::from(PCM_FRAMES_PER_BLOCK) / 60.0;
    let mut owed = 0.0f64;
    let started = std::time::Instant::now();
    let mut marks = Vec::new();
    let mut baseline = std::collections::HashSet::new();
    // A 4 s loop: roll, hop, land, revert, stop. Every voice type a session plays over and over.
    for tick in 0..minutes * 3600 {
        let phase = tick % 240;
        let speed = 3.0 + (tick / 240 % 5) as f32;
        let mut observation = match phase {
            0..120 => rolling(tick, speed),
            120..150 => airborne(tick, speed, (phase - 120) as f32 / 60.0, 0.6),
            150..190 => rolling(tick, speed),
            190..210 => {
                let mut o = rolling(tick, speed);
                o.state = 102;
                o.retail.state = 102;
                o.retail.state_flags[66 - 52] = true;
                o
            }
            _ => rolling(tick, 0.0),
        };
        observation.map = "University".into();
        observation.retail.position = [281.5, 81.5, -447.8];
        side.frame(&mut runtime, &observation, false).expect("game-side frame");
        owed += blocks_per_frame;
        while owed >= 1.0 {
            owed -= 1.0;
            runtime.pump_once().expect("render one block");
        }
        if tick % 1800 == 1799 {
            let usage = runtime.arena_usage();
            eprintln!(
                "t={:>4}s heap high-water {:>6}k live blocks {:>6} (wall {:.0}s)",
                (tick + 1) / 60,
                usage.0 / 1024,
                usage.1,
                started.elapsed().as_secs_f64()
            );
            marks.push(usage);
        }
        if tick == 3599 {
            baseline = runtime.arena_live_blocks().into_iter().collect();
        }
        // Which phase of the loop leaves blocks behind: the live count at each phase boundary.
        if tick >= minutes * 3600 - 480 && [0, 119, 149, 189, 209, 239].contains(&phase) {
            eprintln!("phase end {phase:>3}: live blocks {}", runtime.arena_usage().1);
        }
    }
    // The blocks born after the first minute and still live, by size and first two words (an
    // object's vtable and first field).
    let mut kinds = std::collections::HashMap::<(u32, u32, u32), usize>::new();
    for (at, size) in runtime.arena_live_blocks() {
        if !baseline.contains(&(at, size)) {
            let guest = runtime.guest();
            let word = |offset: u32| guest.u32(at + offset).unwrap_or(0);
            *kinds.entry((size, word(0), word(4))).or_default() += 1;
        }
    }
    let mut kinds: Vec<_> = kinds.into_iter().collect();
    kinds.sort_by(|a, b| b.1.cmp(&a.1));
    for ((size, w0, w4), count) in kinds.iter().take(20) {
        eprintln!("leaked {count:>5} x {size:>5} bytes, words {w0:#010x} {w4:#010x}");
    }
    let (early, last) = (marks[1], *marks.last().unwrap());
    assert!(
        last.0 <= early.0 + 256 * 1024,
        "the graph heap kept growing: {}k at 60 s, {}k at the end",
        early.0 / 1024,
        last.0 / 1024
    );
}

/// Review I2: the replay export's sound is the game's own mix. A 2 s roll goes through the export's private worker
/// ([`crate::replay::export::Sound`], the same boot and frame path as the live one) at 30 fps, then into an MP4
/// whose AAC track is read back. Silence fails.
///
///     SKATE_ASSETS=<install>ssets cargo test -p skate-game --bin skate3rust --no-default-features -- --ignored export_sound --nocapture
#[test]
#[ignore = "needs the owner's assets and Windows Media Foundation; run explicitly"]
#[cfg(windows)]
fn export_sound_is_the_game_mix() {
    use crate::replay::{export, mf};
    let assets = std::env::var_os("SKATE_ASSETS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| DEFAULT_ASSETS.into());
    let started = std::time::Instant::now();
    let mut sound = export::Sound::start(assets, 30);
    let ready = loop {
        if let Some(ready) = sound.ready() {
            break ready;
        }
        assert!(started.elapsed().as_secs() < 120, "the export worker never booted");
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    ready.expect("boot the export's player-audio worker");
    eprintln!("export worker booted in {:.1} s", started.elapsed().as_secs_f32());
    let path = std::env::temp_dir().join(format!("s3r-export-sound-{}.mp4", std::process::id()));
    let settings = export::ExportSettings::from_tags(0, 0, 0, 0);
    let mut writer = mf::Mp4Writer::create(&path, &settings, export::SAMPLE_RATE).unwrap();
    let frame = image::RgbaImage::new(1280, 720);
    let (mut peak, mut timing) = (0f32, Vec::new());
    for f in 0..60u32 {
        let records = export::audio_ticks(f, 30).map(|tick| Some(rolling(u64::from(tick), 6.0))).collect();
        let t = std::time::Instant::now();
        sound.request(records);
        // chunk() never waits (owner 10-18): poll until the frame's sound is mixed.
        let chunk = loop {
            if let Some(chunk) = sound.chunk() {
                break chunk.expect("a frame of sound");
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        };
        timing.push(t.elapsed().as_secs_f64() * 1000.);
        assert_eq!(chunk.len(), 1600 * 2);
        peak = chunk.iter().fold(peak, |p, s| p.max(s.abs()));
        writer.video(frame.as_raw(), f).unwrap();
        writer.audio(&chunk).unwrap();
    }
    writer.finish().unwrap();
    let (duration, frames, _, file_peak) = mf::probe(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    timing.sort_by(f64::total_cmp);
    eprintln!(
        "export sound: rendered peak {peak:.4}, AAC peak {file_peak:.4}, {frames} frames, {duration:.2} s, per-frame audio p50 {:.2} ms max {:.2} ms",
        timing[timing.len() / 2],
        timing[timing.len() - 1]
    );
    assert!(peak > 1e-3, "the roll is audible in the rendered mix");
    assert!(file_peak > 1e-3, "and in the MP4");
}

/// A skater `shift` m along +X: roll 0..60, pop + air 60..102, land and roll 102..170, bail 170..230, lie still
/// 230..270 ([`MOVES`]: the phases heard), its follow camera with it.
fn moves(shift: f32) -> Vec<PlayerAudioObservation> {
    (0..270u64)
        .map(|i| {
            let mut o = match i {
                60..102 => airborne(i, 5.0, (i - 60) as f32 / 60.0, 1.2),
                170..230 => {
                    let mut o = rolling(i, 2.0);
                    o.wiping_out = true;
                    o.retail.state_category = 500;
                    o.retail.state = 500;
                    o.retail.wheel_count = 0;
                    o.retail.wheel_contacts = [false; 4];
                    let c = &mut o.retail.body_contacts;
                    let hit = (i - 170) % 15 == 0;
                    c.contact = [hit; 8];
                    c.weighted_change = [if hit { 1.0 } else { 0.0 }; 8];
                    c.material = [3; 8];
                    c.skater_force = if hit { 800.0 } else { 0.0 };
                    o
                }
                230.. => rolling(i, 0.0),
                _ => rolling(i, 5.0),
            };
            let r = &mut o.retail;
            r.position[0] += shift;
            r.deck_position[0] += shift;
            for p in r.wheel_positions.iter_mut().chain(r.toe_positions.iter_mut()) {
                p[0] += shift;
            }
            if let Some((_, at)) = &mut r.camera {
                at[0] += shift;
            }
            o
        })
        .collect()
}
const MOVES: [(&str, std::ops::Range<usize>); 4] = [("roll", 20..60), ("pop", 60..80), ("land", 102..130), ("bail", 170..230)];

/// Item 30b: another skater is heard through retail's own player code from the record it sends. A sender rolls,
/// pops, lands and bails `d` metres in front of a still listener; its records ride the listener's frames as record 1
/// (`game_side::write_remotes`). Near, each of its phases is louder than the listener alone and 20 m is quieter than
/// 10 m; at 40 m (past retail's 30 m gate, `sub_824F8EF8`) the output is bit-identical to no remote at all; and the
/// listener's own record (its local mix input) never changes.
///
///     SKATE_ASSETS=<install>\assets cargo test -p skate-game --bin skate3rust --no-default-features -- --ignored headless_remote_skater --nocapture
#[test]
#[ignore = "needs the owner's assets; run explicitly"]
fn headless_remote_skater_is_heard_near_and_not_beyond_range() {
    use skate_audio_core::authored::{AuthoredRuntime, PCM_FRAMES_PER_BLOCK};
    let assets = std::env::var_os("SKATE_ASSETS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| DEFAULT_ASSETS.into());
    let boot = || {
        let guest = skate_data::audio::catalog::load_guest_image(&assets.join("private/stock/audio-runtime-image"))
            .expect("audio runtime image");
        let mut runtime = AuthoredRuntime::new(guest, Vec::new(), Vec::new()).expect("runtime");
        let side = super::game_side::GameSide::build(&mut runtime, &assets).expect("boot the game side");
        (runtime, side)
    };
    let blocks_per_frame = 48_000.0 / f64::from(PCM_FRAMES_PER_BLOCK) / 60.0;
    // Each frame's output energy, the local records, and the frame time.
    let run = |script: &[PlayerAudioObservation], remote: &[skate_net::audio::Record]| {
        let (mut runtime, mut side) = boot();
        let (mut owed, mut energies, mut records, mut pcm) = (0.0f64, vec![], vec![], vec![]);
        let mut busy = std::time::Duration::ZERO;
        for (i, observation) in script.iter().enumerate() {
            let mut observation = observation.clone();
            if let Some(record) = remote.get(i) {
                observation.remote_records = vec![(1, Box::new(*record))];
            }
            let started = std::time::Instant::now();
            side.frame(&mut runtime, &observation, false).expect("game-side frame");
            busy += started.elapsed();
            records.push(side.local_record());
            owed += blocks_per_frame;
            let mut energy = 0f64;
            while owed >= 1.0 {
                owed -= 1.0;
                let stereo = super::downmix(&runtime.pump_once().expect("render one block"));
                energy += stereo.iter().map(|s| f64::from(*s) * f64::from(*s)).sum::<f64>();
                pcm.extend(stereo);
            }
            energies.push(energy);
        }
        (energies, records, pcm, busy.as_secs_f64() * 1000.0 / script.len() as f64)
    };
    let db = |e: &[f64]| 10.0 * (e.iter().sum::<f64>() / e.len() as f64).max(1e-18).log10();
    let still: Vec<_> = (0..270).map(|i| rolling(i, 0.0)).collect();
    let (none, local, none_pcm, none_ms) = run(&still, &[]);
    let mut heard = vec![];
    for distance in [10.0f32, 20.0, 40.0] {
        let (own, sent, _, _) = run(&moves(distance), &[]);
        let (energies, records, pcm, ms) = run(&still, &sent);
        assert_eq!(records, local, "{distance} m: the listener's own record changed");
        for (name, range) in MOVES {
            eprintln!(
                "{distance:>4} m {name:<4}: sender itself {:6.1} dB, listener {:6.1} dB (alone {:6.1} dB)",
                db(&own[range.clone()]),
                db(&energies[range.clone()]),
                db(&none[range.clone()])
            );
        }
        eprintln!("{distance:>4} m frame {ms:.2} ms (alone {none_ms:.2} ms)");
        heard.push((distance, own, energies, pcm));
    }
    let (_, _, _, far) = &heard[2];
    assert!(far == &none_pcm, "40 m is past retail's 30 m gate: the output must be exactly the listener's alone");
    let (_, own, near, _) = &heard[0];
    let (_, _, mid, _) = &heard[1];
    for (name, range) in MOVES {
        if db(&own[range.clone()]) < -60.0 {
            eprintln!("{name}: the sender's own {name} is silent here; not compared");
            continue;
        }
        assert!(
            db(&near[range.clone()]) > db(&none[range.clone()]) + 3.0,
            "{name}: the skater 10 m away is not heard"
        );
        assert!(db(&mid[range.clone()]) < db(&near[range.clone()]), "{name}: 20 m is not quieter than 10 m");
    }
}

/// User 10-25, spectating the bot ("the audio faded away when the person I was spectating got farther away"): retail
/// hears another skater from the camera (listener pos0: `sub_824F1FB0` drops one past 30 m of it, `sub_824F8EF8`), so
/// a skater rolling 40 m from our parked one, watched from 3 m behind it, is heard, and as loud as with ours 8 m from
/// it. Each level is the frames' energy less that of our parked skater alone on the same spot.
///
///     SKATE_ASSETS=<install>\assets cargo test -p skate-game --bin skate3rust --no-default-features -- --ignored headless_spectated --nocapture
#[test]
#[ignore = "needs the owner's assets; run explicitly"]
fn headless_spectated_skater_sounds_the_same_wherever_ours_stands() {
    use skate_audio_core::authored::{AuthoredRuntime, PCM_FRAMES_PER_BLOCK};
    const D: f32 = 40.0;
    let assets = std::env::var_os("SKATE_ASSETS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| DEFAULT_ASSETS.into());
    let boot = || {
        let guest = skate_data::audio::catalog::load_guest_image(&assets.join("private/stock/audio-runtime-image"))
            .expect("audio runtime image");
        let mut runtime = AuthoredRuntime::new(guest, Vec::new(), Vec::new()).expect("runtime");
        let side = super::game_side::GameSide::build(&mut runtime, &assets).expect("boot the game side");
        (runtime, side)
    };
    let blocks_per_frame = 48_000.0 / f64::from(PCM_FRAMES_PER_BLOCK) / 60.0;
    let camera = Some(([1.0, 0.0, 0.0], [D - 3.0, 1.5, 0.0]));
    // The watched skater's own records: rolling at 5 m/s, D m along +X.
    let sent: Vec<skate_net::audio::Record> = {
        let (mut runtime, mut side) = boot();
        let mut owed = 0.0f64;
        (0..180u64)
            .map(|i| {
                let mut o = rolling(i, 5.0);
                let r = &mut o.retail;
                r.position[0] += D;
                r.deck_position[0] += D;
                for p in r.wheel_positions.iter_mut().chain(r.toe_positions.iter_mut()) {
                    p[0] += D;
                }
                r.camera = camera;
                side.frame(&mut runtime, &o, false).expect("sender frame");
                owed += blocks_per_frame;
                while owed >= 1.0 {
                    owed -= 1.0;
                    runtime.pump_once().expect("render one block");
                }
                side.local_record()
            })
            .collect()
    };
    // Us: parked `at` m along +X, the camera on the watched skater, its records riding as record 1 (`heard`).
    let listen = |at: f32, heard: bool| {
        let (mut runtime, mut side) = boot();
        let (mut owed, mut energy) = (0.0f64, 0f64);
        for (i, record) in sent.iter().enumerate() {
            let mut o = rolling(i as u64, 0.0);
            let r = &mut o.retail;
            r.position[0] += at;
            r.deck_position[0] += at;
            for p in r.wheel_positions.iter_mut().chain(r.toe_positions.iter_mut()) {
                p[0] += at;
            }
            r.camera = camera;
            if heard {
                o.remote_records = vec![(1, Box::new(*record))];
            }
            side.frame(&mut runtime, &o, false).expect("listener frame");
            owed += blocks_per_frame;
            while owed >= 1.0 {
                owed -= 1.0;
                let stereo = super::downmix(&runtime.pump_once().expect("render one block"));
                if i >= 30 {
                    energy += stereo.iter().map(|s| f64::from(*s) * f64::from(*s)).sum::<f64>();
                }
            }
        }
        energy
    };
    let db = |energy: f64| 10.0 * energy.max(1e-18).log10();
    let (alone_far, alone_near) = (listen(0.0, false), listen(D - 8.0, false));
    let (far, near) = (listen(0.0, true) - alone_far, listen(D - 8.0, true) - alone_near);
    eprintln!(
        "watched skater with ours {D} m off: {:.1} dB (ours alone {:.1}); ours 8 m off: {:.1} dB (ours alone {:.1})",
        db(far),
        db(alone_far),
        db(near),
        db(alone_near)
    );
    assert!(db(far) > db(alone_far) + 3.0, "the watched skater 40 m from ours is not heard: {:.1} dB", db(far));
    assert!((db(far) - db(near)).abs() < 1.0, "it fades with our distance: {:.1} vs {:.1} dB", db(far), db(near));
}

/// User 10-30 (retail, tu3): watching a player makes it the local one (`SwitchCamera` / the spectator UI set the list's
/// vfunc 0x28, `sub_82598C38`; `sub_827A11B0` then builds that player's record with `local` = 1, and `frame+8` is the
/// record with bit 31, 0x827A514C), and our own skater is a remote like any other. Before the watched skater has
/// played a record (a switch), ours stays local and only its position words move (`listen_from`).
///
///     SKATE_ASSETS=<install>\assets cargo test -p skate-game --bin skate3rust --no-default-features -- --ignored headless_watched --nocapture
#[test]
#[ignore = "needs the owner's assets; run explicitly"]
fn headless_watched_skater_is_the_local_one() {
    use skate_audio_core::game::{GAME_OBJECT, SKATER_FRAME, SKATER_RECORD_BYTES, SKATER_RECORDS};
    const D: f32 = 40.0;
    const FLAGS: usize = 152 / 4;
    let assets = std::env::var_os("SKATE_ASSETS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| DEFAULT_ASSETS.into());
    let boot = || {
        let guest = skate_data::audio::catalog::load_guest_image(&assets.join("private/stock/audio-runtime-image"))
            .expect("audio runtime image");
        let mut runtime = skate_audio_core::authored::AuthoredRuntime::new(guest, Vec::new(), Vec::new()).expect("runtime");
        let side = super::game_side::GameSide::build(&mut runtime, &assets).expect("boot the game side");
        (runtime, side)
    };
    // The watched skater's own record, rolling D m along +X.
    let sent = {
        let (mut runtime, mut side) = boot();
        let mut o = rolling(0, 5.0);
        let r = &mut o.retail;
        r.position[0] += D;
        r.deck_position[0] += D;
        for p in r.wheel_positions.iter_mut().chain(r.toe_positions.iter_mut()) {
            p[0] += D;
        }
        side.frame(&mut runtime, &o, false).expect("sender frame");
        side.local_record()
    };
    assert_ne!(sent[FLAGS] & 0x8000_0000, 0, "a sender's record is built local");
    let mut watched = super::remote::clean(&sent, bevy::math::Vec3::new(D, 0., 0.));
    watched[FLAGS] |= 1 << 16;
    let (mut runtime, mut side) = boot();
    let frame = |runtime: &skate_audio_core::authored::AuthoredRuntime| {
        let g = runtime.guest();
        let frame = g.u32(GAME_OBJECT).expect("game object") + SKATER_FRAME;
        let record = |k: u32, i: usize| g.u32(frame + SKATER_RECORDS + SKATER_RECORD_BYTES * k + 4 * i as u32).expect("word");
        let x = |k: u32| f32::from_bits(record(k, 0));
        let local = g.u32(frame + 8).expect("+8") == frame + SKATER_RECORDS;
        (local, g.u32(frame + 0xC).expect("count"), x(0), record(0, FLAGS), x(1), record(1, FLAGS))
    };
    let mut o = rolling(0, 0.0);
    o.remote_records = vec![(1, Box::new(watched))];
    o.watched = Some(Box::new(watched));
    side.frame(&mut runtime, &o, false).expect("watching frame");
    let (local, count, x0, flags0, x1, flags1) = frame(&runtime);
    assert!(local, "+8 is record 0");
    assert_eq!(count, 2);
    assert_eq!(x0, D, "record 0 is the watched skater's");
    assert_eq!((flags0 & 0x8000_0000, (flags0 >> 16) & 0xF), (0x8000_0000, 1), "local, on its own nibble");
    assert!(x1.abs() < 1., "record 1 is ours: {x1}");
    assert_eq!(flags1 & 0x8000_0080, 0, "ours is a remote");
    assert!(f32::from_bits(side.local_record()[0]).abs() < 1., "the record sent to the others is still ours");
    // A switch: the watched skater has played nothing yet, so ours stays local and only hears from its place.
    o.remote_records = vec![];
    side.frame(&mut runtime, &o, false).expect("switch frame");
    let (local, count, x0, flags0, ..) = frame(&runtime);
    assert!(local);
    assert_eq!((count, x0), (1, D));
    assert_ne!(flags0 & 0x8000_0000, 0);
}

/// User 10-30 ("really nail down the sound while watching someone"): retail makes the watched player the local one,
/// so seen through its own camera a skater sounds to us as it does to its own player, each phase within 2 dB. Our
/// parked skater, rolling, popping and bailing 40 m off, never reaches the mix (a remote then, past the 30 m gate,
/// `sub_824F8EF8`), so nothing of the watched skater's sound comes from our own bundle. A watched skater whose records
/// stop (a late link or one leaving: held [`super::remote`]'s 12 frames, then none) fades out over about a second, as
/// any skater's sound does (a remote's alike), and nothing stays.
///
///     SKATE_ASSETS=<install>\assets cargo test -p skate-game --bin skate3rust --no-default-features -- --ignored headless_a_watched --nocapture
#[test]
#[ignore = "needs the owner's assets; run explicitly"]
fn headless_a_watched_skater_sounds_to_us_as_to_itself() {
    use skate_audio_core::authored::{AuthoredRuntime, PCM_FRAMES_PER_BLOCK};
    const D: f32 = 40.0;
    let assets = std::env::var_os("SKATE_ASSETS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| DEFAULT_ASSETS.into());
    let boot = || {
        let guest = skate_data::audio::catalog::load_guest_image(&assets.join("private/stock/audio-runtime-image"))
            .expect("audio runtime image");
        let mut runtime = AuthoredRuntime::new(guest, Vec::new(), Vec::new()).expect("runtime");
        let side = super::game_side::GameSide::build(&mut runtime, &assets).expect("boot the game side");
        (runtime, side)
    };
    let blocks_per_frame = 48_000.0 / f64::from(PCM_FRAMES_PER_BLOCK) / 60.0;
    // Each frame's output energy, the local records and the samples; `each` sets the frame up before it runs.
    let play = |script: &[PlayerAudioObservation], each: &dyn Fn(usize, &mut PlayerAudioObservation)| {
        let (mut runtime, mut side) = boot();
        let (mut owed, mut energies, mut records, mut pcm) = (0.0f64, vec![], vec![], vec![]);
        for (i, observation) in script.iter().enumerate() {
            let mut o = observation.clone();
            each(i, &mut o);
            side.frame(&mut runtime, &o, false).expect("game-side frame");
            records.push(side.local_record());
            owed += blocks_per_frame;
            let mut energy = 0f64;
            while owed >= 1.0 {
                owed -= 1.0;
                let stereo = super::downmix(&runtime.pump_once().expect("render one block"));
                energy += stereo.iter().map(|s| f64::from(*s) * f64::from(*s)).sum::<f64>();
                pcm.extend(stereo);
            }
            energies.push(energy);
        }
        (energies, records, pcm)
    };
    let db = |e: &[f64]| 10.0 * (e.iter().sum::<f64>() / e.len() as f64).max(1e-18).log10();
    let sender = moves(D);
    let (own, sent, _) = play(&sender, &|_, _| {});
    let record = |i: usize| Box::new(super::remote::clean(&sent[i], bevy::math::Vec3::new(D, 0., 0.)));
    // Us parked at the origin with the camera on the watched skater (observer_camera), its records as remote.rs hands
    // them over: slot 0 (nibble 1), and the same record as the watched one (game_side::watch).
    let watch = |ours: &[PlayerAudioObservation]| {
        play(ours, &|i, o| {
            o.retail.camera = sender[i].retail.camera;
            (o.remote_records, o.watched) = (vec![(1, record(i))], Some(record(i)));
        })
    };
    let still: Vec<_> = (0..270).map(|i| rolling(i, 0.0)).collect();
    let (heard, _, quiet) = watch(&still);
    let (busy, _, busy_pcm) = watch(&moves(0.0));
    for (name, range) in MOVES {
        eprintln!(
            "{name:<4}: itself {:6.1} dB, watched by us {:6.1} dB, ours busy meanwhile {:6.1} dB",
            db(&own[range.clone()]),
            db(&heard[range.clone()]),
            db(&busy[range.clone()])
        );
    }
    for (name, range) in MOVES {
        let (itself, us) = (db(&own[range.clone()]), db(&heard[range.clone()]));
        if itself < -60.0 {
            eprintln!("{name}: the sender's own {name} is silent here; not compared");
            continue;
        }
        assert!((us - itself).abs() < 2.0, "{name}: watched {us:.1} dB, itself {itself:.1} dB");
    }
    assert!(busy_pcm == quiet, "our parked skater 40 m off reached the mix");
    // Its records stop in the roll (from frame 30) for good: the last held 12 frames, then none, as next_records plays
    // one; the world is then heard from that record with ours (game_side::listen_from), or (`unmoved`) from ours.
    const STOP: usize = 30;
    let stopped = |unmoved: bool| {
        play(&still, &|i, o| {
            o.retail.camera = sender[i].retail.camera;
            let held = record(STOP - 1);
            match i {
                _ if i < STOP => (o.remote_records, o.watched) = (vec![(1, record(i))], Some(record(i))),
                _ if i < STOP + 12 => (o.remote_records, o.watched) = (vec![(1, held.clone())], Some(held)),
                _ => o.watched = (!unmoved).then_some(held),
            }
        })
        .0
    };
    let (alone, ..) = play(&still, &|i, o| o.retail.camera = sender[i].retail.camera);
    let (watched, unmoved) = (stopped(false), stopped(true));
    // The same stop for a skater we never watched, 10 m off, heard from our own camera.
    let near = moves(10.0);
    let (_, near_sent, _) = play(&near, &|_, _| {});
    let near_record = |i: usize| Box::new(super::remote::clean(&near_sent[i], bevy::math::Vec3::new(10., 0., 0.)));
    let remote = play(&still, &|i, o| match i {
        _ if i < STOP => o.remote_records = vec![(1, near_record(i))],
        _ if i < STOP + 12 => o.remote_records = vec![(1, near_record(STOP - 1))],
        _ => {}
    })
    .0;
    let (ours_alone, ..) = play(&still, &|_, _| {});
    // From the end of the hold (10-30: -6.5, -12, -47, -70 dB watched; -9, -15, -51, -76 remote): the title fades a
    // skater's voices out over about a second once its records stop, watched or not; nothing stays.
    let windows = [45..60, 60..90, 90..150, 150..270];
    let levels = |e: &[f64]| windows.iter().map(|w| db(&e[w.clone()])).collect::<Vec<_>>();
    let shown = |l: &[f64]| l.iter().map(|d| format!("{d:6.1}")).collect::<Vec<_>>().join(" ");
    let roll = db(&own[MOVES[0].1.clone()]);
    eprintln!("records stop at {STOP}, held to {}; dB in frames {windows:?} (its roll {roll:.1} dB):", STOP + 12);
    for (name, e, quiet) in [("watched", &watched, &alone), ("watched, unmoved", &unmoved, &alone), ("remote 10 m", &remote, &ours_alone)] {
        let (l, q) = (levels(e), levels(quiet));
        eprintln!("  {name:<17} {}  (no skater {})", shown(&l), shown(&q));
        assert!(l.windows(2).all(|p| p[1] < p[0]), "{name}: its sound fades once its records stop: {l:?}");
        assert!(l[3] < roll - 60., "{name}: faded out within 4 s: {:.1} dB, the roll {roll:.1} dB", l[3]);
    }
}

/// Round2 T5, user bug 11 (10-13, "Slappy's board makes no sound"): a rider on this machine (physics/riders.rs) is
/// heard through retail's own player code like an online skater. Its inputs ride the observation (`riders`); the game
/// side builds its record with the title's builder `sub_827A1B78` on a second guest skater (its own bundle, builder
/// and conditioner) and writes it as record 1 before the frame runs. A rider rolling 10 m from a still listener is
/// louder than the listener alone, at 40 m (past retail's 30 m gate, `sub_824F8EF8`) the output is bit-identical to
/// no rider at all, and the listener's own record never changes (the player's record is still built last).
///
///     SKATE_ASSETS=<install>\assets cargo test -p skate-game --bin skate3rust --no-default-features -- --ignored headless_rider --nocapture
#[test]
#[ignore = "needs the owner's assets; run explicitly"]
fn headless_rider_is_heard_through_its_own_guest_skater() {
    use skate_audio_core::authored::{AuthoredRuntime, PCM_FRAMES_PER_BLOCK};
    let assets = std::env::var_os("SKATE_ASSETS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| DEFAULT_ASSETS.into());
    let blocks_per_frame = 48_000.0 / f64::from(PCM_FRAMES_PER_BLOCK) / 60.0;
    // A rider rolling at 5 m/s `distance` metres along +X from the listener, or none.
    let rider = |i: u64, distance: f32| {
        let mut r = rolling(i, 5.0).retail;
        r.position[0] += distance;
        r.deck_position[0] += distance;
        for p in r.wheel_positions.iter_mut().chain(r.toe_positions.iter_mut()) {
            p[0] += distance;
        }
        r.camera = None;
        r
    };
    let run = |distance: Option<f32>| {
        let guest = skate_data::audio::catalog::load_guest_image(&assets.join("private/stock/audio-runtime-image"))
            .expect("audio runtime image");
        let mut runtime = AuthoredRuntime::new(guest, Vec::new(), Vec::new()).expect("runtime");
        let mut side = super::game_side::GameSide::build(&mut runtime, &assets).expect("boot the game side");
        let (mut owed, mut energies, mut records, mut pcm) = (0.0f64, vec![], vec![], vec![]);
        for i in 0..120u64 {
            let mut observation = rolling(i, 0.0);
            if let Some(distance) = distance {
                observation.riders = vec![("slappy#ai1".to_owned(), rider(i, distance))];
            }
            side.frame(&mut runtime, &observation, false).expect("game-side frame");
            records.push(side.local_record());
            owed += blocks_per_frame;
            let mut energy = 0f64;
            while owed >= 1.0 {
                owed -= 1.0;
                let stereo = super::downmix(&runtime.pump_once().expect("render one block"));
                energy += stereo.iter().map(|s| f64::from(*s) * f64::from(*s)).sum::<f64>();
                pcm.extend(stereo);
            }
            energies.push(energy);
        }
        (energies, records, pcm)
    };
    let db = |e: &[f64]| 10.0 * (e.iter().sum::<f64>() / e.len() as f64).max(1e-18).log10();
    let (alone, local, alone_pcm) = run(None);
    let (near, near_records, _) = run(Some(10.0));
    let (_, far_records, far_pcm) = run(Some(40.0));
    eprintln!("rider 10 m: {:6.1} dB, listener alone {:6.1} dB", db(&near[20..]), db(&alone[20..]));
    assert_eq!(near_records, local, "10 m: the listener's own record changed");
    assert_eq!(far_records, local, "40 m: the listener's own record changed");
    assert!(db(&near[20..]) > db(&alone[20..]) + 3.0, "the rider rolling 10 m away is not heard");
    assert!(far_pcm == alone_pcm, "40 m is past retail's 30 m gate: the output must be exactly the listener's alone");
}

/// 3.1 fuzz: adversarial records (random bits, NaN/inf, extreme ints, real records with words mutated) through the
/// receive side ([`super::remote::clean`]) into retail's player code as records 1..3, every frame. No panic, every
/// frame completes inside a budget, the output stays finite and bounded, and a real skater is still heard afterwards.
/// `SKATE_FUZZ_RAW=1` feeds the records uncleaned (what an unpatched client runs); `SKATE_FUZZ_FRAMES` sets the length.
///
///     SKATE_ASSETS=<install>\assets cargo test -p skate-game --bin skate3rust -- --ignored headless_remote_record_fuzz --nocapture
#[test]
#[ignore = "needs the owner's assets; run explicitly"]
fn headless_remote_record_fuzz() {
    use skate_audio_core::authored::{AuthoredRuntime, PCM_FRAMES_PER_BLOCK};
    let assets = std::env::var_os("SKATE_ASSETS").map(std::path::PathBuf::from).unwrap_or_else(|| DEFAULT_ASSETS.into());
    let raw = std::env::var_os("SKATE_FUZZ_RAW").is_some();
    let frames: u64 = std::env::var("SKATE_FUZZ_FRAMES").ok().and_then(|v| v.parse().ok()).unwrap_or(600);
    let boot = || {
        let guest = skate_data::audio::catalog::load_guest_image(&assets.join("private/stock/audio-runtime-image")).expect("image");
        let mut runtime = AuthoredRuntime::new(guest, Vec::new(), Vec::new()).expect("runtime");
        let side = super::game_side::GameSide::build(&mut runtime, &assets).expect("boot");
        (runtime, side)
    };
    // Real records to mutate: a sender rolling, popping and landing 5 m away.
    let (mut runtime, mut side) = boot();
    let mut real = vec![];
    for i in 0..120u64 {
        let mut o = if (60..90).contains(&i) { airborne(i, 5.0, (i - 60) as f32 / 60.0, 1.2) } else { rolling(i, 5.0) };
        o.retail.position[0] += 5.0;
        side.frame(&mut runtime, &o, false).expect("sender frame");
        let record = side.local_record();
        let at = bevy::prelude::Vec3::new(f32::from_bits(record[0]), f32::from_bits(record[1]), f32::from_bits(record[2]));
        assert_eq!(super::remote::clean(&record, at), record, "frame {i}: the whitelist changed a real record");
        real.push(record);
    }
    let (mut runtime, mut side) = boot();
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let special = [0u32, u32::MAX, 0x7FFF_FFFF, 0x8000_0000, 0x7FC0_0000, 0x7F80_0000, 0xFF80_0000, 0x7F7F_FFFF, 143, 144, 0xFFFF];
    let blocks_per_frame = 48_000.0 / f64::from(PCM_FRAMES_PER_BLOCK) / 60.0;
    let (mut owed, mut errors, mut worst, mut peak) = (0.0f64, 0u32, std::time::Duration::ZERO, 0f32);
    let at = bevy::prelude::Vec3::new(5.0, 0.0, 0.0);
    for i in 0..frames {
        let records = (1..=3u8)
            .map(|nibble| {
                let r = next();
                let mut record: skate_net::audio::Record = match r % 3 {
                    0 => std::array::from_fn(|_| next() as u32),
                    1 => std::array::from_fn(|_| special[(next() % special.len() as u64) as usize]),
                    _ => real[(r >> 8) as usize % real.len()],
                };
                for _ in 0..(next() % 8) {
                    let w = (next() % record.len() as u64) as usize;
                    record[w] = if next() % 2 == 0 { next() as u32 } else { special[(next() % special.len() as u64) as usize] };
                }
                let record = if raw { record } else { super::remote::clean(&record, at) };
                (nibble, Box::new(record))
            })
            .collect();
        let mut observation = rolling(i, 0.0);
        observation.remote_records = records;
        let started = std::time::Instant::now();
        if let Err(e) = side.frame(&mut runtime, &observation, false) {
            errors += 1;
            if errors <= 5 {
                eprintln!("frame {i}: {e}");
            }
        }
        worst = worst.max(started.elapsed());
        owed += blocks_per_frame;
        while owed >= 1.0 {
            owed -= 1.0;
            match runtime.pump_once() {
                Ok(block) => {
                    let stereo = super::downmix(&block);
                    assert!(stereo.iter().all(|s| s.is_finite()), "frame {i}: a non-finite sample");
                    peak = stereo.iter().fold(peak, |p, s| p.max(s.abs()));
                }
                Err(e) => {
                    errors += 1;
                    eprintln!("frame {i} block: {e}");
                }
            }
        }
    }
    eprintln!("fuzz raw={raw} frames={frames} errors={errors} worst_frame={:.1} ms peak={peak:.3}", worst.as_secs_f64() * 1e3);
    assert!(worst < std::time::Duration::from_millis(500), "a frame took {worst:?}");
    if !raw {
        assert_eq!(errors, 0, "a cleaned record failed the player code");
    }
    // The fuzzed voices release once their records stop: 3 s alone, the last second near silent.
    let mut last_second = 0f64;
    for i in 0..180u64 {
        side.frame(&mut runtime, &rolling(frames + i, 0.0), false).expect("frame after the fuzz");
        owed += blocks_per_frame;
        while owed >= 1.0 {
            owed -= 1.0;
            let stereo = super::downmix(&runtime.pump_once().expect("block after the fuzz"));
            if i >= 120 {
                last_second += stereo.iter().map(|s| f64::from(*s) * f64::from(*s)).sum::<f64>();
            }
        }
    }
    eprintln!("after the fuzz: the third second alone {last_second:.3e}");
    assert!(last_second < 1e-2, "a fuzzed voice still plays 2 s after its records stopped: {last_second:.3e}");
    let frames = frames + 180;
    // Still working: a real skater 5 m away is heard over the listener alone.
    let (mut energy, mut after) = ([0f64; 2], [0u32; 2]);
    for (k, with) in [false, true].into_iter().enumerate() {
        for i in 0..120u64 {
            let mut observation = rolling(frames + 120 * k as u64 + i, 0.0);
            if with {
                observation.remote_records = vec![(1, Box::new(super::remote::clean(&real[i as usize], at)))];
            }
            if let Err(e) = side.frame(&mut runtime, &observation, false) {
                after[k] += 1;
                if after[k] == 1 { eprintln!("after the fuzz (with a skater: {with}): {e}"); }
            }
            owed += blocks_per_frame;
            while owed >= 1.0 {
                owed -= 1.0;
                let stereo = super::downmix(&runtime.pump_once().unwrap_or_default());
                energy[k] += stereo.iter().map(|s| f64::from(*s) * f64::from(*s)).sum::<f64>();
            }
        }
    }
    eprintln!("after the fuzz: alone {:.3e} ({} failed frames), with a skater {:.3e} ({} failed)", energy[0], after[0], energy[1], after[1]);
    assert_eq!(after, [0, 0], "the fuzz left the game side failing");
    assert!(energy[1] > energy[0] * 2.0, "a real skater is no longer heard after the fuzz");
}

/// User 10-22 ("when i do a trick and im in the air i feel like the sound instantly stops on the skate 2 pysics unlike
/// the skate 3"): the same 0.7 s hop heard with Skate 3's air record, with the record Skate 2 physics sent (no time
/// until landing), and with a kickflip scoring (EScorableID 96). The middle of the air is mostly the wheel spin-down.
#[test]
#[ignore = "needs the owner's assets; run explicitly"]
fn headless_the_wheel_spin_holds_through_the_air() {
    use skate_audio_core::authored::PCM_FRAMES_PER_BLOCK;
    let assets = std::env::var_os("SKATE_ASSETS").map(std::path::PathBuf::from).unwrap_or_else(|| DEFAULT_ASSETS.into());
    let blocks_per_frame = 48_000.0 / f64::from(PCM_FRAMES_PER_BLOCK) / 60.0;
    let hear = |air: &dyn Fn(PlayerAudioObservation) -> PlayerAudioObservation| {
        let super::Prepared { mut runtime, mut sound, .. } = super::prepare(&assets).expect("prepare the worker");
        let script: Vec<PlayerAudioObservation> = (0..60)
            .map(|i| rolling(i, 5.0))
            .chain((0..42).map(|i| air(airborne(60 + i, 5.0, i as f32 / 60.0, 1.2))))
            .chain((0..30).map(|i| rolling(102 + i, 5.0)))
            .collect();
        let (mut owed, mut energy) = (0.0f64, Vec::new());
        for observation in script {
            sound.frame(&mut runtime, &observation).expect("player-sound frame");
            owed += blocks_per_frame;
            let mut sum = 0.0f64;
            while owed >= 1.0 {
                owed -= 1.0;
                let native = runtime.pump_once().expect("render one block");
                sum += super::downmix(&native).iter().map(|s| f64::from(*s) * f64::from(*s)).sum::<f64>();
            }
            energy.push(sum);
        }
        // Early air (the first 0.15 s) against the middle and late air.
        let db = |range: std::ops::Range<usize>| 10.0 * (energy[range].iter().sum::<f64>().max(1e-12)).log10();
        (db(62..71), db(71..85), db(85..100))
    };
    let skate3 = hear(&|o| o);
    let port = hear(&|mut o| {
        o.retail.air_time_until_landing = 0.0;
        o
    });
    let flip = hear(&|mut o| {
        o.retail.scorable_id = 96;
        o
    });
    eprintln!("air energy dB (early, middle, late): skate3 {skate3:.1?}, port record {port:.1?}, kickflip {flip:.1?}");
    for (name, (_, middle, late)) in [("port record", port), ("kickflip", flip)] {
        assert!(middle > skate3.1 - 6.0 && late > skate3.2 - 6.0, "{name}: the spin drops away: {middle:.1}/{late:.1} vs {:.1}/{:.1}", skate3.1, skate3.2);
    }
}
