//! Voice plan Task 1: the numbers that pick the codec and the noise filters (crates/skate-voice/DECISIONS.md). Ignored:
//! `SKATE_VOICE_CLIP=<48 kHz mono 16-bit wav> cargo test --release -j 1 -p skate-voice --test decide -- --ignored
//! --nocapture --test-threads 1` (one thread: the opus-rs census swaps the panic hook). `SKATE_VOICE_DF_LL=<libDF's
//! DeepFilterNet3_ll_onnx.tar.gz>` also times the low-latency model.
use std::time::{Duration, Instant};

fn clip() -> Option<Vec<f32>> {
    let path = std::env::var_os("SKATE_VOICE_CLIP")?;
    let mut wav = hound::WavReader::open(path).expect("SKATE_VOICE_CLIP is a wav");
    let spec = wav.spec();
    assert_eq!((spec.sample_rate, spec.channels, spec.bits_per_sample), (48_000, 1, 16), "48 kHz mono 16-bit");
    Some(wav.samples::<i16>().map(|s| f32::from(s.unwrap()) / 32768.).collect())
}

fn correlation(a: &[f32], b: &[f32]) -> f64 {
    let n = a.len().min(b.len());
    let (a, b) = (&a[..n], &b[..n]);
    let mean = |x: &[f32]| x.iter().map(|&v| f64::from(v)).sum::<f64>() / n as f64;
    let (ma, mb) = (mean(a), mean(b));
    let (mut ab, mut aa, mut bb) = (0., 0., 0.);
    for (&x, &y) in a.iter().zip(b) {
        let (x, y) = (f64::from(x) - ma, f64::from(y) - mb);
        (ab, aa, bb) = (ab + x * y, aa + x * x, bb + y * y);
    }
    ab / (aa * bb).sqrt().max(f64::MIN_POSITIVE)
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

/// Seconds of `x` that reach full scale: a decoder gone unstable (the speech clip peaks at 0.65).
fn saturated(x: &[f32]) -> usize {
    x.chunks(48_000).filter(|s| s.iter().any(|v| !v.is_finite() || v.abs() > 0.99)).count()
}

/// The codec delay (samples) at which `decoded` best matches `pcm` over 10..12 s.
fn delay(decoded: &[f32], pcm: &[f32]) -> usize {
    let w = 48_000 * 10..48_000 * 12;
    (0..2000).max_by(|&x, &y| correlation(&decoded[w.start + x..w.end + x], &pcm[w.clone()]).total_cmp(&correlation(&decoded[w.start + y..w.end + y], &pcm[w.clone()]))).unwrap()
}

/// opus-rs at `bitrate` with `ms` frames: how many frames' encodes panic, and the first (the encoder is remade after
/// each).
fn opus_rs_panics(pcm: &[f32], ms: usize, bitrate: i32, fec: bool) -> (usize, Option<usize>) {
    let samples = 48 * ms;
    let encoder = || {
        let mut e = opus_rs::OpusEncoder::new(48_000, 1, opus_rs::Application::Voip).unwrap();
        (e.bitrate_bps, e.use_inband_fec, e.packet_loss_perc) = (bitrate, fec, 10);
        e
    };
    let (mut enc, mut bad, mut first) = (encoder(), 0, None);
    for (i, frame) in pcm.chunks_exact(samples).enumerate() {
        let mut packet = [0u8; 400];
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| enc.encode(frame, samples, &mut packet).map(|_| ()))).is_err() {
            (bad, enc) = (bad + 1, encoder());
            first.get_or_insert(i);
        }
    }
    (bad, first)
}

/// `pcm` in `ms` frames through libopus (unsafe-libopus: 1.3.1 through c2rust), VoIP at `bitrate` with in-band FEC for
/// 10 % loss and DTX on or off, each setting accepted: the packets and the time spent encoding.
fn libopus_encode(pcm: &[f32], ms: usize, bitrate: i32, dtx: i32) -> (Vec<Vec<u8>>, Duration) {
    let samples = 48 * ms;
    let mut error = 0;
    let e = unsafe { unsafe_libopus::opus_encoder_create(48_000, 1, unsafe_libopus::OPUS_APPLICATION_VOIP, &mut error) };
    assert_eq!(error, unsafe_libopus::OPUS_OK);
    for (request, value) in [
        (unsafe_libopus::OPUS_SET_BITRATE_REQUEST, bitrate),
        (unsafe_libopus::OPUS_SET_INBAND_FEC_REQUEST, 1),
        (unsafe_libopus::OPUS_SET_PACKET_LOSS_PERC_REQUEST, 10),
        (unsafe_libopus::OPUS_SET_DTX_REQUEST, dtx),
    ] {
        assert_eq!(unsafe { unsafe_libopus::opus_encoder_ctl!(e, request, value) }, unsafe_libopus::OPUS_OK, "request {request}");
    }
    let (mut packets, mut time) = (vec![], Duration::ZERO);
    for frame in pcm.chunks_exact(samples) {
        let mut packet = [0u8; 400];
        let t = Instant::now();
        let n = unsafe { unsafe_libopus::opus_encode_float(e, frame.as_ptr(), samples as i32, packet.as_mut_ptr(), 400) };
        time += t.elapsed();
        assert!(n > 0, "libopus encodes it ({n})");
        packets.push(packet[..n as usize].to_vec());
    }
    unsafe { unsafe_libopus::opus_encoder_destroy(e) };
    (packets, time)
}

/// libopus decoding 20 ms `packets`, every `nth` one lost (0: none) and concealed.
fn libopus_decode(packets: &[Vec<u8>], nth: usize) -> Vec<f32> {
    let mut error = 0;
    let decoder = unsafe { unsafe_libopus::opus_decoder_create(48_000, 1, &mut error) };
    assert_eq!(error, unsafe_libopus::OPUS_OK);
    let mut pcm = vec![];
    for (i, p) in packets.iter().enumerate() {
        let mut out = vec![0f32; 960];
        let (data, len) = if nth > 0 && i % nth == nth / 2 { (std::ptr::null(), 0) } else { (p.as_ptr(), p.len() as i32) };
        assert_eq!(unsafe { unsafe_libopus::opus_decode_float(decoder, data, len, out.as_mut_ptr(), 960, 0) }, 960);
        pcm.extend(out);
    }
    unsafe { unsafe_libopus::opus_decoder_destroy(decoder) };
    pcm
}

/// opus-rs decoding `packets` of `frame` samples, every `nth` one lost (0: none): concealed, or with `fec` rebuilt from
/// the next packet's in-band FEC. Also the time spent decoding.
fn opus_rs_decode(packets: &[Vec<u8>], frame: usize, nth: usize, fec: bool) -> (Vec<f32>, Duration) {
    let mut d = opus_rs::OpusDecoder::new(48_000, 1).unwrap();
    let (mut pcm, mut time) = (vec![], Duration::ZERO);
    for (i, p) in packets.iter().enumerate() {
        let mut out = vec![0f32; frame];
        let t = Instant::now();
        let got = match (nth > 0 && i % nth == nth / 2, fec, packets.get(i + 1)) {
            (true, true, Some(next)) => d.decode_fec(next, frame, &mut out),
            (true, _, _) => d.decode(&[], frame, &mut out),
            (false, _, _) => d.decode(p, frame, &mut out),
        };
        time += t.elapsed();
        assert_eq!(got.unwrap(), frame, "opus-rs decodes it");
        pcm.extend(out);
    }
    (pcm, time)
}

/// The codec. opus-rs (first candidate) panics encoding with in-band FEC at 24 kbps 20 ms, and libopus's decoder
/// (unsafe-libopus, second) goes to full scale concealing a lost packet or a DTX gap, so libopus encodes and opus-rs
/// decodes (DECISIONS.md). What the voice path uses: VoIP, 24 kbps 20 ms (up to 8 listeners), 16 kbps 40 ms and 12 kbps
/// 60 ms (bigger lobbies), in-band FEC, DTX and concealment; opus-rs decodes libopus's packets as libopus does
/// (correlation >= 0.99); under 2 ms to encode a 20 ms frame.
#[test]
#[ignore]
fn opus_candidates_agree() {
    let Some(pcm) = clip() else { return eprintln!("no SKATE_VOICE_CLIP: skipped") };
    // Each candidate alone: opus-rs's encoder census, libopus's concealment.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let census: Vec<_> = [(20, 24_000, true), (20, 24_000, false), (40, 16_000, true), (60, 12_000, true)]
        .into_iter()
        .map(|(ms, bitrate, fec)| (ms, bitrate, fec, opus_rs_panics(&pcm, ms, bitrate, fec)))
        .collect();
    std::panic::set_hook(hook);
    for (ms, bitrate, fec, (bad, first)) in census {
        eprintln!("opus-rs encoder, {ms} ms at {bitrate} bps, FEC {fec}: {bad} of {} frames panic (first: {first:?})", pcm.len() / (48 * ms));
    }
    let no_dtx = libopus_encode(&pcm, 20, 24_000, 0).0;
    eprintln!("libopus decoder, every 10th packet lost: {} of 60 seconds at full scale", saturated(&libopus_decode(&no_dtx, 10)));
    // The pair. Where libopus's decoder still works (no loss, no DTX gaps), opus-rs decodes its packets the same.
    let cross = correlation(&opus_rs_decode(&no_dtx, 960, 0, false).0, &libopus_decode(&no_dtx, 0));
    let (speech, time) = libopus_encode(&pcm, 20, 24_000, 1);
    let (decoded, decode_time) = opus_rs_decode(&speech, 960, 0, false);
    let n = speech.len() as u128;
    let lag = delay(&decoded, &pcm);
    eprintln!(
        "libopus encode {} us, opus-rs decode {} us a 20 ms frame; {:.1} kbps; opus-rs decodes libopus's packets as libopus does: {cross:.5}; signal correlation {:.3} (delay {lag})",
        time.as_micros() / n,
        decode_time.as_micros() / n,
        speech.iter().map(Vec::len).sum::<usize>() as f64 * 8. / (n as f64 * 0.02) / 1000.,
        correlation(&decoded[lag..], &pcm),
    );
    assert!(cross >= 0.99, "opus-rs decodes libopus's packets as libopus does");
    assert!(time.as_micros() / n < 2000, "under 2 ms to encode a 20 ms frame");
    assert_eq!(saturated(&decoded), 0, "opus-rs decodes the DTX stream");
    // 40 and 60 ms frames at the big-lobby bitrates carry the voice, inside the 400-byte payload with its header byte.
    for (ms, bitrate) in [(40usize, 16_000), (60, 12_000)] {
        let packets = libopus_encode(&pcm, ms, bitrate, 1).0;
        let largest = packets.iter().map(Vec::len).max().unwrap();
        eprintln!("{ms} ms at {bitrate}: largest packet {largest} bytes");
        assert!(rms(&opus_rs_decode(&packets, 48 * ms, 0, false).0) > 0.3 * rms(&pcm), "{ms} ms frames carry the voice");
        assert!(largest < 400);
    }
    // DTX: a second of silence after speech goes out as packets of 2 bytes or less.
    let mut then_silence = pcm[..48_000].to_vec();
    then_silence.extend(vec![0f32; 48_000]);
    let tiny = libopus_encode(&then_silence, 20, 24_000, 1).0[50..].iter().filter(|p| p.len() <= 2).count();
    eprintln!("DTX: {tiny} of 50 silent frames are 2 bytes or less");
    assert!(tiny >= 25, "DTX");
    // Every 10th packet lost: concealment fills the gaps, and in-band FEC from the next packet rebuilds more of them.
    let (plc, fec) = (opus_rs_decode(&speech, 960, 10, false).0, opus_rs_decode(&speech, 960, 10, true).0);
    let (plc_c, fec_c) = (correlation(&plc, &decoded), correlation(&fec, &decoded));
    eprintln!("every 10th packet lost: concealment correlation {plc_c:.3}, in-band FEC {fec_c:.3}");
    assert!(saturated(&plc) + saturated(&fec) == 0 && plc_c > 0.5 && fec_c > plc_c, "concealment and in-band FEC");
}

/// DeepFilterNet with `params`: its total delay (analysis window past the hop, plus lookahead) and cost a frame.
fn deep_filter_cost(name: &str, params: df::tract::DfParams, pcm: &[f32]) {
    let mut df = df::tract::DfTract::new(params, &df::tract::RuntimeParams::default()).unwrap();
    let hop = df.hop_size;
    eprintln!(
        "{name}: sr {} hop {} fft {} lookahead {} hops: {} ms total delay",
        df.sr,
        hop,
        df.fft_size,
        df.lookahead,
        (df.fft_size - hop + df.lookahead * hop) * 1000 / df.sr
    );
    let (mut noisy, mut enhanced) = (ndarray::Array2::<f32>::zeros((1, hop)), ndarray::Array2::<f32>::zeros((1, hop)));
    let (mut n, mut worst) = (0u32, 0u128);
    let t = Instant::now();
    for frame in pcm.chunks_exact(hop) {
        noisy.row_mut(0).assign(&ndarray::ArrayView1::from(frame));
        let f = Instant::now();
        df.process(noisy.view(), enhanced.view_mut()).unwrap();
        worst = worst.max(f.elapsed().as_micros());
        n += 1;
    }
    eprintln!("{name}: {} us a {} ms frame (worst {worst} us)", t.elapsed().as_micros() / u128::from(n), hop * 1000 / df.sr);
}

/// RNNoise (Standard) and both DeepFilterNet models (High): cost a 10 ms frame, DeepFilterNet's delay.
#[test]
#[ignore]
fn filters_cost() {
    let Some(pcm) = clip() else { return eprintln!("no SKATE_VOICE_CLIP: skipped") };
    let mut rnnoise = nnnoiseless::DenoiseState::new();
    let (mut out, mut speech, mut frames) = ([0f32; 480], 0f32, 0u32);
    let t = Instant::now();
    for frame in pcm.chunks_exact(480) {
        let scaled: Vec<f32> = frame.iter().map(|s| s * 32768.).collect();
        speech += rnnoise.process_frame(&mut out, &scaled);
        frames += 1;
    }
    eprintln!("RNNoise: {} us a 10 ms frame; mean speech probability {:.2}", t.elapsed().as_micros() / u128::from(frames), speech / frames as f32);
    deep_filter_cost("DeepFilterNet3", df::tract::DfParams::default(), &pcm);
    // The low-latency model (libDF's "default-model-ll", 36 MB, not compiled in): SKATE_VOICE_DF_LL=<its .tar.gz>.
    match std::env::var_os("SKATE_VOICE_DF_LL") {
        Some(path) => deep_filter_cost("DeepFilterNet3 LL", df::tract::DfParams::new(path.into()).unwrap(), &pcm),
        None => eprintln!("no SKATE_VOICE_DF_LL: the low-latency model is skipped"),
    }
}
