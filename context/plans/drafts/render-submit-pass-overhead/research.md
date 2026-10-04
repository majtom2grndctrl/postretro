# render-submit-pass-overhead — research

Derivation behind `index.md`. Source read at `a6a67bcce` (postretro-briefs). Profiles and timing windows are the recorded `release-indirect-validation` runtime set, taken 2026-10-01 at `dcde8f292`, which predates `visible-span-draws`. Written 2026-10-04.

## Question

`render_submit` is the largest CPU stage on the compatibility-floor Mac. `drafts/bvh-leaf-clustering/research.md` §Reconciled render_submit estimates ~1.7–1.9 ms remains after the four landed briefs, "nearly all per pass: chiefly the new `MTLCommandBuffer` and encoder each render pass creates". That was an estimate from a pre-batching profile. Is per-pass overhead real, how big, what drives it, and should a brief reduce it?

## What `render_submit` wraps

`RenderStage::Submit` (`crates/renderer/src/render/cpu_stages.rs`) scopes `Renderer::submit_windowed_frame` inside `render_frame_indirect` (`renderer_render_frame.rs`). It holds:

- `wgpu::CommandEncoder::finish` on the one frame encoder (`"Frame Encoder"`). In wgpu 29 every render and compute pass is recorded as a command and *replayed* here (`wgpu_core::command::CommandEncoder::finish` → `encode_render_pass` / `encode_compute_pass`). So pass replay, hal encoder creation, and the driver's per-draw work all land in this stage, not in `render_record`.
- `UploadQueue::submit` (`uploads/mod.rs`): records the frame's staged upload copies into a second encoder (`"Per-frame Upload Copies"`) when the batch is non-empty, then one raw `Queue::submit`.
- `wgpu_core::Global::queue_submit` → `Queue::submit` → `Device::maintain(Poll)` → `LifetimeTracker::triage_submissions`: drops the temp resources of completed submissions.
- `FrameTiming::post_submit` (`device.poll` + timestamp readback) only when GPU timestamps are supported; absent on this Mac. The `dev-tools` readbacks (`encode_sh_probe_readback`, `CandidateCull::post_submit`) are absent in the plain release.

Present is outside the stage: `PresentHandle::present` (`renderer_types.rs`) is called by the binary and timed as `wait_present` (`cpu_timing/frame_timer.rs`), ~0.03 ms in the logs.

## Passes per frame

Pass sites (non-test renderer source), from `begin_render_pass` / `begin_compute_pass` plus the dispatch helpers:

| Group | Passes | Gate |
|---|---|---|
| Pre-scene compute | camera cull (`CandidateCull::dispatch` or `ComputeCull::dispatch`), `ShadowCull::dispatch` (only when a slot needs world depth), `AnimatedLightmapResources::dispatch`, SH compose, direct SH compose, animated direct SH, billboard scatter compose, streamed compose | per `rendering_pipeline.md` §7.1; most are change-gated |
| Shadow depth | one pass per occupied spot slot, one per occupied cube face; promoted slots clear the live layer every frame and draw entity occluders only (`record_spot_shadow_depth`, `record_cube_shadow_depth`) | per occupied slot/face |
| Depth + SDF | `Depth Pre-Pass`; `SdfShadowPass::dispatch` when an SDF atlas is present | always / atlas |
| Scene | `Textured Pass`; `Kinematic Brush Pass`; `Skinned Mesh Pass`; `Billboard Sprite Pass`; `Fog Raymarch Pass` + `Fog Composite Pass`; `Skinned Viewmodel Pass` | has draws / fog active |
| Bloom | `Bloom::record`: bright, per-level downsample, two blur passes per level, per-level upsample, composite — `BLOOM_LEVEL_COUNT` levels | `bloom.enabled()`, on by default, `POSTRETRO_BLOOM=0` disables |
| Window-only | `Wireframe Overlay Pass`, `Debug Lines Pass` (`dev-tools`), `UI Pass`, `Screen Effects Resolve Pass` | windowed frame |

Counted at `BLOOM_LEVEL_COUNT = 5`, the bloom chain alone is 1 + 4 + 10 + 4 + 1 = 20 render passes, about half of a typical frame's passes.

Metal System Trace counts at `dcde8f292` (`measurements/release-indirect-validation/runtime/*-unset-cache-summary.json`, labelled passes only): hallway 113 camera frames with 904 `Promoted Spot Entity Shadow Depth Pass` (8 per frame), 113 depth pre-passes, 113 textured; campaign 137 frames with 838 promoted cube entity passes (6 per frame), 140 promoted spot entity, 133 dynamic spot entity, 6 dynamic spot world-cache refreshes. Adding the untraced groups gives roughly 36–40 passes per frame on the hallway and 40–44 on campaign. These are estimates; the brief's instrument replaces them.

## How wgpu 29 turns a pass into Metal objects

Read in `~/.cargo/registry/src/*/wgpu-core-29.0.1` and `wgpu-hal-29.0.1`.

- `encode_render_pass` (`wgpu-core/src/command/render.rs`) begins with `InnerCommandEncoder::close_if_open` then `open_pass`; `open_pass` calls hal `begin_encoding`. At pass end it calls `close`, then `open_pass("(wgpu internal) Pre Pass")`, records the pass's resource transitions (and indirect validation when on) into that buffer, and `close_and_swap` inserts it *before* the pass's buffer. `encode_compute_pass` does the same.
- wgpu-hal Metal `CommandEncoder::begin_encoding` (`wgpu-hal/src/metal/command.rs`) eagerly takes a new `MTLCommandBuffer` from the queue (`commandBuffer` / `commandBufferWithUnretainedReferences`). `end_encoding` returns it as a hal `CommandBuffer`. Metal `transition_buffers` / `transition_textures` are empty: the "Pre Pass" buffer carries nothing on Metal.
- `begin_render_pass` builds an `MTLRenderPassDescriptor` and calls `renderCommandEncoderWithDescriptor:`; the AMD driver performs load actions (clears) and resource-manager setup inside `initWithCommandBuffer:descriptor:`.
- wgpu-hal Metal `Queue::submit` (`wgpu-hal/src/metal/mod.rs`) commits every hal command buffer in the list, one `commit` each.

So each pass costs: one `MTLRenderCommandEncoder` (or compute encoder) and **two** `MTLCommandBuffer`s, each separately committed and later deallocated. Transfer commands between passes open further buffers. A frame of ~40 passes submits ~80 Metal command buffers plus the upload-copies buffer.

Per-draw work is separate: Metal `draw_indexed_indirect` loops over `draw_count` and the driver validates state per draw (`amdMtl_GFX10_ResourceMgrValidateGfxConstState`). That is the bucket `visible-span-draws` removed.

## Sample attribution at `dcde8f292` (validation off)

Release, no `dev-tools`, 10 s `sample` at 1 ms, in level, vsync on. Inclusive main-thread samples by outermost match; `render_submit` ≈ `CommandEncoderInterface::finish` + `Global::queue_submit` + upload batch `record_reusing`. Fractions are of that sum; ms are the fraction of the untraced window medians (3.81 / 3.48 ms). Overlaps between rows are noted.

| Bucket | Hallway samples | share | ≈ ms | Campaign samples | share | ≈ ms |
|---|---:|---:|---:|---:|---:|---:|
| `render_submit` sum | 1,868 | 100% | 3.81 | 1,629 | 100% | 3.48 |
| hal `draw_indexed_indirect` incl. driver (per indirect draw) | 408 | 21.8% | 0.83 | 28 | 1.7% | 0.06 |
| hal `begin_render_pass` + `RenderPassInfo::finish` (per pass: Metal encoder create, load actions, end, dealloc) | 299 | 16.0% | 0.61 | 308 | 18.9% | 0.66 |
| hal `begin_encoding` + `commit` + `reset_all` (per Metal command buffer: create, commit, dealloc) | 377 | 20.2% | 0.77 | 450 | 27.6% | 0.96 |
| hal `set_bind_group` + `set_render_pipeline` (per binding; overlaps `flush_bindings_helper`) | 125 | 6.7% | 0.25 | 127 | 7.8% | 0.27 |
| `triage_submissions` → drop `TempResource` → `MTLIOAccelBuffer dealloc` → `ioAccelResourceFinalize` (per staging buffer freed) | 403 | 21.6% | 0.82 | 404 | 24.8% | 0.86 |
| upload batch record + `copy_buffer_to_*` | 77 | 4.1% | 0.16 | 56 | 3.4% | 0.12 |
| remainder (wgpu-core replay bookkeeping, tracking, compute-pass replay not covered above) | 179 | 9.6% | 0.37 | 256 | 15.7% | 0.55 |

Per-pass total (encoder + command buffers): **676 samples, 36% ≈ 1.38 ms** on the hallway; **758, 47% ≈ 1.62 ms** on campaign. Half of the command-buffer bucket is the empty "Pre Pass" buffer: ≈ 0.4 / 0.5 ms. Divided by the estimated pass counts, both maps land near **35–40 µs per pass**, which is what a fixed per-pass cost predicts and the strongest evidence the bucket is per pass rather than per draw or per map.

The `hallway-release-on` profile (validation on) shows the same per-pass rows within noise (214 + 35, 202 + 82 + 24), so validation did not inflate them.

Projected hallway `render_submit` after `visible-span-draws`: 3.81 − 0.83 ≈ **3.0 ms**, of which per pass ≈ 1.4 ms (≈ 46%) and staging free ≈ 0.8 ms (≈ 27%). Campaign is unchanged at ≈ 3.5 ms with per pass ≈ 1.6 ms. This is a projection; the visible-span-draws timing rows were waived and never re-measured.

**Reconciliation with `bvh-leaf-clustering`.** Its "1.7–1.9 ms remains, nearly all per pass" is right in magnitude and wrong in attribution: about a third of the remainder is staging-buffer frees, not passes.

## The non-pass bucket: glyphon staging churn

Every `queue_write_buffer` sample in both profiles sits under `glyphon::TextRenderer::prepare_with_depth_and_custom` (125 / 138 samples, inside `rec_ui`). `UiTextRenderer::prepare_text_batches` (`render/ui/text.rs`) calls `Viewport::update` and one `TextRenderer::prepare_with_depth` per text batch every frame; glyphon writes each renderer's vertex buffer with the raw queue (`glyphon-0.11.0/src/text_render.rs`). wgpu 29 allocates a `StagingBuffer` per write (`Queue::write_buffer`, `wgpu-core/src/device/queue.rs`) and frees it at a later `queue_submit` through `triage_submissions`; on Metal each free is a synchronous `ioAccelResourceFinalize`. `plans/done/per-frame-upload-batching` Decisions "Direct writes left" deliberately left glyphon's writes direct; its plan of record reported `maintain` at ~61% of the submit stack after batching with glyphon and indirect validation as the only creators. With validation now off, glyphon is the sole per-frame staging creator, and the free cost is ≈ 0.8 ms on both maps, plus ≈ 0.3 ms creation inside `rec_ui`. `rec_ui` itself is 0.5–0.77 ms, of which cosmic-text reshaping every frame (`Buffer::new`, `set_text`) is another ≈ 0.2 ms. None of this is per pass; it is a sibling problem.

`UploadQueue`'s `direct_writes` counter does not see these writes: glyphon receives `queue.raw()`.

## Is the hallway CPU-bound with vsync off?

Recorded windows are vsync on: hallway `work` 7.3–7.7 ms against a 17.1 ms frame, `wait_acquire` ≈ 10 ms. The only vsync-off evidence is the owner's informal ~8–9 ms hallway frame, which would put `work` at ~85–95% of the frame — co-bound at best. `work` can shift with vsync off (thermal and cache state differ). No post-`visible-span-draws` number exists for either mode. The brief measures both.

## Candidate levers (not chosen; sized for the continuation gate)

| Lever | Passes removed per frame (hallway / campaign, est.) | CPU stake at ~37 µs/pass | Risk |
|---|---|---|---|
| Bloom chain as one compute pass (all levels as dispatches in one pass) or a dual-filter chain | ≈ 19 / 19, or ≈ 12 / 12 | ≈ 0.7 ms, or ≈ 0.45 ms | GPU time on AMD; visual change is manual-visual |
| Skip a promoted-slot entity shadow pass when the live layer already holds far plane and nothing is eligible to draw (writes driven by change, `development_guide.md` §1.4) | up to 8 / 7 | ≈ 0.3 / 0.26 ms | stale depth if a writer is missed; needs the pool's clear-state invariant |
| wgpu-hal Metal: create the `MTLCommandBuffer` lazily so an empty "Pre Pass" buffer is never created or committed | 0 passes; halves command buffers | ≈ 0.4 / 0.5 ms | a dependency patch (`[patch.crates-io]` or upstream); owner door |
| Merge same-attachment `Load` passes (kinematic, skinned, smoke; fog composite with bloom composite) | ≈ 2–4 | ≈ 0.1 ms | the forward pass cannot join: it samples the depth it would write |
| Glyphon churn (separate brief): prepare only changed batches, or vendor the text path onto the upload batch | n/a | ≈ 0.8 ms submit + 0.3 ms record | fork or change-detection in UI text |

The bloom-off switch already exists, so the per-pass cost can be measured today by subtraction with no new code. That is the spike's cheapest experiment.

## Adjacent drafts

- `drafts/bvh-leaf-clustering`: its render_submit remainder estimate is corrected above; its M2 AC measures `render_submit` and would benefit from the pass count.
- `drafts/shadow-fill-cost`: removes the shadow-cull compute pass and shadow indirect draws; it does not change the per-slot entity pass count.
- `drafts/per-frame-upload-redundant-writes`, `drafts/per-frame-upload-mesh-write-merging`: batch-side copy costs; neither touches glyphon or passes.
- `drafts/E19--render-stack-decomposition`: crate layering; a renderer-owned pass chokepoint would be a natural seam there but is not required here.
- `done/release-indirect-validation`: its runtime set is the baseline this research reads; its measurement discipline (same binary, 8 windows, median of last 5, foreground checks, `ioreg` snapshots) is reused.

## Measurement conditions to pin

From `rendering_pipeline.md` §12 and `done/release-indirect-validation`: compatibility-floor Mac (Radeon Pro 5300M, Metal), plain release with symbols, no `dev-tools`, `POSTRETRO_CPU_TIMING=1`, window in front, spawn pose on `stress-warren-hallway-inspection.prl` and `campaign-test.prl`, warm shadow cache, eight 120-frame windows with the median of the final five, `ioreg -c IOAccelerator` idle snapshots before each run, and the thermal record `done/per-frame-upload-batching` kept (CPU speed limit, GPU temperature). Vsync-off runs add the total frame time. No profiler during timing windows; `sample` and Metal System Trace in separate runs.
