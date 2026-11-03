// crates/skate-launcher/src/update.rs
//! Releases from the repo: pick the newest build, download, verify, install. Requests go to api.github.com with no
//! key (the repository is public); its redirects (signed CDN URLs) are followed without the API's headers.
use crate::app::{App, Command};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, io::{Read, Seek, SeekFrom, Write}, path::{Path, PathBuf}, sync::{atomic::{AtomicBool, AtomicU64, Ordering}, Arc},
          time::{Duration, Instant}};

pub const REPO: &str = "daytryan/S3R";
pub const API: &str = "https://api.github.com/repos/daytryan/S3R/releases";
pub const REFUSED: &str = "GitHub refused the update check. Try again later.";
pub const NOT_FOUND: &str = "GitHub could not find that release. Try again later, or ask the owner.";
/// N8 (release review 10-18): no key, no update. Fail closed rather than install what GitHub serves unsigned.
pub const UNSIGNED: &str = "Updates are off: this launcher has no release-signing key. Download the next version from GitHub by hand.";
pub const RATE_LIMITED: &str = "GitHub is limiting update checks right now. Try again in an hour.";
const OCTET: &str = "application/octet-stream";

#[derive(Debug)]
pub struct Release { pub build: u64, pub meta: Value, assets: Vec<Value>, pub notes: Vec<String> }

impl Release {
    fn asset(&self, kind: &str) -> Option<&Value> {
        let name = self.meta[kind]["name"].as_str()?;
        self.assets.iter().find(|a| a["name"] == name)
    }
    /// U3: the download size GitHub lists for a package (0 when it doesn't say).
    pub fn size(&self, kind: &str) -> u64 { self.asset(kind).and_then(|a| a["size"].as_u64()).unwrap_or(0) }
}

fn hex(bytes: &[u8]) -> String { bytes.iter().map(|b| format!("{b:02x}")).collect() }

/// F1: one agent for the launcher's life, so requests to a host reuse its connection (ureq pools them). B1: it trusts
/// the Mozilla roots and the Windows root store, and goes through the Windows (or HTTPS_PROXY) proxy. The local test
/// servers are reached directly, whatever proxy this PC has.
fn agent(url: &str) -> &'static ureq::Agent {
    static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    static DIRECT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    let build = |proxy: bool| {
        let b = ureq::AgentBuilder::new().redirects(0).timeout_connect(Duration::from_secs(15)).timeout_read(Duration::from_secs(120))
            .tls_config(tls());
        match system_proxy().filter(|_| proxy).and_then(|p| ureq::Proxy::new(p).ok()) {
            Some(p) => b.proxy(p),
            None => b.try_proxy_from_env(proxy),
        }.build()
    };
    if url.starts_with("http://127.0.0.1") { DIRECT.get_or_init(|| build(false)) } else { AGENT.get_or_init(|| build(true)) }
}

/// B1: the Mozilla roots ureq uses, plus every root in the Windows store (antivirus HTTPS scanning and company proxies
/// add theirs there). Both, not either: stale Windows roots were build 8's problem. A cert rustls can't read is skipped.
fn tls() -> Arc<rustls::ClientConfig> {
    Arc::new(rustls::ClientConfig::builder_with_provider(rustls::crypto::ring::default_provider().into())
        .with_protocol_versions(&[&rustls::version::TLS12, &rustls::version::TLS13]).expect("ring does TLS 1.2 and 1.3")
        .with_root_certificates(roots()).with_no_client_auth())
}

fn roots() -> rustls::RootCertStore {
    use windows_sys::Win32::Security::Cryptography::*;
    let mut roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
    let mut windows = Vec::new();
    unsafe {
        let store = CertOpenSystemStoreW(0, wide("ROOT").as_ptr());
        if !store.is_null() {
            let mut cert = CertEnumCertificatesInStore(store, std::ptr::null());
            while !cert.is_null() {
                windows.push(rustls::pki_types::CertificateDer::from(std::slice::from_raw_parts((*cert).pbCertEncoded, (*cert).cbCertEncoded as usize).to_vec()));
                cert = CertEnumCertificatesInStore(store, cert);  // frees the previous one
            }
            CertCloseStore(store, 0);
        }
    }
    roots.add_parsable_certificates(windows);
    roots
}

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }

/// B1: the proxy set in Windows' settings (HKCU Internet Settings), as `http://host:port`. ponytail: a manual proxy only;
/// a PAC script or auto-detect is not followed, and its bypass list is ignored.
fn system_proxy() -> Option<String> {
    use windows_sys::Win32::System::Registry::*;
    let key = wide(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings");
    let (mut on, mut size) = (0u32, 4u32);
    let read = unsafe { RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), wide("ProxyEnable").as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), &mut on as *mut _ as *mut _, &mut size) };
    if read != 0 || on == 0 { return None; }
    let (mut buf, mut size) = ([0u16; 512], 1024u32);
    let read = unsafe { RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), wide("ProxyServer").as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut size) };
    if read != 0 { return None; }
    proxy_url(&String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap_or(0)]))
}

/// `host:port`, or `http=a:1;https=b:2` (the https one wins), as a URL ureq takes.
fn proxy_url(setting: &str) -> Option<String> {
    let pick = |scheme: &str| setting.split(';').find_map(|p| p.trim().strip_prefix(scheme).map(str::to_owned));
    let server = if setting.contains('=') { pick("https=").or_else(|| pick("http="))? } else { setting.trim().to_owned() };
    (!server.is_empty()).then(|| if server.contains("://") { server } else { format!("http://{server}") })
}

/// B1: a certificate the PC doesn't trust (antivirus or network inspection) gets fetch_tools' explanation.
fn reach_error(e: ureq::Error, what: &str) -> String {
    let text = e.to_string();
    if text.contains("UnknownIssuer") || text.contains("certificate") {
        format!("Your antivirus or network is checking secure connections, and the launcher can't confirm GitHub's \
                 certificate. Turn off HTTPS scanning in your antivirus for a moment, or try another network. ({text})")
    } else { format!("{what}: {text}") }
}

/// One request to the API, following its redirect. `range` is an HTTP Range value.
fn follow(api: &str, url: &str, accept: &str, range: Option<&str>) -> Result<ureq::Response, String> {
    follow_if(api, url, accept, range, None)
}

/// [`follow`] with `If-None-Match` (B3): a 304 comes back as is, and GitHub does not count it against the limit.
fn follow_if(api: &str, url: &str, accept: &str, range: Option<&str>, etag: Option<&str>) -> Result<ureq::Response, String> {
    if !url.starts_with(api) { return Err("Unexpected download URL".into()); }
    let mut request = agent(url).get(url).set("Accept", accept)
        .set("User-Agent", "Skate3Remaster-Launcher/1").set("X-GitHub-Api-Version", "2022-11-28");
    if let Some(range) = range { request = request.set("Range", range); }
    if let Some(etag) = etag { request = request.set("If-None-Match", etag); }
    let response = match request.call() {
        Ok(r) => r,
        // Without a key GitHub allows 60 requests an hour: that answers 429, or 403 with no requests remaining.
        Err(ureq::Error::Status(code @ (403 | 429), r)) if code == 429 || r.header("x-ratelimit-remaining") == Some("0") =>
            return Err(RATE_LIMITED.into()),
        Err(ureq::Error::Status(401 | 403, _)) => return Err(REFUSED.into()),
        Err(ureq::Error::Status(404, _)) => return Err(NOT_FOUND.into()),
        Err(e) => return Err(reach_error(e, "Could not reach GitHub")),
    };
    if response.status() == 304 { return Ok(response); }
    if (300..400).contains(&response.status()) {
        let next = response.header("Location").ok_or("Bad redirect")?.to_owned();
        if !(next.starts_with("https://") || api.starts_with("http://127.0.0.1")) { return Err("Insecure redirect".into()); }
        return cdn_get(&next, range);
    }
    Ok(response)
}

/// A request to the signed download URL a redirect gave (no key, no API headers).
fn cdn_get(url: &str, range: Option<&str>) -> Result<ureq::Response, String> {
    let mut request = agent(url).get(url).set("User-Agent", "Skate3Remaster-Launcher/1");
    if let Some(range) = range { request = request.set("Range", range); }
    request.call().map_err(|e| reach_error(e, "Download failed"))
}

fn get(api: &str, url: &str, accept: &str, limit: u64) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    follow(api, url, accept, None)?.into_reader().take(limit + 1).read_to_end(&mut body).map_err(|e| e.to_string())?;
    if body.len() as u64 > limit { return Err("Download exceeds size limit".into()); }
    Ok(body)
}

/// The published releases (GitHub lists newest first), drafts, pre-releases and releases without a valid release.json
/// skipped: the newest, then every older one still newer than `installed` (U4: their notes), at most 10. One request for
/// the list plus one release.json per release returned, well inside 60 an hour without a key.
pub fn releases(api: &str, installed: u64) -> Result<Vec<Release>, String> {
    // The tests' fake GitHub serves no signatures: only a test build reaches it unsigned (signing has its own tests).
    #[cfg(test)]
    let key = TEST_UNSIGNED;
    #[cfg(not(test))]
    let key = RELEASE_KEY;
    releases_signed(api, installed, key)
}
/// [`releases_signed`]'s key in tests that are about something other than signing.
#[cfg(test)]
const TEST_UNSIGNED: &str = "unsigned (tests only)";

/// The owner's offline release-signing public key: 64 hex characters (ed25519). Empty: not set up yet, and no release
/// is taken at all ([`UNSIGNED`]; N8, release review 10-18: before, they were trusted on GitHub's word alone). A release
/// is only taken with a `release.json.sig` asset: the raw 64-byte ed25519 signature of the exact release.json bytes. release.json holds every
/// package's and file's sha256, so this one signature covers the whole update.
/// Set up once (Git for Windows ships openssl; keep release-key.pem offline, never in the repo):
///   openssl genpkey -algorithm ed25519 -out release-key.pem
///   openssl pkey -in release-key.pem -pubout -outform DER | tail -c 32 | xxd -p -c 64     -> paste here
/// Each release (Build-Release.ps1, after release.json is written; upload the .sig beside it):
///   Build-Release.ps1 -Build N -ReleaseKey <path to release-key.pem, outside the repo> (tools/pipeline/package.py
///   signs the standalone release.json and checks the key is the one pasted here)
/// ponytail: one key, no rotation; a lost key means a new launcher with a new key, installed by hand.
pub const RELEASE_KEY: &str = "bad53add666ec6c1bc5a78bcefc748ffc6bad98fba765a8edf7b4fcc7425489e";

/// `key`'s 32 bytes: exactly 64 hex characters, else None (a PEM, a wrong length or a stray character all fail closed).
fn public_key(key: &str) -> Option<Vec<u8>> {
    (key.len() == 64 && key.is_ascii()).then(|| (0..32).map(|i| u8::from_str_radix(&key[2 * i..2 * i + 2], 16).ok()).collect()).flatten()
}

/// `sig` is a valid signature of `meta` by `key` (64 hex characters).
fn signed(meta: &[u8], sig: Option<&[u8]>, key: &str) -> bool {
    match (public_key(key), sig) {
        (Some(key), Some(sig)) => ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key).verify(meta, sig).is_ok(),
        _ => false,
    }
}

/// A path from release.json or a package: plain and relative, so `game.join(rel)` stays inside the game folder.
fn safe_rel(rel: &str) -> bool {
    !rel.is_empty() && !rel.contains('\\') && !rel.contains(':') && !rel.contains('\0') && rel.split('/').all(|p| !p.is_empty() && p != "." && p != "..")
}

/// B3: the release list, answered from `cache` when GitHub says it has not changed (a 304 is free; the limit without a
/// key is 60 an hour per IP, shared by a whole school or CGNAT). No cache: a plain request.
fn release_list(api: &str, cache: Option<&Path>) -> Result<Vec<u8>, String> {
    let saved = cache.and_then(|c| std::fs::read(c).ok()).and_then(|b| serde_json::from_slice::<Value>(&b).ok()).unwrap_or(Value::Null);
    let etag = saved["etag"].as_str().filter(|_| saved["body"].is_string());
    let response = follow_if(api, &format!("{api}?per_page=30"), "application/vnd.github+json", None, etag)?;
    if response.status() == 304 { if let Some(body) = saved["body"].as_str() { return Ok(body.as_bytes().to_vec()); } }
    let tag = response.header("ETag").map(str::to_owned);
    let mut body = Vec::new();
    response.into_reader().take((8 << 20) + 1).read_to_end(&mut body).map_err(|e| e.to_string())?;
    if body.len() > 8 << 20 { return Err("Download exceeds size limit".into()); }
    if let (Some(cache), Some(tag), Ok(text)) = (cache, tag, std::str::from_utf8(&body)) {
        let temp = cache.with_extension("tmp");  // a torn write is only a cache miss
        let _ = std::fs::create_dir_all(cache.parent().unwrap_or(Path::new("."))).and_then(|_| std::fs::write(&temp, json!({"etag": tag, "body": text}).to_string()))
            .and_then(|_| std::fs::rename(&temp, cache));
    }
    Ok(body)
}

fn releases_signed(api: &str, installed: u64, key: &str) -> Result<Vec<Release>, String> {
    #[cfg(test)]
    let unsigned = key == TEST_UNSIGNED;
    #[cfg(not(test))]
    let unsigned = false;
    if key.is_empty() {
        crate::maintenance::log("update check refused: update signing not set up (RELEASE_KEY is empty)");
        return Err(UNSIGNED.into());
    }
    if !unsigned && public_key(key).is_none() {
        crate::maintenance::log("update check refused: RELEASE_KEY is not 64 hex characters");
        return Err(UNSIGNED.into());
    }
    let cache = (api == API).then(|| crate::maintenance::local_data().join("release-list.json"));
    let list: Vec<Value> = serde_json::from_slice(&release_list(api, cache.as_deref())?)
        .map_err(|_| "GitHub sent an unreadable release list".to_string())?;
    let mut out: Vec<Release> = Vec::new();
    for release in list.into_iter().filter(|r| r["draft"] == false && r["prerelease"] == false) {
        let assets = release["assets"].as_array().cloned().unwrap_or_default();
        let Some(url) = assets.iter().find(|a| a["name"] == "release.json").and_then(|a| a["url"].as_str()) else { continue };
        // B2: only the newest one must answer; an older one that fails just ends the notes there (U4)
        // 8 MiB like the release list: it holds every program file's sha256 (1,407 files, 313 KB in build 10)
        let bytes = match get(api, url, OCTET, 8 << 20) {
            Ok(bytes) => bytes,
            Err(e) if !out.is_empty() => { crate::maintenance::log(&format!("older release skipped: {e} ({url})")); break; }
            Err(e) => return Err(e),
        };
        if !unsigned {
            let sig = assets.iter().find(|a| a["name"] == "release.json.sig").and_then(|a| a["url"].as_str())
                .and_then(|u| get(api, u, OCTET, 64).ok());  // a signature that will not download is no signature
            if !signed(&bytes, sig.as_deref(), key) { crate::maintenance::log(&format!("release skipped: bad or missing signature ({url})")); continue; }
        }
        let Ok(meta) = serde_json::from_slice::<Value>(&bytes) else { continue };
        let Some(build) = meta["build"].as_u64().filter(|_| meta["schema"] == 1 && meta["repository"] == REPO) else { continue };
        if meta["files"].as_object().is_some_and(|f| f.keys().any(|rel| !safe_rel(rel))) { continue; }
        if !out.is_empty() && build <= installed { break; }
        out.push(Release { build, meta, assets, notes: notes_from(release["body"].as_str().unwrap_or("")) });
        if build <= installed || out.len() >= 10 { break; }
    }
    Ok(out)
}

/// The newest published release. An older release is never an update: `needs` wants a higher build.
#[cfg(test)]
pub fn latest(api: &str) -> Result<Option<Release>, String> { Ok(releases(api, u64::MAX)?.into_iter().next()) }

/// The release text as the launcher's list: a heading becomes "# Heading" (a section of What's new), bullets and plain
/// lines stay lines, 400 at most (the whole change log, user 10-29). An indented line that is not a bullet of its own
/// carries on the bullet above (a wrapped line).
fn notes_from(body: &str) -> Vec<String> {
    let mut notes: Vec<String> = Vec::new();
    for raw in body.lines() {
        let line = raw.trim();
        let text = line.trim_start_matches(['-', '*', '•']).trim();
        if text.is_empty() { continue; }
        if line.starts_with('#') {
            let heading = line.trim_start_matches('#').trim();
            if !heading.is_empty() { notes.push(format!("# {heading}")); }
            continue;
        }
        match notes.last_mut() {
            Some(prev) if raw.starts_with([' ', '\t']) && text.len() == line.len() => { prev.push(' '); prev.push_str(text); }
            _ => notes.push(text.into()),
        }
    }
    notes.into_iter().take(400).map(|note| note.chars().take(300).collect()).collect()
}

/// A package into memory (the launcher's own, a few MB), checked against release.json and GitHub's digest.
pub fn download(api: &str, release: &Release, kind: &str) -> Result<Vec<u8>, String> {
    let want = release.meta[kind]["sha256"].as_str().ok_or("Release is missing a checksum")?.to_lowercase();
    let asset = release.asset(kind).ok_or("Release is missing a package")?;
    let bytes = get(api, asset["url"].as_str().unwrap_or(""), OCTET, 256 << 20)?;  // D5: the game zip streams (download_to)
    check_digest(asset, &want, &hex(&Sha256::digest(&bytes)))?;
    Ok(bytes)
}

fn check_digest(asset: &Value, want: &str, got: &str) -> Result<(), String> {
    if got != want { return Err("Download checksum mismatch".into()); }
    if let Some(digest) = asset["digest"].as_str() { if digest != format!("sha256:{got}") { return Err("GitHub digest mismatch".into()); } }
    Ok(())
}

/// Download progress for the page (#8): at most 4 a second, and the last one always.
pub struct Progress<'a> { total: u64, done: u64, started: Instant, last: Option<Instant>, send: Box<dyn FnMut(Value) + 'a> }

impl<'a> Progress<'a> {
    pub fn new(total: u64, send: impl FnMut(Value) + 'a) -> Self {
        Self { total, done: 0, started: Instant::now(), last: None, send: Box::new(send) }
    }
    fn add(&mut self, bytes: u64) {
        self.done += bytes;
        let now = Instant::now();
        if self.done < self.total && self.last.is_some_and(|t| now - t < Duration::from_millis(250)) { return; }
        self.last = Some(now);
        let speed = (self.done as f64 / (now - self.started).as_secs_f64().max(0.001)) as u64;
        (self.send)(json!({"type": "download", "done": self.done.min(self.total), "total": self.total, "speed": speed}));
    }
}

/// #8: stream a package to `part` (not memory), resuming what an earlier try left there with an HTTP Range request,
/// then check the whole file against release.json and GitHub's digest. A bad file is deleted, so the next try is clean.
pub fn download_to(api: &str, release: &Release, kind: &str, part: &Path, progress: &mut Progress) -> Result<(), String> {
    let want = release.meta[kind]["sha256"].as_str().ok_or("Release is missing a checksum")?.to_lowercase();
    let asset = release.asset(kind).ok_or("Release is missing a package")?;
    let size = asset["size"].as_u64().unwrap_or(u64::MAX);
    std::fs::create_dir_all(part.parent().ok_or("bad download folder")?).map_err(|e| e.to_string())?;
    let mut have = std::fs::metadata(part).map_or(0, |m| m.len());
    // D5: a part from another upload of the asset (same tag, re-uploaded) is not resumed: its end would not match
    let (stamp, upload) = (part.with_extension("asset"), format!("{} {}", asset["id"], asset["updated_at"]));
    if have > size || std::fs::read_to_string(&stamp).is_ok_and(|s| s != upload) { let _ = std::fs::remove_file(part); have = 0; }
    let _ = std::fs::write(&stamp, &upload);
    // D5: hashed as it is written (a resumed part is read once first), not read back whole afterwards
    let mut hasher = Sha256::new();
    let hash_file = |hasher: &mut Sha256| -> Result<(), String> {
        std::io::copy(&mut std::fs::File::open(part).map_err(|e| e.to_string())?, hasher).map(|_| ()).map_err(|e| e.to_string())
    };
    if have < size {
        let response = follow(api, asset["url"].as_str().unwrap_or(""), OCTET, (have > 0).then(|| format!("bytes={have}-")).as_deref())?;
        let resumed = response.status() == 206;
        if resumed { hash_file(&mut hasher)?; }
        let mut file = std::fs::OpenOptions::new().create(true).write(true).append(resumed).truncate(!resumed).open(part).map_err(|e| e.to_string())?;
        progress.add(if resumed { have } else { 0 });
        let (mut reader, mut buf) = (response.into_reader().take(size.saturating_add(1)), vec![0u8; 256 << 10]);
        loop {
            let n = reader.read(&mut buf).map_err(|e| format!("Download failed: {e}"))?;
            if n == 0 { break; }
            file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            hasher.update(&buf[..n]);
            progress.add(n as u64);
        }
    } else { hash_file(&mut hasher)?; }
    let checked = check_digest(asset, &want, &hex(&hasher.finalize()));
    if checked.is_err() { let _ = std::fs::remove_file(part); }
    checked
}

/// #8 delta: a package read in place over HTTP Range requests, so only the zip entries an update changed are fetched.
/// The first request finds the signed download URL; a URL that expires mid-way is looked up again once.
/// F1: zip reads every entry's local header when it opens an archive (and jumps back to the central directory after
/// each), so a miss fetches a small window, a read that carries on doubles it, and every window fetched is kept.
pub struct Remote<'a> { api: String, url: String, cdn: Option<String>, len: u64, pos: u64,
                        windows: Vec<(u64, Vec<u8>)>, next: u64, run: u64, pub fetched: u64, progress: Option<Progress<'a>> }

/// The first fetch at a new place (a local header, a small file). ponytail: fixed; 30 entries cost ~30 requests.
const MISS: u64 = 16 << 10;
const MAX_RUN: u64 = 4 << 20;
/// The windows kept (oldest dropped first): both archive opens and the entries share them.
const KEEP: u64 = 32 << 20;

impl<'a> Remote<'a> {
    pub fn new(api: &str, release: &Release, kind: &str) -> Result<Self, String> {
        let asset = release.asset(kind).ok_or("Release is missing a package")?;
        let len = asset["size"].as_u64().ok_or("Release does not list the package size")?;
        Ok(Self { api: api.into(), url: asset["url"].as_str().unwrap_or("").into(), cdn: None, len, pos: 0,
                  windows: Vec::new(), next: u64::MAX, run: MISS, fetched: 0, progress: None })
    }
    fn fetch(&mut self, from: u64) -> std::io::Result<()> {
        self.run = if from == self.next { (self.run * 2).min(MAX_RUN) } else { MISS };
        let end = (from + self.run).min(self.len) - 1;
        let range = format!("bytes={from}-{end}");
        let response = match self.cdn.as_deref().map(|cdn| cdn_get(cdn, Some(&range))) {
            Some(Ok(r)) => r,
            _ => {  // first request, or the signed URL ran out: through the API again
                let r = follow(&self.api, &self.url, OCTET, Some(&range)).map_err(std::io::Error::other)?;
                self.cdn = (r.get_url() != self.url).then(|| r.get_url().to_owned());
                r
            }
        };
        if response.status() != 206 { return Err(std::io::Error::other("the server does not resume downloads")); }
        let mut buf = Vec::with_capacity((end - from + 1) as usize);
        response.into_reader().take(end - from + 1).read_to_end(&mut buf)?;
        if buf.len() as u64 != end - from + 1 { return Err(std::io::Error::other("the download ended early")); }
        self.fetched += buf.len() as u64;
        if let Some(p) = self.progress.as_mut() { p.add(buf.len() as u64); }
        self.next = end + 1;
        self.windows.push((from, buf));
        while self.windows.len() > 1 && self.windows.iter().map(|w| w.1.len() as u64).sum::<u64>() > KEEP { self.windows.remove(0); }
        Ok(())
    }
}

impl Read for Remote<'_> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.pos >= self.len || out.is_empty() { return Ok(0); }
        let pos = self.pos;
        let hit = |w: &(u64, Vec<u8>)| (w.0..w.0 + w.1.len() as u64).contains(&pos);
        if !self.windows.iter().any(hit) { self.fetch(pos)?; }
        let (at, buf) = self.windows.iter().rev().find(|w| hit(w)).unwrap();
        let from = (pos - at) as usize;
        let n = out.len().min(buf.len() - from);
        out[..n].copy_from_slice(&buf[from..from + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for Remote<'_> {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let pos = match to { SeekFrom::Start(p) => p as i64, SeekFrom::End(d) => self.len as i64 + d, SeekFrom::Current(d) => self.pos as i64 + d };
        if pos < 0 { return Err(std::io::Error::other("seek before start")); }
        self.pos = pos as u64;
        Ok(self.pos)
    }
}

/// The release's program files that differ from what is on disk (missing, changed, damaged): all that an install
/// has to write. Everything else is left alone: no download (delta), no backup copy, no move.
/// F3: a release.json with `sizes` (package.py) lets a file whose size differs skip the hash.
pub fn changed_files(release: &Release, game: &Path) -> HashSet<String> {
    let sizes = &release.meta["sizes"];
    release.meta["files"].as_object().into_iter().flatten()
        .filter(|(rel, want)| sizes[rel.as_str()].as_u64().is_some_and(|n| std::fs::metadata(game.join(rel)).map_or(true, |m| m.len() != n))
            || file_hash(&game.join(rel)).as_deref() != want.as_str().map(str::to_lowercase).as_deref())
        .map(|(rel, _)| rel.clone()).collect()
}

fn file_hash(path: &Path) -> Option<String> {
    let mut hasher = Sha256::new();
    std::io::copy(&mut std::fs::File::open(path).ok()?, &mut hasher).ok()?;
    Some(hex(&hasher.finalize()))
}

/// The most an install unpacks. ponytail: a fixed cap, well over the packages today; raise it if one ever grows past it.
const MAX_UNPACK: u64 = 8 << 30;

/// Unpack `prefix` entries (only the `only` ones, by their path under `prefix`, when given) into `dest`.
pub fn unpack<R: Read + Seek>(zip: R, prefix: &str, dest: &Path, only: Option<&HashSet<String>>) -> Result<Vec<PathBuf>, String> {
    let mut zip = zip::ZipArchive::new(zip).map_err(|_| "Damaged download".to_string())?;
    let (mut written, mut total) = (Vec::new(), 0u64);
    for i in 0..zip.len() {
        let mut file = zip.by_index(i).map_err(|_| "Damaged download".to_string())?;
        let name = file.name().to_owned();
        let rel = name.strip_prefix(prefix).ok_or("Unsafe archive path")?;
        if !(safe_rel(rel) || file.is_dir() && (rel.is_empty() || safe_rel(rel.trim_end_matches('/')))) { return Err("Unsafe archive path".into()); }
        if file.is_dir() || only.is_some_and(|only| !only.contains(rel)) { continue; }
        let out = dest.join(rel);
        std::fs::create_dir_all(out.parent().unwrap()).map_err(|e| e.to_string())?;
        // a delta reads entries of a zip whose own hash is never checked: no entry may inflate past the size it declares
        let size = file.size();
        total = total.saturating_add(size);
        if total > MAX_UNPACK { return Err("Damaged download".into()); }
        let out_file = std::fs::File::create(&out).map_err(|e| e.to_string())?;
        let wrote = std::io::copy(&mut (&mut file).take(size), &mut std::io::BufWriter::new(out_file)).map_err(|e| e.to_string())?;
        if wrote != size || file.read(&mut [0]).map_err(|e| e.to_string())? != 0 { return Err("Damaged download".into()); }
        written.push(out);
    }
    Ok(written)
}

/// Bytes still to fetch for the `only` entries of a zip (their compressed sizes), to choose delta or whole (#8), plus
/// one Remote miss per entry: opening the archive reads every local header (F1).
fn entry_bytes<R: Read + Seek>(zip: R, prefix: &str, only: &HashSet<String>) -> Result<u64, String> {
    let mut zip = zip::ZipArchive::new(zip).map_err(|_| "Damaged download".to_string())?;
    let mut sum = zip.len() as u64 * MISS;
    for i in 0..zip.len() {
        let file = zip.by_index_raw(i).map_err(|_| "Damaged download".to_string())?;
        if file.name().strip_prefix(prefix).is_some_and(|rel| only.contains(rel)) { sum += file.compressed_size() + 64; }
    }
    Ok(sum)
}

/// The folder an update keeps the program files it replaced in, until the update's build finishes (#9, #16).
pub fn backup_dir(game: &Path) -> PathBuf { game.with_extension("bak") }

/// What a backup's `journal.json` says: written before the first file moves ("installing") and again once the install
/// is complete ("installed": kept until the build seals, then dropped).
fn journal(game: &Path) -> Value {
    std::fs::read(backup_dir(game).join("journal.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null)
}

fn write_journal(game: &Path, state: &str, added: &[String]) -> Result<(), String> {
    let text = json!({"state": state, "added": added}).to_string();
    let (dir, temp) = (backup_dir(game), backup_dir(game).join("journal.json.tmp"));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(&temp, text).and_then(|_| std::fs::rename(&temp, dir.join("journal.json"))).map_err(|e| e.to_string())
}

/// Put the program files an update replaced back, and take away the ones it added. True when the old build is back.
/// A file that can't be put back (the game running from it) keeps the backup for the next start.
pub fn restore_backup(game: &Path) -> bool {
    let backup = backup_dir(game);
    if !backup.is_dir() { return false; }
    let added = journal(game)["added"].as_array().cloned().unwrap_or_default();
    for rel in added.iter().filter_map(Value::as_str) { let _ = std::fs::remove_file(game.join(rel)); }
    let mut ok = true;
    for saved in walk(&backup) {
        let rel = saved.strip_prefix(&backup).unwrap();
        if rel == Path::new("journal.json") || rel.extension().is_some_and(|e| e == "part") { continue; }
        let target = game.join(rel);
        if file_hash(&target).is_some() && file_hash(&target) == file_hash(&saved) { continue; }  // never left (a running exe)
        ok &= std::fs::create_dir_all(target.parent().unwrap()).and_then(|_| std::fs::copy(&saved, &target)).is_ok();
    }
    if ok { let _ = std::fs::remove_dir_all(&backup); }
    ok
}

/// #9: the update's build finished (or it needed none): its backup is no longer needed. Only a complete install's
/// backup is dropped; an interrupted one is never thrown away (#16).
pub fn drop_backup(game: &Path) {
    if journal(game)["state"] == "installed" { let _ = std::fs::remove_dir_all(backup_dir(game)); }
}

/// #16, at launcher start: an install that stopped half-way (power cut, crash) is undone from its backup.
pub fn recover_interrupted(game: &Path) -> bool {
    backup_dir(game).is_dir() && journal(game)["state"] != "installed" && restore_backup(game)
}

/// Unpack `game/` from the zip next to the game folder, check every file against the release's hashes, then
/// move the lot into place. The staging folder is always removed, success or not.
pub fn install_game<R: Read + Seek>(release: &Release, zip: R, game: &Path) -> Result<(), String> {
    // One rollback point: an earlier update still waiting for its build is undone first, back to the last playable files.
    // Audit 10-24: before what changed is measured, or the files this build shares with that one stay old or missing.
    if backup_dir(game).is_dir() && !restore_backup(game) { return Err("Close the game, then press Update again.".into()); }
    let changed = changed_files(release, game);
    let staging = game.with_extension("new");
    let _ = std::fs::remove_dir_all(&staging);
    let result = move_into_place(release, zip, &changed, &staging, game);
    let _ = std::fs::remove_dir_all(&staging);
    // and the full download an earlier try left to resume from (audit 10-24: a delta install never cleared it)
    if result.is_ok() { let _ = std::fs::remove_dir_all(game.with_extension("download")); }
    result
}

/// The program files the installed build listed that the new one doesn't (B1: the single setup exe became a folder).
fn removed_files(release: &Release, game: &Path) -> Vec<String> {
    let old: Value = std::fs::read(game.join("release.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
    old["files"].as_object().into_iter().flatten().map(|(rel, _)| rel)
        .filter(|rel| release.meta["files"].get(rel.as_str()).is_none() && safe_rel(rel))
        .cloned().collect()
}

fn move_into_place<R: Read + Seek>(release: &Release, zip: R, changed: &HashSet<String>, staging: &Path, game: &Path) -> Result<(), String> {
    let mut wanted = changed.clone();
    wanted.insert("release.json".into());
    for path in unpack(zip, "game/", staging, Some(&wanted))? {
        let rel = path.strip_prefix(staging).unwrap().to_string_lossy().replace('\\', "/");
        if rel == "release.json" {
            // the packaged release.json must describe this very release (a delta never checks the zip's own hash)
            let inner: Value = serde_json::from_slice(&std::fs::read(&path).map_err(|e| e.to_string())?).map_err(|_| "Damaged download")?;
            if inner["build"] != release.meta["build"] || inner["files"] != release.meta["files"] { return Err("Program checksum mismatch".into()); }
        } else if file_hash(&path).as_deref() != release.meta["files"][&rel].as_str() {
            return Err("Program checksum mismatch".into());
        }
    }
    if walk(staging).len() != wanted.len() { return Err("Program checksum mismatch".into()); }  // a listed file the zip lacks
    let had_build = game.join("release.json").is_file();
    // release.json moves LAST: it is what says "this build is installed", so a half-done update is offered again.
    let mut files = walk(staging);
    files.sort_by_key(|p| (p.strip_prefix(staging).unwrap() == Path::new("release.json"), p.clone()));
    let removed = removed_files(release, game);
    let added: Vec<String> = files.iter().map(|p| p.strip_prefix(staging).unwrap().to_string_lossy().replace('\\', "/"))
        .filter(|rel| !game.join(rel).is_file()).collect();
    write_journal(game, "installing", &added)?;
    // Every file that gets replaced or removed is copied to the backup first (a copy, not a rename: a running exe can be
    // renamed but not deleted, and a running game must stop the update). Any failure puts every file back.
    let backup = backup_dir(game);
    let save = |rel: &Path| -> Result<(), String> {
        let (target, copy) = (game.join(rel), backup.join(rel));
        if !target.is_file() { return Ok(()); }
        std::fs::create_dir_all(copy.parent().unwrap()).map_err(|e| e.to_string())?;
        let part = copy.with_extension("part");  // complete copies only: a half copy must never be restored
        std::fs::copy(&target, &part).and_then(|_| std::fs::rename(&part, &copy)).map_err(|e| e.to_string())?;
        // a check stopped a moment ago, or a virus scan, lets go within a second or two; a running game never does
        let gone = (0..10).any(|attempt| {
            if attempt > 0 { std::thread::sleep(Duration::from_millis(300)); }
            std::fs::remove_file(&target).is_ok()
        });
        if gone { Ok(()) } else { Err("Close the game, then press Update again.".into()) }
    };
    let result = (|| -> Result<(), String> {
        for rel in &removed { save(Path::new(rel))?; }
        for entry in &files {
            let rel = entry.strip_prefix(staging).unwrap();
            save(rel)?;
            std::fs::create_dir_all(game.join(rel).parent().unwrap()).map_err(|e| e.to_string())?;
            std::fs::rename(entry, game.join(rel)).map_err(|e| e.to_string())?;
        }
        Ok(())
    })();
    match &result {
        Err(_) => { restore_backup(game); }
        Ok(()) if had_build => write_journal(game, "installed", &added)?,
        Ok(()) => { let _ = std::fs::remove_dir_all(&backup); }  // a first install has nothing to go back to
    }
    result
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = entry.path();
        if p.is_dir() { out.extend(walk(&p)) } else { out.push(p) }
    }
    out
}

/// The game package: only the changed entries over Range requests when that is clearly less (#8 delta), else the
/// whole zip streamed to disk with resume. Installs either way.
fn fetch_and_install(app: &App, api: &str, release: &Release) -> Result<(), String> {
    let game = app.game();
    let changed = changed_files(release, &game);
    let mut wanted = changed.clone();
    wanted.insert("release.json".into());
    let size = release.size("game");
    let send = |m: Value| app.send(m);
    if game.join("release.json").is_file() && size > 0 {
        if let Ok(mut remote) = Remote::new(api, release, "game") {
            if let Ok(need) = entry_bytes(&mut remote, "game/", &wanted) {
                if need * 10 < size * 7 {
                    remote.progress = Some(Progress::new(need, send));
                    remote.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
                    begin_install(app)?;
                    return install_game(release, remote, &game);
                }
            }
        }
    }
    let part = game.with_extension("download").join("game.zip.part");
    let mut progress = Progress::new(size, send);
    download_to(api, release, "game", &part, &mut progress)?;
    begin_install(app)?;
    install_game(release, std::fs::File::open(&part).map_err(|e| e.to_string())?, &game)
}

static STOPPED: AtomicBool = AtomicBool::new(false);
/// The program files are about to be replaced: never under a build. The flag is set under the build's lock, and a build
/// checks it under the same lock before it starts (pipeline::spawn), so the two can't both begin. Free clears it.
pub fn begin_install(app: &App) -> Result<(), String> {
    let running = app.running.lock().unwrap();
    if running.is_some() { return Err("Wait for the build to finish, then update.".into()); }
    INSTALLING.store(true, Ordering::SeqCst);
    drop(running);
    STOPPED.fetch_or(crate::pipeline::stop_helpers(&app.helpers), Ordering::SeqCst);  // the startup check runs from support/
    Ok(())
}

/// #5: a repair takes the files of the installed build only; another build's would mix versions.
pub fn repair_release(list: &[Release], build: u64) -> Result<&Release, String> {
    list.iter().find(|r| r.build == build).ok_or_else(|| "This version is no longer available to repair. Update instead.".into())
}

/// #5: hash the program files `game\release.json` lists. A damaged or missing one (an antivirus quarantine, a bad
/// disk) is fetched again from the installed build's release (none if that one is gone: Update instead).
pub fn verify_program(app: &Arc<App>) -> Value {
    let game = app.game();
    let Some(meta) = std::fs::read(game.join("release.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else {
        return json!({"ok": true, "files": 0, "bad": 0, "message": ""});
    };
    let installed = Release { build: meta["build"].as_u64().unwrap_or(0), meta, assets: vec![], notes: vec![] };
    let files = installed.meta["files"].as_object().map_or(0, |f| f.len());
    let bad = changed_files(&installed, &game).len();
    if bad == 0 { return json!({"ok": true, "files": files, "bad": 0, "message": ""}); }
    if UPDATING.swap(true, Ordering::SeqCst) { return json!({"ok": false, "files": files, "bad": bad, "message": "An update is running."}); }
    let _free = Free;
    app.toast(&format!("{bad} program file{} damaged. Downloading {} again…", if bad == 1 { " is" } else { "s are" }, if bad == 1 { "it" } else { "them" }));
    let fixed = (|| -> Result<(), String> {
        let list = releases(API, installed.build.saturating_sub(1))?;
        let release = repair_release(&list, installed.build)?;
        fetch_and_install(app, API, release)
    })();
    match fixed {
        Ok(()) => json!({"ok": true, "files": files, "bad": bad,
                         "message": format!("{bad} damaged program file{} downloaded again.", if bad == 1 { " was" } else { "s were" })}),
        Err(e) => json!({"ok": false, "files": files, "bad": bad, "message": format!("{bad} program files are damaged and could not be downloaded again: {e}")}),
    }
}

/// `Skate 3 Remaster.exe` -> `Skate 3 Remaster.old.exe` / `.new.exe`, whatever the exe is called.
fn beside(exe: &Path, tag: &str) -> PathBuf { exe.with_extension(format!("{tag}.exe")) }

pub fn swap_launcher(exe: &Path, new: &[u8]) -> Result<(), String> {
    let (staged, old) = (beside(exe, "new"), beside(exe, "old"));
    std::fs::write(&staged, new).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&old);
    // Windows allows renaming a running exe
    if let Err(e) = std::fs::rename(exe, &old) { let _ = std::fs::remove_file(&staged); return Err(e.to_string()); }
    if let Err(e) = std::fs::rename(&staged, exe) {
        let _ = std::fs::rename(&old, exe);   // put the working launcher back
        let _ = std::fs::remove_file(&staged);
        return Err(e.to_string());
    }
    Ok(())
}

pub fn cleanup_old(exe: &Path) { for tag in ["old", "new"] { let _ = std::fs::remove_file(beside(exe, tag)); } }
pub fn finish_self_update() { if let Ok(exe) = std::env::current_exe() { cleanup_old(&exe) } }

/// What the old launcher passes the new one: wait for my process to end (its exe, now `.old.exe`, can only be deleted
/// then), and `--rebuild` when an update changed the game (the old one's build_after_check, see pipeline::check).
pub fn relaunch_args(pid: u32, rebuild: bool) -> Vec<String> {
    let mut args = vec!["--after".to_owned(), pid.to_string()];
    if rebuild { args.push("--rebuild".to_owned()); }
    args
}

/// The new launcher's side of relaunch_args: (the old launcher's pid, rebuild after the check).
pub fn relaunched(args: &[String]) -> (Option<u32>, bool) {
    let after = args.iter().position(|a| a == "--after").and_then(|i| args.get(i + 1)).and_then(|p| p.parse().ok());
    (after, args.iter().any(|a| a == "--rebuild"))
}

/// Start the launcher a self-update put at `exe` (the path this one was started from: after the swap, current_exe()
/// can name either file). The caller then closes this one.
pub fn relaunch(exe: &Path, rebuild: bool) -> std::io::Result<()> {
    std::process::Command::new(exe).args(relaunch_args(std::process::id(), rebuild)).spawn().map(|_| ())
}

/// Wait (at most 10 s) for the old launcher to end. ponytail: a pid reused in that time only costs the 10 s.
pub fn wait_for_exit(pid: u32) { crate::system::wait_for_exit(pid, 10_000) }

pub fn install_launcher(release: &Release, zip: &[u8]) -> Result<(), String> {
    let temp = std::env::temp_dir().join(format!("s3r-launcher-{}", release.build));
    let _ = std::fs::remove_dir_all(&temp);
    let result = (|| -> Result<(), String> {
        let files = unpack(std::io::Cursor::new(zip), "Skate 3 Remaster/", &temp, None)?;
        let exe = files.iter().find(|p| p.file_name().is_some_and(|n| n == "Skate 3 Remaster.exe")).ok_or("Launcher missing from package")?;
        swap_launcher(&std::env::current_exe().map_err(|e| e.to_string())?, &std::fs::read(exe).map_err(|e| e.to_string())?)
    })();
    let _ = std::fs::remove_dir_all(&temp);
    result
}

/// This launcher's build number, set by `$env:SKATE_BUILD` when Build-Release.ps1 compiles it. A plain
/// `cargo build` has none, and such a launcher never replaces itself.
fn own_build() -> Option<u64> { option_env!("SKATE_BUILD").and_then(|b| b.parse().ok()) }

/// What the page shows as the launcher's version: the build number, or "dev" for a plain `cargo build`.
pub fn version() -> &'static str { option_env!("SKATE_BUILD").unwrap_or("dev") }

/// The game build recorded in `game\release.json` (written last by an install), if the game was installed.
pub fn installed_build(game: &Path) -> Option<u64> {
    std::fs::read(game.join("release.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).and_then(|v| v["build"].as_u64())
}

/// The build a successful self-update has already put on disk (the running exe is still the old one).
static STAGED: AtomicU64 = AtomicU64::new(0);
static UPDATING: AtomicBool = AtomicBool::new(false);
static INSTALLING: AtomicBool = AtomicBool::new(false);

/// True while an update check or install runs.
pub fn updating() -> bool { UPDATING.load(Ordering::SeqCst) }
/// True while an update is replacing program files: only then must Play wait (U1).
pub fn installing() -> bool { INSTALLING.load(Ordering::SeqCst) }

struct Free;
impl Drop for Free {
    fn drop(&mut self) {
        INSTALLING.store(false, Ordering::SeqCst);
        UPDATING.store(false, Ordering::SeqCst);
        crate::hero::poke();
    }
}

/// (the game needs the release, the launcher needs the release)
fn needs(release: u64, installed: u64, own: Option<u64>) -> (bool, bool) {
    (release > installed, own.is_some_and(|mine| release > mine))
}

fn failure(message: impl Into<String>) -> Value { json!({"type": "error", "scope": "update", "message": message.into()}) }

/// U2/U3: what installing `release` would rebuild, and whether it can be built here (sources connected, disk space).
/// The installed setup tool answers (`--probe`); one too old to know it answers nothing, which blocks nothing.
fn probe(app: &App, release: &Release) -> Value {
    use std::os::windows::process::CommandExt;
    let game = app.game();
    if !game.join("data").join("installation.json").is_file() { return Value::Null; }  // never built: Build says it all
    let file = game.with_extension("download").join(format!("release-{}.json", release.build));
    if std::fs::create_dir_all(file.parent().unwrap()).and_then(|_| std::fs::write(&file, release.meta.to_string())).is_err() { return Value::Null; }
    let (env, flags) = crate::pipeline::run_setup(&game, false, false, false);
    let mut command = std::process::Command::new(crate::pipeline::setup_exe(&game));
    command.arg("--pipeline").arg("--probe").arg(&file).arg("--base").arg(game.join("data")).arg("--game-exe").arg(game.join("skate3rust.exe"));
    for folder in app.folders() { command.arg("--sources").arg(folder); }
    let Ok(mut child) = command.envs(env).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null()).creation_flags(flags).spawn() else { return Value::Null };
    crate::pipeline::dies_with_launcher(&child);
    app.helpers.lock().unwrap().push(child.id());
    // A12: a tool that hangs costs 60 s, then answers nothing (which blocks nothing). B6: lines read lossily.
    let out = crate::pipeline::stdout_within(&mut child, Duration::from_secs(60)).unwrap_or_default();
    let answer = String::from_utf8_lossy(&out).lines().filter_map(crate::pipeline::parse_line).find(|e| e["type"] == "probe").unwrap_or(Value::Null);
    app.helpers.lock().unwrap().retain(|&p| p != child.id());
    let _ = std::fs::remove_file(file);
    answer
}

/// U6: look again every few hours while the launcher stays open (one sleeping thread; 2 requests per look).
pub fn check_every_few_hours(app: Arc<App>) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(4 * 3600));
        if app.running.lock().unwrap().is_none() && !updating() {
            let auto = app.settings.lock().unwrap().auto_update;
            handle(&app, if auto { Command::UpdateAutomatically } else { Command::CheckUpdatesQuietly });
        }
    });
}

pub fn handle(app: &Arc<App>, command: Command) {
    match command {
        Command::CheckUpdates | Command::UpdateNow | Command::CheckUpdatesQuietly | Command::UpdateAutomatically => {
            let install = matches!(command, Command::UpdateNow | Command::UpdateAutomatically);
            let automatic = matches!(command, Command::UpdateAutomatically);
            // startup (a check, or the automatic install): failures only on the Updates status line, no error toasts
            let quiet = matches!(command, Command::CheckUpdatesQuietly | Command::UpdateAutomatically);
            if install && app.running.lock().unwrap().is_some() {
                return app.send(failure("Wait for the build to finish, then update."));
            }
            if install && !automatic && app.game_running() {
                return app.send(failure("Close the game, then press Update again."));
            }
            if UPDATING.swap(true, Ordering::SeqCst) {
                if !quiet { app.send(failure("An update check is already running.")); }
                return;
            }
            let app = app.clone();
            std::thread::spawn(move || {
                let _free = Free;
                STOPPED.store(false, Ordering::SeqCst);
                let mut relaunch = None;  // Some(rebuild) once a new launcher is in place
                let mut lost = false;     // A13: another check took UPDATING while this one waited for the game
                let result = (|| -> Result<Value, String> {
                    let installed = installed_build(&app.game()).unwrap_or(0);
                    // a first install needs only the newest release; an installed game hears about every newer one (U4)
                    let list = releases(API, if installed == 0 { u64::MAX } else { installed })?;
                    // no release at all: "build" is null, so the page never says "Updated to build undefined"
                    let Some(release) = list.first() else {
                        return Ok(json!({"type": "update", "available": false, "build": null, "installed": installed}))
                    };
                    let own = own_build().map(|b| b.max(STAGED.load(Ordering::SeqCst)));
                    let (game, launcher) = needs(release.build, installed, own);
                    let probe = if game { probe(&app, release) } else { Value::Null };
                    let blocked = probe["blocked"].as_str().filter(|b| !b.is_empty()).map(str::to_owned);
                    if !install {
                        let size = if game { release.size("game") } else { 0 } + if launcher { release.size("launcher") } else { 0 };
                        let newer: Vec<Value> = list.iter().filter(|r| r.build > installed).map(|r| json!({"build": r.build, "notes": r.notes})).collect();
                        return Ok(json!({"type": "update", "available": game || launcher, "build": release.build,
                                         "installed": installed, "notes": release.notes, "releases": newer, "size": size,
                                         "rebuild": probe["rebuild"], "blocked": blocked}))
                    }
                    if let Some(blocked) = blocked { return Err(blocked); }  // U2: never install what can't be built here
                    if game {
                        if app.game_running() {  // F1: an automatic update waits for the game to close
                            let pid = *app.game_pid.lock().unwrap();
                            if let Some(pid) = pid {
                                // A13: say so, and let checks and repairs run meanwhile (the wait can be hours)
                                app.send(json!({"type": "update", "available": true, "build": release.build, "installed": installed,
                                                "quiet": true, "waiting_for_game": true}));
                                UPDATING.store(false, Ordering::SeqCst);
                                crate::system::wait_for_exit(pid, u32::MAX);
                                if UPDATING.swap(true, Ordering::SeqCst) { lost = true; return Err(String::new()); }  // someone else's now
                            }
                        }
                    }
                    if game || launcher { app.toast("Downloading the update…"); }
                    if game {
                        fetch_and_install(&app, API, release)?;
                        app.build_after_check.store(true, Ordering::SeqCst);  // U5: manual updates go on into the rebuild too
                    }
                    if launcher {
                        install_launcher(release, &download(API, release, "launcher")?)?;
                        STAGED.store(release.build, Ordering::SeqCst);
                        relaunch = Some(game);
                    }
                    Ok(json!({"type": "update", "available": false, "build": release.build, "installed": if game { release.build } else { installed }}))
                })();
                if lost { std::mem::forget(_free); return; }  // quietly: its flags are the other check's
                if let Err(e) = &result { crate::maintenance::log(&format!("update failed: {e}")); }
                let stopped = STOPPED.load(Ordering::SeqCst);
                let refresh = install && (result.is_ok() || stopped) && relaunch.is_none();  // a stopped check never answered
                match result {
                    Ok(mut message) => { message["quiet"] = json!(quiet); app.send(message); }
                    Err(message) if quiet => app.send(json!({"type": "update", "failed": true, "quiet": true, "message": message})),
                    Err(message) => app.send(failure(message)),
                }
                if let Some(rebuild) = relaunch { app.send(json!({"relaunch": rebuild})); }  // main.rs opens the new launcher
                if refresh { app.refresh(); }
            });
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    type Routes = std::sync::Arc<std::sync::Mutex<Vec<(String, u16, Vec<u8>)>>>;

    /// A local stand-in for the Releases API. Routes can be added after the address is known.
    fn serve(routes: Routes) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/repos/daytryan/S3R/releases", listener.local_addr().unwrap());
        std::thread::spawn(move || for stream in listener.incoming() {
            let mut s = stream.unwrap(); let mut buf = [0u8; 4096]; let n = s.read(&mut buf).unwrap();
            let path = String::from_utf8_lossy(&buf[..n]).split_whitespace().nth(1).unwrap_or("").to_string();
            let found = routes.lock().unwrap().iter().find(|r| path.ends_with(&r.0)).cloned();
            let (_, code, body) = found.unwrap_or((String::new(), 404, vec![]));
            let _ = write!(s, "HTTP/1.1 {code} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = s.write_all(&body);
        });
        base
    }

    fn route(routes: &Routes, path: &str, code: u16, body: impl Into<Vec<u8>>) {
        routes.lock().unwrap().push((path.into(), code, body.into()));
    }

    /// Answers exactly one request with `reply`, and hands back the raw request it saw.
    fn one_shot(reply: String) -> (String, std::sync::mpsc::Receiver<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let host = listener.local_addr().unwrap().to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096]; let n = s.read(&mut buf).unwrap();
            tx.send(String::from_utf8_lossy(&buf[..n]).into_owned()).unwrap();
            let _ = s.write_all(reply.as_bytes());
        });
        (host, rx)
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("s3r-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn game_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        { let mut z = zip::ZipWriter::new(&mut buf);
          for (name, body) in files { z.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap(); z.write_all(body).unwrap(); }
          z.finish().unwrap(); }
        buf.into_inner()
    }

    fn release_with(files: Value) -> Release { Release { build: 3, meta: json!({"build": 3, "files": files}), assets: vec![], notes: vec![] } }
    /// The release.json a package carries: the release's own.
    fn inner(release: &Release) -> Vec<u8> { release.meta.to_string().into_bytes() }

    /// The real packages from scripts/Build-Release.ps1 (target/release-out), served as a GitHub release, installed
    /// into an empty folder by the launcher's own code. Run: S3R_RELEASE_OUT=<dir> cargo test -p skate-launcher -- --ignored
    #[test]
    #[ignore]
    fn the_built_release_installs_into_an_empty_folder() {
        let out = PathBuf::from(std::env::var("S3R_RELEASE_OUT").expect("set S3R_RELEASE_OUT"));
        let routes = Routes::default();
        let api = serve(routes.clone());
        let names = ["release.json", "game.zip", "Skate3Remaster-launcher.zip"];
        let assets: Vec<Value> = names.iter().enumerate().map(|(i, n)| json!({"name": n, "url": format!("{api}/assets/{i}")})).collect();
        route(&routes, "releases?per_page=30", 200, json!([{"draft": false, "prerelease": false, "body": "", "assets": assets}]).to_string());
        for (i, n) in names.iter().enumerate() { route(&routes, &format!("assets/{i}"), 200, std::fs::read(out.join(n)).unwrap()); }
        let release = latest(&api).unwrap().expect("the release is picked");
        let game = temp("release-e2e").join("game");
        install_game(&release, std::io::Cursor::new(download(&api, &release, "game").unwrap()), &game).unwrap();
        assert_eq!(installed_build(&game), Some(release.build));
        for (rel, want) in release.meta["files"].as_object().unwrap() {
            assert_eq!(&json!(hex(&Sha256::digest(std::fs::read(game.join(rel)).unwrap()))), want, "{rel}");
        }
        let launcher = download(&api, &release, "launcher").unwrap();
        let unpacked = unpack(std::io::Cursor::new(&launcher), "Skate 3 Remaster/", &game.with_extension("launcher"), None).unwrap();
        assert!(unpacked.iter().any(|p| p.ends_with("Skate 3 Remaster.exe")));
        let _ = std::fs::remove_dir_all(game.parent().unwrap());
    }

    #[test]
    fn a_listed_size_that_differs_is_changed_without_a_hash() {
        let game = temp("sizes");
        std::fs::write(game.join("a.bin"), b"same").unwrap();
        std::fs::write(game.join("b.bin"), b"longer now").unwrap();
        let sum = hex(&Sha256::digest(b"same"));
        let mut release = release_with(json!({"a.bin": sum, "b.bin": sum}));
        release.meta["sizes"] = json!({"a.bin": 4, "b.bin": 4});
        assert_eq!(changed_files(&release, &game), HashSet::from(["b.bin".to_owned()]));
        release.meta["sizes"] = json!({"a.bin": 4, "b.bin": 10});  // the size fits, the hash still decides
        assert_eq!(changed_files(&release, &game), HashSet::from(["b.bin".to_owned()]));
        let _ = std::fs::remove_dir_all(game);
    }

    /// B2: an older release whose release.json fails to download does not cost the player the newest one.
    #[test]
    fn a_failing_older_release_still_gives_the_newest() {
        let routes = Routes::default();
        let api = serve(routes.clone());
        let list = json!([9, 8, 7].map(|b| json!({"draft": false, "prerelease": false, "body": format!("- build {b}"),
            "assets": [{"name": "release.json", "url": format!("{api}/assets/{b}")}]}))).to_string();
        route(&routes, "releases?per_page=30", 200, list);
        route(&routes, "assets/9", 200, json!({"schema": 1, "build": 9, "repository": REPO}).to_string());
        route(&routes, "assets/8", 500, "");
        route(&routes, "assets/7", 200, json!({"schema": 1, "build": 7, "repository": REPO}).to_string());
        assert_eq!(releases(&api, 6).unwrap().iter().map(|r| r.build).collect::<Vec<_>>(), [9]);
        let broken = Routes::default();
        let api = serve(broken.clone());
        route(&broken, "releases?per_page=30", 200, json!([{"draft": false, "prerelease": false,
            "assets": [{"name": "release.json", "url": format!("{api}/assets/9")}]}]).to_string());
        assert!(releases(&api, 6).is_err(), "the newest failing is still an error");
    }

    /// B3: the release list is asked for with the last ETag, and a 304 is answered from the saved copy.
    #[test]
    fn an_unchanged_release_list_is_not_downloaded_again() {
        let cache = temp("etag").join("release-list.json");
        let (host, _first) = one_shot("HTTP/1.1 200 OK\r\nETag: \"v1\"\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]".into());
        let api = format!("http://{host}/repos/daytryan/S3R/releases");
        assert_eq!(release_list(&api, Some(&cache)).unwrap(), b"[]");
        let (host, saw) = one_shot("HTTP/1.1 304 Not Modified\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into());
        let api = format!("http://{host}/repos/daytryan/S3R/releases");
        assert_eq!(release_list(&api, Some(&cache)).unwrap(), b"[]", "the saved list");
        assert!(saw.recv().unwrap().to_lowercase().contains("if-none-match: \"v1\""));
        let _ = std::fs::remove_dir_all(cache.parent().unwrap());
    }

    #[test]
    fn https_trusts_the_windows_roots_too_and_reads_the_windows_proxy() {
        assert!(roots().len() > webpki_roots::TLS_SERVER_ROOTS.len(), "this PC's Windows roots were added to Mozilla's");
        let _ = tls();
        assert_eq!(proxy_url("proxy.corp:8080").as_deref(), Some("http://proxy.corp:8080"));
        assert_eq!(proxy_url("http=a:1;https=b:2").as_deref(), Some("http://b:2"));
        assert_eq!(proxy_url("ftp=c:3;http=a:1").as_deref(), Some("http://a:1"));
        assert_eq!(proxy_url("ftp=c:3"), None);
        assert_eq!(proxy_url(""), None);
        let _ = system_proxy();  // reads this PC's setting without failing
        assert!(std::ptr::eq(agent("https://api.github.com/x"), agent("https://api.github.com/y")), "one agent (F1)");
        assert!(!std::ptr::eq(agent("http://127.0.0.1:1/x"), agent("https://api.github.com/x")), "tests never go through a proxy");
    }

    #[test]
    fn a_rate_limit_says_so() {
        for reply in ["HTTP/1.1 403 X\r\nx-ratelimit-remaining: 0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                      "HTTP/1.1 429 X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"] {
            let (host, _seen) = one_shot(reply.into());
            let api = format!("http://{host}/repos/daytryan/S3R/releases");
            assert_eq!(latest(&api).unwrap_err(), RATE_LIMITED);
        }
        let (host, _seen) = one_shot("HTTP/1.1 403 X\r\nx-ratelimit-remaining: 41\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into());
        let api = format!("http://{host}/repos/daytryan/S3R/releases");
        assert_eq!(latest(&api).unwrap_err(), REFUSED, "a plain 403 is refused");
    }

    #[test]
    fn update_checks_carry_no_authorization() {
        let (host, saw) = one_shot("HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]".into());
        let api = format!("http://{host}/repos/daytryan/S3R/releases");
        assert!(latest(&api).unwrap().is_none(), "the public repository answers");
        assert!(!saw.recv().unwrap().to_lowercase().contains("authorization"));
    }

    #[test]
    fn a_refusal_and_a_missing_release_have_their_own_messages() {
        let routes = Routes::default();
        let api = serve(routes.clone());
        route(&routes, "releases?per_page=30", 401, "{}");
        assert_eq!(latest(&api).unwrap_err(), REFUSED);
        let forbidden = Routes::default();
        let api = serve(forbidden.clone());
        route(&forbidden, "releases?per_page=30", 403, "{}");
        assert_eq!(latest(&api).unwrap_err(), REFUSED);
        let missing = Routes::default();
        let api = serve(missing.clone());
        route(&missing, "releases?per_page=30", 404, "{}");
        assert_eq!(latest(&api).unwrap_err(), NOT_FOUND);
        assert_ne!(NOT_FOUND, REFUSED);
    }

    #[test]
    fn picks_the_newest_published_release_and_rejects_bad_hash() {
        let routes = Routes::default();
        let api = serve(routes.clone());
        let meta = |b: u64| serde_json::json!({"schema":1,"build":b,"repository":REPO,
            "game":{"name":"game.zip","sha256":"00"},"launcher":{"name":"l.zip","sha256":"00"}}).to_string();
        let list = serde_json::json!([   // GitHub's order: newest first
            {"draft":true,"prerelease":false,"assets":[{"name":"release.json","url":format!("{api}/assets/4")}]},
            {"draft":false,"prerelease":true,"assets":[{"name":"release.json","url":format!("{api}/assets/4")}]},
            {"draft":false,"prerelease":false,"assets":[{"name":"notes.txt","url":format!("{api}/assets/9")}]},
            {"draft":false,"prerelease":false,"assets":[{"name":"release.json","url":format!("{api}/assets/2")},
                                                        {"name":"game.zip","url":format!("{api}/assets/3")}]},
            {"draft":false,"prerelease":false,"assets":[{"name":"release.json","url":format!("{api}/assets/1")}]}]).to_string();
        route(&routes, "releases?per_page=30", 200, list);
        route(&routes, "assets/1", 200, meta(5));
        route(&routes, "assets/2", 200, meta(7));
        route(&routes, "assets/4", 200, meta(9));   // a draft and a pre-release: must be ignored
        route(&routes, "assets/3", 200, "PK");
        let release = latest(&api).unwrap().unwrap();
        assert_eq!(release.build, 7);
        assert_eq!(download(&api, &release, "game").unwrap_err(), "Download checksum mismatch");
    }

    #[test]
    fn a_release_for_another_repository_or_schema_is_ignored() {
        let routes = Routes::default();
        let api = serve(routes.clone());
        let list = serde_json::json!([
            {"draft":false,"prerelease":false,"assets":[{"name":"release.json","url":format!("{api}/assets/1")}]},
            {"draft":false,"prerelease":false,"assets":[{"name":"release.json","url":format!("{api}/assets/2")}]}]).to_string();
        route(&routes, "releases?per_page=30", 200, list);
        route(&routes, "assets/1", 200, serde_json::json!({"schema":1,"build":8,"repository":"someone/else"}).to_string());
        route(&routes, "assets/2", 200, serde_json::json!({"schema":2,"build":9,"repository":REPO}).to_string());
        assert!(latest(&api).unwrap().is_none());
    }

    #[test]
    fn a_release_json_listing_thousands_of_files_is_read() {
        // Build 10's lists 1,407 files (313 KB): the old 64 KiB cap failed every install with "Download exceeds size limit".
        let routes = Routes::default();
        let api = serve(routes.clone());
        let files: serde_json::Map<String, Value> =
            (0..4000).map(|i| (format!("support/_internal/file-{i:04}.dll"), serde_json::json!("0".repeat(64)))).collect();
        let meta = serde_json::json!({"schema":1,"build":10,"repository":REPO,"files":files}).to_string();
        assert!(meta.len() > 300 << 10);
        route(&routes, "releases?per_page=30", 200,
            serde_json::json!([{"draft":false,"prerelease":false,"assets":[{"name":"release.json","url":format!("{api}/assets/1")}]}]).to_string());
        route(&routes, "assets/1", 200, meta);
        assert_eq!(latest(&api).unwrap().unwrap().build, 10);
    }

    #[test]
    fn a_matching_download_is_returned_and_an_oversized_one_is_refused() {
        let routes = Routes::default();
        let api = serve(routes.clone());
        let body = b"0123456789".to_vec();
        route(&routes, "assets/5", 200, body.clone());
        assert_eq!(get(&api, &format!("{api}/assets/5"), "application/octet-stream", 10).unwrap(), body);
        assert_eq!(get(&api, &format!("{api}/assets/5"), "application/octet-stream", 9).unwrap_err(), "Download exceeds size limit");
        assert_eq!(get(&api, "https://elsewhere.example/x", "*/*", 10).unwrap_err(), "Unexpected download URL");
        let digest_ok = Release { build: 1, notes: vec![], assets: vec![json!({"name":"g.zip","url":format!("{api}/assets/5"),"digest":"sha256:ff"})],
            meta: json!({"game":{"name":"g.zip","sha256":hex(&Sha256::digest(&body))}}) };
        assert_eq!(download(&api, &digest_ok, "game").unwrap_err(), "GitHub digest mismatch");
    }

    #[test]
    fn a_redirect_goes_to_the_signed_url_without_the_api_headers() {
        let (cdn, cdn_saw) = one_shot("HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello".into());
        let (api_host, api_saw) = one_shot(format!("HTTP/1.1 302 Found\r\nLocation: http://{cdn}/signed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"));
        let api = format!("http://{api_host}/repos/daytryan/S3R/releases");
        let body = get(&api, &format!("{api}/assets/9"), "application/octet-stream", 100).unwrap();
        assert_eq!(body, b"hello");
        assert!(api_saw.recv().unwrap().to_lowercase().contains("x-github-api-version"));
        let cdn_request = cdn_saw.recv().unwrap().to_lowercase();
        assert!(cdn_request.starts_with("get /signed"), "{cdn_request}");
        assert!(!cdn_request.contains("authorization") && !cdn_request.contains("x-github-api-version"), "{cdn_request}");
    }

    #[test]
    fn unsafe_zip_paths_are_refused() {
        let mut buf = std::io::Cursor::new(Vec::new());
        { let mut z = zip::ZipWriter::new(&mut buf);
          z.start_file("game/../evil.exe", zip::write::SimpleFileOptions::default()).unwrap(); z.write_all(b"x").unwrap(); z.finish().unwrap(); }
        let dir = std::env::temp_dir().join(format!("s3r-unzip-{}", std::process::id()));
        assert_eq!(unpack(std::io::Cursor::new(buf.get_ref()), "game/", &dir, None).unwrap_err(), "Unsafe archive path");
        for bad in ["elsewhere/x.exe", "game//x.exe", "game/C:evil.exe", "game/a\\b.exe"] {
            assert_eq!(unpack(std::io::Cursor::new(game_zip(&[(bad, b"x")])), "game/", &dir, None).unwrap_err(), "Unsafe archive path", "{bad}");
        }
        assert!(!dir.exists(), "nothing may be written for a refused archive");
    }

    /// A delta's entries come from a zip nobody hashed whole: an entry that inflates past its declared size is refused.
    #[test]
    fn an_entry_bigger_than_it_says_is_refused() {
        let mut zip = std::io::Cursor::new(Vec::new());
        { let mut z = zip::ZipWriter::new(&mut zip);
          z.start_file("game/a.bin", zip::write::SimpleFileOptions::default()).unwrap(); z.write_all(&[7u8; 100_000]).unwrap(); z.finish().unwrap(); }
        let mut zip = zip.into_inner();
        let central = zip.windows(4).rposition(|w| w == [0x50, 0x4b, 0x01, 0x02]).unwrap();
        zip[central + 24..central + 28].copy_from_slice(&10u32.to_le_bytes());  // claims 10 bytes
        let dir = temp("unzip-bomb");
        assert_eq!(unpack(std::io::Cursor::new(&zip), "game/", &dir, None).unwrap_err(), "Damaged download");
        assert!(unpack(std::io::Cursor::new(game_zip(&[("game/a.bin", &[7u8; 100_000])])), "game/", &dir, None).is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_removed_file_outside_the_game_folder_is_never_touched() {
        let game = temp("removed-outside").join("game");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(game.join("release.json"), json!({"build": 2, "files": {"/Windows/x.dll": "00", "\\x": "00", "ok.bin": "00"}}).to_string()).unwrap();
        assert_eq!(removed_files(&release_with(json!({})), &game), ["ok.bin"]);
        let _ = std::fs::remove_dir_all(game.parent().unwrap());
    }

    /// With a key set, only a release whose release.json carries a good signature is taken; an unsafe file path never is.
    #[test]
    fn a_release_needs_its_signature_once_a_key_is_set() {
        use ring::signature::{Ed25519KeyPair, KeyPair};
        let pair = Ed25519KeyPair::from_pkcs8(Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new()).unwrap().as_ref()).unwrap();
        let key = hex(pair.public_key().as_ref());
        let routes = Routes::default();
        let api = serve(routes.clone());
        let meta = |b: u64, files: Value| json!({"schema": 1, "build": b, "repository": REPO, "files": files}).to_string();
        let list = json!([9, 8, 7, 6].map(|b| json!({"draft": false, "prerelease": false, "body": "",
            "assets": [{"name": "release.json", "url": format!("{api}/assets/{b}")}, {"name": "release.json.sig", "url": format!("{api}/sigs/{b}")}]}))).to_string();
        route(&routes, "releases?per_page=30", 200, list);
        let (bad_path, good) = (meta(9, json!({"../evil.exe": "00"})), meta(6, json!({"a.bin": "00"})));
        route(&routes, "assets/9", 200, bad_path.clone());
        route(&routes, "sigs/9", 200, pair.sign(bad_path.as_bytes()).as_ref().to_vec());  // signed, but escapes the game folder
        route(&routes, "assets/8", 200, meta(8, json!({})));
        route(&routes, "sigs/8", 200, pair.sign(b"something else").as_ref().to_vec());  // a signature of other bytes
        route(&routes, "assets/7", 200, meta(7, json!({})));                         // no signature at all (404)
        route(&routes, "assets/6", 200, good.clone());
        route(&routes, "sigs/6", 200, pair.sign(good.as_bytes()).as_ref().to_vec());
        assert_eq!(releases_signed(&api, 0, &key).unwrap().iter().map(|r| r.build).collect::<Vec<_>>(), [6]);
        let other = hex(Ed25519KeyPair::from_pkcs8(Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new()).unwrap().as_ref())
            .unwrap().public_key().as_ref());
        assert!(releases_signed(&api, 0, &other).unwrap().is_empty(), "another key's signatures count for nothing");
        // N8: no key, or a key that isn't 64 hex characters (a PEM pasted in, one character short), takes nothing
        for broken in ["", "not hex", &key[..63], &*format!("{key}0"), "-----BEGIN PUBLIC KEY-----", &*format!("{}é", &key[..62])] {
            assert_eq!(releases_signed(&api, 0, broken).unwrap_err(), UNSIGNED, "{broken:?}");
        }
        // The signature covers release.json whole: one sum changed under the old signature is refused.
        let tampered = good.replace("\"00\"", "\"ff\"");
        assert_ne!(tampered, good);
        routes.lock().unwrap().retain(|r| r.0 != "assets/6");
        route(&routes, "assets/6", 200, tampered);
        assert!(releases_signed(&api, 0, &key).unwrap().is_empty(), "a swapped sum breaks the signature");
    }

    #[test]
    fn self_update_swaps_and_cleans_up() {
        let dir = std::env::temp_dir().join(format!("s3r-self-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("Skate 3 Remaster.exe"); std::fs::write(&exe, b"old").unwrap();
        swap_launcher(&exe, b"new").unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        assert!(dir.join("Skate 3 Remaster.old.exe").is_file());
        cleanup_old(&exe);
        assert!(!dir.join("Skate 3 Remaster.old.exe").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_failed_swap_keeps_the_working_launcher_and_leaves_no_litter() {
        let dir = temp("swap-fail");
        let exe = dir.join("Skate 3 Remaster.exe"); std::fs::write(&exe, b"old").unwrap();
        // a non-empty folder where the .old.exe copy must go: the first rename cannot succeed
        std::fs::create_dir_all(dir.join("Skate 3 Remaster.old.exe")).unwrap();
        std::fs::write(dir.join("Skate 3 Remaster.old.exe").join("x"), b"x").unwrap();
        assert!(swap_launcher(&exe, b"new").is_err());
        assert_eq!(std::fs::read(&exe).unwrap(), b"old");
        assert!(!dir.join("Skate 3 Remaster.new.exe").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn install_game_verifies_then_replaces_and_keeps_other_files() {
        let (dir, sum) = (temp("install-ok"), |b: &[u8]| hex(&Sha256::digest(b)));
        let game = dir.join("game");
        std::fs::create_dir_all(game.join("data")).unwrap();
        std::fs::write(game.join("skate3rust.exe"), b"old").unwrap();
        std::fs::write(game.join("data/keep.bin"), b"built by the pipeline").unwrap();
        let release = release_with(json!({"skate3rust.exe": sum(b"NEW"), "support/tool.exe": sum(b"T")}));
        let zip = game_zip(&[("game/skate3rust.exe", b"NEW"), ("game/support/tool.exe", b"T"), ("game/release.json", &inner(&release))]);
        install_game(&release, std::io::Cursor::new(zip), &game).unwrap();
        assert_eq!(std::fs::read(game.join("skate3rust.exe")).unwrap(), b"NEW");
        assert_eq!(std::fs::read(game.join("support/tool.exe")).unwrap(), b"T");
        assert!(game.join("release.json").is_file());
        assert_eq!(std::fs::read(game.join("data/keep.bin")).unwrap(), b"built by the pipeline");
        assert!(!game.with_extension("new").exists(), "staging must be removed");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_failed_swap_puts_every_file_back_and_keeps_the_old_release_json() {
        use std::os::windows::fs::OpenOptionsExt;
        let (dir, sum) = (temp("install-rollback"), |b: &[u8]| hex(&Sha256::digest(b)));
        let game = dir.join("game");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(game.join("a.bin"), b"old-a").unwrap();
        std::fs::write(game.join("zzz.exe"), b"old-z").unwrap();
        std::fs::write(game.join("release.json"), br#"{"build":1}"#).unwrap();
        // the running game: an open handle that does not share delete, so zzz.exe cannot be replaced
        let running = std::fs::OpenOptions::new().read(true).share_mode(1).open(game.join("zzz.exe")).unwrap();
        let release = release_with(json!({"a.bin": sum(b"NEW-A"), "zzz.exe": sum(b"NEW-Z")}));
        let zip = game_zip(&[("game/a.bin", b"NEW-A"), ("game/zzz.exe", b"NEW-Z"), ("game/release.json", &inner(&release))]);
        assert_eq!(install_game(&release, std::io::Cursor::new(zip), &game).unwrap_err(), "Close the game, then press Update again.");
        assert_eq!(std::fs::read(game.join("a.bin")).unwrap(), b"old-a", "a.bin moved before the failure and must be restored");
        assert_eq!(std::fs::read(game.join("zzz.exe")).unwrap(), b"old-z");
        assert_eq!(std::fs::read(game.join("release.json")).unwrap(), br#"{"build":1}"#, "the old release.json stays, so the update is offered again");
        assert_eq!(installed_build(&game), Some(1));
        assert!(!game.with_extension("new").exists() && !game.with_extension("bak").exists(), "no staging or backup litter");
        drop(running);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_new_launcher_waits_for_the_old_one_then_cleans_up() {
        assert_eq!(relaunched(&relaunch_args(42, true)), (Some(42), true));
        assert_eq!(relaunched(&relaunch_args(42, false)), (Some(42), false));
        assert_eq!(relaunched(&["Skate 3 Remaster.exe".to_owned()]), (None, false));  // opened by the player
        // the old launcher: still running from its renamed exe for a moment after it starts the new one
        let dir = temp("relaunch");
        let exe = dir.join("Skate 3 Remaster.exe");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::copy(r"C:\Windows\System32\PING.EXE", beside(&exe, "old")).unwrap();
        let mut old = std::process::Command::new(beside(&exe, "old")).args(["-n", "3", "127.0.0.1"])
            .stdout(std::process::Stdio::null()).spawn().unwrap();
        let started = std::time::Instant::now();
        wait_for_exit(old.id());
        assert!(old.try_wait().unwrap().is_some(), "returns once the old launcher has ended");
        assert!(started.elapsed() < Duration::from_secs(8), "not the 10 s limit");
        cleanup_old(&exe);
        assert!(!beside(&exe, "old").exists(), "the old exe is gone once its process is");
        let started = std::time::Instant::now();
        wait_for_exit(old.id());  // already ended: no wait
        assert!(started.elapsed() < Duration::from_secs(1));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_file_held_for_a_moment_is_still_replaced() {
        use std::os::windows::fs::OpenOptionsExt;
        let (dir, sum) = (temp("install-held"), |b: &[u8]| hex(&Sha256::digest(b)));
        let game = dir.join("game");
        std::fs::create_dir_all(game.join("support")).unwrap();
        std::fs::write(game.join("support/skate3setup.exe"), b"old").unwrap();
        // a check that is still shutting down: it lets go of the file 0.6 s into the install
        let held = std::fs::OpenOptions::new().read(true).share_mode(1).open(game.join("support/skate3setup.exe")).unwrap();
        let letting_go = std::thread::spawn(move || { std::thread::sleep(Duration::from_millis(600)); drop(held); });
        let release = release_with(json!({"support/skate3setup.exe": sum(b"NEW")}));
        let zip = game_zip(&[("game/support/skate3setup.exe", b"NEW"), ("game/release.json", &inner(&release))]);
        install_game(&release, std::io::Cursor::new(zip), &game).unwrap();
        letting_go.join().unwrap();
        assert_eq!(std::fs::read(game.join("support/skate3setup.exe")).unwrap(), b"NEW");
        assert_eq!(installed_build(&game), Some(3));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn release_json_moves_last_so_a_finished_install_is_the_only_one_that_counts() {
        let (dir, sum) = (temp("install-order"), |b: &[u8]| hex(&Sha256::digest(b)));
        let game = dir.join("game");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(game.join("release.json"), br#"{"build":1}"#).unwrap();
        let release = release_with(json!({"a.bin": sum(b"A")}));
        let zip = game_zip(&[("game/release.json", &inner(&release)), ("game/a.bin", b"A")]);
        install_game(&release, std::io::Cursor::new(zip), &game).unwrap();
        assert_eq!(installed_build(&game), Some(3));
        assert_eq!(std::fs::read(game.join("a.bin")).unwrap(), b"A");
        assert_eq!(journal(&game)["state"], "installed", "#9: the old files wait for the update's build");
        drop_backup(&game);
        assert!(!game.with_extension("bak").exists(), "and go once it seals");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Audit 10-24: build 2 still waits for its rebuild (its backup "installed") when build 3 comes. What changed was
    /// measured against build 2's files, then the backup put build 1's back: the files 3 shares with 2 stayed at 1 or
    /// missing.
    #[test]
    fn an_update_over_one_still_waiting_for_its_build_installs_every_file() {
        let (dir, sum) = (temp("install-twice"), |b: &[u8]| hex(&Sha256::digest(b)));
        let game = dir.join("game");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(game.join("release.json"), br#"{"build":1}"#).unwrap();
        std::fs::write(game.join("a.bin"), b"A1").unwrap();
        std::fs::write(game.join("b.bin"), b"B1").unwrap();
        let build = |n: u64, a: &[u8]| Release { build: n, meta: json!({"build": n, "files": {"a.bin": sum(a), "b.bin": sum(b"B2"), "c.bin": sum(b"C2")}}),
                                                assets: vec![], notes: vec![] };
        let (two, three) = (build(2, b"A2"), build(3, b"A3"));
        let zip = game_zip(&[("game/a.bin", b"A2"), ("game/b.bin", b"B2"), ("game/c.bin", b"C2"), ("game/release.json", &inner(&two))]);
        install_game(&two, std::io::Cursor::new(zip), &game).unwrap();
        assert_eq!(journal(&game)["state"], "installed", "build 2 waits for its rebuild");
        let zip = game_zip(&[("game/a.bin", b"A3"), ("game/b.bin", b"B2"), ("game/c.bin", b"C2"), ("game/release.json", &inner(&three))]);
        install_game(&three, std::io::Cursor::new(zip), &game).unwrap();
        for (rel, want) in [("a.bin", &b"A3"[..]), ("b.bin", b"B2"), ("c.bin", b"C2")] {
            assert_eq!(std::fs::read(game.join(rel)).ok().as_deref(), Some(want), "{rel}");
        }
        assert_eq!(installed_build(&game), Some(3));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Audit 10-24: a full download that failed keeps its part file to resume from; an update that then went the delta
    /// way left it (up to a whole game.zip) for good.
    #[test]
    fn a_finished_install_leaves_no_download_behind() {
        let (dir, sum) = (temp("install-part"), |b: &[u8]| hex(&Sha256::digest(b)));
        let game = dir.join("game");
        std::fs::create_dir_all(game.with_extension("download")).unwrap();
        std::fs::write(game.with_extension("download").join("game.zip.part"), b"half a zip").unwrap();
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(game.join("release.json"), br#"{"build":1}"#).unwrap();
        let release = release_with(json!({"a.bin": sum(b"A")}));
        let zip = game_zip(&[("game/a.bin", b"A"), ("game/release.json", &inner(&release))]);
        install_game(&release, std::io::Cursor::new(zip), &game).unwrap();
        assert!(!game.with_extension("download").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn install_game_with_a_wrong_or_unlisted_file_changes_nothing() {
        let dir = temp("install-bad");
        let game = dir.join("game");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(game.join("skate3rust.exe"), b"old").unwrap();
        let zip = game_zip(&[("game/skate3rust.exe", b"NEW")]);
        let wrong = release_with(json!({"skate3rust.exe": "00"}));
        assert_eq!(install_game(&wrong, std::io::Cursor::new(&zip), &game).unwrap_err(), "Program checksum mismatch");
        let unlisted = release_with(json!({}));
        assert_eq!(install_game(&unlisted, std::io::Cursor::new(&zip), &game).unwrap_err(), "Program checksum mismatch");
        assert_eq!(std::fs::read(game.join("skate3rust.exe")).unwrap(), b"old");
        assert!(!game.with_extension("new").exists(), "staging must be removed after a refusal too");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn release_text_becomes_its_sections_and_every_line() {
        let body = "## Online\n\n- Graphic Creator\n* Co-host for online sessions\n  \nPlain line\n### Maps\n- Zen\n";
        assert_eq!(notes_from(body), ["# Online", "Graphic Creator", "Co-host for online sessions", "Plain line", "# Maps", "Zen"]);
        assert_eq!(notes_from(&"x\n".repeat(1000)).len(), 400);
        assert!(notes_from("").is_empty());
        let wrapped = "- Crash reports open in their own window,\n  styled like the launcher.\n  - A nested bullet\n- Next\n";
        assert_eq!(notes_from(wrapped), ["Crash reports open in their own window, styled like the launcher.", "A nested bullet", "Next"]);
    }

    #[test]
    fn the_whole_change_log_shows_in_whats_new() {
        // ui/changelog.md is this version's release text and the page's offline notes (main.rs PAGE)
        let log = include_str!("../ui/changelog.md");
        let lines = log.lines().filter(|l| !l.trim().is_empty()).count();
        let notes = notes_from(log);
        assert_eq!(notes.len(), lines, "every line of the change log");
        assert_eq!(notes[0], "# ONLINE");
        assert!(!log.contains("</"), "the log sits inside a <script> block of the page");
        assert!(crate::PAGE.contains(log));
    }

    #[test]
    fn a_launcher_without_a_build_number_never_asks_for_a_self_update() {
        assert_eq!(needs(7, 7, None), (false, false));
        assert_eq!(needs(8, 7, None), (true, false));
        assert_eq!(needs(8, 8, Some(7)), (false, true));
        assert_eq!(needs(8, 0, Some(8)), (true, false));
        assert_eq!(needs(7, 9, Some(9)), (false, false));
    }

    #[test]
    fn credential_round_trip_never_touches_files() {
        let target = &format!("Skate3Remaster/test-only-{}", std::process::id());
        crate::credentials::delete_from(target);
        crate::credentials::save_to(target, "github_pat_test").unwrap();
        assert_eq!(crate::credentials::load_from(target).as_deref(), Some("github_pat_test"));
        crate::credentials::delete_from(target);
        assert!(crate::credentials::load_from(target).is_none());
    }

    /// Serves `body` at any path, honouring `Range: bytes=a-b` / `bytes=a-` with a 206 (unless `ranges` is false).
    /// Hands back how many body bytes it sent.
    fn range_server(body: Vec<u8>, ranges: bool) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        let (api, sent, _) = range_server_counting(body, ranges);
        (api, sent)
    }
    /// range_server that also counts requests.
    fn range_server_counting(body: Vec<u8>, ranges: bool) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let api = format!("http://{}/repos/daytryan/S3R/releases", listener.local_addr().unwrap());
        let sent = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (count, hits) = (sent.clone(), std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)));
        let seen = hits.clone();
        std::thread::spawn(move || for stream in listener.incoming() {
            seen.fetch_add(1, Ordering::SeqCst);
            let mut s = stream.unwrap(); let mut buf = [0u8; 4096]; let n = s.read(&mut buf).unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).to_lowercase();
            let range = request.lines().find_map(|l| l.strip_prefix("range: bytes=")).filter(|_| ranges).map(|r| {
                let (a, b) = r.trim().split_once('-').unwrap();
                (a.parse::<usize>().unwrap(), b.parse::<usize>().map_or(body.len() - 1, |b| b.min(body.len() - 1)))
            });
            let (code, part) = match range { Some((a, b)) => (206, &body[a..=b]), None => (200, &body[..]) };
            count.fetch_add(part.len(), Ordering::SeqCst);
            let _ = write!(s, "HTTP/1.1 {code} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", part.len());
            let _ = s.write_all(part);
        });
        (api, sent, hits)
    }

    fn served_release(api: &str, files: Value, zip: &[u8]) -> Release {
        Release { build: 3, meta: json!({"build": 3, "files": files, "game": {"name": "game.zip", "sha256": hex(&Sha256::digest(zip))}}),
                  assets: vec![json!({"name": "game.zip", "url": format!("{api}/assets/1"), "size": zip.len()})], notes: vec![] }
    }

    #[test]
    fn a_download_streams_to_disk_resumes_and_reports_progress() {
        let zip: Vec<u8> = (0..300_000u32).map(|i| (i * 7 % 251) as u8).collect();
        let dir = temp("stream");
        let part = dir.join("game.zip.part");
        for ranges in [true, false] {
            let (api, sent) = range_server(zip.clone(), ranges);
            let release = served_release(&api, json!({}), &zip);
            std::fs::write(&part, &zip[..100_000]).unwrap();  // an earlier try stopped a third of the way
            let mut events = Vec::new();
            download_to(&api, &release, "game", &part, &mut Progress::new(zip.len() as u64, |m| events.push(m))).unwrap();
            assert_eq!(std::fs::read(&part).unwrap(), zip);
            let fetched = sent.load(Ordering::SeqCst);
            if ranges { assert_eq!(fetched, 200_000, "only the missing part"); } else { assert_eq!(fetched, zip.len(), "no Range: start over"); }
            let last = events.last().unwrap();
            assert_eq!((last["type"].as_str(), last["done"].as_u64(), last["total"].as_u64()), (Some("download"), Some(300_000), Some(300_000)));
            assert!(events.len() <= 4, "at most 4 a second: {}", events.len());
        }
        // D5: a part of another upload of the asset starts over instead of mixing two files
        let (api, sent) = range_server(zip.clone(), true);
        std::fs::write(&part, &zip[..100_000]).unwrap();
        std::fs::write(part.with_extension("asset"), "41 \"an older upload\"").unwrap();
        download_to(&api, &served_release(&api, json!({}), &zip), "game", &part, &mut Progress::new(1, |_| {})).unwrap();
        assert_eq!(sent.load(Ordering::SeqCst), zip.len());
        std::fs::write(&part, b"garbage that is not the zip").unwrap();
        let (api, _) = range_server(zip.clone(), true);
        let mut bad = served_release(&api, json!({}), &zip);
        bad.meta["game"]["sha256"] = json!("00");
        assert_eq!(download_to(&api, &bad, "game", &part, &mut Progress::new(1, |_| {})).unwrap_err(), "Download checksum mismatch");
        assert!(!part.exists(), "a bad download is not resumed from");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_update_fetches_only_the_files_that_changed() {
        let sum = |b: &[u8]| hex(&Sha256::digest(b));
        let big: Vec<u8> = (0..3_000_000u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();  // does not compress
        let files = json!({"skate3rust.exe": sum(b"NEW EXE"), "support/skate3setup/big.bin": sum(&big)});
        let meta = json!({"build": 3, "files": files}).to_string();
        let zip = game_zip(&[("game/skate3rust.exe", b"NEW EXE"), ("game/support/skate3setup/big.bin", &big), ("game/release.json", meta.as_bytes())]);
        let (api, sent) = range_server(zip.clone(), true);
        let release = served_release(&api, files, &zip);
        let game = temp("delta").join("game");
        std::fs::create_dir_all(game.join("support/skate3setup")).unwrap();
        std::fs::write(game.join("support/skate3setup/big.bin"), &big).unwrap();  // unchanged since the last build
        std::fs::write(game.join("skate3rust.exe"), b"OLD EXE").unwrap();
        std::fs::write(game.join("release.json"), r#"{"build":2}"#).unwrap();
        assert_eq!(changed_files(&release, &game), HashSet::from(["skate3rust.exe".to_owned()]));
        install_game(&release, Remote::new(&api, &release, "game").unwrap(), &game).unwrap();
        assert_eq!(std::fs::read(game.join("skate3rust.exe")).unwrap(), b"NEW EXE");
        assert_eq!(installed_build(&game), Some(3));
        assert!(sent.load(Ordering::SeqCst) < 2 << 20, "the 3 MB file was not fetched: {}", sent.load(Ordering::SeqCst));
        assert!(!backup_dir(&game).join("support/skate3setup/big.bin").exists(), "an unchanged file is not even backed up");
        std::fs::remove_dir_all(game.parent().unwrap()).unwrap();
    }

    /// F1: zip reads every entry's local header when it opens the archive; a delta of 1 file in 30 must not pay a
    /// full chunk per header.
    #[test]
    fn a_delta_of_one_file_in_thirty_fetches_little() {
        let sum = |b: &[u8]| hex(&Sha256::digest(b));
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;  // xorshift: bytes that do not compress
        let bodies: Vec<Vec<u8>> = (0..30).map(|_| (0..100_000).map(|_| { seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17; (seed >> 32) as u8 }).collect()).collect();
        let names: Vec<String> = (0..30).map(|n| format!("f{n:02}.bin")).collect();
        let mut files = serde_json::Map::new();
        for (n, b) in names.iter().zip(&bodies) { files.insert(n.clone(), json!(sum(b))); }
        files.insert("f07.bin".into(), json!(sum(b"NEW")));
        let files = Value::Object(files);
        let meta = json!({"build": 3, "files": files}).to_string();
        let mut entries: Vec<(String, Vec<u8>)> = names.iter().cloned().zip(bodies.iter().cloned()).collect();
        entries[7].1 = b"NEW".to_vec();
        entries.push(("release.json".into(), meta.into_bytes()));
        let named: Vec<(String, &[u8])> = entries.iter().map(|(n, b)| (format!("game/{n}"), b.as_slice())).collect();
        let zip = game_zip(&named.iter().map(|(n, b)| (n.as_str(), *b)).collect::<Vec<_>>());
        let (api, sent, hits) = range_server_counting(zip.clone(), true);
        let release = served_release(&api, files, &zip);
        let game = temp("delta30").join("game");
        std::fs::create_dir_all(&game).unwrap();
        for (n, b) in names.iter().zip(&bodies) { std::fs::write(game.join(n), b).unwrap(); }
        std::fs::write(game.join("release.json"), r#"{"build":2}"#).unwrap();
        let mut wanted = changed_files(&release, &game);
        assert_eq!(wanted, HashSet::from(["f07.bin".to_owned()]));
        wanted.insert("release.json".into());
        let mut remote = Remote::new(&api, &release, "game").unwrap();
        let need = entry_bytes(&mut remote, "game/", &wanted).unwrap();
        remote.seek(SeekFrom::Start(0)).unwrap();
        install_game(&release, remote, &game).unwrap();
        assert_eq!(std::fs::read(game.join("f07.bin")).unwrap(), b"NEW");
        let (bytes, requests) = (sent.load(Ordering::SeqCst), hits.load(Ordering::SeqCst));
        eprintln!("delta 1 of 30 (zip {} bytes): {bytes} bytes in {requests} requests, need {need}", zip.len());
        // before F1: 45 MB in 85 requests (a 1 MB chunk per header, twice) for this 2.9 MB zip
        assert!(bytes < 1 << 20 && requests < 40, "{bytes} bytes, {requests} requests");
        std::fs::remove_dir_all(game.parent().unwrap()).unwrap();
    }

    #[test]
    fn an_update_that_moves_the_setup_tool_into_a_folder_removes_the_old_exe_and_can_be_undone() {
        let sum = |b: &[u8]| hex(&Sha256::digest(b));
        let game = temp("onedir").join("game");
        std::fs::create_dir_all(game.join("support")).unwrap();
        std::fs::create_dir_all(game.join("mods")).unwrap();
        std::fs::write(game.join("support/skate3setup.exe"), b"ONEFILE").unwrap();
        std::fs::write(game.join("mods/mine.zip"), b"player's mod").unwrap();
        std::fs::write(game.join("release.json"), json!({"build": 2, "files": {"support/skate3setup.exe": sum(b"ONEFILE")}}).to_string()).unwrap();
        let release = release_with(json!({"support/skate3setup/skate3setup.exe": sum(b"ONEDIR"), "support/skate3setup/_internal/a.dll": sum(b"DLL")}));
        let zip = game_zip(&[("game/support/skate3setup/skate3setup.exe", b"ONEDIR"), ("game/support/skate3setup/_internal/a.dll", b"DLL"),
                             ("game/release.json", &inner(&release))]);
        install_game(&release, std::io::Cursor::new(zip), &game).unwrap();
        assert!(!game.join("support/skate3setup.exe").exists(), "the old single exe is gone");
        assert_eq!(crate::pipeline::setup_exe(&game), game.join("support/skate3setup/skate3setup.exe"));
        assert!(game.join("mods/mine.zip").is_file(), "mods are never touched");
        // #9: the build after it failed: the old build comes back whole
        assert!(restore_backup(&game));
        assert_eq!(std::fs::read(game.join("support/skate3setup.exe")).unwrap(), b"ONEFILE");
        assert!(!game.join("support/skate3setup/skate3setup.exe").exists() && !game.join("support/skate3setup/_internal/a.dll").exists());
        assert_eq!(installed_build(&game), Some(2));
        assert_eq!(crate::pipeline::setup_exe(&game), game.join("support/skate3setup.exe"));
        assert!(!backup_dir(&game).exists());
        std::fs::remove_dir_all(game.parent().unwrap()).unwrap();
    }

    #[test]
    fn an_install_cut_off_half_way_is_undone_at_the_next_start_and_a_finished_one_is_kept() {
        let game = temp("interrupted").join("game");
        std::fs::create_dir_all(game.join("support")).unwrap();
        // the power went off after a.bin was replaced and new.bin added, before release.json moved
        std::fs::write(game.join("a.bin"), b"NEW-A").unwrap();
        std::fs::write(game.join("new.bin"), b"NEW").unwrap();
        std::fs::write(game.join("release.json"), r#"{"build":1}"#).unwrap();
        std::fs::create_dir_all(backup_dir(&game)).unwrap();
        std::fs::write(backup_dir(&game).join("a.bin"), b"old-a").unwrap();
        std::fs::write(backup_dir(&game).join("half.bin.part"), b"a copy cut off").unwrap();
        write_journal(&game, "installing", &["new.bin".to_owned()]).unwrap();
        assert!(recover_interrupted(&game));
        assert_eq!(std::fs::read(game.join("a.bin")).unwrap(), b"old-a");
        assert!(!game.join("new.bin").exists() && !game.join("half.bin").exists() && !game.join("half.bin.part").exists());
        assert!(!backup_dir(&game).exists());
        // an older launcher's backup has no journal: it was always an interrupted one (#16)
        std::fs::create_dir_all(backup_dir(&game)).unwrap();
        std::fs::write(backup_dir(&game).join("a.bin"), b"older-a").unwrap();
        assert!(recover_interrupted(&game));
        assert_eq!(std::fs::read(game.join("a.bin")).unwrap(), b"older-a");
        // a finished install waiting for its build is not touched at start, and drop_backup leaves an unfinished one alone
        write_journal(&game, "installed", &[]).unwrap();
        assert!(!recover_interrupted(&game) && backup_dir(&game).exists());
        write_journal(&game, "installing", &[]).unwrap();
        drop_backup(&game);
        assert!(backup_dir(&game).exists());
        std::fs::remove_dir_all(game.parent().unwrap()).unwrap();
    }

    #[test]
    fn every_release_newer_than_the_installed_one_comes_with_its_notes() {
        let routes = Routes::default();
        let api = serve(routes.clone());
        let meta = |b: u64| json!({"schema": 1, "build": b, "repository": REPO}).to_string();
        let list = json!([9, 8, 7, 6].map(|b| json!({"draft": false, "prerelease": false, "body": format!("- build {b}"),
            "assets": [{"name": "release.json", "url": format!("{api}/assets/{b}")}]}))).to_string();
        route(&routes, "releases?per_page=30", 200, list);
        for b in [9, 8, 7, 6] { route(&routes, &format!("assets/{b}"), 200, meta(b)); }
        let got = releases(&api, 7).unwrap();
        assert_eq!(got.iter().map(|r| (r.build, r.notes[0].as_str())).collect::<Vec<_>>(), [(9, "build 9"), (8, "build 8")]);
        assert_eq!(releases(&api, 9).unwrap().len(), 1, "up to date: just the newest");
        assert_eq!(latest(&api).unwrap().unwrap().build, 9);
    }

    #[test]
    fn only_an_install_blocks_builds_and_an_install_never_starts_under_a_build() {
        let game = temp("busy").join("game");
        let app = crate::app::test_app(&game);
        UPDATING.store(true, Ordering::SeqCst);  // a startup, 4-hour or probe check
        assert!(!app.refuse_while_busy(), "an update check is not busy");
        UPDATING.store(false, Ordering::SeqCst);
        *app.running.lock().unwrap() = Some(1);
        assert_eq!(begin_install(&app).unwrap_err(), "Wait for the build to finish, then update.");
        assert!(!installing());
        *app.running.lock().unwrap() = None;
        begin_install(&app).unwrap();
        assert!(installing() && app.refuse_while_busy(), "an install is");
        INSTALLING.store(false, Ordering::SeqCst);
    }

    #[test]
    fn a_repair_takes_only_the_installed_build() {
        let r = |b: u64| Release { build: b, meta: json!({"build": b}), assets: vec![], notes: vec![] };
        let list = [r(9), r(7)];
        assert_eq!(repair_release(&list, 7).unwrap().build, 7);
        assert_eq!(repair_release(&list, 8).map(|r| r.build).unwrap_err(), "This version is no longer available to repair. Update instead.");
    }
}
