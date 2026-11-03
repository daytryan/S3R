# Local Bevy patch

## Bindless material resource reuse

`bevy_pbr/src/material_bind_groups.rs` chooses a fitting slab requiring the fewest
new resource slots instead of first-fit allocation. It prefers already resident
textures, then fuller slabs, while keeping existing allocation/refcount/free logic
and capacity limits. This adds work during material allocation, not each frame.
The game's explicit Vulkan shader probe tests reuse and retirement using real GPU
texture identities. See `docs/cpu-followup-optimizations.md` for scope and limits.

## Bindless slab free fixes

Two upstream bugs in the same file leaked bindless slab entries on every map switch:

- `get_bindless_index_table` used a reversed binary-search comparator. A material
  with two index tables (any `ExtendedMaterial` whose extension declares its own,
  such as the game's `PropMaterial`) then never released the base material's
  textures and samplers on `free`. The props slab grew by about 350 entries, with
  175 live wgpu textures and 174 samplers, on every University-to-NSV round trip.
- `insert_resources` counted a resource bound twice by one material as two
  allocations, while `free` released it once. The count crept up, and the slab
  chooser opened new slabs early.

The tests are `material_bind_groups::tests` (`cargo test -p bevy_pbr --lib`).

## Conservative occlusion depth pyramid

`bevy_core_pipeline` is vendored from crates.io 0.18.1 with its original licenses.
Its `src/experimental/mip_generation/{mod.rs,downsample_depth.wgsl}` backports
[Bevy #22603](https://github.com/bevyengine/bevy/pull/22603), commit
`43f4552308a29f4130c3ad71ea65d48f77daea6e`. The upstream file paths changed
between releases; the algorithm is the upstream fix. Power-of-two pyramid
levels and conservative source sampling prevent false occlusion at arbitrary
viewport sizes, including scaled targets and MSAA. The associated meshlet
texture-dimension fix is included in `bevy_pbr/src/meshlet/meshlet_cull_shared.wgsl`.
The game uses ordinary meshes, not meshlets.

The late mesh preprocessing pass in 0.18.1 also binds its indirect dispatch
buffer as writable storage, which fails wgpu validation when occlusion is on.
`bevy_pbr/src/render/{gpu_preprocess.rs,mesh_preprocess.wgsl}` now declares that
binding read-only in the late pass. Its counters are plain `u32` in that shader
variant, preserving the same buffer layout; the early producer retains atomic
counters and writable storage. No dispatch count or culling decision changes.

`bevy_pbr` is the unmodified crates.io 0.18.1 source except for the changes
described below. Its original MIT and Apache licenses are included. Cargo selects
it through the workspace `[patch.crates-io]` entry; the Cargo registry is untouched.

## Cached lightmap bindings

Bevy 0.18.1 rebuilds a bind group for every allocated lightmap slab and every
render phase on each frame, even when all bound GPU resources are unchanged.
On San Vanelona, mesh binding preparation measured roughly 10 ms per frame.

Local changes are restricted to:

- `src/lightmap/mod.rs`: each slab retains at most eight bindings. Lookup compares
  the actual wgpu buffer identity, binding offset and size, and layout identity.
  Texture insertion/removal clears the cache. FIFO eviction bounds retained old
  buffers when phases disappear or instance buffers resize.
- `src/render/mesh_bindings.rs`: reuse or create the binding through that cache.
  The lightmap slab parameter is now mutable.
- `src/lightmap/cache_tests.rs`: explicit GPU regression test.
- `src/render/mesh.rs`: retain complete lookup tables for both alternating
  instance buffers per phase. Keys include buffer identity, offset, size, layout,
  and the lightmap binding revision. A slab creation, texture upload, or removal
  changes that revision. Tables are shared by `Arc`, so a cache hit avoids
  rebuilding and cloning thousands of individual entries. Keep only two tables
  per phase, and discard caches for phases that disappear.
- `src/render/mesh.rs`: cache the IDs of meshes with morph targets when
  `RenderAssets<RenderMesh>` changes. Each phase still rebuilds its dynamic morph
  bindings from the current GPU resources, but skips scanning static city meshes.
  Since 10-12 (perf audit) a change updates that list incrementally: it drops IDs that no longer
  have morph targets and checks only the meshes prepared this frame (`ExtractedAssets::added`)
  plus the ones retried from last frame (`PrepareNextFrameAssets::ids`, a bevy_render accessor).
  The Skate 1 minimap rewrites up to 75 meshes per frame, which used to rescan every
  `RenderMesh` (9-18k on a city). Without those resources it falls back to the full scan.

Writing new instance data into an existing buffer does not require rebuilding a
bind group. Changing the buffer, bound range, layout, or texture does. The patch
does not skip extraction, uploads, animation, or visibility updates. It retains
the existing single-texture and binding-array rendering paths and shaders.

Run the Vulkan test explicitly:

```powershell
cargo test -p bevy_pbr --lib cache_tracks_binding_identity_and_texture_replacement --locked -- --ignored
```

It covers cloned versus replaced buffers, changed binding ranges/layouts, writes
to an existing buffer, texture insertion/removal, bounded eviction, and actual
bind-group construction/reuse, alternating-buffer table reuse, table revisions,
and morph-target additions/replacements/removals. Regular game tests and a city screenshot/benchmark
also exercise the patched renderer.

Upstream references reviewed September 6, 2026:

- [Bevy issue 23595](https://github.com/bevyengine/bevy/issues/23595) reports
  increasing `prepare_mesh_bind_groups` cost. That report points at mesh scanning;
  local measurements and source inspection identified lightmap binding recreation
  as the opportunity addressed here.
- [Bevy 0.19 rendering improvements](https://bevy.org/news/bevy-0-19/#render-big-scenes-faster)
  explain further GPU batching and CPU overhead reductions. An engine migration
  is separate from this small patch; 0.19.1 source still rebuilds lightmap groups.

## Serial specialization check

`src/material.rs` `check_entities_needing_specialization` iterates serially instead
of `par_iter`. It runs once per material type per frame and almost always finds
nothing changed (`AssetChanged` short-circuits when no asset changed), so the
parallel scope's task fan-out was the cost: the one-entity sky material measured
0.089 ms/frame in the 2026-09-29 University trace, more than the world material's
thousands of entities. The result list is the same.

`bevy_sprite_render` is vendored from crates.io 0.18.1 (original licenses
included) for the same one-function change in `src/mesh2d/material.rs`, the
Material2d version: the session marker effect, HUD and `ColorMaterial` checks
cost ~0.3 ms/frame together in that trace. Nothing else in it differs.

The same function also leaves `AssetChanged<Mesh3d>` out of its shared `Or` filter for entities with
`NoAutoAabb`, through a second query (perf 2026-10-10). The game puts that component, with a precomputed `Aabb`, on its
static map batches, whose mesh assets never change. Before this, any mesh write anywhere (Skate 1's minimap rewrites
its tiles every frame) made this check, and Bevy's `calculate_bounds`, look up every one of ~9k map batches: 0.57 ms
plus 0.30 ms a frame on Skate 1. `Changed<Mesh3d>` and material changes are still seen for those entities. An entity
matched by both queries is specialized twice in that frame, which is harmless.

## Prepass exclusion per view

`src/prepass/mod.rs` adds `PrepassExcluded`, an extracted per-camera set of main-world entities.
`queue_prepass_material_meshes` skips the entities in it before validating cached bins, so they are swept from the
prepass bins that frame and re-added once they leave the set. The main pass is unaffected: its opaque pipeline still
tests and writes depth (`GreaterEqual`) without relying on the prepass. The game fills the set with static retail-world
map batches more than 250 m away (`retail_shadow_visibility.rs` `cull_past_far`). Their shader never reads the prepass
textures, and the depth/normal prepass's only reader is SSAO. The game's post passes read the main depth texture. The
game leaves the set empty when `OcclusionCulling` is on. Facing the city from NSV or Skate 1, ~60% of prepass
triangles were past 400 m.

## Previous transforms only for motion vectors

`src/prepass/mod.rs` `update_mesh_previous_global_transforms` copied every mesh's `GlobalTransform` into its
`PreviousGlobalTransform` each frame whenever any 3D camera or shadow view was active (perf 10-18: 126 us a frame on a
city), and each changed copy made `extract_meshes_for_gpu_building` (`src/render/mesh.rs`, `Changed<PreviousGlobalTransform>`)
re-extract every mesh that had moved, a frame after its own extraction. The previous transform feeds only the prepass's
motion vectors (`prepass.wgsl` under `MOTION_VECTOR_PREPASS`, `mesh_preprocess.wgsl`'s previous input), so the copy now
runs only while an active view has `MotionVectorPrepass` (the game's DLSS camera, `dlss.rs`). Turning DLSS on, the first
frame inserts the missing components and refreshes the stale ones before that frame's extraction, as upstream does
for a newly spawned mesh. `SKATE_PREV_XFORM=1` restores the upstream condition.

## Parallel 3D transparent pass

`bevy_core_pipeline/src/core_3d/main_transparent_pass_3d_node.rs` records the sorted transparent phase in a command
buffer generation task, as the opaque pass already does, instead of on the render graph's own encoder (perf
2026-10-10). The draw walk and its wgpu `finish` (wgpu 27 encodes passes there) leave the serial render thread: ~1.9 ms
a frame at New San Vanelona's blend-heavy spots. Submission order is unchanged: a task keeps its queue slot.

## Depth passes that never read the material

`src/material.rs` adds `Material::depth_passes_skip_material` (default `false`) and
`MaterialProperties::depth_pass_bind_group`. The prepass (`src/prepass/mod.rs`) and shadow (`src/render/light.rs`)
queues key a material's batch sets by `None` instead of its slab when the flag is set, so its slabs share batch sets
there and the passes stop rebinding and re-tracking a slab per batch set (perf 2026-10-10). Binding is unchanged: the
bound group is still a valid slab of the same layout, just not read. The game sets it for retail world materials
without an alpha test, whose depth shader is compiled with `RETAIL_NO_ALPHA_TEST` and reads no material
(`retail_render.rs`).

When upgrading Bevy, reassess this patch and rerun the GPU test, stationary and
moving-camera benchmarks, and visual comparison. Do not assume an unchanged
private renderer implementation across engine versions.

## Public skybox prepass components (DLSS)

`bevy_core_pipeline/src/lib.rs` declares `pub mod skybox` instead of `mod skybox`. Nothing else changes. With the `dlss`
feature, the game's `dlss.rs` attaches `skybox::prepass::{RenderSkyboxPrepassPipeline, SkyboxPrepassBindGroup}` to DLSS
views. The prepass node then draws its fullscreen camera motion-vector pass for the sky, which the retail sky material
leaves out of the prepass. Builds without the feature never use it.

## Mesh slab tail shrink

`bevy_render` is vendored from crates.io 0.18.1 (original licenses included) for one change in
`src/mesh/allocator.rs` (leak audit, item 29). Bevy frees a general mesh slab only once it is wholly empty and never
shrinks one. After New San Vanelona grew the shared slab to 512 MiB, University's 2844 meshes reused its first 113
MiB and kept the whole buffer alive: about 450 MB of VRAM on University for the rest of the session.

`GeneralSlab::shrink_if_mostly_empty` runs after the frame's frees. Once a quarter of a slab has been freed since its
last look, on the first quiet frame after (under 1/64 of the slab freed in it), it finds the end of the highest live
allocation. When the slab is over twice that end times the growth factor, and the saving is at least 16 MiB, the slab
shrinks to that size. The buffer is reallocated the way growth does it, copying the live prefix. Allocations keep
their offsets, so cached slices, `MeshInputUniform`s and draw commands stay valid. A look that finds the top still in
use tries again every 120 frames. Slab sizes while a map is loaded are unchanged, and the next big map regrows the
slab as on a first visit.

To let the crate's tests run in this workspace, its `proptest` dev-dependency is dropped and the one property test
that used it (`src/render_phase/mod.rs`, `ReverseFixedBitSetZeroesIterator`) is compiled out.

Tests: `cargo test -p bevy_render --lib shrink_tests`.

## Dirty-range mesh input upload

`src/batching/gpu_preprocessing.rs` (perf audit 10-12): `InstanceInputUniformBuffer` keeps every mesh instance's
`MeshInputUniform` across frames and Bevy uploaded the whole buffer each frame (about 2 MB on a city, 0.1-0.15 ms
of `write_batched_instance_buffers` plus the copy). The buffer now records the slots written through `add`, `set`
and `ensure_nonempty` and `write_buffer` uploads only those, merged into runs (gaps under 16 slots are bridged).
The whole buffer still goes up after `clear`, on first use, when the GPU buffer was reallocated, or once over half
the slots are dirty. The GPU copy ends up byte-identical to the CPU one, as before: `remove` only frees a slot
without changing its bytes, and every write goes through the recorded methods (a buffer swap is caught by id). `src/render_asset.rs` adds `PrepareNextFrameAssets::ids`, read by the bevy_pbr morph
cache above.

Tests: `cargo test -p bevy_render --lib written_slots_upload_in_runs`.

## DLSS model choice

`dlss_wgpu` is vendored from crates.io 2.0.0 with its original licenses (no NVIDIA SDK files; the SDK stays outside
the repo, see docs/dlss.md). `src/super_resolution.rs` adds `RENDER_PRESET`, a render preset hint applied to every
quality mode when a DLSS context is created. DLSS 310 defaults DLAA/Quality/Balanced/Performance to preset K (the
transformer model, "higher performance cost"), which ran slower than DLSS off on an RTX 2050 laptop (10-11:
Quality K 136 FPS, off 140-146, preset F 174-176 on University). The game sets F (CNN) on RTX 20/30 and K on
RTX 40/50 and later (crates/skate-game/src/dlss.rs).

`build.rs` (`ENGINE PATCH (S3R 10-29)`) picks NVIDIA's static-CRT library `nvsdk_ngx_s` from Cargo's
`CARGO_CFG_TARGET_FEATURE` instead of the build script's own `cfg(target_feature)`. That cfg is the host's: with
`--target`, scripts/Build-Release.ps1's `+crt-static` reaches only the exe, so upstream always linked the `/MD` library
and the v11 release link failed on the dynamic CRT's `__imp_` symbols (`wcsstr`, `_wcsdup`, `wcstok`, `fputs`).

`src/nvsdk_ngx.rs` `check_ngx_result` (`ENGINE PATCH (S3R 10-29)`) matches the result codes under the names bindgen
generates (`NVSDK_NGX_Result_NVSDK_NGX_Result_*`). Upstream's unprefixed names were not constants, so its first arm bound
every code: each NGX failure counted as success, and v11 crashed at start on an AMD card (Proton crash report 10-29:
`NVSDK_NGX_Parameter_GetI` on the null parameters a failed `GetCapabilityParameters` left). Generic and unknown codes are
now `DlssError::Other`, and `#[deny(unreachable_patterns, unused_variables)]` turns a misspelt code into a compile error.
`src/initialization.rs` `register_device_extensions` also reports DLSS unsupported on any GPU whose Vulkan vendor id is not
NVIDIA's (0x10DE), so Bevy never starts NGX there.

## Mailbox never falls back to tearing

`bevy_render/src/view/window/mod.rs` `present_mode`: a requested `PresentMode::Mailbox` (the game's V-Sync "Fast")
falls back to FIFO where the surface lacks Mailbox (often AMD and Intel on Vulkan), not to Immediate, which tears.

## Frame wait timings

`bevy_render/src/pipelined_rendering.rs` `RENDER_WAIT_NANOS` (the main thread's wait in `renderer_extract` for the
render thread) and `view/window/mod.rs` `ACQUIRE_WAIT_NANOS` (the last `get_current_texture` call) record how long
each frame sat idle on the renderer. The game's frame pacer (crates/skate-game/src/graphics_menu.rs `just_in_time`)
sleeps about that long before it reads input instead, so input is that much fresher at the same frame rate. Nothing
else changes; without the game reading them they are two stores a frame.

## Batch sets bound by material slab first

`bevy_core_pipeline/src/core_3d/mod.rs` (`Opaque3dBatchSetKey`), `bevy_core_pipeline/src/prepass/mod.rs`
(`OpaqueNoLightmap3dBatchSetKey`) and `bevy_pbr/src/render/light.rs` (`ShadowBatchSetKey`) replace the derived `Ord`
(pipeline first) with a hand-written one that compares the material bind group index (the bindless slab) before the
pipeline and draw function, then the remaining fields as before (perf 10-18). Binned phases sort their batch sets by
this key, and the render pass skips rebinding an unchanged bind group, so each slab is now bound once per pass instead
of once per pipeline that draws from it; wgpu re-validates every slot of a slab on each bind. `Eq`/`Hash` are
unchanged, so batching is the same and only the order of opaque, alpha-mask, prepass and shadow batch sets changes
(sorted transparent phases are untouched). Opaque output is order-independent except where two surfaces of different
pipelines or slabs are exactly coplanar; that order was already an arbitrary function of pipeline and slab ids.
Slab indices are per material type, so different material types' slabs with the same index sort next to each other.
`core_3d::BATCH_SETS_BY_BIND_GROUP` (set once at startup; the game's `SKATE_BG_FIRST=0`) restores the upstream order.

Test: `batch_sets_sort_by_material_slab_before_pipeline` (game, `retail_render.rs`).

## Runtime bindless slab size

`bevy_render_macros` is vendored from crates.io 0.18.1 (original licenses included) for one change in
`src/as_bind_group.rs`: `#[bindless(limit(..))]` takes any expression instead of an integer literal (perf 10-18). The
derive already used the value only at runtime (layout `count`, `bindless_supported`, `bindless_slot_count`), so a call
works as a literal did; it is pasted unparenthesized, so use a literal or a call. The game's `RetailWorldMaterial` uses
`limit(bindless_slab())`, which reads `SKATE_BINDLESS_SLAB` once (default 256), so 256/512/1024 can be compared in one
build. Every other material's expansion is token-for-token the same.

## One-pass view visibility and far-plane culling

`bevy_camera` is vendored from crates.io 0.18.1 (original licenses included) for `src/visibility/mod.rs`
`check_visibility` (perf 10-18):

- Upstream runs one parallel walk over every visibility-checked entity per active camera (~17.6k entities on a city,
  ~0.45 ms per camera with the game's HUD and presentation cameras). It now walks the entities once and tests every
  active view inside the loop, with one queue per view in each thread's local storage, then drains them into each
  view's `VisibleEntities` as before. The per-view test (layers, visibility range, sphere then OBB frustum test) and
  the `ViewVisibility` marking are unchanged; an entity's world bounds are computed once instead of once per view.
  `visibility::VISIBILITY_ONE_PASS` (the game's `SKATE_VIS_ONEPASS=0`) restores the per-view walks.
- `visibility::FarPlaneCulled` marks entities that are also tested against each view's far plane
  (`intersects_obb(.., true, true)`); upstream never tests it. The game puts it on its static map batches (required
  by `StaticShadowCaster`, never on backdrops, sky, props or skaters), which `retail_shadow_visibility.rs`
  `cull_past_far` used to trim from each camera's list afterwards. That trim stays, now on the shorter list: it cuts by
  distance from the eye, which also drops batches in the far corners that the plane keeps, so the drawn set is
  unchanged. Those batches are now no longer `ViewVisibility`-visible (nor extracted) when no view or shadow cascade
  sees them. `visibility::FAR_PLANE_CULLING` (the game's `SKATE_FAR_CULL=0`) ignores the marker.

Test: `cargo test -p bevy_camera --lib one_pass_visibility_matches_the_per_view_walks` compares both walks' per-view
lists and `ViewVisibility` on a synthetic world (several cameras with different layers, an inactive one, one without
CPU culling; entities hidden, layered, unbounded, unculled, far-plane marked) and checks the far plane.

## Optional Rg11b10Ufloat HDR target (perf 10-19)

`bevy_render/src/view/mod.rs` adds `ViewTarget::hdr_format()`, fixed once in `renderer/mod.rs` right after the
device is created: `Rgba16Float` (the `TEXTURE_FORMAT_HDR` constant, unchanged) unless the game's
`SKATE_HDR_RG11B10=1` is set and the device has `RG11B10UFLOAT_RENDERABLE`. `prepare_view_targets` makes HDR main
textures (and their MSAA textures) in that format. So that every crate drawing into them agrees (FXAA, SMAA, CAS,
UI, gizmos, `bevy_pbr`, the game's tone pass all name the constant), `render_resource/pipeline_cache.rs`
`retarget_hdr` builds each render pipeline's colour targets of `TEXTURE_FORMAT_HDR` for `hdr_format()`. A render
pipeline that targets an `Rgba16Float` texture other than a main texture would then mismatch it; the game has none
(`bevy_pbr`'s atmosphere sky, unused, targets the main texture too). Off by default: the format has no alpha, 6/6/5-bit
mantissas and no negatives, so gradients can band. The game's passes read only the scene's RGB (its tone pass
writes alpha 1, which a format without alpha samples as). DLSS with this format is untested.

Test: the game's `retail_render::exposure::tests::hdr_targets_follow_the_chosen_format`.

## Opaque pass leaves the MSAA resolve to the transparent pass (perf 10-19)

With MSAA every pass that takes `ViewTarget::get_color_attachment()` resolves the whole multisampled main texture
when it ends, so a frame with blended draws resolved it twice. `bevy_render` adds
`ColorAttachment::get_attachment_unresolved()` and `ViewTarget::get_color_attachment_unresolved()` (the same
attachment, no resolve target; the samples are stored as before). `bevy_core_pipeline`'s
`core_3d/main_opaque_pass_3d_node.rs` takes it when the view's transparent phase has items (so its pass runs and
resolves) and its transmissive phase has none (that pass copies the resolved texture). No node between the opaque
and transparent passes reads the resolved texture in the game's graph (its own nodes run after tonemapping), and a
resolve rewrites every pixel from the samples, so the frame is the same. One full-screen resolve less: at 1080p 4x
`Rgba16Float`, 66 MB read and 17 MB written. The game's `SKATE_LATE_RESOLVE=0` resolves after both passes again.

## The prepass depth copy only when something reads it (perf 11-01)

`bevy_core_pipeline`'s `prepass/node.rs` copies the view depth into `ViewPrepassTextures::depth` after every
prepass. That copy is sampled by SSAO (which needs the normal prepass), DLSS, TAA and motion blur (motion vectors),
deferred lighting (its own node), transmission, forward decals and SSR. The game uses none of the last four, and its
own passes and occlusion culling's depth pyramid read `ViewDepthTexture`. So a view without normal or motion-vector
prepass textures skips the copy: the game's default settings (depth prepass, MSAA 4x) copied 33 MB of 1080p depth
a frame for nothing. The game's `SKATE_SKIP_DEPTH_COPY=0` copies always.

Test: the game's `retail_render::shadow_visibility::tests::nothing_else_reads_the_prepass_depth_copy` (no game
source or shader names a reader of the copy).

## Lost exclusive full-screen mode rebuilds the swapchain (crash report 10-26)

`wgpu-hal` is vendored from crates.io 27.0.4 (original licenses included) for `src/vulkan/{mod.rs,instance.rs}`, the
lines marked `ENGINE PATCH (S3R 10-26)`. Upstream turns any result it doesn't expect from `vkAcquireNextImageKHR` or
`vkQueuePresentKHR` into a device error, and wgpu-core then loses the whole device. Drivers return
`VK_ERROR_FULL_SCREEN_EXCLUSIVE_MODE_LOST_EXT` from those calls even to programs that never enabled
`VK_EXT_full_screen_exclusive`, and a new swapchain recovers
([gfxstream](https://android.googlesource.com/device/generic/vulkan-cereal/+/37cfd9ff%5E%21/), b/217229121). It is
the prime suspect for the 10-26 Sentry report (AMD RX 6700S, exclusive Fullscreen, device lost 1.3 s after start-up,
when the window switches to Fullscreen), unconfirmed: that report has no error code.

That result is now reported as `Outdated`. From an acquire, Bevy's `prepare_windows` reconfigures the surface (a new
swapchain) and acquires again. From a present, that frame is dropped and the next acquire returns `Outdated`. After
300 in a row with no present getting through (a few seconds), the device is lost as upstream does, so the game's crash
report still names it (`gpu_lost`). Counting tries rather than time keeps a long alt-tab, when nothing is presented,
from using up the allowance. No other result changes, and a driver that never returns it sees no difference.

Everything else, `Cargo.toml` included, is the crates.io source. No test drives the new arms: that takes a driver
that returns the error. The check is that the game builds with them.

## A slow swapchain is waited out, not a lost device (audit 10-30)

Two more ways the 10-26 report's start-up switch to Fullscreen could end the game, lines marked
`ENGINE PATCH (S3R 10-30)`:

- `wgpu-hal/src/vulkan/instance.rs` `acquire_texture`: after `vkAcquireNextImageKHR`, upstream waits on the image's
  fence with wgpu-core's 1 s frame timeout and turns a timeout into `Unexpected`, which loses the whole device. A
  display mode change can hold the presentation engine longer, and the image is already acquired, so
  `wait_through_timeouts` waits on through up to 10 timeouts; past that the timeout goes on as before.
- `wgpu-hal/src/vulkan/mod.rs` `get_unexpected_err` logs the Vulkan result, so the next report of a device lost this
  way names it (the 10-26 one said only "Unexpected").
- `bevy_render/src/view/window/mod.rs` `prepare_windows`: a `SurfaceError::Timeout` from `get_current_texture`
  (an acquire past 1 s) skips the frame on every system. Upstream forgives it only on Linux, for Mesa's spurious
  timeouts, and panics elsewhere; the Windows exe runs on Mesa too under Proton.

Which of these (or the exclusive-mode loss above) the 10-26 crash was is still unconfirmed. `wait_through_timeouts`
is checked by a scratch harness that compiles it alone; the other two arms need a driver that times out.

## The chosen output device (voice chat, user 10-26)

`bevy_audio` is vendored from crates.io 0.18.1 with its original licenses. Settings > Audio's Output Device names the
device all game sound and voices play through. Upstream `AudioOutput::default` opens only the system default
(`OutputStream::try_default`), once, when the plugin builds. `src/audio_output.rs` adds `set_output_device` (a
`OnceLock` the game sets from its saved settings before `DefaultPlugins`, re-exported from `lib.rs`) and opens that
device with `OutputStream::try_from_device` when it is plugged in and opens; anything else falls back to the default as
before. The setting therefore applies on the next start, as its help line says.

Everything else, `Cargo.toml` included, is the crates.io source. The test is
`audio_output::output_device_tests` (`cargo test --release -p bevy_audio --lib`); opening a real device has no test.
