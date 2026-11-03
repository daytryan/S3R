// crates/skate-launcher/src/credentials.rs
//! Keys kept in Windows Credential Manager only (never a file or a log): the playtime file's signing key (playtime.rs).
use windows_sys::Win32::Security::Credentials::{CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW,
    CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC};

const TARGET: &str = "Skate3Remaster/github";  // the private releases key launchers before v12 saved
fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }

/// The repository is public since v12, so nothing reads the old key: uninstalling clears it.
pub fn delete() { delete_from(TARGET) }

/// A key is one token: anything with spaces or control characters inside is a bad paste, and it would
/// otherwise end up in an HTTP header.
fn clean(key: &str) -> Result<&str, String> {
    let key = key.trim();
    if key.is_empty() { return Err("The key is empty.".into()); }
    if key.chars().any(|c| c.is_whitespace() || c.is_control()) { return Err("The key has spaces or control characters in it.".into()); }
    Ok(key)
}

pub fn save_to(target: &str, key: &str) -> Result<(), String> {
    let (mut target, mut user, blob) = (wide(target), wide("access-key"), clean(key)?.as_bytes().to_vec());
    let credential = CREDENTIALW {
        Flags: 0, Type: CRED_TYPE_GENERIC, TargetName: target.as_mut_ptr(), Comment: std::ptr::null_mut(),
        LastWritten: unsafe { std::mem::zeroed() }, CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_ptr() as *mut u8, Persist: CRED_PERSIST_LOCAL_MACHINE, AttributeCount: 0,
        Attributes: std::ptr::null_mut(), TargetAlias: std::ptr::null_mut(), UserName: user.as_mut_ptr(),
    };
    if unsafe { CredWriteW(&credential, 0) } == 0 { Err("Could not save the key in Windows Credential Manager".into()) } else { Ok(()) }
}

pub fn load_from(target: &str) -> Option<String> {
    let target = wide(target);
    let mut found: *mut CREDENTIALW = std::ptr::null_mut();
    if unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut found) } == 0 { return None; }
    let (blob, size) = unsafe { ((*found).CredentialBlob, (*found).CredentialBlobSize as usize) };
    let bytes = if blob.is_null() || size == 0 { Vec::new() } else { unsafe { std::slice::from_raw_parts(blob, size) }.to_vec() };
    unsafe { CredFree(found as *const _) };
    String::from_utf8(bytes).ok().filter(|key| !key.is_empty())
}

pub fn delete_from(target: &str) {
    let target = wide(target);
    unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) };
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_blank_or_broken_key_is_refused_before_anything_is_stored() {
        assert!(save_to("Skate3Remaster/never-written", "   ").is_err());
        assert!(save_to("Skate3Remaster/never-written", "two words").is_err());
        assert!(save_to("Skate3Remaster/never-written", "line\nbreak").is_err());
        assert!(load_from("Skate3Remaster/never-written").is_none());
        assert_eq!(clean("  github_pat_x \r\n").unwrap(), "github_pat_x");
    }
}
