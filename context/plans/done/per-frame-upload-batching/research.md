# per-frame-upload-batching — research

Derivation and evidence behind the brief. Source was read by symbol at 832e20c8a (`main`). The measurements were taken at 97cf912ec; no crate source changed between the two. Paths are relative to `crates/renderer/src/` unless they name another crate.

## Measurement conditions

| Item | Value |
|---|---|
| Machine | Compatibility-floor Mac: MacBook Pro 2019, Radeon Pro 5300M 4 GB, Metal. No `TIMESTAMP_QUERY`, so GPU pass timing is unavailable here. |
| Binary | Cargo release build of the engine. |
| Render scale | Auto, 1280×720 logical window. |
| Metric | `[CpuTiming]` window medians (`POSTRETRO_CPU_TIMING=1 RUST_LOG=info`), untraced: no `tracy` feature, no profiler attached. A separate wall-clock `sample` profile attributes time inside `render_submit` and `render_record`. |
| Machine state | Record idle VRAM and GPU utilization first (`rendering_pipeline.md` §12, Machine-state confounders). Take before and after numbers under the same state. |
| Maps | `content/dev/maps/stress-warren-hallway-inspection.prl`, `content/dev/maps/campaign-test.prl`. Record the pose; the baseline did not pin one, so re-take before numbers at the pose used for after numbers. |
| Date | 2026-10-01 |

## Baseline numbers

| Map | work | total | `render_submit` | `maintain` dropping temp resources | staging create plus memcpy inside `write_buffer` |
|---|---|---|---|---|---|
| stress-warren-hallway-inspection | 12.1 ms | 16.6 ms (vsync) | 6.3 ms | 3.09 ms | about 1.9 ms |
| campaign-test | 10.1 ms | — | 4.5 ms | 2.49 ms | about 1.6 ms |

- Sampling inflates the `sample` profile figures by about 15%. Read them as shares, not absolutes.
- Kernel plus driver time is 64–69% of `render_submit`. The top self-time function is `mach_msg2_trap`, at 39%.
- Each drop runs `MTLIOAccelBuffer dealloc` → `ioAccelResourceFinalize` → `IOConnectCallMethod`, a synchronous kernel call. The profile shows this. Whether wgpu-hal's Metal buffer destroy makes one kernel call per buffer was not read in source.
- `render/sh_streaming/gpu/staged_uploads.rs` measured about 40 µs per write call on Metal when it was built (`plans/done/sh-streaming--warm-set-and-io-contract`, T4: install CPU 1208 ms → 223 ms).
- Reconciled and de-inflated (stress / campaign): `maintain` drops of `write_buffer` staging cost 2.46 / 1.97 ms inside `render_submit`. Staging creation under `queue.write_buffer`, outside `render_submit`, costs 1.58 / 1.41 ms. Together that is about 4.0 / 3.4 ms. The drops of wgpu's indirect-validation staging buffers (0.15 / 0.21 ms) are excluded: batching does not touch them.
- Method: bucket-proportional de-inflation, unverified. Each `render_submit` bucket is scaled by the untraced `render_submit` median over the sampled root (0.864 / 0.876); work outside it, by the `work` ratio (0.854 / 0.901). Trap-heavy buckets may inflate differently under `sample`. Validation's share of the drops is split off by its share of staging creations.
- The memcpy into fresh staging (about 1.0 ms raw on each map) survives batching, apart from first-touch page faults.

## Why wgpu pays per call

All in wgpu 29.0.1, the version `Cargo.lock` pins.

- `wgpu-core` `Queue::write_buffer` calls `StagingBuffer::new` on every call. `Queue::write_texture` does too.
- `wgpu` `Queue::write_buffer_with` goes through `create_staging_buffer`, which is `StagingBuffer::new` again. It saves only the CPU copy.
- The staging buffer joins the pending writes as `TempResource::StagingBuffer`. `Queue::submit` ends with `Device::maintain(Poll)`. That calls `triage_submissions`, which drops the temp resources of every completed submission.
- No device or queue option pools staging buffers.
- Upgrading wgpu is not a known fix. The transient-buffer pooling PR #10232 is Vulkan-only.
- `wgpu::util::StagingBelt` exists. It records `copy_buffer_to_buffer` into a caller's encoder at write time, and recycles chunks through `finish`, `recall` and `map_async`. It handles buffers only, not textures.

**Map callbacks fire inside submit.** `Queue::submit`'s `maintain` call also runs `handle_mapping` and fires the mapping closures once the submission that used a buffer has completed. So `StagingPool::recycle` gets its buffer back during a later `queue.submit`, with no added `device.poll`. Windowed surfaces configure `desired_maximum_frame_latency: 2` (`renderer_init.rs`). A frame's staging buffer is therefore free again about two to three submits later, and a pool of three or four buffers reaches steady state.

**Same-resource copies are ordered.** `COPY_DST` is an exclusive use in `wgpu-types`, and `skip_barrier` in `wgpu-core/src/track` skips a barrier only for ordered uses. Two copies into one buffer in one command buffer therefore get a transfer barrier between them, and the later copy wins on every backend. `StagedUploads` already relies on this.

- On Metal the barrier is not issued. wgpu-hal 29.0.1's Metal backend drops every buffer and texture barrier, so the order of copies into one resource rests on Metal's tracked hazards. Today's pending-writes copies already rely on the same thing. The Ordering rows, run on the compatibility-floor Mac, are the proof there.

## Existing seam

`render/sh_streaming/gpu/staged_uploads.rs` and `staging_pool.rs`, re-exported as `crate::render::{StagedUploads, StagingPool}`.

- `StagedUploads::write_buffer` and `write_texture` append bytes to one CPU vector and push a copy record. A buffer write that continues the previous copy in both source and target extends it. Alignment and bounds follow `Queue::write_buffer` rules.
- `StagedUploads::record` acquires one mapped buffer from the pool, copies the bytes in, unmaps, and records the copies in order into a caller's encoder. `RecordedUploads::finish` recycles the buffer after the caller submits. An empty batch acquires nothing.
- `StagingPool` picks the best-fit free buffer and recycles through `map_async`. New buffers are 1 MiB minimum, and the free list keeps at most 64 MiB.
- Users today: SH drain (`StreamingGpuPools::begin_uploads`, `submit_uploads`) and lightmap streaming (`lighting/lightmap/stream/execute.rs`). Each owns its own `StagingPool`.
- Gaps for per-frame use:
  - Errors are `ShResidencyDrainError`.
  - Each batch allocates a fresh `copies` vector.
  - Nothing deduplicates.
  - Merging works only for writes that are contiguous in both source and target. Interleaved per-instance and per-palette writes never merge.
  - Texture writes stage mip 0, aspect All and tightly packed rows only. That covers every per-frame need: at 832e20c8a every steady-state per-frame renderer write is a buffer write, and every renderer write_texture call runs at load or install time.

## Per-frame write inventory (steady state, without dev-tools)

The count per frame is an estimate of 40–100, made from source. No counter has measured it. Sites, by symbol:

| Site | Writes per frame | Gate |
|---|---|---|
| `Renderer::upload_light_bridge_snapshot` (`renderer_lighting.rs`) | lights, influence, plus up to two more | bridge dirty: every frame while animated lights run |
| fog uploads (`upload_fog_volumes`, `upload_fog_points`, fog params) | several | fog present |
| `Renderer::update_per_frame_uniforms` | full `uniform_buffer`, `write_dynamic_direct_params`, `upload_descriptors_if_dirty` | descriptors are dirty-gated |
| `update_viewmodel_view_projection` → `MeshPass::write_viewmodel_view_projection` | 1 | — |
| `update_dynamic_light_slots_with_capture_overrides` (`renderer_light_slots.rs`) | up to 7: lights (diff-gated against `last_lights_upload`), influence, `TOTAL_LIGHT_COUNT_OFFSET` patch, slot matrices, shadow VS uniforms, cube VS uniforms, promoted weights | the lights rewrite fires almost every frame: the bridge writes sentinel slots, the patch restores them |
| `CandidateCull::dispatch`, `ComputeCull::dispatch` | 3 and 2 | path-dependent |
| `MeshPass::plan_and_upload` | 2 per planned instance: instance entry plus bone palette | mesh instances present |
| `ShadowCull::dispatch_occupied_slots_filtered` | 1 per uncached spot slot or cube face | occupied slots |
| `MeshPass::write_light_params` | called twice with identical bytes (skinned pass, viewmodel pass) | plans present |
| `SmokePass::record_draws` | 1 per non-empty collection | particles present |
| `UiPass::encode` | uniform, plus 1 per quad batch and 1 per ring batch | — |
| glyphon (`UiTextRenderer::prepare_text_batches`) | 1 vertex write per text span, viewport params, atlas texture writes on new glyphs | internal to glyphon |
| `ScreenEffects::encode_resolve` | 1 | — |
| SDF params, animated-lightmap dispatch tiles, SH compose grid and indirection, kinematic brush instances and params | gated | feature or content present |

## Submit inventory

Every renderer `queue.submit`, outside tests:

| Site | When | Notes |
|---|---|---|
| `submit_windowed_frame` (`renderer_render_frame.rs`) | each drawn windowed frame | the frame's single scene command buffer |
| `StreamingGpuPools::submit_uploads` via `StagedUploads::submit`; `sh_streaming/gpu/growth.rs` (two) | SH drain, at `render_frame_indirect` entry, before acquire | drain-internal direct writes also exist: `compose_indirection`, sparse `row_pairs` zeroing |
| `lighting/lightmap/stream/execute.rs` | lightmap drain, which the binary calls before `render_frame_indirect` | |
| `capture_measurement_frame_indirect`, `capture_frame_indirect` (`renderer_capture.rs`) | capture | each calls `update_per_frame_uniforms` itself |
| `read_texture_rgba8` (`renderer_frame.rs`) | capture PNG readback | |
| `renderer_splash.rs` | splash frames | |
| `render_debug_ui` (dev-tools) | after the frame submit, before present | egui writes internally |
| `encode_sh_probe_readback` (dev-tools) | after the frame submit, only when the overlay is on | |

**Frame order in the binary** (`crates/postretro/src/main.rs`, gameplay branch): light bridge upload → fog uploads → `update_per_frame_uniforms` → `update_viewmodel_view_projection` → SH stream batch build → lightmap drain (submits) → mesh and mover draw lists → `render_frame_indirect` (SH drain, which submits; acquire; record; frame submit) → `render_debug_ui` → present. Every early return between `update_per_frame_uniforms` and present sets `exit_result` and exits the event loop. The only non-fatal skip is the acquire-failure `Ok(None)` inside `render_frame_indirect`.

**The drain boundary today.** A direct write recorded before a drain lands in that drain's submit, ahead of the drain's copies. Writes recorded after it land in the frame submit. If a per-frame batch stayed open across drains, the binary-side writes would land after the drain copies instead of before. That is safe only while no drain copies into a resource the batch writes, and nothing enforces it. Carrying the batch first in the frame, drain and capture submits keeps today's order exactly, and costs a second staging acquisition on frames that drain. Splash, readback and dev-tools submits follow no per-frame write, so they carry none.

## Why the batch is never dropped on a skipped frame

Several writers record a CPU belief that their bytes reached the GPU as soon as they call the queue:
- the `last_lights_upload` mirror behind the lights diff gate;
- the animated-descriptor `dirty` flag, cleared in `upload_descriptors_if_dirty`;
- the light bridge, which reports the snapshot committed.

Dropping a batch after any of those runs leaves the GPU stale until an unrelated change re-dirties the data. Today a skipped frame's direct writes stay pending in wgpu and land at the next submit. Unbounded carry-over in a renderer batch would grow every skipped frame, for example while the surface times out repeatedly. Submitting the batch alone on the skipped frame bounds it to one frame and keeps every mirror true.

## Ordering pins

Each pin is cited by an Acceptance row.

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| P1 | Lightmap drain fails and rolls back | binary-side writes staged → lightmap drain errors before its submit → SH drain → frame submit | The drain submits nothing. The staged writes land in the next submit that frame, once. None is dropped. |
| P2 | Submit with nothing pending | a drain, a dev-tools or readback submit, or an acquire-failure skip runs with an empty batch | No staging buffer is acquired. No batch command buffer joins the submit. A skip with an empty batch submits nothing. |
| P3 | Drain submits more than once | writes staged → SH drain grows a pool (submit) → drain uploads (submit) | The staged writes ride the first submit, ahead of the growth copies. Later submits in that drain, and the frame submit, carry none of them. |
| P4 | Two drains in one frame | W1 → lightmap drain submit → W2 → SH drain submit → W3 → frame submit | W1 rides the lightmap drain's submit, W2 the SH drain's, W3 the frame's. Each lands once. |
| P5 | Back-to-back submits, GPU behind | frame N acquires S and submits → frame N+1 acquires before N completes | N+1 gets a buffer other than S. S rejoins the pool only after N's submit completes, through its map callback. The batch path calls no `device.poll`. |
| P6 | Lifecycle event with a pending batch | last submit of frame N → frame tail, hot-reload commit, level install or unload → frame N+1 entry | The batch is empty at each. A debug build fails loudly if a write is pending there. |
| P7 | Batch-carrying submit | pending writes → a frame, drain or capture submit in §Submit inventory | The batch's command buffer is first in the submit. |
| P8 | Dev-tools setter mid-frame | per-frame writes staged → dev-tools panel setter writes → frame submit | Each setter's write is staged in the batch and counted. It lands before the frame's passes. |
| P9 | Acquire fails twice in a row | frame N stages, acquire fails, submits alone → frame N+1 stages, acquire fails, submits alone → frame N+2 draws | Each skip submits only its own frame's writes. The batch's CPU storage does not grow across skips. N+2 finds nothing pending. |
| P10 | Neither mesh pass draws | no world mesh plan and no viewmodel plan this frame | The mesh light parameters are not written. |
| P11 | Boot splash frame | full renderer absent → splash uniform write → splash submit | The splash submit carries no batch. Its uniform write stays direct, in the boot splash class. A debug build fails loudly if a write is pending there. |
| P12 | Staging rejects a write | a per-frame writer stages a misaligned or out-of-bounds write | It fails as loudly as a direct write does today. It never drops silently. A rejected bridge write never reports the snapshot committed. |
| P13 | Options-menu setter mid-session | settings change (for example fog step size) → setter writes → next frame's passes | The write is staged in the batch and counted. It lands before the next frame's passes. |

## Rivals considered

- **`wgpu::util::StagingBelt`.** Rejected, for three reasons:
  - It records a copy into an encoder at write time. But many per-frame writes come through `Renderer` methods the binary calls before any frame encoder exists: the light bridge, fog, per-frame uniforms, viewmodel.
  - It moves writes from the queue timeline to the encoder timeline. `ui.md` §5 relies on queue-timeline resolution: a draw recorded between overlapping writes does not snapshot the buffer. A deferred batch submitted first keeps that, and the build updates the sentence to name the batch.
  - It has no texture writes.

  It would also be a second mechanism beside `StagedUploads`, and it cannot take the writes `UiPass::encode` makes while its render pass is open.
- **Coalescing at each writer.** Each module dedups, merges or diff-gates its own writes. It is weaker as the main fix, because every remaining write still pays a staging allocation. It is the right home for dedup and merging, which is why the batch does neither and the brief lists them as follow-ups.
- **Batching only at record time.** Route only writes made while recording the scene and keep the binary-side writes direct. That means fewer submit sites change. But the uniform, bridge, fog and viewmodel writes keep paying per call. Direct and batched writes would also mix within one submit window, which is the ordering hazard the brief's direct-write rule forbids.
- **Upstream wgpu issue.** Filing an issue asking to pool Metal staging buffers is worth doing. No release has a fix, and the transient-buffer pooling PR #10232 covers Vulkan only. It is not a substitute.
- **`Queue::write_buffer_with`.** The same `StagingBuffer::new` per call; it saves only a memcpy.
- **Persistent per-resource mapped rings.** One ring per written resource is more machinery and VRAM bookkeeping than one recycled batch, for the same effect.
- **Upgrade or fork wgpu.** No released fix pools staging buffers on Metal.
