//! The listening previews (plan Task 5): the real stage recordings through the mixer, written as WAVs for the user
//! to judge before anything reaches the game. Run with SKATE_WEATHER_AUDIO_ROOT (the stage weather-audio folder)
//! and SKATE_WEATHER_PREVIEW_OUT set, `-- --ignored --nocapture`.
use super::{assets::Bank, mixer::*, thunder::delay_s};
use std::{path::Path, sync::Arc};

const HEAVY_STREET: [[f32; 2]; 5] = [[0.45, 0.35], [0.05, 0.15], [0., 0.], [0.2, 0.1], [0.2, 0.2]];
const LIGHT_HARD: [[f32; 2]; 5] = [[0.6, 0.6], [0.; 2], [0.; 2], [0.; 2], [0.; 2]];
/// Parameters are set every 10 ms of preview.
const STEP: usize = RATE as usize / 100;

fn heavy() -> Params {
    Params { rain: 1., surfaces: HEAVY_STREET, master: 1., ..Default::default() }
}

/// Renders `seconds`, asking `script(t)` for the parameters every [`STEP`]; `strikes` are (time, distance, azimuth).
fn render(bank: &Arc<Bank>, seconds: f32, script: impl Fn(f32) -> Params, strikes: &[(f32, f32, f32)]) -> Vec<f32> {
    let mut m = Mixer::new(bank.clone(), 0x5EED);
    let frames = (seconds * RATE as f32) as usize;
    let mut out = vec![0.; frames * 2];
    for (n, chunk) in out.chunks_mut(STEP * 2).enumerate() {
        let (t0, t1) = ((n * STEP) as f32 / RATE as f32, ((n + 1) * STEP) as f32 / RATE as f32);
        for (i, &(at, d, azimuth)) in strikes.iter().enumerate() {
            if (t0..t1).contains(&at) {
                m.strike(Thunder { azimuth, distance_m: d, seed: 11 + i as u32 }, delay_s(d));
            }
        }
        m.set(script(t0));
        m.render(chunk);
    }
    out
}

fn write_wav(path: &Path, samples: &[f32]) {
    let data = (samples.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + data as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&RATE.to_le_bytes());
    bytes.extend_from_slice(&(RATE * 4).to_le_bytes());
    bytes.extend_from_slice(&4u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        bytes.extend_from_slice(&((s.clamp(-1., 1.) * 32767.).round() as i16).to_le_bytes());
    }
    std::fs::write(path, bytes).expect("write preview");
}

#[test]
#[ignore = "needs the stage recordings; writes listening WAVs"]
fn render_weather_previews() {
    let root = std::env::var("SKATE_WEATHER_AUDIO_ROOT").expect("SKATE_WEATHER_AUDIO_ROOT");
    let out = std::path::PathBuf::from(std::env::var("SKATE_WEATHER_PREVIEW_OUT").expect("SKATE_WEATHER_PREVIEW_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    let bank = Arc::new(Bank::load(Path::new(&root)));
    assert!(!bank.is_empty(), "no recordings at {root}");
    let save = |name: &str, samples: Vec<f32>| {
        write_wav(&out.join(name), &samples);
        println!("wrote {name}");
    };
    let covered = |roof_metal: bool| move |t: f32| Params { cover: if (12. ..28.).contains(&t) { 1. } else { 0. }, roof_metal, ..heavy() };
    save("01-light-rain-concrete.wav", render(&bank, 30., |_| Params { rain: 0.5, surfaces: LIGHT_HARD, master: 1., ..Default::default() }, &[]));
    save("02-heavy-rain-street.wav", render(&bank, 30., |_| heavy(), &[]));
    save("03-heavy-rain-under-metal-roof.wav", render(&bank, 40., covered(true), &[]));
    save("04-heavy-rain-under-concrete-bridge.wav", render(&bank, 40., covered(false), &[]));
    for (name, d, azimuth) in
        [("05-thunder-1-2km.wav", 1200., 0.4), ("06-thunder-2-5km.wav", 2500., -0.6), ("07-thunder-3-8km.wav", 3800., 0.)]
    {
        println!("{name}: strike at 2 s, {d} m, lands at {:.2} s", 2. + delay_s(d));
        save(name, render(&bank, 25., |_| heavy(), &[(2., d, azimuth)]));
    }
    let storm = [(3., 1600., 0.5), (22., 3400., -1.), (41., 2200., 0.2)];
    for (at, d, _) in storm {
        println!("08-storm-minute.wav: strike at {at} s, {d} m, lands at {:.2} s", at + delay_s(d));
    }
    save("08-storm-minute.wav", render(&bank, 60., |_| heavy(), &storm));
    save(
        "09-wet-wheels.wav",
        render(
            &bank,
            20.,
            |t| Params { rain: 0.5, surfaces: LIGHT_HARD, master: 1., wheel_speed: 8. * (1. - (t - 10.).abs() / 10.), ..Default::default() },
            &[],
        ),
    );
    save("10-paused-menu.wav", render(&bank, 20., |t| Params { paused: t >= 8., ..heavy() }, &[]));
}

/// Review M8: the mixer's CPU at its worst, the real recordings: heavy rain on every surface, half under a concrete
/// roof (8 loops decoding), a strike in each thunder bin every 20 s (3 voices more), in the mixer thread's
/// 512-frame refills. Run like the previews (SKATE_WEATHER_AUDIO_ROOT).
#[test]
#[ignore = "needs the stage recordings; measures the mixer's CPU"]
fn mixer_cpu_budget() {
    let root = std::env::var("SKATE_WEATHER_AUDIO_ROOT").expect("SKATE_WEATHER_AUDIO_ROOT");
    let bank = Arc::new(Bank::load(Path::new(&root)));
    assert!(!bank.is_empty(), "no recordings at {root}");
    let mut m = Mixer::new(bank, 7);
    let every = [[0.4, 0.4], [0.15, 0.15], [0.15, 0.15], [0.15, 0.15], [0.15, 0.15]];
    m.set(Params { rain: 1., surfaces: every, cover: 0.6, master: 1., ..Default::default() });
    let mut buf = vec![0.; 512 * 2];
    let blocks = |seconds: usize| seconds * RATE as usize / 512;
    for _ in 0..blocks(10) {
        m.render(&mut buf); // every layer up to full level first
    }
    let start = std::time::Instant::now();
    for block in 0..blocks(60) {
        if block % blocks(20) == 0 {
            for d in [1200., 2200., 3600.] {
                m.strike(Thunder { azimuth: 0.3, distance_m: d, seed: block as u32 }, 0.);
            }
        }
        m.render(&mut buf);
    }
    let core = start.elapsed().as_secs_f64() / 60. * 100.;
    println!("weather mixer: {core:.2}% of a core (cargo test profile)");
    // 3.2% measured on the user's machine (2026-10-06): the bound is about twice that.
    assert!(core < 6.5, "{core:.2}% of a core");
}

/// Review I2: the packet decoder (headers parsed once, a byte seek to wrap) gives exactly what lewton's own reader
/// gives, for every recording: from the start, across a loop's wrap and from a granule seek.
#[test]
#[ignore = "needs the stage recordings"]
fn packet_decode_matches_lewton() {
    use super::{
        assets::{ClipData, ROLES},
        stream::LoopVoice,
    };
    use lewton::{inside_ogg::OggStreamReader, samples::InterleavedSamples};
    let root = std::env::var("SKATE_WEATHER_AUDIO_ROOT").expect("SKATE_WEATHER_AUDIO_ROOT");
    let bank = Bank::load(Path::new(&root));
    assert!(!bank.is_empty(), "no recordings at {root}");
    let decode = |reader: &mut OggStreamReader<std::io::Cursor<Arc<[u8]>>>, limit: usize| {
        let mut out = Vec::new();
        while out.len() < limit
            && let Some(packet) = reader.read_dec_packet_generic::<InterleavedSamples<f32>>().unwrap()
        {
            out.extend(packet.samples);
        }
        out
    };
    let mut clips = 0;
    for role in ROLES {
        for (n, clip) in bank.clips(role).iter().enumerate() {
            let ClipData::Ogg(bytes, _) = &clip.data else { continue };
            let ch = clip.channels;
            let frame = |s: &[f32], i: usize| [s[i * ch], s[i * ch + ch - 1]];
            let mut reference = OggStreamReader::new(std::io::Cursor::new(bytes.clone())).unwrap();
            let want = decode(&mut reference, usize::MAX);
            let mut voice = LoopVoice::new(clip, 0.).unwrap();
            for i in 0..want.len() / ch {
                assert_eq!(voice.next_frame(), Some(frame(&want, i)), "{role} {n}: frame {i}");
            }
            if clip.looped {
                for i in 0..RATE as usize {
                    assert_eq!(voice.next_frame(), Some(frame(&want, i)), "{role} {n}: frame {i} after the wrap");
                }
                // A start halfway in: lewton's page seek.
                let start = clip.frames / 2;
                reference.seek_absgp_pg(start).unwrap();
                let from = decode(&mut reference, RATE as usize * ch);
                let mut voice = LoopVoice::new(clip, 0.5).unwrap();
                for i in 0..from.len() / ch {
                    assert_eq!(voice.next_frame(), Some(frame(&from, i)), "{role} {n}: frame {i} from the middle");
                }
            } else {
                assert_eq!(voice.next_frame(), None, "{role} {n}: a one-shot ends");
            }
            clips += 1;
        }
    }
    println!("{clips} clips decode exactly as lewton's reader");
}
