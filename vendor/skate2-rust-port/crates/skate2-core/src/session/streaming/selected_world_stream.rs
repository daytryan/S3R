//! Mounted outer stream composition over supplied owners and live settings.
use crate::{
    game::{
        WorldStreamBackend, WorldStreamControllerError, WorldStreamHandle,
        WorldStreamInvariantError, WorldStreamLoads, WorldStreamPoll, WorldStreamSpec,
    },
    input_cvar_runtime::InputCvarReferences,
    mounted_stream_backend::{MountedStreamBackend, MountedStreamBackendError},
    world_stream_adapter::{WorldStreamAdapter, WorldStreamAdapterError},
    world_stream_catalog::{CatalogError, WorldStreamCatalog},
    world_stream_runtime::{
        TextureStreamGate, TextureStreamGateError, WorldBoundsError, WorldBoundsPublication,
        WorldStreamEffects, WorldStreamRuntime, WorldStreamRuntimeError,
    },
};
use skate2_formats::mount::MountSet;
use std::sync::Arc;

pub(crate) struct LiveTextureGate(InputCvarReferences);
impl TextureStreamGate for LiveTextureGate {
    fn texture_stream_enabled(&mut self) -> Result<bool, TextureStreamGateError> {
        self.0.try_enable_texture_stream()
    }
}
type Adapter<E> = WorldStreamAdapter<
    MountedStreamBackend<WorldStreamCatalog>,
    WorldStreamRuntime<LiveTextureGate, E>,
>;
type StreamError =
    Arc<WorldStreamAdapterError<MountedStreamBackendError<CatalogError>, WorldStreamRuntimeError>>;
#[derive(Debug)]
pub(crate) enum Error {
    Mounted(MountedStreamBackendError<CatalogError>),
    Start(WorldStreamInvariantError),
    Poll(Arc<WorldStreamControllerError<StreamError>>),
    Bounds(WorldBoundsError),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Mounted(e) => write!(f, "mounted world stream: {e:?}"),
            Self::Start(e) => write!(f, "world stream start: {e}"),
            Self::Poll(e) => write!(f, "world stream poll: {e}"),
            Self::Bounds(e) => write!(f, "world stream bounds: {e}"),
        }
    }
}
/// Owns the existing stream controller, files and publication for their consumers.
/// The caller supplies the actual high-collection effects; none are invented here.
pub(crate) struct SelectedWorldStream<E: WorldStreamEffects> {
    pub(crate) catalog: WorldStreamCatalog,
    pub(crate) bounds: WorldBoundsPublication,
    adapter: Adapter<E>,
    loads: WorldStreamLoads<<Adapter<E> as WorldStreamBackend>::StreamFile, StreamError>,
    handle: WorldStreamHandle,
}
impl<E: WorldStreamEffects> SelectedWorldStream<E> {
    pub(crate) fn try_new(
        mounts: Arc<MountSet>,
        catalog: WorldStreamCatalog,
        cvars: InputCvarReferences,
        spec: WorldStreamSpec,
        effects: E,
    ) -> Result<Self, Error> {
        let inner = MountedStreamBackend::new(mounts, catalog.clone()).map_err(Error::Mounted)?;
        let runtime = WorldStreamRuntime::new(catalog.clone(), LiveTextureGate(cvars), effects);
        let bounds = runtime.world_bounds();
        let adapter = WorldStreamAdapter::new(inner, runtime);
        let mut loads = WorldStreamLoads::new();
        let handle = loads.start(spec).map_err(Error::Start)?;
        Ok(Self {
            catalog,
            bounds,
            adapter,
            loads,
            handle,
        })
    }
    pub(crate) fn state(&self) -> Result<u32, WorldStreamInvariantError> {
        self.loads
            .state(self.handle)
            .map(crate::game::WorldStreamOuterState::raw)
    }
    /// Performs exactly one existing outer-state step. Successful completion
    /// additionally requires the actual bounds publication, not the void callback.
    pub(crate) fn try_poll(&mut self) -> Result<WorldStreamPoll, Error> {
        let result = self
            .loads
            .poll(self.handle, &mut self.adapter)
            .map_err(Error::Poll)?;
        if result == WorldStreamPoll::CompletedThisPoll {
            self.bounds.read().map_err(Error::Bounds)?;
        }
        Ok(result)
    }
}
#[cfg(test)]
#[path = "selected_world_stream_tests.rs"]
mod tests;
