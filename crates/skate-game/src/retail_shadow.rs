//! Shared frame state: shadow floor, animation clock, authored ocean PCA and weather.
use bevy::{
    prelude::*,
    render::{
        Render, RenderApp, RenderSystems,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_asset::RenderAssets,
        render_resource::BufferUsages,
        renderer::RenderQueue,
        storage::{GpuShaderStorageBuffer, ShaderStorageBuffer},
    },
};

pub(super) const BUFFER: Handle<ShaderStorageBuffer> =
    bevy::asset::uuid_handle!("cd736f89-4882-4a5d-8fcb-32273beeaaf4");

#[derive(Resource, Clone, ExtractResource)]
/// Weather is rain strength, lightning flash, bolt azimuth and bolt seed; snow is
/// cover and snowfall; then the map's authored fog ramp and colour (AuthoredFog);
/// then the time of day (retail_daynight.rs: light, sky, glow, sun).
pub(crate) struct ShadowState(pub Vec4, pub Vec4, pub [Vec4; 7], pub Vec4, pub Vec4, pub Vec4, pub Vec4, pub [Vec4; 4]);
impl Default for ShadowState {
    fn default() -> Self {
        Self(Vec4::ZERO, Vec4::ZERO, [Vec4::ZERO; 7], Vec4::ZERO, Vec4::ZERO, Vec4::ZERO, Vec4::ZERO, NOON)
    }
}
/// Time of day at noon: the authored look, unchanged.
pub(crate) const NOON: [Vec4; 4] = [Vec4::ONE, Vec4::new(1., 1., 1., 0.), Vec4::ZERO, Vec4::ZERO];

/// The map's authored distance fog, as the retail world shader applies it
/// (ramp, colour with the exposure baseline folded in). PBR fallback surfaces
/// (glass, terrain, props) read it from the frame state so they fog with the world.
#[derive(Resource, Clone, Copy, PartialEq)]
pub(crate) struct AuthoredFog { pub ramp: Vec4, pub colour: Vec4 }
impl Default for AuthoredFog {
    /// No fog: a zero ramp and no colour.
    fn default() -> Self { Self { ramp: Vec4::new(0., 0., 1., 0.), colour: Vec4::ZERO } }
}

impl ShadowState {
    pub(crate) fn approach(&mut self, target: Vec3, dt: f32) {
        let target = target.clamp(Vec3::ZERO, Vec3::ONE);
        let value = if self.0.w == 0. {
            target
        } else {
            // Adapter smoothing, not a recovered native constant. Cap a hitch's
            // contribution so one long frame cannot cause a darkness step.
            self.0
                .truncate()
                .lerp(target, 1. - (-dt.clamp(0., 0.05) / 0.35).exp())
        };
        self.0 = value.extend(1.);
    }
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<ShadowState>()
        .init_resource::<AuthoredFog>()
        .add_plugins(ExtractResourcePlugin::<ShadowState>::default())
        .add_systems(Startup, (initialize, load_pca))
        .add_systems(Update, clock);
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render.add_systems(Render, upload.in_set(RenderSystems::PrepareResources));
    }
}

fn initialize(mut buffers: ResMut<Assets<ShaderStorageBuffer>>) {
    let mut values = [Vec4::ZERO; 17];
    values[13..].copy_from_slice(&NOON);
    let mut buffer = ShaderStorageBuffer::from(values);
    buffer.buffer_description.usage |= BufferUsages::COPY_DST;
    buffers
        .insert(BUFFER.id(), buffer)
        .expect("reserved shadow state asset");
}

fn upload(
    state: Res<ShadowState>,
    buffers: Res<RenderAssets<GpuShaderStorageBuffer>>,
    queue: Res<RenderQueue>,
) {
    if let Some(buffer) = buffers.get(BUFFER.id()) {
        let values = [state.0, state.1]
            .into_iter()
            .chain(state.2)
            .chain([state.3, state.4, state.5, state.6])
            .chain(state.7)
            .flat_map(|v| v.to_array())
            .flat_map(f32::to_le_bytes);
        let mut bytes = [0u8; 272];
        for (destination, value) in bytes.iter_mut().zip(values) {
            *destination = value;
        }
        // Keep the buffer and all material bind groups alive; upload 272 frame bytes.
        queue.write_buffer(&buffer.buffer, 0, &bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn probe_transition_is_gradual_and_hitches_are_bounded() {
        let mut state = ShadowState::default();
        state.approach(Vec3::splat(0.1), 0.016);
        assert_eq!(state.0, Vec3::splat(0.1).extend(1.));
        state.approach(Vec3::splat(0.3), 0.016);
        assert!(state.0.x > 0.1 && state.0.x < 0.12);
        let mut hitch = state.clone();
        state.approach(Vec3::splat(0.3), 0.05);
        hitch.approach(Vec3::splat(0.3), 10.);
        assert_eq!(state.0, hitch.0);
        for _ in 0..240 {
            state.approach(Vec3::splat(0.3), 1. / 60.);
        }
        assert!((state.0.x - 0.3).abs() < 0.0001);
    }

    /// 82790858 at 20 fps: one PCA frame per update once 1/30 s has gathered, though 0.05 s is one and a half frames.
    #[test]
    fn the_ocean_steps_one_frame_per_update_like_82790858() {
        use bevy::ecs::system::RunSystemOnce;
        let frames: Vec<[[f32; 4]; 7]> = (0..30).map(|f| [[f as f32; 4]; 7]).collect();
        let mut world = World::new();
        world.insert_resource(serde_json::from_value::<OceanPca>(serde_json::json!({"hz": 30., "frames": frames})).unwrap());
        world.init_resource::<ShadowState>();
        world.init_resource::<AuthoredFog>();
        world.init_resource::<Time>();
        let mut shown = vec![];
        for _ in 0..4 {
            world.resource_mut::<Time>().advance_by(std::time::Duration::from_millis(50));
            world.run_system_once(clock).unwrap();
            shown.push(world.resource::<ShadowState>().2[0].x);
        }
        assert_eq!(shown, [1., 2., 3., 4.]);
    }

    /// TU3's tables from the installed image piece, each float times 82140F5C's f32 1/255 as 82790858 does, in
    /// frame-state order (mean X/Z/Y, weight pairs R/B/G). The digest is numpy's float32 result for the same bytes.
    #[test]
    #[ignore = "requires SKATE3_ASSET_ROOT (install assets, read-only)"]
    fn the_ocean_pca_comes_from_the_installed_tu3_image() {
        use sha2::{Digest, Sha256};
        let root = std::path::PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap());
        assert!(pca_available(&root), "no ocean PCA under {}", root.display());
        let frames = read_pca(&root).unwrap().frames;
        let bytes: Vec<u8> = frames.iter().flatten().flatten().flat_map(|v| v.to_le_bytes()).collect();
        assert_eq!(
            format!("{:x}", Sha256::digest(&bytes)),
            "ca1578c40e7ee69b3f11ff3141062ed6321fe2899a2f6538ff734926070b2e59"
        );
    }

    /// Another image of the same size keeps the fallback, without a panic, rather than drawing wrong water.
    #[test]
    fn another_image_keeps_the_water_fallback() {
        let root = std::env::temp_dir().join(format!("sk8-ocean-pca-{}", std::process::id()));
        let piece = root.join("private/stock/audio-runtime-image/g_8300.bin");
        std::fs::create_dir_all(piece.parent().unwrap()).unwrap();
        std::fs::write(&piece, vec![0u8; 1_769_472]).unwrap();
        assert!(read_pca(&root).is_none());
        let _ = std::fs::remove_dir_all(root);
    }
}

#[derive(Resource, Clone, serde::Deserialize)]
struct OceanPca {
    hz: f32,
    frames: Vec<[[f32; 4]; 7]>,
    /// 82790858's state: time gathered towards the next frame (+0x144) and the u16 frame counter (+0x140).
    #[serde(skip)]
    gathered: f32,
    #[serde(skip)]
    counter: u16,
}
impl OceanPca {
    /// cPCAWaterAnimationData's update (TU3 82790858): at most one frame per update, once more than 1/30 s
    /// (8232BA4C) has gathered; the frame shown is the counter modulo the frame count.
    fn advance(&mut self, dt: f32) -> usize {
        self.gathered += dt;
        if self.gathered > 1. / 30. {
            self.gathered -= 1. / 30.;
            self.counter = self.counter.wrapping_add(1);
        }
        usize::from(self.counter) % self.frames.len()
    }
}

/// TU3's PCA tables (Init 827905B0): 30 means of 3 floats at 0x830118D8, then 30 sets of 24 weights, big-endian,
/// 0x118D8 into the installed image piece that maps 0x83000000. Only this slice is read; its hash names the build.
const PCA_PIECE: &str = "private/stock/audio-runtime-image/g_8300.bin";
const PCA_PIECE_SIZE: u64 = 1_769_472;
const PCA_SLICE_SHA256: &str = "6febf907c06ae35dc1d7d6b366bd0fab56783cb52bcdc8bc8dd67163136a162a";
fn image_pca(root: &std::path::Path) -> Result<OceanPca, String> {
    use sha2::{Digest, Sha256};
    use std::io::{Read, Seek, SeekFrom};
    let path = root.join(PCA_PIECE);
    let mut file = std::fs::File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if file.metadata().map_err(|e| e.to_string())?.len() != PCA_PIECE_SIZE {
        return Err(format!("{} is not TU3's", path.display()));
    }
    let mut table = [0u8; 3240];
    file.seek(SeekFrom::Start(0x118D8))
        .and_then(|_| file.read_exact(&mut table))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if format!("{:x}", Sha256::digest(table)) != PCA_SLICE_SHA256 {
        return Err(format!("{} holds other PCA tables", path.display()));
    }
    // 82790858 scales by the f32 1/255 at 82140F5C; frame-state order is mean X/Z/Y, then weight pairs R/B/G.
    let at = |i: usize| f32::from_be_bytes(table[4 * i..4 * i + 4].try_into().unwrap()) * (1. / 255.);
    let frames = (0..30)
        .map(|f| {
            let pair = |s: usize| -> [f32; 4] { std::array::from_fn(|k| at(90 + 24 * f + s + k)) };
            [[at(3 * f), at(3 * f + 2), at(3 * f + 1), 0.], pair(0), pair(4), pair(16), pair(20), pair(8), pair(12)]
        })
        .collect();
    Ok(OceanPca { hz: 30., frames, gathered: 0., counter: 0 })
}
fn read_pca(root: &std::path::Path) -> Option<OceanPca> {
    let pca = match image_pca(root) {
        Ok(pca) => pca,
        // Else ocean_pca.py's sidecar, for an install that has one.
        Err(why) => match std::fs::read(root.join("private/ocean-pca.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<OceanPca>(&b).ok())
        {
            Some(pca) => pca,
            None => {
                warn!("RETAIL_OCEAN: no ocean PCA ({why}); ocean and still water keep the fallback material");
                return None;
            }
        },
    };
    (pca.hz == 30.
        && pca.frames.len() == 30
        && pca.frames.iter().flatten().flatten().all(|v| v.is_finite()))
    .then_some(pca)
}
/// One read per asset root and run, however many maps load.
fn pca(root: &std::path::Path) -> Option<OceanPca> {
    static READ: std::sync::Mutex<Vec<(std::path::PathBuf, Option<OceanPca>)>> = std::sync::Mutex::new(Vec::new());
    let mut read = READ.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, pca)) = read.iter().find(|(r, _)| r == root) {
        return pca.clone();
    }
    let pca = read_pca(root);
    read.push((root.to_path_buf(), pca.clone()));
    pca
}
pub(super) fn pca_available(root: &std::path::Path) -> bool {
    pca(root).is_some()
}
fn load_pca(mut commands: Commands, config: Res<crate::config::Config>) {
    if let Some(pca) = pca(&config.asset_root) {
        info!("RETAIL_OCEAN: loaded 30 authored PCA frames");
        commands.insert_resource(pca);
    }
}
fn clock(mut state: ResMut<ShadowState>, time: Res<Time>, pca: Option<ResMut<OceanPca>>, fog: Res<AuthoredFog>) {
    state.1.x = time.elapsed_secs();
    (state.5, state.6) = (fog.ramp, fog.colour);
    if let Some(mut pca) = pca {
        let frame = pca.advance(time.delta_secs());
        state.2 = pca.frames[frame].map(Vec4::from_array);
        state.1.y = 1.;
    }
}
