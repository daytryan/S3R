//! Little-endian SKATE01..15 reader. Layout follows the supplied
//! SK8R15/Source/tools/blender_owned_map/SKATE_FORMAT.md and
//! owned/world/src/owned_map_package.cpp. No geometry or physics is inferred.
use std::{io::Read, path::Path, time::Instant};
mod storage_v15;
mod texture_decode;

#[derive(Debug, PartialEq)]
pub struct SkateMap {
    pub version: u8,
    pub name: String,
    pub spawn: [f32; 3],
    pub heading: f32,
    /// Authored environment fields in wire order (12, 14, or 45 floats).
    pub environment: Vec<f32>,
    pub materials: Vec<Material>,
    pub textures: Vec<Texture>,
    pub geometry: Geometry,
    pub rails: Vec<Rail>,
    pub doors: Vec<Door>,
    pub lights: Vec<Light>,
    pub routes: Vec<Route>,
    pub extensions: Vec<Extension>,
}
#[derive(Debug, PartialEq)]
pub struct Material {
    pub name: String,
    pub flags: u32,
    pub friction: f32,
    pub restitution: f32,
    pub color: [f32; 3],
    pub roughness: f32,
    pub emissive: f32,
    /// Albedo, indirect, normal, ORM, emissive. One-based, zero absent.
    pub textures: [u32; 5],
    pub indirect_strength: f32,
    pub alpha_mode: u32,
    pub alpha_cutoff: f32,
    pub audio: u32,
    pub physics: u32,
    pub pattern: u32,
    pub depth_layer: Option<u32>,
    /// Complete v12+ retail definition bytes; distinct from portable PBR fields.
    pub retail_definition: Option<Vec<u8>>,
}
#[derive(Debug, PartialEq)]
pub struct Texture {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub color_space: u32,
    pub rgba: Vec<u8>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub lightmap_uv: [f32; 2],
    pub material: u32,
    pub decal_uv: Option<[f32; 2]>,
    pub tangent_frame: Option<[u8; 4]>,
}
#[derive(Debug, PartialEq)]
pub struct Collision {
    pub points: [[f32; 3]; 3],
    pub surface: u32,
    pub material: u32,
    pub native_edges: Option<[u8; 3]>,
}
#[derive(Debug, PartialEq)]
pub struct Geometry {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub collision: Vec<Collision>,
}
#[derive(Debug, PartialEq)]
pub struct Rail {
    pub name: String,
    pub closed: bool,
    pub points: Vec<[f32; 3]>,
    pub native: Option<Vec<u8>>,
}
#[derive(Debug, PartialEq)]
pub struct Door {
    pub name: String,
    /// Hinge position/axis, closed width/depth axes, local min/max.
    pub frame: [[f32; 3]; 6],
    /// Minimum/maximum/initial angle, mass, damping.
    pub motion: [f32; 5],
    /// Spring, max angular speed, contact impulse scale (absent in v4).
    pub response: Option<[f32; 3]>,
    pub friction: f32,
    pub restitution: f32,
    pub surface: u32,
    pub geometry: Geometry,
}
#[derive(Debug, PartialEq)]
pub struct Light {
    pub name: String,
    pub kind: u32,
    pub position: [f32; 3],
    pub direction: [f32; 3],
    pub color: [f32; 3],
    pub intensity: f32,
    pub range: f32,
    pub radius: f32,
    pub inner_cos: f32,
    pub outer_cos: f32,
}
#[derive(Debug, PartialEq)]
pub struct Route {
    pub rail: Rail,
    pub skaters: u32,
    pub speed: f32,
    pub spacing: f32,
}
#[derive(Debug, PartialEq)]
pub struct Extension {
    pub tag: [u8; 4],
    pub schema: u32,
    pub payload: Vec<u8>,
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

/// MOBJ schema 3 stores editor ownership of ranges in the base geometry.
/// Physics-disabled objects can use those unchanged static arrays. Explicit
/// body types still require a runtime adapter and must not be flattened.
pub fn validate_static_objects(map: &SkateMap, extension: &Extension) -> Result<(), String> {
    if extension.schema != 3 {
        return Err("Unsupported MOBJ schema".into());
    }
    let mut r = Reader {
        bytes: &extension.payload,
        at: 0,
    };
    let count = r.u()?;
    r.check_count(count, 80)?;
    let mut ids = std::collections::HashSet::new();
    for _ in 0..count {
        if !ids.insert(r.u()?) {
            return Err("Duplicate MOBJ identity".into());
        }
        let name = r.string()?;
        r.floats::<3>()?;
        for limit in [map.geometry.indices.len(), map.geometry.collision.len()] {
            let first = r.u()? as usize;
            let length = r.u()? as usize;
            if first.checked_add(length).is_none_or(|end| end > limit) {
                return Err(format!("MOBJ {name} geometry range is invalid"));
            }
        }
        let rails = r.u()?;
        r.check_count(rails, 4)?;
        for _ in 0..rails {
            if r.u()? as usize >= map.rails.len() {
                return Err(format!("MOBJ {name} rail index is invalid"));
            }
        }
        if r.u()? != 0 {
            return Err(format!(
                "MOBJ {name} requests object physics, which requires a body adapter"
            ));
        }
        if r.u()? > 2 {
            return Err(format!("MOBJ {name} collision shape is invalid"));
        }
        r.floats::<6>()?;
        for _ in 0..2 {
            if r.u()? > 1 {
                return Err(format!("MOBJ {name} has an invalid boolean"));
            }
        }
    }
    if r.at != r.bytes.len() {
        return Err("MOBJ has trailing data".into());
    }
    Ok(())
}
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).ok_or("SKATE offset overflow")?;
        let result = self
            .bytes
            .get(self.at..end)
            .ok_or_else(|| format!("Truncated SKATE at byte {} (need {n})", self.at))?;
        self.at = end;
        Ok(result)
    }
    fn u(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn f(&mut self) -> Result<f32, String> {
        let value = f32::from_bits(self.u()?);
        if !value.is_finite() {
            return Err(format!("Non-finite SKATE float at {}", self.at - 4));
        }
        Ok(value)
    }
    fn floats<const N: usize>(&mut self) -> Result<[f32; N], String> {
        let mut result = [0.; N];
        for v in &mut result {
            *v = self.f()?;
        }
        Ok(result)
    }
    fn string(&mut self) -> Result<String, String> {
        let n = self.u()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).map_err(|_| "Invalid SKATE UTF-8".into())
    }
    fn check_count(&self, count: u32, minimum_bytes: usize) -> Result<(), String> {
        if count as usize > (self.bytes.len() - self.at) / minimum_bytes {
            return Err("SKATE count exceeds remaining data".into());
        }
        Ok(())
    }
    fn points(&mut self) -> Result<Vec<[f32; 3]>, String> {
        let n = self.u()?;
        self.check_count(n, 12)?;
        (0..n).map(|_| self.floats()).collect()
    }
    fn stored_block(&mut self, expected: usize) -> Result<StoredBlock<'a>, String> {
        if expected > 2_147_483_648 {
            return Err("SKATE decoded block exceeds 2 GiB reader limit".into());
        }
        let method = self.u()?;
        let n = self.u()? as usize;
        let bytes = self.take(n)?;
        Ok(StoredBlock {
            expected,
            method,
            bytes,
        })
    }
    fn stored(&mut self, expected: usize) -> Result<Vec<u8>, String> {
        self.stored_block(expected)?.decode()
    }
    fn geometry(
        &mut self,
        counts: [u32; 3],
        materials: usize,
        version: u8,
    ) -> Result<Geometry, String> {
        let [nv, ni, nc] = counts;
        let vertices = self.vertices(nv, version)?;
        let indices = self.indices(ni)?;
        let collision = self.collision(nc, version)?;
        checked_geometry(vertices, indices, collision, materials)
    }
    // Each record list is sized exactly: a collect through Result grows by doubling (New San Vanelona's 11.8M
    // vertices held 16.8M slots, 320 MB unused for the whole load).
    fn vertices(&mut self, nv: u32, version: u8) -> Result<Vec<Vertex>, String> {
        self.check_count(nv, if version >= 12 { 56 } else { 44 })?;
        let mut vertices = Vec::with_capacity(nv as usize);
        for _ in 0..nv {
            vertices.push(Vertex {
                position: self.floats()?,
                normal: self.floats()?,
                uv: self.floats()?,
                lightmap_uv: self.floats()?,
                material: self.u()?,
                decal_uv: if version >= 12 {
                    Some(self.floats()?)
                } else {
                    None
                },
                tangent_frame: if version >= 12 {
                    Some(self.take(4)?.try_into().unwrap())
                } else {
                    None
                },
            });
        }
        Ok(vertices)
    }
    fn indices(&mut self, ni: u32) -> Result<Vec<u32>, String> {
        self.check_count(ni, 4)?;
        let mut indices = Vec::with_capacity(ni as usize);
        for _ in 0..ni {
            indices.push(self.u()?);
        }
        Ok(indices)
    }
    fn collision(&mut self, nc: u32, version: u8) -> Result<Vec<Collision>, String> {
        self.check_count(nc, if version >= 11 { 48 } else { 44 })?;
        let mut collision = Vec::with_capacity(nc as usize);
        for _ in 0..nc {
            let points = [self.floats()?, self.floats()?, self.floats()?];
            let surface = self.u()?;
            let material = self.u()?;
            let native_edges = if version >= 11 {
                let bytes = self.take(4)?;
                if bytes[3] != 0 {
                    Some(bytes[..3].try_into().unwrap())
                } else {
                    None
                }
            } else {
                None
            };
            collision.push(Collision {
                points,
                surface,
                material,
                native_edges,
            });
        }
        Ok(collision)
    }
}
fn checked_geometry(
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
    collision: Vec<Collision>,
    materials: usize,
) -> Result<Geometry, String> {
    let valid_material = |id: u32| id > 0 && id as usize <= materials;
    if vertices.iter().any(|v| !valid_material(v.material))
        || collision
            .iter()
            .any(|v| !valid_material(v.material) || v.surface == 0)
    {
        return Err("SKATE invalid material/surface reference".into());
    }
    if indices.len() % 3 != 0 || indices.iter().any(|&i| i as usize >= vertices.len()) {
        return Err("SKATE invalid triangle indices".into());
    }
    for tri in indices.chunks_exact(3) {
        if tri
            .iter()
            .any(|&i| vertices[i as usize].material != vertices[tri[0] as usize].material)
        {
            return Err("SKATE triangle mixes materials".into());
        }
    }
    for tri in &collision {
        let [a, b, c] = tri.points;
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let cross = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let area = cross.iter().map(|x| x * x).sum::<f32>();
        if !area.is_finite() || area <= 0. {
            return Err("SKATE degenerate collision triangle".into());
        }
    }
    Ok(Geometry {
        vertices,
        indices,
        collision,
    })
}
/// Vertex records per parse job: 4 MB of parsed vertices.
const VERTEX_CHUNK: usize = 1 << 16;
/// The decoded vertex block's records, parsed by `workers` threads (G3: ~1 s of New San Vanelona's load on one).
/// Records are fixed-size, so each job is a `chunk`-record run taken in order, and this thread appends finished runs
/// in order: the list is never held twice, and the first error is the one a single pass reports (its offset is into
/// the same block).
fn parallel_vertices(bytes: &[u8], nv: u32, version: u8, workers: usize, chunk: usize) -> Result<Vec<Vertex>, String> {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
    let stride = if version >= 12 { 56 } else { 44 };
    Reader { bytes, at: 0 }.check_count(nv, stride)?;
    let (n, jobs) = (nv as usize, (nv as usize).div_ceil(chunk));
    if workers <= 1 || jobs <= 1 {
        return Reader { bytes, at: 0 }.vertices(nv, version);
    }
    let (next, stop) = (AtomicUsize::new(0), AtomicBool::new(false));
    let (send, done) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        let mut spawned = 0;
        for _ in 0..workers.min(jobs) {
            let (send, next, stop) = (send.clone(), &next, &stop);
            let worker = std::thread::Builder::new().name("map-vertex-parse".into()).spawn_scoped(scope, move || loop {
                let job = next.fetch_add(1, Relaxed);
                if job >= jobs || stop.load(Relaxed) {
                    break;
                }
                let (start, end) = (job * chunk, ((job + 1) * chunk).min(n));
                let parsed = Reader { bytes: &bytes[..end * stride], at: start * stride }.vertices((end - start) as u32, version);
                if send.send((job, parsed)).is_err() {
                    break;
                }
            });
            spawned += usize::from(worker.is_ok());
        }
        drop(send);
        if spawned == 0 {
            return Reader { bytes, at: 0 }.vertices(nv, version);
        }
        let mut vertices = Vec::with_capacity(n);
        let mut early = std::collections::BTreeMap::new();
        for (job, parsed) in done {
            early.insert(job, parsed);
            while let Some(parsed) = early.remove(&(vertices.len() / chunk)) {
                match parsed {
                    Ok(run) => vertices.extend(run),
                    Err(error) => {
                        stop.store(true, Relaxed);
                        return Err(error);
                    }
                }
            }
        }
        if vertices.len() != n {
            return Err("SKATE vertex parse stopped early".into());
        }
        Ok(vertices)
    })
}
/// Borrow compressed payloads while scanning; decompression does not mutate
/// the package or depend on any other texture.
#[derive(Clone, Copy)]
struct StoredBlock<'a> {
    expected: usize,
    method: u32,
    bytes: &'a [u8],
}
/// Decode blocks on their own threads when more than one worker is allowed.
fn decode_together<const N: usize>(
    blocks: &[StoredBlock<'_>; N],
    workers: usize,
) -> [Result<Vec<u8>, String>; N] {
    if workers <= 1 {
        return blocks.each_ref().map(StoredBlock::decode);
    }
    std::thread::scope(|scope| {
        let spawned = blocks.each_ref().map(|block| {
            std::thread::Builder::new()
                .name("map-block-decode".into())
                .spawn_scoped(scope, move || block.decode())
        });
        spawned.map(|worker| match worker {
            Ok(worker) => worker
                .join()
                .unwrap_or_else(|_| Err("Block decoder failed unexpectedly".into())),
            Err(error) => Err(format!("Block decoder: {error}")),
        })
    })
}
/// perf 10-18: a block's decoded size up front (the 8 MB cap regrew bigger blocks several times), capped at 64x its
/// compressed input (never below the old 8 MB) so a corrupt header can't reserve gigabytes. SKATE_MAP_RESERVE=0: 8 MB.
fn reserve(expected: usize, input: usize) -> usize {
    const OLD: usize = 8 * 1024 * 1024;
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let on = *ON.get_or_init(|| std::env::var_os("SKATE_MAP_RESERVE").is_none_or(|v| v != "0"));
    expected.min(if on { input.saturating_mul(64).max(OLD) } else { OLD })
}
impl StoredBlock<'_> {
    fn decode(&self) -> Result<Vec<u8>, String> {
        let Self {
            expected,
            method,
            bytes,
        } = *self;
        let decoded = match method {
            0 => bytes.to_vec(),
            1 => {
                let mut result = Vec::with_capacity(reserve(expected, bytes.len()));
                let mut decoder = flate2::read::ZlibDecoder::new(bytes);
                decoder
                    .by_ref()
                    .take(expected as u64 + 1)
                    .read_to_end(&mut result)
                    .map_err(|e| format!("SKATE DEFLATE: {e}"))?;
                if decoder.total_in() as usize != bytes.len() {
                    return Err("SKATE compressed block has unused input".into());
                }
                result
            }
            2 => {
                let mut result = Vec::with_capacity(reserve(expected, bytes.len()));
                zstd::stream::read::Decoder::new(bytes)
                    .map_err(|e| e.to_string())?
                    .take(expected as u64 + 1)
                    .read_to_end(&mut result)
                    .map_err(|e| format!("SKATE Zstandard: {e}"))?;
                result
            }
            3..=10 => return storage_v15::decode(method, bytes, expected),
            _ => return Err(format!("Unsupported SKATE storage method {method}")),
        };
        if decoded.len() != expected {
            return Err("SKATE decoded block size mismatch".into());
        }
        Ok(decoded)
    }
}
impl SkateMap {
    pub fn load(path: &Path) -> Result<Self, String> {
        let started = Instant::now();
        let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let disk = started.elapsed();
        let parse_started = Instant::now();
        let map = Self::parse(&data).map_err(|e| format!("{}: {e}", path.display()))?;
        eprintln!(
            "MAP_READ_TIMING name={:?} disk_ms={} parse_ms={} file_bytes={}",
            map.name,
            disk.as_millis(),
            parse_started.elapsed().as_millis(),
            data.len()
        );
        Ok(map)
    }
    pub fn parse(data: &[u8]) -> Result<Self, String> {
        // Leave two cores for the running game (loading screen, uploads).
        let workers = std::thread::available_parallelism()
            .map_or(1, |n| n.get().saturating_sub(2).clamp(1, 8));
        Self::parse_with_decode_workers(data, workers)
    }
    /// Data tools can request one worker for reproducible serial comparisons.
    /// The decoder caps concurrency at eight and skips threading for small maps.
    pub fn parse_with_decode_workers(data: &[u8], workers: usize) -> Result<Self, String> {
        Self::parse_inner(data, workers, true, true)
    }

    /// Decode a supplemental presentation package without requiring collision.
    /// Playable map loading continues to use `parse`/`load`.
    pub fn parse_render_only(data: &[u8]) -> Result<Self, String> {
        Self::render_only(Self::parse_inner(data, 4, false, true)?)
    }

    /// Render-only package without texture pixels (`rgba` stays empty), for
    /// hosts that only need the triangles, e.g. movable prop collision.
    pub fn parse_geometry_only(data: &[u8]) -> Result<Self, String> {
        Self::render_only(Self::parse_inner(data, 4, false, false)?)
    }

    fn render_only(map: Self) -> Result<Self, String> {
        if !map.geometry.collision.is_empty()
            || !map.rails.is_empty()
            || !map.doors.is_empty()
            || !map.lights.is_empty()
            || !map.routes.is_empty()
            || map.extensions.iter().any(|e| e.tag != *b"WMET")
        {
            return Err("SKATE render-only package contains non-presentation data".into());
        }
        Ok(map)
    }
    fn parse_inner(
        data: &[u8],
        workers: usize,
        require_collision: bool,
        decode_textures: bool,
    ) -> Result<Self, String> {
        let parse_started = Instant::now();
        let mut r = Reader { bytes: data, at: 0 };
        let magic = r.take(8)?;
        if &magic[..5] != b"SKATE"
            || magic[7] != 0
            || !magic[5].is_ascii_digit()
            || !magic[6].is_ascii_digit()
        {
            return Err("Invalid SKATE magic/version".into());
        }
        let version = (magic[5] - b'0') * 10 + magic[6] - b'0';
        if !(1..=15).contains(&version) {
            return Err("Unsupported SKATE version: reader supports 01 through 15; newer packages require an update".into());
        }
        if r.u()? != 0x12345678 {
            return Err("Invalid SKATE endian marker".into());
        }
        let name = r.string()?;
        if name.is_empty() {
            return Err("Empty SKATE map name".into());
        }
        let spawn = r.floats()?;
        let heading = r.f()?;
        let ne = if version >= 6 {
            45
        } else if version >= 3 {
            14
        } else {
            12
        };
        let environment = (0..ne).map(|_| r.f()).collect::<Result<_, _>>()?;
        let mut counts = [0; 9];
        let n = if version >= 8 {
            9
        } else if version >= 7 {
            8
        } else if version >= 4 {
            7
        } else {
            6
        };
        for c in &mut counts[..n] {
            *c = r.u()?;
        }
        let material_bytes = if version >= 15 {
            let size = r.u()? as usize;
            Some(r.stored(size)?)
        } else {
            None
        };
        let mut package_reader = None;
        if let Some(bytes) = &material_bytes {
            package_reader = Some(r);
            r = Reader { bytes, at: 0 };
        }
        r.check_count(counts[0], if version >= 2 { 80 } else { 48 })?;
        let mut materials = Vec::new();
        for _ in 0..counts[0] {
            let mut m = Material {
                name: r.string()?,
                flags: r.u()?,
                friction: r.f()?,
                restitution: r.f()?,
                color: r.floats()?,
                roughness: r.f()?,
                emissive: r.f()?,
                textures: [r.u()?, r.u()?, 0, 0, 0],
                indirect_strength: r.f()?,
                alpha_mode: 0,
                alpha_cutoff: 0.5,
                audio: 3,
                physics: 1,
                pattern: 0,
                depth_layer: None,
                retail_definition: None,
            };
            if version >= 2 {
                for id in &mut m.textures[2..] {
                    *id = r.u()?;
                }
                m.alpha_mode = r.u()?;
                m.alpha_cutoff = r.f()?;
                m.audio = r.u()?;
                m.physics = r.u()?;
                m.pattern = r.u()?;
            }
            if version >= 13 {
                let layer = r.u()?;
                if layer > 3 {
                    return Err("Invalid SKATE depth layer".into());
                }
                m.depth_layer = Some(layer);
            }
            if version >= 12 && r.u()? != 0 {
                let start = r.at;
                r.take(16)?;
                r.string()?;
                r.take(8)?;
                let bindings = r.u()?;
                r.check_count(bindings, 20)?;
                for _ in 0..bindings {
                    r.string()?;
                    let id = r.u()?;
                    let uv = r.u()?;
                    let au = r.u()?;
                    let av = r.u()?;
                    if id > counts[1] || uv > 2 || au > 1 || av > 1 {
                        return Err("Invalid SKATE retail texture binding".into());
                    }
                }
                let parameters = r.u()?;
                r.check_count(parameters, 8)?;
                for _ in 0..parameters {
                    r.string()?;
                    let n = r.u()?;
                    r.check_count(n, 4)?;
                    for _ in 0..n {
                        r.string()?;
                    }
                }
                r.string()?;
                m.retail_definition = Some(r.bytes[start..r.at].to_vec());
            }
            if m.name.is_empty()
                || m.textures.iter().any(|&id| id > counts[1])
                || m.alpha_mode > 2
                || m.audio > 127
                || m.physics > 13
                || m.pattern > 15
                || m.friction < 0.
                || m.restitution < 0.
                || !(0.0..=1.0).contains(&m.roughness)
                || !(0.0..=1.0).contains(&m.alpha_cutoff)
                || m.indirect_strength < 0.
            {
                return Err("Invalid SKATE material".into());
            }
            materials.push(m);
        }
        if let Some(package) = package_reader {
            if r.at != r.bytes.len() {
                return Err("SKATE material block has trailing bytes".into());
            }
            r = package;
        }
        let texture_started = Instant::now();
        r.check_count(counts[1], 20)?;
        let mut textures = Vec::new();
        let mut texture_blocks: Vec<StoredBlock<'_>> = Vec::new();
        for _ in 0..counts[1] {
            let name = r.string()?;
            let width = r.u()?;
            let height = r.u()?;
            let color_space = r.u()?;
            if (width == 0) != (height == 0) || width > 16384 || height > 16384 || color_space > 1 {
                return Err("Invalid SKATE embedded texture".into());
            }
            let expected = width as usize * height as usize * 4;
            let block = if version >= 9 {
                r.stored_block(expected)?
            } else {
                let bytes = r.u()? as usize;
                if bytes != expected {
                    return Err("Invalid SKATE embedded texture size".into());
                }
                StoredBlock {
                    expected,
                    method: 0,
                    bytes: r.take(bytes)?,
                }
            };
            let block = if version >= 15 && block.method == 11 {
                let index = u32::from_le_bytes(
                    block
                        .bytes
                        .try_into()
                        .map_err(|_| "Invalid SKATE texture reference size")?,
                ) as usize;
                let source = *texture_blocks
                    .get(index)
                    .ok_or("Invalid SKATE forward texture reference")?;
                if source.expected != expected {
                    return Err("SKATE texture reference size mismatch".into());
                }
                source
            } else {
                block
            };
            texture_blocks.push(block);
            textures.push(Texture {
                name,
                width,
                height,
                color_space,
                rgba: Vec::new(),
            });
        }
        let texture_workers = if decode_textures {
            texture_decode::decode(&mut textures, &texture_blocks, workers)?
        } else {
            0
        };
        let texture_time = texture_started.elapsed();
        let geometry_started = Instant::now();
        let geometry_counts = [counts[2], counts[3], counts[4]];
        let geometry = if version >= 9 {
            let blocks = [
                r.stored_block(counts[2] as usize * if version >= 12 { 56 } else { 44 })?,
                r.stored_block(counts[3] as usize * 4)?,
                r.stored_block(counts[4] as usize * if version >= 11 { 48 } else { 44 })?,
            ];
            // Vertices, indices and collision are independent compressed blocks.
            let [vertices, indices, collision] = decode_together(&blocks, workers);
            let (vertices, indices, collision) = (vertices?, indices?, collision?);
            // Each block is read where it was decoded and dropped once parsed: appending them into one buffer
            // reallocated it to twice the vertex bytes (New San Vanelona: 1.3 GB) on top of the parsed copy.
            let vertices = parallel_vertices(&vertices, counts[2], version, workers, VERTEX_CHUNK)?;
            let indices = Reader { bytes: &indices, at: 0 }.indices(counts[3])?;
            let collision = Reader { bytes: &collision, at: 0 }.collision(counts[4], version)?;
            checked_geometry(vertices, indices, collision, materials.len())?
        } else {
            r.geometry(geometry_counts, materials.len(), version)?
        };
        let geometry_time = geometry_started.elapsed();
        if materials.is_empty() || geometry.vertices.is_empty() || geometry.indices.is_empty() {
            return Err("SKATE requires materials and render geometry".into());
        }
        r.check_count(counts[5], 12)?;
        let mut rails = Vec::new();
        for _ in 0..counts[5] {
            let name = r.string()?;
            let closed = r.u()? != 0;
            let representation = if version >= 10 { r.u()? } else { 0 };
            let (points, native) = match representation {
                0 => (r.points()?, None),
                1 => {
                    let start = r.at;
                    r.take(24)?;
                    let n = r.u()?;
                    r.check_count(n, 120)?;
                    r.take(n as usize * 120)?;
                    (vec![], Some(r.bytes[start..r.at].to_vec()))
                }
                _ => return Err("Unsupported SKATE rail representation".into()),
            };
            rails.push(Rail {
                name,
                closed,
                points,
                native,
            });
        }
        r.check_count(counts[6], 120)?;
        let mut doors = Vec::new();
        for _ in 0..counts[6] {
            let name = r.string()?;
            let frame = [
                r.floats()?,
                r.floats()?,
                r.floats()?,
                r.floats()?,
                r.floats()?,
                r.floats()?,
            ];
            let motion = r.floats()?;
            let response = if version >= 5 {
                Some(r.floats()?)
            } else {
                None
            };
            let friction = r.f()?;
            let restitution = r.f()?;
            let surface = r.u()?;
            let counts = [r.u()?, r.u()?, r.u()?];
            doors.push(Door {
                name,
                frame,
                motion,
                response,
                friction,
                restitution,
                surface,
                geometry: r.geometry(counts, materials.len(), version)?,
            });
        }
        r.check_count(counts[7], 64)?;
        let mut lights = Vec::new();
        for _ in 0..counts[7] {
            let light = Light {
                name: r.string()?,
                kind: r.u()?,
                position: r.floats()?,
                direction: r.floats()?,
                color: r.floats()?,
                intensity: r.f()?,
                range: r.f()?,
                radius: r.f()?,
                inner_cos: r.f()?,
                outer_cos: r.f()?,
            };
            if light.kind > 2
                || light.range <= 0.
                || light.intensity < 0.
                || light.radius < 0.
                || !(-1.0..=1.0).contains(&light.inner_cos)
                || !(-1.0..=1.0).contains(&light.outer_cos)
            {
                return Err("Invalid SKATE light".into());
            }
            lights.push(light);
        }
        r.check_count(counts[8], 24)?;
        let mut routes = Vec::new();
        for _ in 0..counts[8] {
            let name = r.string()?;
            let closed = r.u()? != 0;
            let skaters = r.u()?;
            let speed = r.f()?;
            let spacing = r.f()?;
            let points = r.points()?;
            routes.push(Route {
                rail: Rail {
                    name,
                    closed,
                    points,
                    native: None,
                },
                skaters,
                speed,
                spacing,
            });
        }
        let extension_started = Instant::now();
        let mut extensions = Vec::new();
        if version >= 12 {
            let n = r.u()?;
            r.check_count(n, 20)?;
            for _ in 0..n {
                let tag = r.take(4)?.try_into().unwrap();
                let schema = r.u()?;
                let bytes = r.u()? as usize;
                extensions.push(Extension {
                    tag,
                    schema,
                    payload: r.stored(bytes)?,
                });
            }
        }
        if require_collision
            && geometry.collision.is_empty()
            && !extensions
                .iter()
                .any(|e| e.tag == *b"RWCM" && e.schema == 1 && !e.payload.is_empty())
        {
            return Err("SKATE requires triangle collision or an embedded RWCM archive".into());
        }
        if r.at != data.len() {
            return Err(format!(
                "SKATE contains {} unexplained trailing bytes",
                data.len() - r.at
            ));
        }
        eprintln!(
            "MAP_DECODE_TIMING name={:?} textures_ms={} texture_workers={} geometry_ms={} extensions_ms={} total_ms={}",
            name,
            texture_time.as_millis(),
            texture_workers,
            geometry_time.as_millis(),
            extension_started.elapsed().as_millis(),
            parse_started.elapsed().as_millis()
        );
        Ok(Self {
            version,
            name,
            spawn,
            heading,
            environment,
            materials,
            textures,
            geometry,
            rails,
            doors,
            lights,
            routes,
            extensions,
        })
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn block_reserve_is_its_size_within_64x_its_input() {
        assert_eq!(super::reserve(40 << 20, 1 << 20), 40 << 20);
        assert_eq!(super::reserve(usize::MAX, 1 << 20), 64 << 20, "a corrupt size is capped");
        assert_eq!(super::reserve(usize::MAX, 10), 8 << 20, "never below the old cap");
    }
    #[test]
    fn parallel_vertex_parse_equals_one_pass_including_its_first_error() {
        for (version, stride) in [(12u8, 56usize), (9, 44)] {
            let mut bytes: Vec<u8> = (0..1000 * stride / 4).flat_map(|i| (i as f32 * 0.37).to_le_bytes()).collect();
            let serial = |b: &[u8]| super::Reader { bytes: b, at: 0 }.vertices(1000, version);
            let parallel = |b: &[u8]| super::parallel_vertices(b, 1000, version, 4, 7);
            assert_eq!(parallel(&bytes).unwrap(), serial(&bytes).unwrap());
            // Two bad floats in different runs: both report the earlier one.
            for at in [900 * stride + 8, 500 * stride + 4] {
                bytes[at..at + 4].copy_from_slice(&f32::NAN.to_le_bytes());
            }
            assert_eq!(parallel(&bytes).unwrap_err(), serial(&bytes).unwrap_err());
            assert_eq!(parallel(&bytes[1..]).unwrap_err(), serial(&bytes[1..]).unwrap_err());
        }
    }
}
