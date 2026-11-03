//! Discovery for the direct route (AUDIO_HANDOFF §9.4): the retail audio game side booted as
//! original code over the engine's real runtime (banks, projects, MixMap) and the title's files.

use std::path::{Path, PathBuf};

use skate_audio_core::authored::AuthoredRuntime;

use super::catalog::PlayerAudioCatalog;
use super::title::{files as title_files, roots, OldGame};

fn assets() -> Option<PathBuf> {
    std::env::var_os("SKATE_ASSETS").map(PathBuf::from)
}

#[test]
#[ignore = "needs an installation: SKATE_ASSETS=<inst>/assets"]
fn the_game_side_boots_over_the_engine_runtime() {
    let Some(assets) = assets() else { return };
    let cache = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|root| root.join("Skate3RustEngine/audio-pcm-cache"));
    let catalog = PlayerAudioCatalog::from_assets(&assets, cache.as_deref()).unwrap();
    let mixmap = catalog.mixmap.clone().unwrap();
    // The game side loads every project and bank itself.
    let mut runtime = AuthoredRuntime::new(catalog.guest, vec![], vec![]).unwrap();
    // The master section FE the runtime's init built as original code; retail (University, free
    // skate): DCl0 100, Gai0 1, LI20 25000 Hz, HI20 0 Hz.
    {
        let g = runtime.guest();
        let root = g.u32(0x830C_FDBC).unwrap();
        let table = g.u32(root + 0x2C).unwrap();
        for m in 0..6 {
            let module = g.u32(table + 4 * m).unwrap();
            let data = g.u32(module + 16).unwrap();
            let class = g.u32(module + 20).unwrap();
            let id = g.u32(class + 0x24).unwrap().to_be_bytes();
            let words: Vec<String> = (0..3)
                .map(|k| format!("{:08X}/{}", g.u32(data + 4 * k).unwrap_or(0), g.f32(data + 4 * k).unwrap_or(0.0)))
                .collect();
            eprintln!("FE {m} {} {words:?}", String::from_utf8_lossy(&id));
        }
    }
    runtime.load_mixmap(&mixmap).unwrap();
    let db = assets.join("private/stock/data/db");
    let read = |name: &str| std::fs::read(db.join(name)).unwrap();
    let vault = skate_audio_core::attrib::Vault::parse(
        &read("skaterschema.vlt"),
        &read("skaterschema.bin"),
        &read("skatercollections.vlt"),
        &read("skatercollections.bin"),
    )
    .unwrap();
    let (result, traces) = runtime.game_boot(vault, title_files(roots(&assets)));
    for (i, trace) in traces.iter().enumerate() {
        eprintln!("step {i}: {} distinct calls", trace.calls.len());
        if std::env::var_os("SKATE_BOOT_CALLS").is_some() {
            for (target, count) in &trace.calls {
                eprintln!("  call {target:08X} x{count}");
            }
        }
    }
    if std::env::var_os("SKATE_BOOT_FILES").is_some() {
        for (path, at) in &runtime.game_files() {
            eprintln!("  file {path} -> {at:08X}");
        }
    }
    match result {
        Ok(root) => eprintln!("BOOTED root={root:08X}"),
        Err(e) => panic!("stopped: {e}"),
    }
    let frames: usize = std::env::var("SKATE_FRAMES").ok().and_then(|v| v.parse().ok()).unwrap_or(60);
    let skater = runtime.game_skater().unwrap();
    runtime.game_set_district(super::world::hash64("dist_university")).unwrap();
    {
        // Riding on four wheels at the Library (State 101, category 100; Collision+0 = 4).
        let g = runtime.guest_mut();
        let s = &skater.slots;
        g.set_u32(s[7] + 12, 100).unwrap();
        g.set_u32(s[7] + 16, 101).unwrap();
        g.set_u32(s[6], 4).unwrap();
        for (i, v) in [281.5f32, 81.5, -447.8].iter().enumerate() {
            g.set_u32(s[9] + 64 + 4 * i as u32, v.to_bits()).unwrap();
        }
        g.set_u32(s[1] + 164, 3.0f32.to_bits()).unwrap();
        // The deck (B+0 +144 position, +128 forward) under the skater.
        for (i, v) in [281.5f32, 81.1, -447.8].iter().enumerate() {
            g.set_u32(s[0] + 144 + 4 * i as u32, v.to_bits()).unwrap();
        }
        g.set_u32(s[0] + 128 + 8, (-1.0f32).to_bits()).unwrap();
        // The camera (game +0x2FD20, a 4x4 world matrix: rows right, up, at, position): 3 m behind
        // and 1.5 m above the skater, looking at it.
        let game = g.u32(0x8308_3C38).unwrap();
        let rows: [[f32; 4]; 4] = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, -0.447, -0.894, 0.0],
            [281.5, 83.0, -444.8, 1.0],
        ];
        for (r, row) in rows.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                g.set_u32(game + 0x2_FD20 + 0x1230 + 16 * r as u32 + 4 * c as u32, v.to_bits()).unwrap();
            }
        }
        g.set_u8(game + 0x2_FD20 + 0x1220, 1).unwrap();
        // The frame's painter keys at the Library, as the engine writes them (`game_side.rs`).
        let world = super::world::WorldAudio::load(&assets).unwrap();
        let district = world.district("University").unwrap();
        for (offset, layer) in [(0x40u32, "audio_emitters"), (0x48, "audio_ambience"), (0x50, "audio_reverb")] {
            let key = super::world::sample(district.layer(layer), 281.5, -447.8).unwrap_or(0);
            eprintln!("painter {layer} {key:016X}");
            g.set_u64(game + 0x2_F070 + offset, key).unwrap();
        }
    }
    let mut loudest = 0f32;
    let mut energy = 0f64;
    for frame in 0..frames {
        if let Err(e) = runtime.game_publish(&skater) {
            panic!("frame {frame} record: {e}");
        }
        let (done, trace) = runtime.game_frame(1.0 / 60.0);
        if let Err(e) = done {
            eprintln!("frame {frame}: {} distinct calls", trace.calls.len());
            panic!("frame {frame} stopped: {e}");
        }
        let watch = [0x824B_0DA8u32, 0x828E_2B48, 0x828E_2D18, 0x828E_2C78, 0x824F_1FB0, 0x824C_5CA8];
        let seen: Vec<String> = watch
            .iter()
            .map(|a| format!("{a:08X}x{}", trace.calls.get(a).copied().unwrap_or(0)))
            .collect();
        let mut peak = 0f32;
        for _ in 0..3 {
            let block = match runtime.pump_once() {
                Ok(block) => block,
                Err(e) => {
                    let near = runtime
                        .game_files()
                        .into_iter()
                        .filter(|(_, at)| *at != 0 && *at <= e.address)
                        .max_by_key(|(_, at)| *at);
                    panic!("frame {frame} pump: {e} (in {near:?} +{:#x})", near.as_ref().map_or(0, |n| e.address - n.1));
                }
            };
            for v in block {
                peak = peak.max(v.abs());
                energy += f64::from(v) * f64::from(v);
            }
        }
        if frame == 5 && std::env::var_os("SKATE_FRAME_CALLS").is_some() {
            for (t, n) in &trace.calls {
                eprintln!("  fcall {t:08X} x{n}");
            }
        }
        if frame % 20 == 1 {
            let g = runtime.guest();
            let game = g.u32(0x8308_3C38).unwrap();
            let f = game + 0x2_F070;
            let rec = f + 0xF0;
            eprintln!(
                "record: count {} +98 {:08X} +9c {:08X} +a8 {:08X} +214 {:08X} gamemode {:08X} root230 {:08X}",
                g.u32(f + 0xC).unwrap(),
                g.u32(rec + 0x98).unwrap(),
                g.u32(rec + 0x9C).unwrap(),
                g.u32(rec + 0xA8).unwrap(),
                g.u32(rec + 0x214).unwrap(),
                g.u32(0x8306_70CC).unwrap(),
                g.u32(g.u32(0x830C_FDC4).unwrap() + 0x230).unwrap()
            );
            let v: Vec<f32> = (0..32).map(|i| g.f32(rec + 4 * i).unwrap_or(f32::NAN)).collect();
            eprintln!("record floats {v:?}");
        }
        if frame % 10 == 0 || frame + 1 == frames {
            eprintln!("frame {frame}: calls {} {seen:?} peak {peak}", trace.calls.len());
        }
    }
    loudest = loudest.max(0.0);
    {
        let g = runtime.guest();
        let t = g.u32(g.u32(0x830C_FDC4).unwrap() + 0x1E8).unwrap();
        eprintln!(
            "TRIG +108 {:016X} +110 {:016X} +118 {:08X} +11d {:?}",
            g.u64(t + 0x108).unwrap(),
            g.u64(t + 0x110).unwrap(),
            g.u32(t + 0x118).unwrap(),
            (0..48).map(|i| g.u8(t + 0x11D + i).unwrap() as char).collect::<String>()
        );
    }
    if std::env::var_os("SKATE_MIXMAP_DUMP").is_some() {
        use skate_audio_core::mixmap::{self, controller};
        let g = runtime.guest();
        let root = g.u32(0x830C_FDC4).unwrap();
        let host = g.u32(g.u32(root + 4 + 8).unwrap()).unwrap();
        for (ctrl, key) in mixmap::controllers(g, host).unwrap() {
            let ins: Vec<String> = (0..16).map(|i| format!("{:X}", controller::get(g, ctrl, i).unwrap_or(0xDEAD))).collect();
            let outs: Vec<String> = (0..16).map(|i| format!("{:X}", controller::read_u16(g, ctrl, i).unwrap_or(0xDEAD))).collect();
            eprintln!("MX {key:08X} in [{}] out [{}]", ins.join(" "), outs.join(" "));
        }
    }
    {
        let g = runtime.guest();
        let l = g.u32(0x830C_FDD4).unwrap();
        let v: Vec<f32> = (0..32).map(|i| g.f32(l + 4 * i).unwrap_or(f32::NAN)).collect();
        eprintln!("listener {l:08X}: {v:?}");
    }
    eprintln!("RAN {frames} frames; energy {energy} voices {:?}", runtime.stats());
}

/// A first play: the game side's boot on an empty PCM cache (run with LOCALAPPDATA pointed at an empty folder), with
/// the map's prewarm beside it. SKATE_COLD_MODE: none (boot alone), new (title::prewarm), old (the 11-01 prewarm:
/// every bank through files.decode, the shared decode lock). 11-02 on 12 threads: 150 s one sample at a time, 35 s
/// with decode_ranges' workers.
#[test]
#[ignore = "needs an installation: SKATE_ASSETS=<inst>/assets"]
fn cold_cache_boot_beside_prewarm() {
    let Some(assets) = assets() else { return };
    let mode = std::env::var("SKATE_COLD_MODE").unwrap_or_else(|_| "new".into());
    let world = super::world::WorldAudio::load(&assets).unwrap();
    match mode.as_str() {
        "new" => super::title::prewarm(&assets, &world),
        "old" => {
            let volumes = world.classes.get("audio_volume").unwrap();
            let mut names: Vec<String> = volumes.order.iter()
                .filter_map(|key| volumes.field(*key, "Hash_BE88128A30BE926E"))
                .map(|f| f.data.clone())
                .filter(|n| n.ends_with(".abk") || n.ends_with(".bnk"))
                .collect();
            names.sort();
            names.dedup();
            let roots = roots(&assets);
            std::thread::spawn(move || {
                let mut files = title_files(roots);
                for name in names {
                    if let Some(bytes) = (files.read)(&format!("data/audio/{name}")) {
                        let _ = (files.decode)(&name, &bytes);
                    }
                }
            });
        }
        _ => {}
    }
    std::thread::sleep(std::time::Duration::from_millis(500));
    let cache = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).map(|r| r.join("Skate3RustEngine/audio-pcm-cache"));
    let catalog = PlayerAudioCatalog::from_assets(&assets, cache.as_deref()).unwrap();
    let mixmap = catalog.mixmap.clone().unwrap();
    let mut runtime = AuthoredRuntime::new(catalog.guest, vec![], vec![]).unwrap();
    runtime.load_mixmap(&mixmap).unwrap();
    let db = assets.join("private/stock/data/db");
    let read = |name: &str| std::fs::read(db.join(name)).unwrap();
    let vault = skate_audio_core::attrib::Vault::parse(
        &read("skaterschema.vlt"), &read("skaterschema.bin"), &read("skatercollections.vlt"), &read("skatercollections.bin"),
    ).unwrap();
    let started = std::time::Instant::now();
    let (result, _) = runtime.game_boot(vault, title_files(roots(&assets)));
    result.unwrap_or_else(|e| panic!("stopped: {e}"));
    eprintln!("COLD mode={mode} game side booted (banks installed) in {:.1}s", started.elapsed().as_secs_f32());
}

#[test]
#[ignore = "needs an installation: SKATE_ASSETS=<inst>/assets"]
fn vault_probe() {
    let Some(assets) = assets() else { return };
    let db = assets.join("private/stock/data/db");
    let read = |name: &str| std::fs::read(db.join(name)).unwrap();
    let vault = skate_audio_core::attrib::Vault::parse(
        &read("skaterschema.vlt"),
        &read("skaterschema.bin"),
        &read("skatercollections.vlt"),
        &read("skatercollections.bin"),
    )
    .unwrap();
    let class = 0x11A6_3187_8B23_9355u64;
    let key = 0xD7ED_BD36_2D7D_2152u64;
    let field = 0x6C85_7ADC_9613_515Cu64;
    let c = vault.classes.get(&class);
    eprintln!("class present {}", c.is_some());
    if let Some(c) = c {
        eprintln!("field {:?}", c.fields.get(&field).map(|f| (f.offset, f.size, f.flags)));
        eprintln!("order {:X?}", c.order);
    }
    let col = vault.collections.get(&(class, key));
    eprintln!("collection {:?}", col.map(|c| (c.layout, c.parent, c.entries.len())));
}

/// SFXObj_Ambience as original code over the hand-ported runtime (banks, projects and MixMap
/// installed natively), the district's beds installed, then 10 s of frames at the University
/// Library: the component must pick the painted bed, fade it in and play it.
#[test]
#[ignore = "needs an installation: SKATE_ASSETS=<inst>/assets"]
fn the_ambience_bed_plays_over_the_hand_ported_runtime() {
    use super::world::{sample, WorldAudio};
    let Some(assets) = assets() else { return };
    let cache = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|root| root.join("Skate3RustEngine/audio-pcm-cache"));
    let catalog = PlayerAudioCatalog::from_assets(&assets, cache.as_deref()).unwrap();
    let mixmap = catalog.mixmap.clone().unwrap();
    let mut runtime = AuthoredRuntime::new(catalog.guest, catalog.projects, catalog.banks).unwrap();
    runtime.load_mixmap(&mixmap).unwrap();
    // The player sound's free-skate globals (`PlayerSound::new`).
    for &(key, id, value) in skate_audio_core::mixmap::inputs::FREE_SKATE_GLOBALS
        .iter()
        .chain(skate_audio_core::mixmap::inputs::FREE_SKATE_MUSIC_VU)
    {
        let ctrl = runtime.mixmap_controller(key).unwrap();
        runtime.mixmap_set(ctrl, id, value).unwrap();
    }
    let db = assets.join("private/stock/data/db");
    let read = |name: &str| std::fs::read(db.join(name)).unwrap();
    let vault = skate_audio_core::attrib::Vault::parse(
        &read("skaterschema.vlt"),
        &read("skaterschema.bin"),
        &read("skatercollections.vlt"),
        &read("skatercollections.bin"),
    )
    .unwrap();
    let before: Vec<(u32, Vec<u8>)> = runtime
        .guest()
        .segments()
        .iter()
        .filter(|s| (0x8200_0000..0x8310_0000).contains(&s.base))
        .map(|s| (s.base, s.bytes.clone()))
        .collect();
    let started = std::time::Instant::now();
    let (result, traces) = runtime.ambience_boot(vault, title_files(roots(&assets)));
    // Words the boot overwrote that already held something: the hand-ported runtime's state.
    let mut overwritten = Vec::new();
    for (base, old) in &before {
        let seg = runtime.guest().segments().iter().find(|s| s.base == *base).unwrap();
        for at in (0..old.len().min(seg.bytes.len())).step_by(4) {
            if old[at..at + 4] != seg.bytes[at..at + 4] && old[at..at + 4] != [0; 4] {
                overwritten.push((base + at as u32, old[at..at + 4].to_vec(), seg.bytes[at..at + 4].to_vec()));
            }
        }
    }
    eprintln!("boot overwrote {} nonzero image words:", overwritten.len());
    for (at, old, new) in overwritten.iter().take(80) {
        eprintln!("  {at:08X}: {old:02X?} -> {new:02X?}");
    }
    eprintln!("ambience boot {:.2}s", started.elapsed().as_secs_f32());
    if std::env::var_os("SKATE_BOOT_CALLS").is_some() {
        for (i, trace) in traces.iter().enumerate() {
            for (target, count) in &trace.calls {
                eprintln!("  step {i} call {target:08X} x{count}");
            }
        }
    }
    let component = result.unwrap_or_else(|e| panic!("stopped: {e}"));
    eprintln!("ambience component {component:08X}");
    for (path, at) in runtime.game_files() {
        eprintln!("  file {path} -> {at:08X}");
    }
    let world = WorldAudio::load(&assets).unwrap();
    let mut beds = super::title::Beds::default();
    let n = beds.enter(&mut runtime, &world, &assets, "University", false).unwrap();
    eprintln!("beds installed {n}");
    let district = world.district("University").unwrap();
    let key = sample(district.layer("audio_ambience"), 281.5, -447.8).unwrap_or(0);
    eprintln!("painter key {key:016X}");
    let class = &world.classes["aud_wp_ambiences"];
    let mut palette: Vec<u64> = district.layer("audio_ambience").iter().flat_map(|t| t.palette.clone()).collect();
    palette.sort();
    palette.dedup();
    for k in &palette {
        eprintln!(
            "  palette {k:016X}: bed {:?} fade in/out {:?}/{:?} volume {:?}",
            class.field(*k, "Hash_4042D2F96C4E4B8C").map(|f| &f.data),
            class.field(*k, "Hash_17C3EB9C543E1210").map(|f| &f.data),
            class.field(*k, "Hash_2E2ABEBF53F107EC").map(|f| &f.data),
            class.field(*k, "volume").map(|f| &f.data),
        );
    }
    {
        // Unpainted (key 0): the component falls back to the title's default ambience.
        runtime.ambience_process(0, 1.0 / 60.0).unwrap();
        let (state, bed) = runtime.ambience_status().unwrap();
        eprintln!(
            "unpainted: state {state} bed {bed:016X} = bed {:?}",
            class.field(bed, "Hash_4042D2F96C4E4B8C").map(|f| &f.data)
        );
    }
    let other = palette.iter().copied().find(|k| *k != key).unwrap();
    // SKATE_BED_SKATE2=1: Skate 2's soho (bed 6) then dt_main (bed 8, at the Downtown Skate Line teleport) rows in
    // their Skate 3 slots. SKATE_BED_SKATE1=1: Skate 1's suburbs_1 (bed 1) then finance_downtown (bed 5) rows, at
    // two of BAM.loc's challenge locators. Both from skate2port/old_ambience.py.
    let (key, other) = if std::env::var_os("SKATE_BED_SKATE1").is_some() {
        beds.enter(&mut runtime, &world, &assets, "Skate_1_San_Vanelona", true).unwrap();
        (
            old_key(&mut beds, &mut runtime, &world, &assets, OldGame::Skate1, [-1067.3, -1826.8]),
            old_key(&mut beds, &mut runtime, &world, &assets, OldGame::Skate1, [591.8, 1060.9]),
        )
    } else if std::env::var_os("SKATE_BED_SKATE2").is_some() {
        beds.enter(&mut runtime, &world, &assets, "New_San_Vanelona", true).unwrap();
        (
            old_key(&mut beds, &mut runtime, &world, &assets, OldGame::Skate2, SOHO),
            old_key(&mut beds, &mut runtime, &world, &assets, OldGame::Skate2, DOWNTOWN),
        )
    } else {
        (key, other)
    };
    let dt = 1.0f32 / 60.0;
    let mut peak = 0f32;
    let mut spent = std::time::Duration::ZERO;
    let frames: usize = std::env::var("SKATE_FRAMES").ok().and_then(|v| v.parse().ok()).unwrap_or(600);
    for frame in 0..frames {
        // Halfway, walk into the next painted area: the bed must cross-fade.
        let key = if frame >= frames / 2 { other } else { key };
        let t = std::time::Instant::now();
        runtime.ambience_process(key, dt).unwrap_or_else(|e| panic!("frame {frame} process: {e}"));
        spent += t.elapsed();
        runtime.mixmap_tick(dt).unwrap();
        let t = std::time::Instant::now();
        runtime.ambience_update().unwrap_or_else(|e| panic!("frame {frame} update: {e}"));
        spent += t.elapsed();
        let mut window = 0f32;
        for _ in 0..3 {
            for v in runtime.pump_once().unwrap_or_else(|e| panic!("frame {frame} pump: {e}")) {
                window = window.max(v.abs());
            }
        }
        peak = peak.max(window);
        if frame % 30 == 0 {
            if let Some(ctrl) = runtime.mixmap_controller(skate_audio_core::mixmap::key(2, 2, 0, 0)) {
                let ins: Vec<String> = (0..8).map(|i| format!("{:X}", runtime.mixmap_get(ctrl, i).unwrap_or(0xDEAD))).collect();
                let outs: Vec<String> = (0..8).map(|i| format!("{:X}", runtime.mixmap_raw(ctrl, i))).collect();
                eprintln!("  controller in [{}] out [{}]", ins.join(" "), outs.join(" "));
            }
            let g = runtime.guest();
            eprintln!(
                "frame {frame}: peak {window:.5} voices {:?} state {} bed {:016X}",
                runtime.stats(),
                g.u32(component + 0x40).unwrap(),
                g.u64(component + 0x30).unwrap()
            );
        }
    }
    eprintln!("ambience process+update: {:.3} ms per frame", spent.as_secs_f64() * 1000.0 / frames as f64);
    assert!(peak > 1.0e-4, "the ambience bed made no sound (peak {peak})");
}

/// The master section FE's first parameter per module (`[[bus root+0x2C]+4m]`, data word 1).
/// Points on Skate 2's city painted soho and dt_main.
const SOHO: [f32; 2] = [743.8, -19.2];
const DOWNTOWN: [f32; 2] = [42.0, 537.0];

/// An old game's painter key at (x, z) once the loader thread has its bed in the slot.
fn old_key(
    beds: &mut super::title::Beds,
    runtime: &mut AuthoredRuntime,
    world: &super::world::WorldAudio,
    assets: &Path,
    game: OldGame,
    [x, z]: [f32; 2],
) -> u64 {
    let started = std::time::Instant::now();
    loop {
        let (key, gain) = beds.old_game_key(runtime, world, assets, game, x, z);
        if key != 0 {
            eprintln!("{game:?} ({x}, {z}): key {key:016X} gain {gain} after {:.1}s", started.elapsed().as_secs_f32());
            return key;
        }
        assert!(started.elapsed().as_secs() < 300, "{game:?} ({x}, {z}): no bed");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
#[ignore = "needs an installation: SKATE_ASSETS=<inst>/assets (with skate2port/old_ambience.py run)"]
fn skate3_to_skate2_and_back_leaves_the_skate3_slots_as_loaded() {
    let Some(assets) = assets() else { return };
    let cache = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|root| root.join("Skate3RustEngine/audio-pcm-cache"));
    let catalog = PlayerAudioCatalog::from_assets(&assets, cache.as_deref()).unwrap();
    let mixmap = catalog.mixmap.clone().unwrap();
    let mut runtime = AuthoredRuntime::new(catalog.guest, catalog.projects, catalog.banks).unwrap();
    runtime.load_mixmap(&mixmap).unwrap();
    let db = assets.join("private/stock/data/db");
    let read = |name: &str| std::fs::read(db.join(name)).unwrap();
    let vault = skate_audio_core::attrib::Vault::parse(
        &read("skaterschema.vlt"),
        &read("skaterschema.bin"),
        &read("skatercollections.vlt"),
        &read("skatercollections.bin"),
    )
    .unwrap();
    runtime.ambience_boot(vault, title_files(roots(&assets))).0.unwrap();
    let world = super::world::WorldAudio::load(&assets).unwrap();
    let resident = super::title::read_audio(&roots(&assets), "ambienceresident.big").unwrap();
    let index = skate_audio_formats::eb::Archive::parse(&resident).unwrap();
    let base = super::title::resident_at(&runtime).unwrap();
    // Each bed's slot address and the header the title loaded there.
    let slot = |bed: u32| {
        let entry = super::title::slot(&index, bed).unwrap();
        (base + entry.range().start as u32, resident[entry.range()].to_vec())
    };
    let header = |runtime: &AuthoredRuntime, (at, loaded): &(u32, Vec<u8>)| {
        runtime.guest().span(*at, loaded.len()).unwrap().to_vec()
    };
    let (soho, financial) = (slot(6), slot(8));
    let mut beds = super::title::Beds::default();
    // University's palette: beds 7..10 (Financial's slot 8 among them, SoHo's 6 not).
    beds.enter(&mut runtime, &world, &assets, "University", false).unwrap();
    assert!(runtime.has_pcm(financial.0) && !runtime.has_pcm(soho.0));

    beds.enter(&mut runtime, &world, &assets, "New_San_Vanelona", true).unwrap();
    old_key(&mut beds, &mut runtime, &world, &assets, OldGame::Skate2, SOHO);
    old_key(&mut beds, &mut runtime, &world, &assets, OldGame::Skate2, DOWNTOWN);
    let skate2_soho = std::fs::read(assets.join(super::title::SKATE2_BEDS).join("06_soho_amb.snr")).unwrap();
    assert_eq!(header(&runtime, &soho), skate2_soho);
    assert!(runtime.has_pcm(soho.0));

    beds.enter(&mut runtime, &world, &assets, "University", false).unwrap();
    assert_eq!(header(&runtime, &soho), soho.1, "SoHo slot header restored");
    assert_eq!(header(&runtime, &financial), financial.1, "Financial slot header restored");
    assert!(!runtime.has_pcm(soho.0), "no Skate 2 PCM left in a slot University does not use");
    assert!(runtime.has_pcm(financial.0), "University's bed 8 installed again");
    assert_eq!(beds.len(), 5, "University's 7..10 and the title screen's 23");
}

fn master_section(runtime: &AuthoredRuntime) -> Vec<(String, f32)> {
    let g = runtime.guest();
    let root = g.u32(0x830C_FDBC).unwrap();
    let table = g.u32(root + 0x2C).unwrap();
    (0..6)
        .map(|m| {
            let module = g.u32(table + 4 * m).unwrap();
            let data = g.u32(module + 16).unwrap();
            let class = g.u32(module + 20).unwrap();
            let id = g.u32(class + 0x24).unwrap().to_be_bytes();
            (String::from_utf8_lossy(&id).into_owned(), g.f32(data + 4).unwrap_or(f32::NAN))
        })
        .collect()
}

/// SFXObj_Master as original code over the hand-ported runtime: after its frames the master
/// section FE holds retail's free-skate values (read from the running game, University): DCl0
/// 100, Gai0 1, LI20 25000 Hz, HI20 0 Hz.
#[test]
#[ignore = "needs an installation: SKATE_ASSETS=<inst>/assets"]
fn the_master_section_matches_retail_once_sfxobj_master_runs() {
    let Some(assets) = assets() else { return };
    let cache = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|root| root.join("Skate3RustEngine/audio-pcm-cache"));
    let catalog = PlayerAudioCatalog::from_assets(&assets, cache.as_deref()).unwrap();
    let mixmap = catalog.mixmap.clone().unwrap();
    let mut runtime = AuthoredRuntime::new(catalog.guest, catalog.projects, catalog.banks).unwrap();
    runtime.load_mixmap(&mixmap).unwrap();
    for &(key, id, value) in skate_audio_core::mixmap::inputs::FREE_SKATE_GLOBALS
        .iter()
        .chain(skate_audio_core::mixmap::inputs::FREE_SKATE_MUSIC_VU)
    {
        let ctrl = runtime.mixmap_controller(key).unwrap();
        runtime.mixmap_set(ctrl, id, value).unwrap();
    }
    let db = assets.join("private/stock/data/db");
    let read = |name: &str| std::fs::read(db.join(name)).unwrap();
    let vault = skate_audio_core::attrib::Vault::parse(
        &read("skaterschema.vlt"),
        &read("skaterschema.bin"),
        &read("skatercollections.vlt"),
        &read("skatercollections.bin"),
    )
    .unwrap();
    eprintln!("FE at init: {:?}", master_section(&runtime));
    runtime.ambience_boot(vault, title_files(roots(&assets))).0.unwrap();
    let dt = 1.0f32 / 60.0;
    for frame in 0..120 {
        runtime.master_process(dt).unwrap_or_else(|e| panic!("frame {frame} process: {e}"));
        runtime.mixmap_tick(dt).unwrap();
        runtime.master_update().unwrap_or_else(|e| panic!("frame {frame} update: {e}"));
        runtime.pump_once().unwrap();
    }
    let fe = master_section(&runtime);
    eprintln!("FE after SFXObj_Master: {fe:?}");
    let master = runtime.mixmap_controller(0x4000_0020).unwrap();
    let inputs: Vec<u32> = (0..8).map(|i| runtime.mixmap_get(master, i).unwrap_or(u32::MAX)).collect();
    eprintln!("Master inputs {inputs:?}");
    let expect = [("DCl0", 100.0), ("Gai0", 1.0), ("LI20", 25_000.0), ("HI20", 0.0)];
    for (name, value) in expect {
        let got = fe.iter().find(|(n, _)| n == name).unwrap().1;
        assert!((got - value).abs() <= value.abs() * 1e-3 + 1e-3, "{name}: {got} vs retail {value}");
    }
}

/// The hybrid: the full title boot over the hand-ported runtime (its banks, projects and MixMap
/// already installed), with no skater published, so the title runs the world (reverb, ambience,
/// master, emitters, zones) and the hand-port keeps the player.
#[test]
#[ignore = "needs an installation: SKATE_ASSETS=<inst>/assets"]
fn the_title_world_runs_over_the_hand_ported_runtime() {
    let Some(assets) = assets() else { return };
    let cache = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|root| root.join("Skate3RustEngine/audio-pcm-cache"));
    let catalog = PlayerAudioCatalog::from_assets(&assets, cache.as_deref()).unwrap();
    let mixmap = catalog.mixmap.clone().unwrap();
    let mut runtime = AuthoredRuntime::new(catalog.guest, catalog.projects, catalog.banks).unwrap();
    runtime.load_mixmap(&mixmap).unwrap();
    for &(key, id, value) in skate_audio_core::mixmap::inputs::FREE_SKATE_GLOBALS
        .iter()
        .chain(skate_audio_core::mixmap::inputs::FREE_SKATE_MUSIC_VU)
    {
        let ctrl = runtime.mixmap_controller(key).unwrap();
        runtime.mixmap_set(ctrl, id, value).unwrap();
    }
    let db = assets.join("private/stock/data/db");
    let read = |name: &str| std::fs::read(db.join(name)).unwrap();
    let vault = skate_audio_core::attrib::Vault::parse(
        &read("skaterschema.vlt"),
        &read("skaterschema.bin"),
        &read("skatercollections.vlt"),
        &read("skatercollections.bin"),
    )
    .unwrap();
    let started = std::time::Instant::now();
    let (result, _) = runtime.game_boot(vault, title_files(roots(&assets)));
    let root = result.unwrap_or_else(|e| panic!("stopped: {e}"));
    eprintln!("BOOTED root={root:08X} in {:.1}s", started.elapsed().as_secs_f32());
    runtime.wire_environment().unwrap();
    // `SKATE_TW_MAP` (default University) at its first reverb zone's centre, or the Library.
    let map = std::env::var("SKATE_TW_MAP").unwrap_or_else(|_| "University".into());
    let snake: String = map.chars().enumerate().flat_map(|(i, c)| {
        let lower = c.to_ascii_lowercase();
        if c.is_ascii_uppercase() && i > 0 { vec!['_', lower] } else { vec![lower] }
    }).collect();
    runtime.game_set_district(super::world::hash64(&format!("dist_{snake}"))).unwrap();
    let world = super::world::WorldAudio::load(&assets).unwrap();
    let n = super::title::Beds::default().enter(&mut runtime, &world, &assets, &map, false).unwrap();
    eprintln!("beds installed {n}");
    let district = world.district(&map).unwrap();
    let (x, y, z) = match map.as_str() {
        "University" => (281.5f32, 81.5f32, -447.8f32),
        _ => {
            let p = world.reverb_zones(district)[0].position;
            (p[0], p[1], p[2])
        }
    };
    {
        let g = runtime.guest_mut();
        let game = g.u32(0x8308_3C38).unwrap();
        let rows: [[f32; 4]; 4] =
            [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, -0.447, -0.894, 0.0], [x, y + 1.5, z + 3.0, 1.0]];
        for (r, row) in rows.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                g.set_u32(game + 0x2_FD20 + 0x1230 + 16 * r as u32 + 4 * c as u32, v.to_bits()).unwrap();
            }
        }
        g.set_u8(game + 0x2_FD20 + 0x1220, 1).unwrap();
        for (offset, layer) in [(0x40u32, "audio_emitters"), (0x48, "audio_ambience"), (0x50, "audio_reverb")] {
            let key = super::world::sample(district.layer(layer), x, z).unwrap_or(0);
            g.set_u64(game + 0x2_F070 + offset, key).unwrap();
        }
        // No skater (count 0), but the listener's followed record at the Library.
        let frame = game + 0x2_F070;
        let record = frame + 0xF0;
        for (i, v) in [x, y, z, 1.0].iter().enumerate() {
            g.set_u32(record + 4 * i as u32, v.to_bits()).unwrap();
        }
        g.set_u32(frame + 8, record).unwrap();
        g.set_u32(frame + 0xC, 0).unwrap();
        g.set_u32(frame, 1.0f32.to_bits()).unwrap();
    }
    let frames: usize = std::env::var("SKATE_FRAMES").ok().and_then(|v| v.parse().ok()).unwrap_or(900);
    let mut peak = 0f32;
    let t = std::time::Instant::now();
    let mut sum = 0u64;
    let mut dump: Vec<u8> = Vec::new();
    let mut in_frame = std::time::Duration::ZERO;
    let mut calls: std::collections::HashMap<u32, u64> = Default::default();
    for frame in 0..frames {
        let tf = std::time::Instant::now();
        let (done, trace) = runtime.game_frame(1.0 / 60.0);
        in_frame += tf.elapsed();
        for (a, n) in trace.calls {
            *calls.entry(a).or_default() += n;
        }
        done.unwrap_or_else(|e| panic!("frame {frame}: {e}"));
        runtime.mixmap_tick(1.0 / 60.0).unwrap();
        for _ in 0..3 {
            for v in runtime.pump_once().unwrap_or_else(|e| panic!("frame {frame} pump: {e}")) {
                peak = peak.max(v.abs());
                sum = sum.wrapping_mul(31).wrapping_add(u64::from(v.to_bits()));
                dump.extend_from_slice(&v.to_le_bytes());
            }
        }
        if frame % 150 == 0 {
            eprintln!("frame {frame} peak {peak} stats {:?}", runtime.stats());
        }
    }
    eprintln!("{frames} frames in {:.1}s (title frames {:.1}s), FE {:?}", t.elapsed().as_secs_f32(), in_frame.as_secs_f32(), master_section(&runtime));
    let mut top: Vec<_> = calls.into_iter().collect();
    top.sort_by_key(|t| std::cmp::Reverse(t.1));
    eprintln!("top calls {:X?}", &top[..top.len().min(25)]);
    eprintln!("output checksum {sum:016X} peak {peak}");
    if let Some(path) = std::env::var_os("SKATE_DUMP_OUT") {
        std::fs::write(path, &dump).unwrap();
    }
    assert!(peak > 0.0, "the world is silent");
}

/// Pump cost per block with no world, the ambience-only boot, or the title world
/// (`SKATE_WORLD_MODE=none|ambience|title`).
#[test]
#[ignore = "needs an installation: SKATE_ASSETS=<inst>/assets"]
fn world_mode_pump_cost() {
    let Some(assets) = assets() else { return };
    let cache = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|root| root.join("Skate3RustEngine/audio-pcm-cache"));
    let catalog = PlayerAudioCatalog::from_assets(&assets, cache.as_deref()).unwrap();
    let mixmap = catalog.mixmap.clone().unwrap();
    let mut runtime = AuthoredRuntime::new(catalog.guest, catalog.projects, catalog.banks).unwrap();
    runtime.load_mixmap(&mixmap).unwrap();
    let db = assets.join("private/stock/data/db");
    let read = |name: &str| std::fs::read(db.join(name)).unwrap();
    let vault = skate_audio_core::attrib::Vault::parse(
        &read("skaterschema.vlt"),
        &read("skaterschema.bin"),
        &read("skatercollections.vlt"),
        &read("skatercollections.bin"),
    )
    .unwrap();
    let mode = std::env::var("SKATE_WORLD_MODE").unwrap_or_default();
    match mode.as_str() {
        "ambience" => {
            runtime.ambience_boot(vault, title_files(roots(&assets))).0.unwrap();
            runtime.wire_environment().unwrap();
        }
        "title" => {
            runtime.game_boot(vault, title_files(roots(&assets))).0.unwrap();
            runtime.wire_environment().unwrap();
        }
        _ => {}
    }
    // A tone through the mix so the reverb has something to chew on: a hand-port post.
    if let Some(ctrl) = runtime.mixmap_controller(0x4000_0020) {
        let _ = ctrl;
    }
    let t = std::time::Instant::now();
    let mut sum = 0u64;
    let mut peak = 0f32;
    for _ in 0..600 {
        for v in runtime.pump_once().unwrap() {
            sum = sum.wrapping_mul(31).wrapping_add(u64::from(v.to_bits()));
            peak = peak.max(v.abs());
        }
    }
    eprintln!("mode {mode:?}: {:.3} ms per block, checksum {sum:016X} peak {peak}", t.elapsed().as_secs_f64() * 1000.0 / 600.0);
}

/// The front-end sounds (menus, session marker) as retail plays them: `GlobalFEPlaySound(id, 0)`
/// on the booted game side starts an `sk8_menu` Splice voice that reaches the output.
#[test]
#[ignore = "needs an installation: SKATE_ASSETS=<inst>/assets"]
fn front_end_sounds_play_through_the_original_code() {
    let Some(assets) = assets() else { return };
    let guest = super::catalog::load_guest_image(&assets.join("private/stock/audio-runtime-image")).unwrap();
    let mut runtime = AuthoredRuntime::new(guest, vec![], vec![]).unwrap();
    let db = assets.join("private/stock/data/db");
    let read = |name: &str| std::fs::read(db.join(name)).unwrap();
    let vault = skate_audio_core::attrib::Vault::parse(
        &read("skaterschema.vlt"),
        &read("skaterschema.bin"),
        &read("skatercollections.vlt"),
        &read("skatercollections.bin"),
    )
    .unwrap();
    runtime.wire_environment().unwrap();
    runtime.game_boot(vault, title_files(roots(&assets))).0.unwrap();
    let mut step = |runtime: &mut AuthoredRuntime, frames: usize| -> f32 {
        let mut peak = 0f32;
        for _ in 0..frames {
            runtime.game_frame(1.0 / 60.0).0.unwrap();
            for _ in 0..3 {
                for v in runtime.pump_once().unwrap() {
                    peak = peak.max(v.abs());
                }
            }
        }
        peak
    };
    step(&mut runtime, 60);
    let quiet = step(&mut runtime, 30);
    // core_a_button, crossbar_right, cellphone_place_marker.
    for id in [0x373D_38D7_EA16_7A71u64, 0x5E03_48C8_4D82_83E0, 0x0D6C_88A3_B91C_828F] {
        runtime.game_fe_sound(id).unwrap();
        let g = runtime.guest();
        let splice = g.u32(0x8308_4348).unwrap();
        let queued = g.u32(splice + 0xE18).unwrap();
        let peak = step(&mut runtime, 30);
        eprintln!("fe {id:016X}: queued {queued} peak {peak} (bed {quiet})");
        assert!(queued > 0 || peak > quiet * 1.5, "fe sound {id:016X} started nothing");
    }
}

/// The traffic sounds as retail plays them (item 24c): the root loads the traffic banks (`sub_82487168`), the cars
/// published into the game frame reach CSTATEMGR_TrafficCar and SFXObj_TrafficEngine, whose voice `sub_824D5B20`
/// posts TRAFFIC_CAR; an alarmed car (status 6) starts `c_car_alarm` (`sub_824D0D58`). Then Skate 2's banks (old_traffic.py)
/// in the same slots, byte for byte where the title put them.
#[test]
#[ignore = "needs an installation: SKATE_ASSETS=<inst>/assets (with skate2port/old_traffic.py run)"]
fn the_traffic_cars_sound_through_the_original_code() {
    use skate_audio_core::game::TrafficCar;
    let Some(assets) = assets() else { return };
    let guest = super::catalog::load_guest_image(&assets.join("private/stock/audio-runtime-image")).unwrap();
    let mut runtime = AuthoredRuntime::new(guest, vec![], vec![]).unwrap();
    let db = assets.join("private/stock/data/db");
    let read = |name: &str| std::fs::read(db.join(name)).unwrap();
    let vault = skate_audio_core::attrib::Vault::parse(
        &read("skaterschema.vlt"),
        &read("skaterschema.bin"),
        &read("skatercollections.vlt"),
        &read("skatercollections.bin"),
    )
    .unwrap();
    let set = super::title::TrafficSet::default();
    runtime.wire_environment().unwrap();
    runtime.game_boot(vault, super::title::with_traffic(title_files(roots(&assets)), &assets, set.clone())).0.unwrap();
    let skater = runtime.game_skater().unwrap();
    // Two cars ahead of the listener: a heavy engine (audio row ECC2CA55D565B5E3, engine type 0) driving, and a parked
    // one with its alarm going (status 6).
    let car = |handle: u32, status: u32, x: f32, speed: f32| TrafficCar {
        handle,
        accel: 0.5,
        speed,
        distance: x.abs(),
        flags: status << 28,
        key: 0xECC2_CA55_D565_B5E3,
        forward: [0.0, 0.0, 1.0],
        position: [x, 0.0, 5.0],
    };
    // The next map's cars have their own handles (traffic.rs `Sim::handles`).
    // `SKATE_TRAFFIC_ONLY=engine|alarm`: one car of the two, to hear each alone.
    let only = std::env::var("SKATE_TRAFFIC_ONLY").unwrap_or_default();
    let keep = |c: &TrafficCar| match only.as_str() {
        "engine" => c.flags == 0,
        "alarm" => c.flags != 0,
        _ => true,
    };
    let maps: [Vec<TrafficCar>; 2] = [
        [car(1, 0, 2.0, 8.0), car(2, 6, -3.0, 0.0)].into_iter().filter(keep).collect(),
        [car(0x10_0001, 0, 2.0, 8.0), car(0x10_0002, 6, -3.0, 0.0)].into_iter().filter(keep).collect(),
    ];
    type Calls = std::collections::HashMap<u32, u64>;
    let run = |runtime: &mut AuthoredRuntime, cars: &[TrafficCar], frames: usize| -> (Calls, f32, f32) {
        let (mut calls, mut peak, mut energy, mut samples) = (Calls::new(), 0f32, 0f64, 0usize);
        for _ in 0..frames {
            runtime.game_publish(&skater).unwrap();
            runtime.game_traffic(cars).unwrap();
            let (done, trace) = runtime.game_frame(1.0 / 60.0);
            done.unwrap();
            for (a, n) in trace.calls {
                *calls.entry(a).or_insert(0) += n;
            }
            for _ in 0..3 {
                for v in runtime.pump_once().unwrap() {
                    peak = peak.max(v.abs());
                    energy += f64::from(v) * f64::from(v);
                    samples += 1;
                }
            }
        }
        (calls, peak, (energy / samples.max(1) as f64).sqrt() as f32)
    };
    let banks = |runtime: &AuthoredRuntime| -> Vec<(String, u32)> {
        runtime.game_files().into_iter().filter(|(p, _)| p.contains("C0") || p.contains("Traffic_") || p.contains("car_alarms")).collect()
    };
    let (_, quiet, _) = run(&mut runtime, &[], 30);
    assert_eq!(runtime.game_traffic_banks(false).unwrap(), 3, "root +596: the traffic banks loaded");
    eprintln!("Skate 3 banks {:X?}", banks(&runtime));
    assert_eq!(banks(&runtime).len(), 11, "eight engines, horn, skid, alarms");
    let phases = [("Skate 3", &maps[0]), ("Skate 3 steady", &maps[0]), ("Skate 2", &maps[1]), ("Skate 2 steady", &maps[1])];
    let mut steady = 0f32;
    for (game, cars) in phases {
        let (calls, peak, rms) = run(&mut runtime, cars, 300);
        let n = |a: u32| calls.get(&a).copied().unwrap_or(0);
        eprintln!(
            "{game}: sync {} engine process {} engine voice {} alarm voice {} peak {peak} rms {rms} (quiet {quiet})",
            n(0x824F_2460),
            n(0x824D_6110),
            n(0x824D_5B20),
            n(0x824D_0D58)
        );
        if !game.ends_with("steady") && only.is_empty() {
            assert!(n(0x824D_5B20) > 0, "{game}: no TRAFFIC_CAR voice");
            assert!(n(0x824D_0D58) > 0, "{game}: no c_car_alarm voice");
        }
        // The same cars in the same spots sound about as loud from Skate 2's banks: their TRAFFIC_CAR reads its inputs
        // where Skate 3's payload has them (old_traffic.py car_inputs; unmoved, it took its engine type from Skate 3's
        // +64 and played about 9.5 dB louder).
        if game == "Skate 3 steady" {
            steady = rms;
        } else if game == "Skate 2 steady" {
            assert!(rms < 2.0 * steady && rms > 0.5 * steady, "Skate 2's cars at rms {rms}, Skate 3's {steady}");
        }
        if game == "Skate 3 steady" {
            *set.lock().unwrap() = super::title::traffic_banks(&assets, super::title::SKATE2_MAP);
            assert_eq!(runtime.game_traffic_banks(true).unwrap(), 3, "Skate 2's banks loaded");
            let dir = set.lock().unwrap().clone().unwrap();
            for (path, at) in banks(&runtime).into_iter().rev().take(11) {
                let name = path.replace('\\', "/").rsplit('/').next().unwrap().to_owned();
                // The tail (sample data; the loader patches the header and exports in place).
                let bytes = std::fs::read(dir.join(&name)).unwrap();
                let tail = bytes.len() - 256;
                assert_eq!(runtime.guest().span(at + tail as u32, 256).unwrap(), &bytes[tail..], "{name} is Skate 2's");
            }
        }
    }
}

/// Round2 bug 7 (10-13: dragging the movable ramp makes no sound): a DMO the game thread lists (`sub_827A3FD8`'s 96-byte
/// entry in the frame's `+216` / `+220`) reaches CSTATEMGR_DynamicObject (`sub_824F4108`) and
/// SFXCTL_DynamicObjectPhysics (`sub_824B5558`), and the title's own SFXObj_Moving plays it as retail does
/// (round2-research5): the Skate 2 tutorial wedge (characteristics row wedgeramp_large, slide row A41BAFC3CB72F7BC,
/// Moveable_Objects_Wood.abk) held at 2 m/s on the ground posts c_sliding_start (w8 = row +316 = 10) and
/// c_moveable_sliding_objects (w8 = 10, w19 = row +228 = 15000, w3 slewing up to fctiwz(min(2 * 3.6 / 8, 1) * 1000) =
/// 900); without contact both are released on the 15th frame (+160 against 15 / time scale); at 0.2 m/s at once; a DMO
/// nobody holds never starts (+116 is set only for the held entry, research5 Q4).
#[test]
#[ignore = "needs an installation: SKATE_ASSETS=<inst>/assets"]
fn the_held_wedge_drags_through_the_original_code() {
    use skate_audio_core::game::DmoEntry;
    type Seen = Vec<(u8, String, u32, Vec<u32>)>;
    static SEEN: std::sync::Mutex<Seen> = std::sync::Mutex::new(Vec::new());
    fn observe(kind: u8, object: &str, node: u32, payload: &[u32]) {
        SEEN.lock().unwrap().push((kind, object.to_owned(), node, payload.to_vec()));
    }
    const START: &str = "c_sliding_start";
    const SLIDE: &str = "c_moveable_sliding_objects";
    let Some(assets) = assets() else { return };
    let guest = super::catalog::load_guest_image(&assets.join("private/stock/audio-runtime-image")).unwrap();
    let mut runtime = AuthoredRuntime::new(guest, vec![], vec![]).unwrap();
    let db = assets.join("private/stock/data/db");
    let read = |name: &str| std::fs::read(db.join(name)).unwrap();
    let vault = skate_audio_core::attrib::Vault::parse(
        &read("skaterschema.vlt"),
        &read("skaterschema.bin"),
        &read("skatercollections.vlt"),
        &read("skatercollections.bin"),
    )
    .unwrap();
    runtime.wire_environment().unwrap();
    runtime.game_boot(vault, title_files(roots(&assets))).0.unwrap();
    let skater = runtime.game_skater().unwrap();
    skate_audio_core::game::observe_aems(observe);
    {
        // UNTRACED what they are; neither flag that sends sub_824B5558 (and the collector sub_827A43C8) to the other DMO
        // map at game +0x3A988 is set, and the game mode is none of the 18/19/20 that silence sub_824B5D80.
        let g = runtime.guest();
        assert_eq!((g.u8(0x830B_7C2B).unwrap(), g.u8(0x8308_2929).unwrap()), (0, 0));
        let mode = g.u32(g.u32(0x830C_FDC4).unwrap() + 892).unwrap();
        assert!(!(18..=20).contains(&mode), "game mode {mode}");
    }
    let wedge = |held: bool, speed: f32, contact: bool| DmoEntry {
        id: 1,
        velocity: [speed, 0.0, 0.0],
        held,
        row: 0x733F_4D47_5186_E147, // wedgeramp_large
        contact,
        upright: true,
        position: [2.0, 0.5, 0.0],
        ..Default::default()
    };
    type Calls = std::collections::HashMap<u32, u64>;
    let run = |runtime: &mut AuthoredRuntime, dmos: &[DmoEntry], frames: usize| -> (Calls, Seen) {
        SEEN.lock().unwrap().clear();
        let mut calls = Calls::new();
        for _ in 0..frames {
            runtime.game_publish(&skater).unwrap();
            runtime.game_dmos(dmos).unwrap();
            let (done, trace) = runtime.game_frame(1.0 / 60.0);
            done.unwrap();
            for (a, n) in trace.calls {
                *calls.entry(a).or_insert(0) += n;
            }
            for _ in 0..3 {
                runtime.pump_once().unwrap();
            }
        }
        (calls, std::mem::take(&mut *SEEN.lock().unwrap()))
    };
    let n = |calls: &Calls, a: u32| calls.get(&a).copied().unwrap_or(0);
    let posts = |seen: &Seen, name: &str| -> Vec<(u32, Vec<u32>)> {
        seen.iter().filter(|(k, o, ..)| *k == 0 && o == name).map(|(_, _, node, words)| (*node, words.clone())).collect()
    };
    let released = |seen: &Seen, node: u32| seen.iter().any(|(k, _, n, _)| *k == 2 && *n == node);
    run(&mut runtime, &[], 30);

    // Listed but nobody holds it: the record runs, nothing starts.
    let (calls, seen) = run(&mut runtime, &[wedge(false, 2.0, true)], 60);
    eprintln!(
        "loose: sync {} record {} moving {} posts {:?}",
        n(&calls, 0x824F_4108),
        n(&calls, 0x824B_5558),
        n(&calls, 0x824E_48D0),
        seen.iter().filter(|s| s.0 == 0).map(|s| s.1.clone()).collect::<Vec<_>>()
    );
    assert!(n(&calls, 0x824B_5558) > 0, "no SFXCTL_DynamicObjectPhysics for the listed DMO");
    assert!(posts(&seen, START).is_empty() && posts(&seen, SLIDE).is_empty(), "a loose DMO started the drag");

    // Held at 2 m/s on the ground: each message posted once, with the wedge's slide row's words.
    let (_, seen) = run(&mut runtime, &[wedge(true, 2.0, true)], 120);
    let (start, slide) = (posts(&seen, START), posts(&seen, SLIDE));
    assert_eq!((start.len(), slide.len()), (1, 1), "c_sliding_start, c_moveable_sliding_objects posts");
    assert_eq!(start[0].1[8], 10, "c_sliding_start w8 (row +316)");
    let (node, words) = &slide[0];
    assert_eq!((words[8], words[19]), (10, 15000), "c_moveable_sliding_objects w8 (row +316), w19 (row +228)");
    let last = seen.iter().rev().find(|(k, _, n, _)| *k == 1 && n == node).expect("the slide message redelivered");
    assert_eq!(last.3[3], 900, "w3: 2 m/s against row +308 = 8 km/h");

    // Dragged on with no contact: released on the 15th frame without it.
    let mut after = None;
    for frame in 1..=30 {
        if released(&run(&mut runtime, &[wedge(true, 2.0, false)], 1).1, *node) {
            after = Some(frame);
            break;
        }
    }
    let after = after.expect("no release without contact");
    assert!((15..=17).contains(&after), "released {after} frames after contact stopped");

    // Back on the ground it starts again; at 0.2 m/s it stops at once.
    let (_, seen) = run(&mut runtime, &[wedge(true, 2.0, true)], 60);
    let slide = posts(&seen, SLIDE);
    assert_eq!(slide.len(), 1, "restarted on contact");
    let (_, seen) = run(&mut runtime, &[wedge(true, 0.2, true)], 2);
    assert!(released(&seen, slide[0].0), "no release at 0.2 m/s");
}
