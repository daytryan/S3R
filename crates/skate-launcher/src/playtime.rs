//! User 10-14 (feature #7): total time played, `%LOCALAPPDATA%\Skate3RustEngine\playtime.json`, counted while the game
//! runs and the launcher is open. The file is signed (HMAC-SHA256) with a random key kept in Windows Credential Manager,
//! so an edited or copied file is refused and starts again from 0. ponytail: stops casual editing only; anything kept
//! on the player's own PC can be beaten by someone determined, and only a server could be cheat-proof.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const TARGET: &str = "Skate3Remaster/playtime";

pub fn path() -> PathBuf { crate::maintenance::local_data().join("playtime.json") }

/// This PC's signing key, made on first use. B10: where Credential Manager can't keep it (policy-locked or roaming
/// profiles), it is kept in `playtime.key`, readable only by this Windows account (DPAPI).
pub fn key() -> String {
    if let Some(key) = crate::credentials::load_from(TARGET) { return key; }
    let file = crate::maintenance::local_data().join("playtime.key");
    if let Some(key) = from_file(&file) { return key; }
    use std::hash::{BuildHasher, Hasher};
    let mut seed = Sha256::new();
    for _ in 0..4 { seed.update(std::collections::hash_map::RandomState::new().build_hasher().finish().to_le_bytes()); }
    seed.update(format!("{:?}{}", std::time::SystemTime::now(), std::process::id()));
    let key = hex(&seed.finalize());
    if crate::credentials::save_to(TARGET, &key).is_err() { to_file(&file, &key); }
    key
}

fn from_file(file: &Path) -> Option<String> {
    String::from_utf8(dpapi(&std::fs::read(file).ok()?, false)?).ok().filter(|k| !k.is_empty())
}

fn to_file(file: &Path, key: &str) {
    crate::maintenance::log("playtime: Credential Manager refused the key; it is kept DPAPI-protected in playtime.key");
    let _ = std::fs::create_dir_all(file.parent().unwrap_or(Path::new(".")));
    if let Some(sealed) = dpapi(key.as_bytes(), true) { let _ = std::fs::write(file, sealed); }
}

/// DPAPI for the current user: seal (`protect`) or open `data`.
fn dpapi(data: &[u8], protect: bool) -> Option<Vec<u8>> {
    use windows_sys::Win32::{Foundation::LocalFree, Security::Cryptography::*};
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    let flags = CRYPTPROTECT_UI_FORBIDDEN;
    let ok = unsafe {
        if protect { CryptProtectData(&input, std::ptr::null(), std::ptr::null(), std::ptr::null(), std::ptr::null(), flags, &mut out) }
        else { CryptUnprotectData(&input, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null(), flags, &mut out) }
    };
    if ok == 0 || out.pbData.is_null() { return None; }
    let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
    unsafe { LocalFree(out.pbData as _) };
    Some(bytes)
}

pub fn forget_key() {
    crate::credentials::delete_from(TARGET);
    let _ = std::fs::remove_file(crate::maintenance::local_data().join("playtime.key"));
}

fn hex(bytes: &[u8]) -> String { bytes.iter().map(|b| format!("{b:02x}")).collect() }

/// HMAC-SHA256 (RFC 2104).
fn hmac(key: &[u8], message: &[u8]) -> String {
    let mut block = [0u8; 64];
    if key.len() > 64 { block[..32].copy_from_slice(&Sha256::digest(key)); } else { block[..key.len()].copy_from_slice(key); }
    let pad = |byte: u8| block.map(|b| b ^ byte);
    let inner = Sha256::new().chain_update(pad(0x36)).chain_update(message).finalize();
    hex(&Sha256::new().chain_update(pad(0x5c)).chain_update(inner).finalize())
}

fn sign(key: &str, seconds: u64) -> String { hmac(key.as_bytes(), format!("skate3remaster-playtime:{seconds}").as_bytes()) }

/// Seconds played: 0 without a file, None for a file that was edited (or signed on another PC).
pub fn read(file: &Path, key: &str) -> Option<u64> {
    let Ok(bytes) = std::fs::read(file) else { return Some(0) };
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    let seconds = v["seconds"].as_u64()?;
    (v["sig"].as_str() == Some(sign(key, seconds).as_str())).then_some(seconds)
}

/// Adds a session; an edited file is replaced, starting from this session.
pub fn add(file: &Path, key: &str, seconds: u64) -> u64 {
    let total = read(file, key).unwrap_or_else(|| { crate::maintenance::log("playtime: edited file refused, starting again"); 0 }) + seconds;
    let _ = std::fs::create_dir_all(file.parent().unwrap_or(Path::new(".")));
    let temp = file.with_extension("tmp");
    if std::fs::write(&temp, json!({"seconds": total, "sig": sign(key, total)}).to_string()).is_ok() {
        let _ = std::fs::rename(&temp, file);
    }
    total
}

/// The page's message: the total, or that the file was refused.
pub fn message() -> Value {
    match read(&path(), &key()) {
        Some(seconds) => json!({"type": "playtime", "seconds": seconds}),
        None => json!({"type": "playtime", "seconds": 0, "refused": true}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_matches_rfc_4231() {
        // RFC 4231 test case 2.
        assert_eq!(hmac(b"Jefe", b"what do ya want for nothing?"), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    }

    /// B10: the fallback key file opens again for this account, and doesn't hold the key in plain text.
    #[test]
    fn a_key_credential_manager_refused_is_kept_in_a_dpapi_file() {
        let file = std::env::temp_dir().join(format!("s3r-playtime-{}.key", std::process::id()));
        to_file(&file, "abc123secret");
        assert_eq!(from_file(&file).as_deref(), Some("abc123secret"));
        assert!(!String::from_utf8_lossy(&std::fs::read(&file).unwrap()).contains("abc123secret"));
        std::fs::write(&file, b"junk").unwrap();
        assert_eq!(from_file(&file), None);
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn playtime_adds_up_and_an_edited_file_is_refused() {
        let file = std::env::temp_dir().join(format!("s3r-playtime-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&file);
        assert_eq!(read(&file, "k"), Some(0));
        assert_eq!(add(&file, "k", 60), 60);
        assert_eq!(add(&file, "k", 30), 90);
        assert_eq!(read(&file, "other pc"), None, "signed with another key");
        let edited = std::fs::read_to_string(&file).unwrap().replace("90", "99999");
        std::fs::write(&file, edited).unwrap();
        assert_eq!(read(&file, "k"), None);
        assert_eq!(add(&file, "k", 5), 5, "an edited file starts again");
        let _ = std::fs::remove_file(&file);
    }
}
