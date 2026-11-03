//! Share codes: one item's saved Custom Graphic as text a player can paste into a chat. The code is
//! `S3G1-` + base64url(zstd([item length, item id, the save's JSON])) + `-` + 8 hex digits of blake3 over
//! the body. Decoding is a trust boundary: every step is capped and checked before the next, and the
//! design must pass the same strict checks as a profile's (`graphic_store::Saved::parse`, `Doc::decode`).
use crate::{custom_graphic::Doc, graphic_store::Saved};
use base64::Engine;

pub(crate) const PREFIX: &str = "S3G1-";
/// Characters, before decoding.
pub(crate) const MAX_CODE: usize = 16 * 1024;
/// Bytes, after decompressing.
const MAX_PAYLOAD: usize = 64 * 1024;
const CHECK: usize = 8;
const BASE64: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;

#[derive(Debug, PartialEq)]
pub(crate) enum CodeError {
    /// Not a code, or damaged.
    Format,
    /// A code from a later version of the format.
    Version,
    Checksum,
    TooBig,
    /// A code for another item (its id).
    OtherItem(String),
    /// The design breaks the limits or names art this install lacks.
    Design,
}

fn check(body: &str) -> String {
    blake3::hash(body.as_bytes()).to_hex()[..CHECK].to_owned()
}

pub(crate) fn encode(item: &str, saved: &Saved) -> String {
    let mut payload = vec![item.len().min(255) as u8];
    payload.extend(&item.as_bytes()[..item.len().min(255)]);
    payload.extend(serde_json::to_vec(&saved.json()).unwrap_or_default());
    let body = BASE64.encode(zstd::bulk::compress(&payload, 19).unwrap_or_default());
    let check = check(&body);
    format!("{PREFIX}{body}-{check}")
}

/// `known`: whether a stock logo or game-art id is in the installed library.
pub(crate) fn decode(code: &str, item: &str, known: impl Fn(u64) -> bool) -> Result<Saved, CodeError> {
    let code = code.trim();
    let Some(rest) = code.strip_prefix(PREFIX) else {
        // `S3G<n>-` of another version.
        let later = code.strip_prefix("S3G").and_then(|r| r.split_once('-')).is_some_and(|(v, _)| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()));
        return Err(if later { CodeError::Version } else { CodeError::Format });
    };
    if code.len() > MAX_CODE {
        return Err(CodeError::TooBig);
    }
    let (body, sum) = rest.rsplit_once('-').ok_or(CodeError::Format)?;
    if sum.len() != CHECK || body.is_empty() {
        return Err(CodeError::Format);
    }
    if !sum.eq_ignore_ascii_case(&check(body)) {
        return Err(CodeError::Checksum);
    }
    let packed = BASE64.decode(body).map_err(|_| CodeError::Format)?;
    let payload = zstd::bulk::decompress(&packed, MAX_PAYLOAD).map_err(|_| CodeError::Format)?;
    let (&len, rest) = payload.split_first().ok_or(CodeError::Format)?;
    let (id, json) = (rest.get(..len as usize).ok_or(CodeError::Format)?, &rest[(len as usize).min(rest.len())..]);
    let id = std::str::from_utf8(id).map_err(|_| CodeError::Format)?;
    let value: serde_json::Value = serde_json::from_slice(json).map_err(|_| CodeError::Format)?;
    if id != item {
        return Err(CodeError::OtherItem(id.to_owned()));
    }
    let saved = Saved::parse(&value).ok_or(CodeError::Design)?;
    for text in saved.sides.values() {
        let doc = Doc::decode(text).map_err(|_| CodeError::Design)?;
        if doc.images().any(|id| !known(id)) {
            return Err(CodeError::Design);
        }
    }
    Ok(saved)
}

/// The popup's words for a failed paste.
pub(crate) fn failure(error: &CodeError) -> &'static str {
    match error {
        CodeError::OtherItem(_) => "#That code is for a different item.",
        _ => "#That code could not be loaded.",
    }
}

/// Open the clipboard: another program may hold it a moment, so once more after 15 ms.
#[cfg(windows)]
fn open_clipboard() -> std::io::Result<()> {
    use windows_sys::Win32::System::DataExchange::OpenClipboard;
    for attempt in 0..2 {
        // SAFETY: no window owns it (NULL); the callers close it on every path.
        if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
            return Ok(());
        }
        if attempt == 0 {
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
    }
    Err(std::io::Error::last_os_error())
}

#[cfg(windows)]
pub(crate) fn copy_to_clipboard(text: &str) -> std::io::Result<()> {
    use windows_sys::Win32::{
        Foundation::GlobalFree,
        System::{
            DataExchange::{CloseClipboard, EmptyClipboard, SetClipboardData},
            Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock},
            Ole::CF_UNICODETEXT,
        },
    };
    let wide: Vec<u16> = text.encode_utf16().chain([0]).collect();
    // SAFETY: the clipboard is opened and closed here; the global block is ours until SetClipboardData
    // takes it, and freed on every path where it does not.
    unsafe {
        open_clipboard()?;
        let result = (|| {
            EmptyClipboard();
            let memory = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2);
            if memory.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let at = GlobalLock(memory).cast::<u16>();
            if at.is_null() {
                GlobalFree(memory);
                return Err(std::io::Error::last_os_error());
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), at, wide.len());
            GlobalUnlock(memory);
            if SetClipboardData(u32::from(CF_UNICODETEXT), memory).is_null() {
                GlobalFree(memory);
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        })();
        CloseClipboard();
        result
    }
}

/// The clipboard's text, at most a little over `MAX_CODE` characters (a longer text is no code).
#[cfg(windows)]
pub(crate) fn paste_from_clipboard() -> std::io::Result<String> {
    use windows_sys::Win32::System::{
        DataExchange::{CloseClipboard, GetClipboardData},
        Memory::{GlobalLock, GlobalSize, GlobalUnlock},
        Ole::CF_UNICODETEXT,
    };
    // SAFETY: the handle belongs to the open clipboard; it is read within GlobalSize while locked.
    unsafe {
        open_clipboard()?;
        let result = (|| {
            let handle = GetClipboardData(u32::from(CF_UNICODETEXT));
            if handle.is_null() {
                return Err(std::io::Error::other("no text on the clipboard"));
            }
            let at = GlobalLock(handle).cast::<u16>().cast_const();
            if at.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let units = std::slice::from_raw_parts(at, (GlobalSize(handle) / 2).min(MAX_CODE + 64));
            let end = units.iter().position(|u| *u == 0).unwrap_or(units.len());
            let text = String::from_utf16_lossy(&units[..end]);
            GlobalUnlock(handle);
            Ok(text)
        })();
        CloseClipboard();
        result
    }
}
#[cfg(not(windows))]
pub(crate) fn copy_to_clipboard(_: &str) -> std::io::Result<()> {
    Err(std::io::ErrorKind::Unsupported.into())
}
#[cfg(not(windows))]
pub(crate) fn paste_from_clipboard() -> std::io::Result<String> {
    Err(std::io::ErrorKind::Unsupported.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custom_graphic::{Art, Layer, MAX_LAYERS, MAX_TEXT, SHAPES};

    const TEE: &str = "801095192c838106";
    fn saved(sides: &[(&str, Doc)]) -> Saved {
        Saved { sides: sides.iter().map(|(z, d)| ((*z).to_owned(), d.encode())).collect() }
    }
    fn sample() -> Saved {
        let layers = vec![
            Layer::new(Art::Shape(2), [200, 30, 40, 255]),
            Layer { y: 600, ..Layer::new(Art::Text("PORT CARVERTON".into()), [255; 4]) },
            Layer { x: -300, ..Layer::new(Art::Logo(0xaa), [255; 4]) },
        ];
        saved(&[("front", Doc { layers, background: [0; 4] }), ("left", Doc { layers: vec![Layer::new(Art::Shape(14), [9, 9, 9, 255])], background: [0; 4] })])
    }
    fn known(id: u64) -> bool {
        id == 0xaa
    }

    #[test]
    fn codes_round_trip() {
        let code = encode(TEE, &sample());
        assert!(code.starts_with(PREFIX) && code.len() < 400, "{} chars", code.len());
        assert!(code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'), "chat-safe: {code}");
        assert_eq!(decode(&format!("  {code}\r\n"), TEE, known), Ok(sample()), "pasted with whitespace round it");
    }

    #[test]
    fn codes_reject_tamper_truncation_and_unknown_version() {
        let code = encode(TEE, &sample());
        let (body, sum) = code.rsplit_once('-').unwrap();
        // One character of the body changed: the checksum catches it.
        let mut bytes = body.as_bytes().to_vec();
        let i = PREFIX.len() + 7;
        bytes[i] = if bytes[i] == b'A' { b'B' } else { b'A' };
        assert_eq!(decode(&format!("{}-{sum}", String::from_utf8(bytes).unwrap()), TEE, known), Err(CodeError::Checksum));
        assert_eq!(decode(&code[..code.len() - 1], TEE, known), Err(CodeError::Format));
        assert!(decode(&code[..code.len() / 2], TEE, known).is_err());
        assert_eq!(decode(&code.replacen("S3G1-", "S3G2-", 1), TEE, known), Err(CodeError::Version));
        assert_eq!(decode("hello", TEE, known), Err(CodeError::Format));
        assert_eq!(decode(&format!("{PREFIX}{}", "A".repeat(MAX_CODE)), TEE, known), Err(CodeError::TooBig));
        // A body that checks out but is not zstd, and zstd that is not a save.
        let body = BASE64.encode(b"not zstd");
        assert_eq!(decode(&format!("{PREFIX}{body}-{}", check(&body)), TEE, known), Err(CodeError::Format));
        let mut payload = vec![TEE.len() as u8];
        payload.extend(TEE.as_bytes());
        payload.extend(br#"{"sides":{"front":"nonsense"}}"#);
        let body = BASE64.encode(zstd::bulk::compress(&payload, 3).unwrap());
        assert_eq!(decode(&format!("{PREFIX}{body}-{}", check(&body)), TEE, known), Err(CodeError::Design));
        // A bomb: more than 64 KiB once decompressed.
        let mut payload = vec![TEE.len() as u8];
        payload.extend(TEE.as_bytes());
        payload.extend(vec![b' '; MAX_PAYLOAD + 1]);
        let body = BASE64.encode(zstd::bulk::compress(&payload, 3).unwrap());
        assert!(body.len() < MAX_CODE);
        assert_eq!(decode(&format!("{PREFIX}{body}-{}", check(&body)), TEE, known), Err(CodeError::Format));
    }

    #[test]
    fn codes_refuse_other_models_and_keep_limits() {
        let code = encode(TEE, &sample());
        assert_eq!(decode(&code, "862d97f973760883", known), Err(CodeError::OtherItem(TEE.into())));
        assert_eq!(failure(&CodeError::OtherItem(TEE.into())), "#That code is for a different item.");
        assert_eq!(failure(&CodeError::Checksum), "#That code could not be loaded.");
        // Art this install lacks.
        assert_eq!(decode(&code, TEE, |_| false), Err(CodeError::Design));
        // 64 layers on every side round-trip, the longest text among them.
        let full = |n: usize| Doc {
            layers: (0..n)
                .map(|i| match i {
                    0 => Layer::new(Art::Text("W".repeat(MAX_TEXT)), [255; 4]),
                    _ => Layer { x: i as i16, ..Layer::new(Art::Shape((i % SHAPES.len()) as u8), [i as u8, 0, 0, 255]) },
                })
                .collect(),
            background: [1, 2, 3, 4],
        };
        let big = saved(&crate::custom_graphic::SHIRT.map(|z| (z, full(MAX_LAYERS))));
        let code = encode(TEE, &big);
        assert!(code.len() <= MAX_CODE, "a full item fits: {} chars", code.len());
        assert_eq!(decode(&code, TEE, known), Ok(big));
        // A 65th layer is refused.
        let mut over = full(MAX_LAYERS);
        over.layers.push(Layer::new(Art::Shape(0), [255; 4]));
        let mut payload = vec![TEE.len() as u8];
        payload.extend(TEE.as_bytes());
        payload.extend(serde_json::to_vec(&serde_json::json!({ "sides": { "front": over.encode() } })).unwrap());
        let body = BASE64.encode(zstd::bulk::compress(&payload, 3).unwrap());
        assert_eq!(decode(&format!("{PREFIX}{body}-{}", check(&body)), TEE, known), Err(CodeError::Design));
    }

    /// 10 000 seeded strings, half behind the right prefix (some with a valid checksum): none panics.
    #[test]
    fn random_bytes_never_panic() {
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_ \n\xc3\xa9";
        for i in 0..10_000 {
            let len = (next() % 600) as usize;
            let junk: Vec<u8> = (0..len).map(|_| alphabet[(next() % alphabet.len() as u64) as usize]).collect();
            let junk = String::from_utf8_lossy(&junk).into_owned();
            let code = match i % 4 {
                0 | 1 => junk,
                2 => format!("{PREFIX}{junk}"),
                _ => {
                    let body = BASE64.encode(junk.as_bytes());
                    format!("{PREFIX}{body}-{}", check(&body))
                }
            };
            let _ = decode(&code, TEE, known);
        }
    }

    /// Flip, insert or delete a few bytes.
    fn mutate(bytes: &mut Vec<u8>, next: &mut impl FnMut() -> u64) {
        for _ in 0..1 + next() % 4 {
            let at = (next() % (bytes.len() as u64 + 1)) as usize;
            match next() % 3 {
                0 if at < bytes.len() => bytes[at] ^= 1 << (next() % 8),
                1 => bytes.insert(at, next() as u8),
                _ if at < bytes.len() => {
                    bytes.remove(at);
                }
                _ => {}
            }
        }
    }

    /// Review I7: a real code's payload damaged inside (its JSON, a side's design text, the design's own
    /// bytes), then packed and checksummed as a real code is, 10 000 seeded rounds. Each gets past the outer
    /// checks into the JSON, `Saved::parse` and `Doc::decode`; none panics, and some still load.
    #[test]
    fn damaged_payloads_reach_the_inner_decoders_and_never_panic() {
        use base64::engine::general_purpose::STANDARD_NO_PAD;
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let pack = |payload: &[u8]| {
            let body = BASE64.encode(zstd::bulk::compress(payload, 1).unwrap());
            format!("{PREFIX}{body}-{}", check(&body))
        };
        let real = sample();
        let mut header = vec![TEE.len() as u8];
        header.extend(TEE.as_bytes());
        let json = serde_json::to_vec(&real.json()).unwrap();
        let (front, left) = (real.sides["front"].clone(), real.sides["left"].clone());
        let bytes = STANDARD_NO_PAD.decode(&front).unwrap();
        let with_front = |front: String| serde_json::to_vec(&serde_json::json!({ "sides": { "front": front, "left": left } })).unwrap();
        let (mut loaded, mut refused, mut unreadable) = (0, 0, 0);
        for i in 0..10_000 {
            let body = match i % 3 {
                // The JSON itself.
                0 => {
                    let mut json = json.clone();
                    mutate(&mut json, &mut next);
                    json
                }
                // The design's text (base64).
                1 => {
                    let mut text = front.clone().into_bytes();
                    mutate(&mut text, &mut next);
                    with_front(String::from_utf8_lossy(&text).into_owned())
                }
                // The design's bytes: layer count, art, colours, positions, text.
                _ => {
                    let mut design = bytes.clone();
                    mutate(&mut design, &mut next);
                    with_front(STANDARD_NO_PAD.encode(&design))
                }
            };
            match decode(&pack(&[&header[..], &body].concat()), TEE, known) {
                Ok(saved) => {
                    loaded += 1;
                    assert!(saved.sides.values().all(|t| Doc::decode(t).is_ok()), "what loads is a design");
                }
                Err(CodeError::Design) => refused += 1,
                Err(CodeError::Format) => unreadable += 1,
                Err(other) => panic!("past the checksum only the inner checks fail: {other:?}"),
            }
        }
        assert!(loaded > 100 && refused > 100 && unreadable > 100, "loaded {loaded}, refused {refused}, unreadable {unreadable}");
    }
}
