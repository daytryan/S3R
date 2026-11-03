//! Skate 2's front-end one-shots for its tutorial screens: every `_global.FE_SOUND_<KEY>` the converted screens
//! play (screens/runtime/*.json) and the few the game plays itself (NATIVE), resolved through Skate 2's own `fe` vault record (`<key>`: sample id + Volume)
//! and its Splice banks in data/audio/splicerfe.big (BIG4: Sk82_FE.bnk, sk8_menu.bnk), decoded and written as
//! screens/sfx/<key>.wav.
//!
//! `skate3rust.exe --s2-fe-sounds SKATE2_ROOT S2_VAULT_JSON SCREENS_DIR` (the launcher's pipeline) or
//! `cargo run -p skate-data --example s2_fe_sounds -- SKATE2_ROOT S2_VAULT_JSON SCREENS_DIR`.
//!
//! S2_VAULT_JSON is skate2port's work/screens-cache/skater-collections.json (old_screens.py writes it).
//! The member gains, pan and delays are one deterministic draw of the bank's own randomisation (seed 1), mixed to
//! stereo in float ([`mix`]). The record's Volume and any headroom scale go into sfx/gains.json; playback applies them
//! (8241b2a0, apt_screen::fe_gain).
use crate::audio::{
    ffmpeg,
    splice::{SpliceBanks, SpliceState},
};
use skate_audio_core::authored::oneshot::Rand;
use skate_audio_formats::eaac;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

const SAMPLE: &str = "Hash_8FCC7EF9B9208858";
const VOLUME: &str = "Hash_875BA75341DC8391";
/// Played by the game rather than a tutorial movie: the popup opening (`core_popup`, 0x8252B618), a single-round
/// challenge's end (`challenge_end_success`/`_fail`, 0x8252E3A8; `challenge_round_*` when it has rounds), the HUD's
/// trickdisplay2 multiplier steps (hud-skate2, not a tutorial screen), the cellphone session marker and its phone list
/// opening (0x8258B500), the TimerHUD's last-seconds tick (0x82588710) and the photo pick's step and pick (0x8254B4A8).
const NATIVE: [&str; 13] = [
    "core_popup",
    "challenge_end_success",
    "challenge_end_fail",
    "multiplyer_15",
    "multiplyer_2",
    "multiplyer_3",
    "cellphone_place_marker",
    "cellphone_goto_marker",
    "cellphone_marker_error",
    "cellphone_activate",
    "timer_end",
    "photo_choose",
    "photo_pick",
];

/// Reads `<out>/runtime/*.json` and writes `<out>/sfx/<key>.wav` (`out` is the converted screens directory).
pub fn s2_fe_sounds(s2_root: &Path, collections: &Path, out: &Path) -> Result<(), String> {
    run(s2_root, collections, out).map_err(|e| e.to_string())
}

fn run(root: &Path, vault: &Path, screens: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let data = std::fs::read(root.join("data/audio/splicerfe.big"))?;
    // BIG4: magic, size, count, header size (big-endian), then (offset, size, name\0) per entry.
    let word = |at: usize| u32::from_be_bytes(data[at..at + 4].try_into().unwrap()) as usize;
    let (mut members, mut at) = (Vec::new(), 16);
    for _ in 0..word(8) {
        let end = at + 8 + data[at + 8..].iter().position(|&b| b == 0).ok_or("BIG4: unterminated name")?;
        let name = String::from_utf8_lossy(&data[at + 8..end]).into_owned();
        members.push((name, data[word(at)..word(at) + word(at + 4)].to_vec()));
        at = end + 1;
    }
    let names: Vec<String> = members.iter().map(|(n, _)| n.clone()).collect();
    println!("banks: {names:?}");
    let banks = SpliceBanks::from_bytes(members)?;

    let mut keys: BTreeSet<String> = NATIVE.iter().map(|k| k.to_string()).collect();
    for entry in std::fs::read_dir(screens.join("runtime"))? {
        let text = std::fs::read_to_string(entry?.path())?;
        for (at, _) in text.match_indices("FE_SOUND_") {
            let key: String = text[at + 9..].chars().take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_').collect();
            keys.insert(key.to_ascii_lowercase());
        }
    }

    let vault: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(vault)?)?;
    let rows = vault.get("collections").unwrap_or(&vault).as_array().ok_or("vault: no collections")?;
    let fe_hash = format!("Hash_{:016X}", crate::audio::world::hash64("fe"));
    let out = screens.join("sfx");
    std::fs::create_dir_all(&out)?;
    let mut gains = BTreeMap::new();
    for key in &keys {
        let Some(row) = rows.iter().find(|r| (r["class"] == "fe" || r["class"] == fe_hash.as_str()) && r["key"] == key.as_str()) else {
            println!("{key}: no fe record");
            continue;
        };
        let field = |f: &str| row["fields"][f]["data"].as_str().and_then(|s| u32::from_str_radix(s.trim(), 16).ok());
        let (Some(sample), volume) = (field(SAMPLE).filter(|&s| s != 0), field(VOLUME).map_or(1., f32::from_bits)) else {
            println!("{key}: no sample");
            continue;
        };
        // The field's type name is its bank (`sk8_menu`).
        let Some(kind) = row["fields"][SAMPLE]["type"].as_str() else { continue };
        let bank = format!("{kind}.bnk");
        let members = match banks.resolve(&bank, sample as u16, &mut SpliceState::default(), &mut Rand::default()) {
            Ok(m) => m,
            Err(e) => {
                println!("{key}: {bank} #{sample}: {e:?}");
                continue;
            }
        };
        let bytes = banks.bank_bytes(&bank).unwrap();
        let mut rate = 0;
        let mut decoded = Vec::new();
        for m in &members {
            let at = m.stream_offset as usize;
            let header = eaac::Header::parse(bytes, at)?;
            rate = header.sample_rate;
            let channels = usize::from(header.channels());
            // The bank packs samples back to back with no end flag: stop at the block that completes the declared count.
            let (mut end, mut frames) = (at + header.size(), 0);
            while frames < header.num_samples {
                let block = eaac::Block::parse(bytes, end)?;
                (end, frames) = (end + block.size as usize, frames + block.num_samples);
            }
            let pcm = ffmpeg::decode_resident(&bytes[at..end])?;
            decoded.push((pcm, channels, m.values.gain, m.pan, (m.values.delay * rate as f32) as usize));
        }
        let (stereo, scale) = mix(&decoded);
        gains.insert(key.clone(), played_gain(volume, scale, &stereo));
        let pcm: Vec<u8> = stereo.iter().flatten().flat_map(|v| ((v * 32767.) as i16).to_le_bytes()).collect();
        let mut wav = Vec::with_capacity(44 + pcm.len());
        for part in [&b"RIFF"[..], &(36 + pcm.len() as u32).to_le_bytes(), b"WAVEfmt ", &16u32.to_le_bytes(), &1u16.to_le_bytes(),
            &2u16.to_le_bytes(), &rate.to_le_bytes(), &(rate * 4).to_le_bytes(), &4u16.to_le_bytes(), &16u16.to_le_bytes(), b"data",
            &(pcm.len() as u32).to_le_bytes(), &pcm]
        {
            wav.extend_from_slice(part);
        }
        std::fs::write(out.join(format!("{key}.wav")), wav)?;
        println!("{key}: {bank} #{sample} x{volume:.2} (headroom x{scale:.3}), {} member(s), {:.2}s", members.len(), stereo.len() as f32 / rate as f32);
    }
    std::fs::write(out.join("gains.json"), serde_json::to_vec_pretty(&gains)?)?;
    Ok(())
}

/// Mixes `(pcm, channels, gain, pan, start frame)` members to stereo in float, as retail's mixer does (it has no int16
/// render step, dumpster-sound.md B). A mix past full scale comes back scaled into range, with the scale it took.
fn mix(members: &[(Vec<i16>, usize, f32, f32, usize)]) -> (Vec<[f32; 2]>, f32) {
    let mut mix: Vec<[f32; 2]> = Vec::new();
    for &(ref pcm, channels, gain, pan, start) in members {
        // Constant-power pan of a mono member (degrees, ±90 = hard side); stereo plays as authored.
        let angle = (pan.clamp(-90., 90.) + 90.).to_radians() / 2.;
        for (i, frame) in pcm.chunks_exact(channels).enumerate() {
            let s = |c: usize| f32::from(frame[c.min(channels - 1)]) / 32768. * gain;
            let pair = if channels == 1 { [s(0) * angle.cos(), s(0) * angle.sin()] } else { [s(0), s(1)] };
            if mix.len() <= start + i {
                mix.resize(start + i + 1, [0.; 2]);
            }
            mix[start + i][0] += pair[0];
            mix[start + i][1] += pair[1];
        }
    }
    let peak = mix.iter().flatten().fold(0f32, |m, v| m.max(v.abs()));
    if peak <= 1. {
        return (mix, 1.);
    }
    // Divided rather than multiplied by 1/peak, so the peak lands on exactly 1.0.
    mix.iter_mut().flatten().for_each(|v| *v /= peak);
    (mix, 1. / peak)
}

/// The play-time gain for a written mix: what restores the authored level (`volume / scale`), capped so the file's
/// peak plays at no more than full scale with SFX at 10 (apt_screen::fe_gain × the slider, MASTER 1.0).
// Not retail (user 10-13: "turn it down"): retail's output stage is UNTRACED (F9); without the cap the restored level
// runs past full scale and the device clips it (review I1).
fn played_gain(volume: f32, scale: f32, mix: &[[f32; 2]]) -> f32 {
    let peak = mix.iter().flatten().fold(0f32, |m, v| m.max(v.abs()));
    (volume / scale).min(1. / peak)
}

#[cfg(test)]
mod tests {
    #[test]
    fn nonexistent_folder_is_an_error() {
        let missing = std::env::temp_dir().join(format!("s3r-no-such-folder-{}", std::process::id()));
        let result = super::s2_fe_sounds(&missing, &missing.join("vault.json"), &missing.join("screens"));
        assert!(result.is_err(), "{result:?}");
    }

    #[test]
    fn a_sound_never_plays_past_full_scale_at_the_top_sfx_setting() {
        // A hot mix rescaled to peak 1.0 (scale 0.5) at volume 0.85: restoring 0.85 / 0.5 would play it at 1.7.
        assert_eq!(super::played_gain(0.85, 0.5, &[[1.0, -0.5]; 4]), 1.0);
        // A quiet mix keeps its authored volume, even above 1, while its peak stays in range.
        assert_eq!(super::played_gain(1.2, 1.0, &[[0.5, -0.25]; 4]), 1.2);
        assert_eq!(super::played_gain(0.85, 1.0, &[[0.; 2]]), 0.85, "silence");
    }

    #[test]
    fn a_hot_mix_is_scaled_into_range_not_clipped() {
        // Two full-scale members summed: retail mixes in float (dumpster-sound.md B); the file must not clip.
        let loud = vec![i16::MAX; 480];
        let (mix, scale) = super::mix(&[(loud.clone(), 1, 1.0, 0., 0), (loud, 1, 1.0, 0., 0)]);
        let peak = mix.iter().flatten().fold(0f32, |m, v| m.max(v.abs()));
        assert!(peak <= 1.0 && scale < 1.0, "peak {peak}, scale {scale}");
        // The scale undoes exactly: the authored level is peak / scale.
        assert!((peak / scale - 2.0 * (32767. / 32768.) * std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3);
    }
}
