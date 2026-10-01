# per-frame-upload-batching

Brief · compact · reads: `context/lib/rendering_pipeline.md` §1, §4 (cluster SH residency, upload batches), §12 · `context/lib/development_guide.md` §1.4, §2, §4.1 · `context/lib/testing_guide.md` §Resource bounds · read at 832e20c8a

## Problem
Owner-raised, from a profile on the compatibility-floor Mac (MacBook Pro 2019, Radeon Pro 5300M, Metal). Measured: staging buffer churn costs about 5 ms of a 10–12 ms CPU frame. The cause: every renderer `queue.write_buffer` or `write_texture` call makes wgpu 29 allocate its own staging buffer, and the next `queue.submit` frees it. On Metal each free is a synchronous kernel call, and a steady-state frame makes dozens of these calls. Numbers, conditions and the wgpu call path are in `research.md`. When done, steady-state per-frame uploads reach the GPU through one recycled, renderer-owned staging batch. Most of the staging create and free cost leaves `render_record`, `render_prep` and `render_submit`. Every pass reads the same bytes it reads today.

## Decisions
- **Reuse `StagedUploads` and `StagingPool`; add no second upload mechanism.** Their input domain matches: buffer and texture writes, under queue-write alignment rules. So do their ordered copies and recycled lifetime. They may move out of `sh_streaming` to a renderer-wide home, at a seam chosen under `development_guide.md` §2, with SH-neutral errors. The per-frame batch gets its own pool.
- **The renderer owns per-frame uploads.** Per-frame writes go through the renderer's batch whether they are gated or not. That covers writes made while recording and those the binary triggers through `Renderer` methods before rendering. This stays renderer-internal (`development_guide.md` §4.1), and the binary-facing `Renderer` API does not change.
- **Direct writes left.** Load, install and settings writes stay direct, as do the copies and writes inside streaming drains. glyphon and egui keep their own internal writes on their own resources. Routing those needs a fork, and they touch no resource the batch writes. A direct write never follows a batched write to the same resource before the batch submits. If it did, the direct write would land first and the older bytes would win.
- **The batch goes first in every renderer submit.** The frame submit is the common case. Drains, capture, readback, splash and the dev-tools submits carry the batch too, each recording it into its own command buffer. This matches today's queue semantics: a write lands before the commands of the first submit after it, so no module reorders. It also pins the drain boundary. Writes recorded before the lightmap or SH drain land in that drain's submit, ahead of its copies, as they do today. Writes recorded after the drain land in the frame submit. A batch held open across drains would depend on drain targets staying disjoint, which nothing enforces.
- **Program order holds.** Copies run in call order, and a later write to an overlapping range wins. wgpu puts a barrier between copies into one resource on every backend. A write identical in target, range and bytes to the latest staged write over that range, with no overlapping write between them, is dropped.
- **A batch lives for one frame entry, and is dropped only on a fatal error.** The windowed, capture and acquire-failure paths each submit it; the acquire-failure path submits it alone. Dropping would leave the GPU stale: by then the lights mirror, the descriptor dirty flag and the bridge snapshot all record their bytes as uploaded. Carrying it to the next frame would grow it on every skipped frame. A frame error is fatal (the binary exits), so drop there.
- **Recycling uses wgpu's own callbacks.** wgpu fires map callbacks inside `queue.submit`, so no `device.poll` is added. Pool and CPU storage reach a steady state with no allocation per frame (`development_guide.md` §1.4).
- **Non-goals:**
  - Upgrading or forking wgpu: no released version pools Metal staging buffers.
  - Changing streaming installs: they already batch.
  - Routing glyphon or egui writes.
  - Any shader or bind-group change.

## Acceptance
### Automated
- [ ] On a real map rendered headless, a steady-state frame makes zero direct queue writes to renderer-owned resources, and each submit carries at most one staging batch. A counter proves it, and the test skips itself when no adapter is present.
- [ ] A drift guard derived from source lists every direct queue write left in the renderer. It fails when a per-frame path adds one.
- [ ] In a debug build, a direct write to a resource that already holds a batched write awaiting submit fails loudly.
- [ ] Zero writes in a frame: no staging buffer is acquired and no extra command buffer is submitted.
#### Ordering
- [ ] Two writes to the same range in one frame: the pass reads the later bytes.
- [ ] Two writes that partly overlap: each byte takes the later write's value.
- [ ] The light-count patch survives the full per-frame uniform write that precedes it in the same frame.
- [ ] The shadow-slot patch survives the light bridge's full lights-buffer write in the same frame.
- [ ] Writes of A, then B, then A again to one range leave A. Two identical consecutive writes stage one copy.
- [ ] A write recorded before a streaming drain lands in the drain's submit, ahead of its copies. The frame submit carries only writes recorded after the drain.
#### Lifetime
- [ ] When the surface acquire fails, that frame's writes are submitted once, alone. The next drawn frame finds no pending writes from it. A diff-gated write made in the skipped frame is visible to the next drawn frame.
- [ ] Capture warmup, sample and PNG captures each submit their own writes with their own work. No write leaks into the next capture or applies twice.
- [ ] After warmup, a long run of frames with two frames in flight creates no new staging buffer. The pooled buffer count stays bounded, and the CPU scratch capacity stops growing.
- [ ] A frame whose batch outgrows every pooled buffer gets a fresh buffer, and the pool stays under its cap.
### Manual
- [ ] Measured finding, on the compatibility-floor Mac under `research.md` §Measurement conditions, on both maps: `[CpuTiming]` medians for `work`, `render_submit`, `render_record` and `render_prep`, before and after.
- [ ] A `sample` profile after the change shows `maintain` freeing only glyphon's and egui's staging buffers. Report what share of `render_submit` remains.
- [ ] Side by side on both maps, no visual difference in: HUD text, skinned meshes and their shadows, smoke, dynamic and animated lights, fog, viewmodel.

## Path
Non-binding.
- **Seams.**
  - `StagedUploads::{from_scratch, write_buffer, write_texture, record}`, `RecordedUploads::finish` and `StagingPool::{acquire, recycle}` are the mechanism.
  - Retain the `copies` vector across frames.
  - Merge per target instead of only by contiguity. Mesh instance and palette writes interleave, so they never merge today.
- **One chokepoint.** One renderer-owned upload handle can sit where `queue` is destructured today, in the `let Self { queue, full, .. } = self` pattern.
  - It carries the counter.
  - A debug assertion there can catch a direct write that follows a batched one.
  - One submit helper can serve every row in `research.md` §Submit inventory.
- **Shape and rival.** The chosen shape is a deferred CPU batch, recorded into its own encoder at submit time. The rival, `wgpu::util::StagingBelt`, records into an encoder at write time. That breaks `UiPass::encode`, which writes while its render pass is open, and it has no texture writes.
- **First slice.**
  1. Count queue writes per frame. This settles the 40–100 estimate.
  2. Route `MeshPass::plan_and_upload` and `UiPass::encode` through the batch on the frame submit only.
  3. Measure `render_submit` on the hallway map.

  This falsifies the riskiest assumption: that copy commands cost little next to the staging they replace, and that the pool recycles on Metal with two frames in flight.
- **Large files.** `renderer_light_slots.rs`, `renderer_render_frame.rs`, `mesh_pass.rs` and `smoke.rs` are past 800 lines. They get call-site swaps only, with new logic in the batch's own module, so no split comes first.
- **Source-text tests.** Some tests anchor on call text: `render/tests/pipeline_budget_tests.rs`, the `renderer_render_frame.rs` tests, and `crates/postretro/src/app/render_extents.rs`. Update them with the swap.

## Open questions
- Drain-internal direct writes, such as `compose_indirection` and sparse `row_pairs` zeroing. Route each through the batch only if it shares a target with a batched write. Otherwise leave it direct, and list it in the drift guard. — **delegated**
- The measured per-frame write count, and the pool's steady-state buffer count on Metal. — **delegated**: record both in the plan of record.
