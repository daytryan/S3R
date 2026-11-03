//! Appearance identities reference prepared retail assets; only imported GLBs travel online.
use super::{
    Multiplayer,
    appearance_transfer::{Exchange, MAX_BLOB},
};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::OnceLock,
};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum Look {
    Stock,
    Outfit(Value),
    Native(String),
    Imported(String),
}
#[derive(Component)]
pub(crate) struct RemoteCharacter(pub u64);

/// Party Play's waiting skaters: drawn by the remote renderer's spawn / bind (their looks loaded up front, hidden) but
/// local only, never on the network, the roster or the Player List; party_play owns them. Keyed by [`puppet_id`].
#[derive(Resource, Default)]
pub(crate) struct LocalPuppets(pub BTreeMap<u64, Puppet>);
pub(crate) struct Puppet {
    pub look: Look,
    /// Where it stands (TurnBasedWaitingLocation's slot, in world space); None: hidden (the pad's player).
    pub at: Option<Transform>,
}
/// A puppet's root (in place of [`RemoteCharacter`]).
#[derive(Component)]
pub(crate) struct PuppetCharacter;
/// Party Play player `p`'s puppet id: from the top of the id space, away from network actor ids.
pub(crate) fn puppet_id(p: usize) -> u64 {
    u64::MAX - p as u64
}
#[derive(Resource, Default)]
pub(super) struct Appearances {
    exchange: Exchange,
    connection: Option<(u64, u64)>,
    local: String,
    pub looks: BTreeMap<u64, ([u8; 32], Look)>,
    seen: BTreeMap<u64, [u8; 32]>,
    cached: BTreeMap<[u8; 32], Look>,
    costs: BTreeMap<[u8; 32], Cost>,
    /// The received import being checked off the main thread: (peer, hash, result).
    job: Option<(u64, [u8; 32], std::thread::JoinHandle<Result<Cost, String>>)>,
    pub status: String,
    pub progress: String,
}
pub(super) fn cleanup(mut exit: MessageReader<AppExit>) {
    // Generated process-private directory, never the personal import library.
    if exit.read().next().is_some() {
        let _ = std::fs::remove_dir_all(cache_directory());
    }
}
pub(crate) fn cache_directory() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| std::env::temp_dir().join(format!("skate-online-{:016x}", super::unique())))
}
pub(super) fn sync(
    mut state: ResMut<Appearances>,
    mut net: ResMut<Multiplayer>,
    models: Res<crate::custom_models::CustomModels>,
    parts: Res<crate::customiser_parts::Parts>,
    mut creator: Option<ResMut<crate::graphic_creator::Creator>>,
    mut team: Option<ResMut<crate::team_select::TeamUi>>,
    cas: Option<Res<crate::customiser::Customiser>>,
    mut cas_closed: Local<Option<std::time::Instant>>,
    // perf 10-18: our last published look (key, bytes, digest); outlives `*state = ..` so a new session
    // neither re-reads and re-validates an import (up to 64 MiB) nor re-hashes it.
    mut own: Local<Option<(String, std::sync::Arc<Vec<u8>>, [u8; 32])>>,
) {
    let Some(lobby) = net.lobby.as_mut() else {
        if state.connection.is_some() {
            let _ = std::fs::remove_dir_all(cache_directory());
            *state = default();
        }
        return;
    };
    let connection = (lobby.session, lobby.local);
    if state.connection != Some(connection) {
        *state = Appearances {
            connection: Some(connection),
            ..default()
        };
    }
    let selection = models.online_selection();
    let signature = models
        .active
        .as_ref()
        .map(|id| format!("model:{id}"))
        .unwrap_or_else(|| format!("outfit:{}", parts.applied));
    // Only the committed look is sent: not the Graphic Creator's edit steps (each a new design of up to
    // 25 KB for every peer to rebuild), nor the step the skater has not caught up with after it closes.
    // Nor a teammate's look while Edit Skaters has one in the CAS (decision L2: only the boss is published).
    // Nor Edit Skater's browsing (user 10-16, online CAS): the preview is worn at every step, the tattoo pages strip a
    // garment. Its look goes out once, when it closes, after a second for the skater to put the saved outfit on.
    // ponytail: a fixed settle; a Parts "ready" flag if a slow part load ever publishes a half-built look.
    let editing = cas.is_some_and(|c| c.open);
    if editing {
        *cas_closed = Some(std::time::Instant::now());
    }
    let settling = cas_closed.is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(1));
    let held = creator.as_mut().is_some_and(|c| c.holds_publish(&parts.applied))
        | team.as_mut().is_some_and(|t| t.holds_publish(&parts.applied))
        | editing
        | settling;
    let mut key = String::new();
    if state.local != signature && !held {
        // An import is the same file only at the same length and modification time.
        let stamp = selection
            .as_ref()
            .filter(|(native, _)| native.is_none())
            .and_then(|(_, path)| std::fs::metadata(path).ok())
            .map(|m| (m.len(), m.modified().ok()));
        key = format!("{signature}|{stamp:?}");
        if skate_net::knob!("SKATE_NET_LOOK_CACHE", 1) != 0 {
            if let Some((_, bytes, hash)) = (*own).as_ref().filter(|(k, ..)| *k == key) {
                info!("ONLINE_CHARACTER_PUBLISH bytes={} selection={signature} cached", bytes.len());
                state.exchange.publish_shared(bytes.clone(), *hash);
                state.status.clear();
                state.local = signature.clone();
            }
        }
    }
    if state.local != signature && !held {
        let payload = (|| -> Result<Vec<u8>, String> {
            if let Some((native, path)) = selection {
                if let Some(key) = native {
                    return serde_json::to_vec(&Look::Native(key)).map_err(|e| e.to_string());
                }
                if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() as usize >= MAX_BLOB {
                    return Err("Imported character exceeds the 64 MiB online limit".into());
                }
                let glb = std::fs::read(path).map_err(|e| e.to_string())?;
                validate_glb(&glb)?;
                let mut bytes = vec![0];
                bytes.extend(glb);
                Ok(bytes)
            } else {
                if parts.applied["selections"]
                    .as_object()
                    .is_some_and(|s| !s.is_empty())
                {
                    outfit_bytes(&parts.applied)
                } else {
                    serde_json::to_vec(&Look::Stock).map_err(|e| e.to_string())
                }
            }
        })();
        match payload {
            Ok(bytes) => {
                info!(
                    "ONLINE_CHARACTER_PUBLISH bytes={} selection={signature}",
                    bytes.len()
                );
                let hash = skate_net::blob::digest(&bytes);
                let bytes = std::sync::Arc::new(bytes);
                state.exchange.publish_shared(bytes.clone(), hash);
                *own = Some((key, bytes, hash));
                state.status.clear();
            }
            Err(e) => {
                state.status = e;
                state
                    .exchange
                    .publish(serde_json::to_vec(&Look::Stock).unwrap());
            }
        }
        state.local = signature;
    }
    let now = net.started.elapsed().as_millis() as u64;
    let lobby = net.lobby.as_mut().unwrap();
    state.exchange.tick(lobby, now);
    state.progress = state.exchange.progress();
    state.looks.retain(|id, _| lobby.actors.contains_key(id));
    state.seen.retain(|id, _| lobby.actors.contains_key(id));
    // A received import is checked and written on a worker, one at a time (an 8 MiB JSON parse and a 64 MiB
    // write were a frame stall per peer, and 49 parallel parses a few GiB).
    if let Some((id, hash, job)) = state.job.take() {
        match job.is_finished() {
            false => state.job = Some((id, hash, job)),
            true => {
                let result = job
                    .join()
                    .unwrap_or_else(|_| Err("Character check failed".into()))
                    .map(|cost| {
                        state.costs.insert(hash, cost);
                        Look::Imported(format!("online-characters://{}", cache_name(&hash)))
                    });
                accept(&mut state, id, hash, result);
            }
        }
    }
    let updates: Vec<_> = state
        .exchange
        .ready
        .iter()
        .filter(|(id, (key, _))| state.seen.get(id) != Some(&key.hash))
        .map(|(&id, (key, _))| (id, key.hash))
        .collect();
    for (id, hash) in updates {
        if state.job.is_some() && state.exchange.ready[&id].1.first() == Some(&0) {
            continue;
        }
        let mut bytes = std::mem::take(&mut state.exchange.ready.get_mut(&id).unwrap().1);
        state.seen.insert(id, hash);
        // An import waits as its first byte (appearance_transfer, online safety 10-26): its bytes, now its check starts.
        if bytes == [0] {
            match lobby.blobs.ready(id) {
                Some((blob, data)) if blob.hash == hash => bytes = data.to_vec(),
                _ => {
                    accept(&mut state, id, hash, Err("Character data gone".into()));
                    continue;
                }
            }
        }
        let result = if bytes.is_empty() {
            state
                .cached
                .get(&hash)
                .cloned()
                .ok_or("Cached appearance missing".into())
        } else if bytes[0] == 0 {
            match make_room(&mut state, bytes.len() as u64) {
                Ok(()) => {
                    // ponytail: a job outliving its lobby may leave one file behind; the directory goes on exit.
                    let job = std::thread::spawn(move || check_and_store(&bytes[1..], &hash));
                    state.job = Some((id, hash, job));
                    continue;
                }
                Err(e) => Err(e),
            }
        } else {
            read_look(&bytes)
        };
        accept(&mut state, id, hash, result);
    }
}
fn cache_name(hash: &[u8; 32]) -> String {
    format!("{}.glb", blake3::Hash::from_bytes(*hash).to_hex())
}
/// Validates a received import, then stores it under its verified hash (never a peer-chosen name).
fn check_and_store(glb: &[u8], hash: &[u8; 32]) -> Result<Cost, String> {
    let cost = validate_glb(glb)?;
    std::fs::create_dir_all(cache_directory()).map_err(|e| e.to_string())?;
    let path = cache_directory().join(cache_name(hash));
    if !path.exists() {
        let pending = path.with_extension("tmp");
        std::fs::write(&pending, glb).map_err(|e| e.to_string())?;
        std::fs::rename(pending, path).map_err(|e| e.to_string())?;
    }
    Ok(cost)
}
/// Keeps the cache within [`CACHE_BYTES`]: files no player wears now go first; past that the model is refused.
fn make_room(state: &mut Appearances, size: u64) -> Result<(), String> {
    let files: Vec<_> = std::fs::read_dir(cache_directory())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| Some((e.path(), e.metadata().ok()?.len())))
        .collect();
    let mut used: u64 = files.iter().map(|f| f.1).sum();
    let worn: BTreeSet<String> = state.looks.values().map(|(h, _)| cache_name(h)).collect();
    for (path, len) in files {
        if used + size <= CACHE_BYTES {
            break;
        }
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        if worn.contains(&name) || std::fs::remove_file(&path).is_err() {
            continue;
        }
        used -= len;
        if let Ok(hash) = blake3::Hash::from_hex(name.trim_end_matches(".glb")) {
            state.cached.remove(hash.as_bytes());
            state.costs.remove(hash.as_bytes());
            state.exchange.forget(hash.as_bytes());
        }
    }
    if used + size > CACHE_BYTES {
        return Err("Online character cache is full".into());
    }
    Ok(())
}
fn accept(state: &mut Appearances, id: u64, hash: [u8; 32], result: Result<Look, String>) {
    let mut look = match result {
        Ok(look) => look,
        Err(e) => return warn!("Remote appearance {id}: {e}"),
    };
    let identity = state.exchange.ready.get(&id).map(|(i, _)| i.clone());
    if let Some(identity) = identity.filter(|i| i.hash == hash) {
        state.exchange.remember(identity);
    }
    state.cached.insert(hash, look.clone());
    if matches!(look, Look::Imported(_)) {
        // Lobby budget: what other peers' imports already hold decoded (RAM and GPU).
        // ponytail: one over budget stays stock until it changes look; re-admit when space frees if asked for.
        let cost = state.costs.get(&hash).copied().unwrap_or_default();
        let used = state
            .looks
            .iter()
            .filter(|(peer, (_, l))| **peer != id && matches!(l, Look::Imported(_)))
            .filter_map(|(_, (h, _))| state.costs.get(h))
            .fold(cost, |a, c| Cost {
                pixels: a.pixels + c.pixels,
                elements: a.elements + c.elements,
            });
        if used.pixels > LOBBY_PIXELS || used.elements > LOBBY_ELEMENTS {
            warn!("Remote appearance {id}: the lobby's custom character memory is full; drawn stock");
            look = Look::Stock;
        }
    }
    info!(
        "ONLINE_CHARACTER_READY peer={id} kind={} hash={}",
        if matches!(look, Look::Imported(_)) {
            "import"
        } else {
            "retail"
        },
        blake3::Hash::from_bytes(hash)
    );
    state.looks.insert(id, (hash, look));
    // Outfit/native looks no peer wears or is delivering go (a peer re-dressing all session grew this ~100 KB a
    // look); forgotten, so a return brings its bytes again. Imports leave with their cache file (make_room).
    let keep: BTreeSet<[u8; 32]> = state.looks.values().map(|(h, _)| *h).chain(state.exchange.ready.values().map(|(i, _)| i.hash)).collect();
    let gone: Vec<_> = state.cached.iter().filter(|(h, l)| !keep.contains(*h) && !matches!(l, Look::Imported(_))).map(|(h, _)| *h).collect();
    for h in gone {
        state.cached.remove(&h);
        state.exchange.forget(&h);
    }
}
/// The most JSON a peer accepts for an outfit. A full profile (six sides of 64 layers) is about 25 KB.
pub(crate) const MAX_OUTFIT: usize = 32768;
/// An outfit as published. If its creator designs would push it past what peers accept it goes
/// without them (every peer would drop the whole outfit otherwise).
pub(super) fn outfit_bytes(applied: &Value) -> Result<Vec<u8>, String> {
    let encode = |profile: &Value| serde_json::to_vec(&Look::Outfit(profile.clone())).map_err(|e| e.to_string());
    let bytes = encode(applied)?;
    if bytes.len() <= MAX_OUTFIT {
        return Ok(bytes);
    }
    let mut plain = applied.clone();
    crate::custom_graphic::without_designs(&mut plain);
    let bytes = encode(&plain)?;
    if bytes.len() > MAX_OUTFIT {
        return Err("This outfit is too large to share online".into());
    }
    Ok(bytes)
}
/// A peer's look from its outfit or native bytes (not an import: those start with a zero byte).
pub(crate) fn read_look(bytes: &[u8]) -> Result<Look, String> {
    if bytes.len() > MAX_OUTFIT {
        return Err("Oversized outfit".into());
    }
    let look: Look = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    match &look {
        Look::Imported(_) => return Err("Peer supplied an asset path".into()),
        Look::Native(key) if key.len() > 96 => {
            return Err("Invalid native identity".into());
        }
        _ => (),
    }
    Ok(look)
}
/// Texture pixels one model may decode (256 MiB as RGBA8).
const MAX_PIXELS: u64 = 64 * 1024 * 1024;
/// Accessor elements one model may decode, counted per use (at most 16 bytes each: 256 MiB).
const MAX_ELEMENTS: u64 = 16 * 1024 * 1024;
/// What peers' imported models may hold decoded at once, all players together: 512 MiB of RGBA8 textures and at
/// most 512 MiB of geometry, each kept in RAM and on the GPU (bevy_gltf's default asset usage). Was 49 x 64M pixels.
/// Past it a peer is drawn stock.
const LOBBY_PIXELS: u64 = 128 * 1024 * 1024;
const LOBBY_ELEMENTS: u64 = 32 * 1024 * 1024;
/// Disk the received-model cache may use (the blob layer's RAM cache is 768 MiB too).
const CACHE_BYTES: u64 = 768 * 1024 * 1024;
/// What a validated model costs once bevy_gltf decodes it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Cost {
    pub pixels: u64,
    pub elements: u64,
}
/// Online imports are self-contained data. Never let peer JSON load a URI or path.
/// Everything bevy_gltf would allocate per texture, primitive and channel is bounded here, before it sees the bytes.
pub(super) fn validate_glb(bytes: &[u8]) -> Result<Cost, String> {
    let bad = || "Invalid or unsupported online character GLB".to_owned();
    let word = |offset: usize| {
        bytes
            .get(offset..offset + 4)
            .and_then(|b| b.try_into().ok())
            .map(u32::from_le_bytes)
    };
    if bytes.len() < 28
        || bytes.len() >= MAX_BLOB
        || &bytes[..4] != b"glTF"
        || word(4) != Some(2)
        || word(8) != Some(bytes.len() as u32)
        || word(16) != Some(0x4e4f534a)
    {
        return Err(bad());
    }
    let length = word(12).ok_or_else(bad)? as usize;
    if length > 8 * 1024 * 1024 {
        return Err(bad());
    }
    let json: Value =
        serde_json::from_slice(bytes.get(20..20 + length).ok_or_else(bad)?).map_err(|_| bad())?;
    fn external(v: &Value) -> bool {
        match v {
            Value::Object(m) => {
                m.contains_key("uri")
                    || m.iter().any(|(k, v)| {
                        k == "extensionsRequired" && v.as_array().is_some_and(|a| !a.is_empty())
                            || external(v)
                    })
            }
            Value::Array(a) => a.iter().any(external),
            _ => false,
        }
    }
    if external(&json) {
        return Err("Online GLBs must embed all buffers and textures and use standard glTF".into());
    }
    let binary = 20 + length;
    if word(binary + 4) != Some(0x004e4942)
        || word(binary).is_none_or(|n| binary + 8 + n as usize != bytes.len())
    {
        return Err(bad());
    }
    let bin = &bytes[binary + 8..];
    if json["buffers"].as_array().is_none_or(|a| a.len() != 1)
        || json["buffers"][0]["byteLength"]
            .as_u64()
            .is_none_or(|n| n > bin.len() as u64)
    {
        return Err(bad());
    }
    // Cameras and KHR_lights_punctual would spawn into the receiver's world (bevy_gltf loads them by default):
    // an active camera takes over their view, a light floods their map.
    if json["extensions"].get("KHR_lights_punctual").is_some() {
        return Err(bad());
    }
    for (key, max) in [
        ("nodes", 4096),
        ("meshes", 256),
        ("skins", 128),
        ("images", 128),
        ("materials", 256),
        ("accessors", 8192),
        ("bufferViews", 8192),
        // bevy_gltf decodes per texture, not per image: 10k textures of one 8k image was 10k decodes.
        ("textures", 128),
        ("samplers", 128),
        // Every scene is instantiated, not only the one shown.
        ("scenes", 4),
        ("animations", 64),
        ("cameras", 0),
    ] {
        if json.get(key).is_some_and(|v| v.as_array().is_none_or(|a| a.len() > max)) {
            return Err(bad());
        }
    }
    let list = |key: &str| json[key].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let (views, accessors) = (list("bufferViews"), list("accessors"));
    // A present field that is not a non-negative integer is rejected, not defaulted.
    let num = |v: &Value, key: &str, default: u64| match v.get(key) {
        None => Some(default),
        Some(n) => n.as_u64(),
    };
    for v in views {
        let offset = num(v, "byteOffset", 0).ok_or_else(bad)?;
        let size = v["byteLength"].as_u64().ok_or_else(bad)?;
        if v["buffer"].as_u64() != Some(0) || offset.saturating_add(size) > bin.len() as u64 {
            return Err(bad());
        }
    }
    // Every accessor's data must lie inside its view: gltf's reader panics on a stride shorter than an element.
    // ponytail: sparse accessors are refused (tools/mixamo_to_skate never writes them); bound them if one is needed.
    let mut counts = Vec::with_capacity(accessors.len());
    for a in accessors {
        let count = a["count"].as_u64().filter(|n| (1..=2_000_000).contains(n)).ok_or_else(bad)?;
        let component = match a["componentType"].as_u64() {
            Some(5120 | 5121) => 1,
            Some(5122 | 5123) => 2,
            Some(5125 | 5126) => 4,
            _ => return Err(bad()),
        };
        let width = match a["type"].as_str() {
            Some("SCALAR") => 1,
            Some("VEC2") => 2,
            Some("VEC3") => 3,
            Some("VEC4" | "MAT2") => 4,
            Some("MAT3") => 9,
            Some("MAT4") => 16,
            _ => return Err(bad()),
        };
        // A view-less accessor is all zeros (a POSITION without data makes compute_flat_normals panic).
        if a.get("sparse").is_some() {
            return Err(bad());
        }
        {
            let view = a["bufferView"].as_u64().and_then(|i| views.get(i as usize)).ok_or_else(bad)?;
            let element = component * width;
            let stride = num(view, "byteStride", 0).ok_or_else(bad)?;
            let stride = if stride == 0 { element } else { stride };
            let end = num(a, "byteOffset", 0)
                .ok_or_else(bad)?
                .saturating_add(stride.saturating_mul(count - 1))
                .saturating_add(element);
            if stride < element || end > view["byteLength"].as_u64().ok_or_else(bad)? {
                return Err(bad());
            }
        }
        counts.push(count);
    }
    let accessor = |v: &Value| -> Result<(usize, &Value), String> {
        let i = v.as_u64().filter(|&i| (i as usize) < accessors.len()).ok_or_else(bad)? as usize;
        Ok((i, &accessors[i]))
    };
    let is = |a: &Value, component: u64, kind: &str| a["componentType"].as_u64() == Some(component) && a["type"] == kind;
    // The largest index an index accessor holds, read once per accessor (bounds checked above).
    let mut largest: BTreeMap<usize, u64> = BTreeMap::new();
    let mut max_index = |i: usize, a: &Value| -> Option<u64> {
        if let Some(&m) = largest.get(&i) {
            return Some(m);
        }
        let size = match a["componentType"].as_u64()? {
            5121 => 1,
            5123 => 2,
            5125 => 4,
            _ => return None,
        };
        let view = views.get(a["bufferView"].as_u64()? as usize)?;
        let start = (num(view, "byteOffset", 0)? + num(a, "byteOffset", 0)?) as usize;
        let stride = match num(view, "byteStride", 0)? as usize {
            0 => size,
            s => s,
        };
        let mut m = 0;
        for n in 0..counts[i] as usize {
            let b = bin.get(start + n * stride..start + n * stride + size)?;
            m = m.max(b.iter().rev().fold(0u64, |v, &x| v << 8 | x as u64));
        }
        largest.insert(i, m);
        Some(m)
    };
    // What the loader allocates: every primitive decodes its accessors on its own, so one 2M-vertex accessor
    // shared by 100k primitives was 100k copies. Counted per use, capped per model and per lobby (Cost).
    let mut elements = 0u64;
    let mut primitives = 0;
    for mesh in list("meshes") {
        if mesh["weights"].as_array().is_some_and(|w| w.len() > 64) {
            return Err(bad());
        }
        for p in mesh["primitives"].as_array().into_iter().flatten() {
            primitives += 1;
            let attributes = p["attributes"].as_object().ok_or_else(bad)?;
            // Triangles only, with float3 positions: compute_flat_normals panics on anything else.
            let (_, position) = accessor(attributes.get("POSITION").ok_or_else(bad)?)?;
            if primitives > 1024 || num(p, "mode", 4) != Some(4) || !is(position, 5126, "VEC3") {
                return Err(bad());
            }
            let vertices = position["count"].as_u64().ok_or_else(bad)?;
            // Attributes of unequal length index past the shorter one when the mesh is packed (render thread).
            let mut uses = 0;
            for v in attributes.values() {
                if accessor(v)?.1["count"].as_u64() != Some(vertices) {
                    return Err(bad());
                }
                uses += 1;
            }
            let targets = p["targets"].as_array().map(Vec::as_slice).unwrap_or(&[]);
            if targets.len() > 64 {
                return Err(bad());
            }
            for t in targets {
                for v in t.as_object().ok_or_else(bad)?.values() {
                    if accessor(v)?.1["count"].as_u64() != Some(vertices) {
                        return Err(bad());
                    }
                    uses += 1;
                }
            }
            let mut drawn = vertices;
            if let Some(v) = p.get("indices") {
                let (i, a) = accessor(v)?;
                drawn = counts[i];
                elements += drawn;
                // Online safety 10-26: the budget before the scan, which reads every index. A model that reuses one big
                // view for 1024 primitives took seconds of CPU before the check after the loop refused it.
                if elements > MAX_ELEMENTS {
                    return Err("Online character geometry exceeds the memory limit".into());
                }
                if a["type"] != "SCALAR" || max_index(i, a).is_none_or(|m| m >= vertices) {
                    return Err(bad());
                }
            }
            let normal = attributes.get("NORMAL").map(accessor).transpose()?;
            if normal.is_none_or(|(_, n)| !is(n, 5126, "VEC3")) {
                // No usable normals: the loader duplicates every vertex per index and makes flat normals per
                // triangle; a count that is not whole triangles leaves the normals short.
                if drawn % 3 != 0 {
                    return Err(bad());
                }
                elements = elements.saturating_add(drawn * (uses + 1));
            } else {
                elements = elements.saturating_add(vertices * uses);
            }
        }
    }
    for skin in list("skins") {
        if skin["joints"].as_array().is_none_or(|j| j.is_empty() || j.len() > 256) {
            return Err(bad());
        }
        if let Some(v) = skin.get("inverseBindMatrices") {
            elements += counts[accessor(v)?.0];
        }
    }
    for animation in list("animations") {
        let samplers = animation["samplers"].as_array().ok_or_else(bad)?;
        for c in animation["channels"].as_array().into_iter().flatten() {
            let s = c["sampler"].as_u64().and_then(|s| samplers.get(s as usize)).ok_or_else(bad)?;
            elements = elements
                .saturating_add(counts[accessor(&s["input"])?.0])
                .saturating_add(counts[accessor(&s["output"])?.0]);
        }
    }
    if elements > MAX_ELEMENTS {
        return Err("Online character geometry exceeds the memory limit".into());
    }
    let images = list("images");
    let mut sizes = Vec::with_capacity(images.len());
    let mut pixels = 0u64;
    for image in images {
        let i = image["bufferView"].as_u64().ok_or_else(bad)? as usize;
        let view = views.get(i).ok_or_else(bad)?;
        let offset = num(view, "byteOffset", 0).ok_or_else(bad)? as usize;
        let size = view["byteLength"].as_u64().ok_or_else(bad)? as usize;
        // Dimensions from the header only; nothing is decoded here.
        let reader = image::ImageReader::new(std::io::Cursor::new(&bin[offset..offset + size]))
            .with_guessed_format()
            .map_err(|_| bad())?;
        if !matches!(
            reader.format(),
            Some(image::ImageFormat::Png | image::ImageFormat::Jpeg)
        ) {
            return Err(bad());
        }
        // Online safety 10-26: and the colour depth bevy uploads it at. A 16-bit PNG becomes Rgba16Unorm, 8 bytes a
        // pixel, twice what MAX_PIXELS and LOBBY_PIXELS budget.
        let decoder = reader.into_decoder().map_err(|_| bad())?;
        if image::ImageDecoder::color_type(&decoder).bytes_per_pixel() > 4 {
            return Err("Online character textures must be 8-bit PNG or JPEG".into());
        }
        let (w, h) = image::ImageDecoder::dimensions(&decoder);
        pixels = pixels.saturating_add(w as u64 * h as u64);
        if w == 0 || h == 0 || w > 8192 || h > 8192 || pixels > MAX_PIXELS {
            return Err("Online character textures exceed the memory limit".into());
        }
        sizes.push(w as u64 * h as u64);
    }
    // What is decoded: one image per texture.
    let mut decoded = 0u64;
    for t in list("textures") {
        let source = t["source"].as_u64().and_then(|s| sizes.get(s as usize)).ok_or_else(bad)?;
        decoded += source;
        if decoded > MAX_PIXELS {
            return Err("Online character textures exceed the memory limit".into());
        }
    }
    let nodes = json["nodes"].as_array().ok_or_else(bad)?;
    let mut parents = vec![false; nodes.len()];
    for (i, node) in nodes.iter().enumerate() {
        for c in node["children"].as_array().into_iter().flatten() {
            let c = c.as_u64().ok_or_else(bad)? as usize;
            if c >= nodes.len() || c == i || parents[c] {
                return Err(bad());
            }
            parents[c] = true;
        }
    }
    fn visit(i: usize, nodes: &[Value], seen: &mut [u8], depth: usize) -> bool {
        if depth > 128 || seen[i] == 1 {
            return false;
        }
        if seen[i] == 2 {
            return true;
        }
        seen[i] = 1;
        for c in nodes[i]["children"].as_array().into_iter().flatten() {
            if !visit(c.as_u64().unwrap() as usize, nodes, seen, depth + 1) {
                return false;
            }
        }
        seen[i] = 2;
        true
    }
    let mut seen = vec![0; nodes.len()];
    for i in 0..nodes.len() {
        if !visit(i, nodes, &mut seen, 0) {
            return Err(bad());
        }
    }
    if json["skins"].as_array().is_none_or(|s| s.is_empty()) {
        return Err("Online character has no skin".into());
    }
    // Each scene is instantiated from its roots: a root listed twice or that is someone's child is built twice,
    // and a skin whose joint is outside the scene panics the loader (node map index).
    let skins = list("skins");
    for scene in list("scenes") {
        let mut reached = vec![false; nodes.len()];
        let mut stack = vec![];
        for r in scene["nodes"].as_array().into_iter().flatten() {
            let r = r.as_u64().filter(|&r| (r as usize) < nodes.len()).ok_or_else(bad)? as usize;
            if parents[r] || reached[r] {
                return Err(bad());
            }
            reached[r] = true;
            stack.push(r);
        }
        while let Some(i) = stack.pop() {
            for c in nodes[i]["children"].as_array().into_iter().flatten() {
                let c = c.as_u64().unwrap() as usize;
                reached[c] = true;
                stack.push(c);
            }
        }
        for (i, node) in nodes.iter().enumerate() {
            let Some(s) = node.get("skin").filter(|_| reached[i]) else {
                continue;
            };
            let skin = s.as_u64().and_then(|s| skins.get(s as usize)).ok_or_else(bad)?;
            for j in skin["joints"].as_array().into_iter().flatten() {
                if !j.as_u64().is_some_and(|j| reached.get(j as usize) == Some(&true)) {
                    return Err(bad());
                }
            }
        }
    }
    Ok(Cost {
        pixels: decoded,
        elements,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn glb(json: Value) -> Vec<u8> {
        let mut j = serde_json::to_vec(&json).unwrap();
        while j.len() % 4 != 0 {
            j.push(b' ');
        }
        let mut b = b"glTF".to_vec();
        b.extend(2u32.to_le_bytes());
        b.extend((28u32 + j.len() as u32).to_le_bytes());
        b.extend((j.len() as u32).to_le_bytes());
        b.extend(0x4e4f534au32.to_le_bytes());
        b.extend(j);
        b.extend(0u32.to_le_bytes());
        b.extend(0x004e4942u32.to_le_bytes());
        b
    }
    #[test]
    fn online_appearance_glb_rejects_paths_cycles_and_bad_ranges() {
        let base = serde_json::json!({"asset":{"version":"2.0"},"buffers":[{"byteLength":0}],"nodes":[{}],"skins":[{"joints":[0]}]});
        assert!(validate_glb(&glb(base.clone())).is_ok());
        let mut external = base.clone();
        external["images"] = serde_json::json!([{"uri":"../../private/file.png"}]);
        assert!(validate_glb(&glb(external)).is_err());
        let mut cyclic = base.clone();
        cyclic["nodes"] = serde_json::json!([{"children":[1]},{"children":[0]}]);
        assert!(validate_glb(&glb(cyclic)).is_err());
        let mut range = base.clone();
        range["bufferViews"] = serde_json::json!([{"buffer":0,"byteLength":99999}]);
        assert!(validate_glb(&glb(range)).is_err());
        let mut bytes = glb(base);
        bytes[8] = 0;
        assert!(validate_glb(&bytes).is_err());
    }
    fn glb_with(json: &Value, bin: &[u8]) -> Vec<u8> {
        let mut j = serde_json::to_vec(json).unwrap();
        while j.len() % 4 != 0 {
            j.push(b' ');
        }
        let mut bin = bin.to_vec();
        while bin.len() % 4 != 0 {
            bin.push(0);
        }
        let mut b = b"glTF".to_vec();
        b.extend(2u32.to_le_bytes());
        b.extend((28 + j.len() as u32 + bin.len() as u32).to_le_bytes());
        b.extend((j.len() as u32).to_le_bytes());
        b.extend(0x4e4f534au32.to_le_bytes());
        b.extend(j);
        b.extend((bin.len() as u32).to_le_bytes());
        b.extend(0x004e4942u32.to_le_bytes());
        b.extend(bin);
        b
    }
    /// A PNG of only its header (signature, IHDR, an empty IDAT): what the validator reads, never decodes.
    fn png_header(w: u32, h: u32) -> Vec<u8> {
        png_header_of_depth(w, h, 8)
    }
    /// An RGBA PNG's header with `depth` bits a channel.
    fn png_header_of_depth(w: u32, h: u32, depth: u8) -> Vec<u8> {
        fn crc(data: &[u8]) -> u32 {
            let mut c = !0u32;
            for &b in data {
                c ^= b as u32;
                for _ in 0..8 {
                    c = if c & 1 != 0 { 0xedb88320 ^ (c >> 1) } else { c >> 1 };
                }
            }
            !c
        }
        let mut ihdr = b"IHDR".to_vec();
        ihdr.extend(w.to_be_bytes());
        ihdr.extend(h.to_be_bytes());
        ihdr.extend([depth, 6, 0, 0, 0]);
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend(13u32.to_be_bytes());
        png.extend(&ihdr);
        png.extend(crc(&ihdr).to_be_bytes());
        png.extend(0u32.to_be_bytes());
        png.extend(b"IDAT");
        png.extend(crc(b"IDAT").to_be_bytes());
        png
    }
    /// One skinned triangle: positions, normals, u16 indices.
    fn triangle() -> (Value, Vec<u8>) {
        let mut bin = vec![];
        for f in [0f32, 0., 0., 1., 0., 0., 0., 1., 0., 0., 0., 1., 0., 0., 1., 0., 0., 1.] {
            bin.extend(f.to_le_bytes());
        }
        for i in [0u16, 1, 2] {
            bin.extend(i.to_le_bytes());
        }
        let json = serde_json::json!({
            "asset": {"version": "2.0"},
            "buffers": [{"byteLength": bin.len()}],
            "bufferViews": [{"buffer": 0, "byteLength": 36}, {"buffer": 0, "byteOffset": 36, "byteLength": 36}, {"buffer": 0, "byteOffset": 72, "byteLength": 6}],
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0, 0, 0], "max": [1, 1, 0]},
                {"bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3"},
                {"bufferView": 2, "componentType": 5123, "count": 3, "type": "SCALAR"}],
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0, "NORMAL": 1}, "indices": 2}]}],
            "nodes": [{"mesh": 0, "skin": 0, "children": [1]}, {"name": "root"}],
            "skins": [{"joints": [1]}],
            "scenes": [{"nodes": [0]}],
            "scene": 0,
        });
        (json, bin)
    }
    #[test]
    fn online_appearance_glb_bounds_what_the_loader_would_allocate_or_panic_on() {
        let (base, bin) = triangle();
        let check = |json: &Value, bin: &[u8]| validate_glb(&glb_with(json, bin));
        assert_eq!(check(&base, &bin), Ok(Cost { pixels: 0, elements: 9 }));
        let hostile = |edit: &dyn Fn(&mut Value)| {
            let mut json = base.clone();
            edit(&mut json);
            check(&json, &bin)
        };
        // Accessor past its view; stride shorter than an element (gltf's reader panics); sparse; empty; no data.
        assert!(hostile(&|j| j["accessors"][0]["count"] = 4.into()).is_err());
        assert!(hostile(&|j| j["bufferViews"][0]["byteStride"] = 4.into()).is_err());
        assert!(hostile(&|j| j["accessors"][0]["sparse"] = serde_json::json!({"count": 1})).is_err());
        assert!(hostile(&|j| j["accessors"][0]["count"] = 0.into()).is_err());
        assert!(hostile(&|j| {
            j["accessors"][1].as_object_mut().unwrap().remove("bufferView");
        })
        .is_err());
        // Attribute lengths that differ, non-float positions, non-triangles.
        assert!(hostile(&|j| j["accessors"][1]["count"] = 2.into()).is_err());
        assert!(hostile(&|j| j["accessors"][0]["componentType"] = 5123.into()).is_err());
        assert!(hostile(&|j| j["meshes"][0]["primitives"][0]["mode"] = 0.into()).is_err());
        // Morph weights past bevy's 64 (MeshMorphWeights::new(..).unwrap()).
        assert!(hostile(&|j| j["meshes"][0]["weights"] = serde_json::json!(vec![0.; 65])).is_err());
        // Cameras and lights would spawn into the receiver's world.
        assert!(hostile(&|j| {
            j["cameras"] = serde_json::json!([{"type": "perspective", "perspective": {"yfov": 1, "znear": 0.1}}]);
            j["nodes"][1]["camera"] = 0.into();
        })
        .is_err());
        assert!(hostile(&|j| j["extensions"] = serde_json::json!({"KHR_lights_punctual": {"lights": [{"type": "point"}]}})).is_err());
        // A joint outside the scene (loader node-map index panic); a root listed twice; too many scenes.
        assert!(hostile(&|j| {
            j["nodes"].as_array_mut().unwrap().push(serde_json::json!({"name": "loose"}));
            j["skins"][0]["joints"] = serde_json::json!([2]);
        })
        .is_err());
        assert!(hostile(&|j| j["scenes"][0]["nodes"] = serde_json::json!([0, 0])).is_err());
        assert!(hostile(&|j| j["scenes"] = serde_json::json!(vec![serde_json::json!({"nodes": [0]}); 5])).is_err());
        // Index past the vertices.
        let mut far = bin.clone();
        far[76] = 5;
        assert!(check(&base, &far).is_err());
        // Primitives sharing one accessor: each is decoded on its own.
        let primitive = base["meshes"][0]["primitives"][0].clone();
        assert!(hostile(&|j| j["meshes"][0]["primitives"] = serde_json::json!(vec![primitive.clone(); 1025])).is_err());
        let mut indices = bin.clone();
        indices.extend(vec![0u8; 999_999]);
        let mut big = base.clone();
        big["buffers"][0]["byteLength"] = indices.len().into();
        big["bufferViews"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"buffer": 0, "byteOffset": 78, "byteLength": 999_999}));
        big["accessors"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"bufferView": 3, "componentType": 5121, "count": 999_999, "type": "SCALAR"}));
        let mut shared = primitive.clone();
        shared["indices"] = 3.into();
        big["meshes"][0]["primitives"] = serde_json::json!(vec![shared.clone(); 16]);
        assert!(check(&big, &indices).is_ok());
        big["meshes"][0]["primitives"] = serde_json::json!(vec![shared; 17]);
        assert!(check(&big, &indices).is_err(), "17 x 1M decoded indices");
        // Deep or huge node trees.
        let chain: Vec<_> = (0..200)
            .map(|i| if i < 199 { serde_json::json!({"children": [i + 1]}) } else { serde_json::json!({}) })
            .collect();
        assert!(hostile(&|j| j["nodes"] = Value::Array(chain.clone())).is_err());
        let wide = serde_json::json!({"asset": {"version": "2.0"}, "buffers": [{"byteLength": 0}], "nodes": vec![serde_json::json!({}); 1_000_000], "skins": [{"joints": [0]}]});
        assert!(validate_glb(&glb(wide)).is_err());
        // Any URI: files, the web, data URIs.
        for uri in ["file:///C:/Windows/win.ini", "../../secret.bin", "http://example.com/x.bin", "data:application/octet-stream;base64,AAAA"] {
            assert!(hostile(&|j| j["buffers"][0]["uri"] = uri.into()).is_err(), "{uri}");
        }
    }
    /// The triangle with one image (from `png`) and `textures` textures of it.
    fn textured(png: &[u8], textures: usize) -> Vec<u8> {
        let (mut json, mut bin) = triangle();
        bin.extend([0, 0]);
        json["bufferViews"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"buffer": 0, "byteOffset": bin.len(), "byteLength": png.len()}));
        bin.extend(png);
        json["buffers"][0]["byteLength"] = bin.len().into();
        json["images"] = serde_json::json!([{"bufferView": 3, "mimeType": "image/png"}]);
        json["textures"] = serde_json::json!(vec![serde_json::json!({"source": 0}); textures]);
        glb_with(&json, &bin)
    }
    #[test]
    fn online_appearance_glb_counts_pixels_per_texture_and_checks_png_headers_first() {
        let image = png_header(1024, 1024);
        assert_eq!(validate_glb(&textured(&image, 1)).map(|c| c.pixels), Ok(1 << 20));
        // bevy_gltf decodes once per texture: 65 textures of one 1024x1024 image is 65M pixels.
        assert!(validate_glb(&textured(&image, 65)).is_err());
        // A header claiming 65535x65535 is refused before anything is decoded.
        assert!(validate_glb(&textured(&png_header(65535, 65535), 1)).is_err());
        // Online safety 10-26: 16 bits a channel is 8 bytes a pixel on the GPU, past what the pixel caps budget.
        assert_eq!(
            validate_glb(&textured(&png_header_of_depth(1024, 1024, 16), 1)).err().as_deref(),
            Some("Online character textures must be 8-bit PNG or JPEG")
        );
    }
    #[test]
    fn online_appearance_lobby_budget_draws_the_peer_past_it_stock() {
        let mut state = Appearances::default();
        let big = Cost {
            pixels: LOBBY_PIXELS / 2 + 1,
            elements: 0,
        };
        let model = || Ok(Look::Imported("online-characters://x.glb".into()));
        for (peer, hash) in [(1, [1; 32]), (2, [2; 32])] {
            state.costs.insert(hash, big);
            accept(&mut state, peer, hash, model());
        }
        assert!(matches!(state.looks[&1].1, Look::Imported(_)));
        assert_eq!(state.looks[&2].1, Look::Stock, "over the lobby budget");
        // Once the first peer goes stock, the second fits on its next delivery.
        accept(&mut state, 1, [3; 32], Ok(Look::Stock));
        accept(&mut state, 2, [2; 32], model());
        assert!(matches!(state.looks[&2].1, Look::Imported(_)));
    }
    #[test]
    fn online_appearance_outfits_a_peer_took_off_are_dropped() {
        let mut state = Appearances::default();
        state.cached.insert([0xee; 32], Look::Imported(String::new()));
        for i in 0..100u8 {
            accept(&mut state, 1, [i; 32], Ok(Look::Outfit(serde_json::json!({ "n": i }))));
        }
        accept(&mut state, 2, [0xfe; 32], Ok(Look::Native("x".into())));
        assert_eq!(state.cached.len(), 3, "peer 1's last outfit, peer 2's look and the import");
        assert!(state.cached.contains_key(&[99; 32]) && state.cached.contains_key(&[0xee; 32]));
    }
    #[test]
    fn online_appearance_cache_evicts_unworn_models_and_refuses_past_its_cap() {
        let mut state = Appearances::default();
        std::fs::create_dir_all(cache_directory()).unwrap();
        let (worn, old) = ([0xa1; 32], [0xa2; 32]);
        let file = |h: &[u8; 32]| cache_directory().join(cache_name(h));
        std::fs::write(file(&worn), vec![0; 4096]).unwrap();
        std::fs::write(file(&old), vec![0; 4096]).unwrap();
        state.looks.insert(7, (worn, Look::Imported(String::new())));
        state.cached.insert(old, Look::Imported(String::new()));
        // Needs the old file's space: it goes, and its cached look with it.
        assert!(make_room(&mut state, CACHE_BYTES - 6000).is_ok());
        assert!(!file(&old).exists() && file(&worn).exists());
        assert!(!state.cached.contains_key(&old));
        // Past the cap with only worn models left: refused, nothing deleted.
        assert!(make_room(&mut state, CACHE_BYTES).is_err());
        assert!(file(&worn).exists());
        let _ = std::fs::remove_file(file(&worn));
    }
    /// The biggest honest profile: every slot worn and tinted, every morph, tattoos, and a full
    /// design (64 layers of the longest text) on all six graphic locations, a stock logo over two (a top keeps one).
    fn full_profile() -> serde_json::Value {
        let slots = ["Accessory", "Arm", "Feet", "Glasses", "Hair", "Hat", "InnerTorso", "Jewellery", "Leg", "Organ", "OuterTorso", "Pants", "Rostral", "SkateBoard", "SkateTruck", "SkateWheel", "Sock", "WristItem",
            "Belt", "Chain", "Backpack", "Gloves", "AccessoryOther"];
        let id = |n: usize| format!("{:016x}", 0x801095192c838106u64.wrapping_mul(n as u64 + 1));
        let selections: serde_json::Map<String, Value> = slots.iter().enumerate().map(|(i, s)| (s.to_string(), serde_json::json!({"asset_id": id(i), "material_id": id(i + 99)}))).collect();
        let colours: serde_json::Map<String, Value> = slots.iter().map(|s| (s.to_string(), serde_json::json!([0.123456789, 0.987654321, 0.5]))).collect();
        let morphs: serde_json::Map<String, Value> = (0..22).map(|i| (format!("local_brows_height_{i:02}"), serde_json::json!(0.123456789))).collect();
        let design = crate::custom_graphic::Doc {
            layers: vec![crate::custom_graphic::Layer::new(crate::custom_graphic::Art::Text("W".repeat(crate::custom_graphic::MAX_TEXT)), [255; 4]); crate::custom_graphic::MAX_LAYERS],
            background: [255; 4],
        }
        .encode();
        let side = |logo: bool| if logo { serde_json::json!({"design": design, "logo": id(7), "x": 0.123, "y": -0.234, "size": 1.5, "angle": 45.0}) } else { serde_json::json!({"design": design}) };
        serde_json::json!({
            "gender": "female", "selections": selections, "colours": colours, "morphs": morphs, "hair_choice": selections["Hair"],
            "skin_tint": [0.123456789, 0.5, 0.5], "hair_tint": [0.123456789, 0.5, 0.5], "grip": {"rgb": [0.123456789, 0.5, 0.5], "solid": true},
            "truck": 0.7, "wheel": 0.7, "stance": 1, "style": 3, "posture": 3, "gestures": {"0": 36, "1": 35, "2": 34, "3": 33},
            "tattoos": {"Arm": {"id": id(1), "side": 3, "x": 0.123, "y": 0.123, "size": 1.234, "angle": 12.5}, "Leg": {"id": id(2), "side": 1, "x": 0.123, "y": 0.123, "size": 1.234, "angle": 12.5}},
            "graphics": {"front": side(true), "back": side(false), "left": side(false), "right": side(false), "deck": side(true), "hat": side(false)},
        })
    }
    #[test]
    fn a_full_profile_of_designs_goes_through_the_appearance_encode_within_the_limit() {
        let applied = full_profile();
        let bytes = outfit_bytes(&applied).unwrap();
        eprintln!("MESSAGE: full six-side profile = {} bytes of {MAX_OUTFIT} ({} bytes without designs)", bytes.len(), {
            let mut plain = applied.clone();
            crate::custom_graphic::without_designs(&mut plain);
            serde_json::to_vec(&Look::Outfit(plain)).unwrap().len()
        });
        assert!(bytes.len() <= MAX_OUTFIT, "{} bytes", bytes.len());
        // What a peer makes of it: read, then sanitized as Parts::resolve does.
        let Look::Outfit(mut seen) = read_look(&bytes).unwrap() else { panic!("an outfit") };
        crate::custom_graphic::sanitize(&mut seen, |id| id == format!("{:016x}", 0x801095192c838106u64.wrapping_mul(8)));
        assert_eq!(seen["graphics"], applied["graphics"], "all six designs and both logos arrive");
        // An outfit past the limit goes without its designs rather than be dropped by every peer.
        let mut huge = applied.clone();
        for n in 0..6 {
            huge["graphics"][format!("extra{n}")] = applied["graphics"]["back"].clone();
        }
        let sent = outfit_bytes(&huge).unwrap();
        assert!(sent.len() <= MAX_OUTFIT && !String::from_utf8(sent).unwrap().contains("\"design\""));
        assert!(read_look(&vec![b' '; MAX_OUTFIT + 1]).is_err());
        // And past the limit even without designs: not sent at all.
        huge["notes"] = serde_json::json!("x".repeat(MAX_OUTFIT));
        assert!(outfit_bytes(&huge).is_err());
    }
    #[test]
    #[ignore = "requires an owned imported GLB via SKATE_ONLINE_TEST_GLB"]
    fn online_appearance_accepts_owned_import() {
        let path = std::env::var("SKATE_ONLINE_TEST_GLB").unwrap();
        validate_glb(&std::fs::read(path).unwrap()).unwrap();
    }
}
