# per-frame-upload-batching

Brief · compact · reads: `context/lib/rendering_pipeline.md` §1, §4 (cluster SH residency, upload batches), §12 · `context/lib/development_guide.md` §1.4, §2, §4.1 · `context/lib/testing_guide.md` §Resource bounds · read at 832e20c8a

> Validated (`/validate-plan`: Direction sound); the owner ruled the amendments. Landing order: this brief lands first, then `release-indirect-validation`, then `visible-span-draws`, then `bvh-leaf-clustering` (after `bake-parallelism-large-maps`). Take the manual `render_submit` proof before those siblings land.

## Problem
Owner-raised, from a profile on the compatibility-floor Mac (MacBook Pro 2019, Radeon Pro 5300M, Metal). Measured: staging buffer churn costs about 4.3 ms of a 10–12 ms CPU frame (de-inflated estimate). The cause: every renderer `queue.write_buffer` or `write_texture` call makes wgpu 29 allocate its own staging buffer, and the next `queue.submit` frees it. On Metal each free is a synchronous kernel call, and a steady-state frame makes dozens of these calls. Numbers, conditions and the wgpu call path are in `research.md`. When done, steady-state per-frame uploads reach the GPU through one recycled, renderer-owned staging batch. Most of the staging create and free cost leaves `render_record`, `render_prep` and `render_submit`. Every pass reads the same bytes it reads today.

## Decisions
- **Reuse `StagedUploads` and `StagingPool`; add no second upload mechanism.** Their input domain matches: buffer and texture writes, under queue-write alignment rules. So do their ordered copies and recycled lifetime. They may move out of `sh_streaming` to a renderer-wide home, at a seam chosen under `development_guide.md` §2, with SH-neutral errors. The per-frame batch gets its own pool.
- **The renderer owns per-frame uploads.** Per-frame writes go through the renderer's batch whether they are gated or not. That covers writes made while recording and those the binary triggers through `Renderer` methods before rendering. This stays renderer-internal (`development_guide.md` §4.1), and the binary-facing `Renderer` API does not change. `ui.md` §5 says UI writes resolve on the queue timeline; the build updates that sentence to name the batch, which keeps the same semantics.
- **Direct writes left.** Load, install and settings writes stay direct, as do the copies and writes inside streaming drains. glyphon and egui keep their own internal writes on their own resources. Routing those needs a fork, and they touch no resource the batch writes. A direct write never follows a batched write to the same resource before the batch submits. If it did, the direct write would land first and the older bytes would win.
- **The batch goes first in every renderer submit.** The frame submit is the common case. Drains, capture, readback, splash and the dev-tools submits carry the batch too, each recording it into its own command buffer. This matches today's queue semantics: a write lands before the commands of the first submit after it, so no module reorders. It also pins the drain boundary. Writes recorded before the lightmap or SH drain land in that drain's submit, ahead of its copies, as they do today. Writes recorded after the drain land in the frame submit. A batch held open across drains would depend on drain targets staying disjoint, which nothing enforces.
- **Program order holds.** Copies run in call order, and a later write to an overlapping range wins. wgpu puts a barrier between copies into one resource on every backend. The batch stages every write as made: it neither deduplicates nor merges by target. Dedup and coalescing belong at each writer, where the change is known.
- **A batch lives for one frame entry, and is dropped only on a fatal error.** The windowed, capture and acquire-failure paths each submit it; the acquire-failure path submits it alone. Dropping would leave the GPU stale: by then the lights mirror, the descriptor dirty flag and the bridge snapshot all record their bytes as uploaded. Carrying it to the next frame would grow it on every skipped frame. A frame error is fatal (the binary exits), so drop there.
- **The duplicate mesh light-params write is deleted at its source.** The skinned pass and the viewmodel pass in `renderer_render_frame.rs` each write the same bytes. Write them once per frame, before whichever of the two passes runs first. This is a direct fix in scope.
- **This brief diverges from change-driven writes.** `development_guide.md` §1.4 asks for writes driven by change rather than by frame. This brief makes each write cheaper; it does not remove writes. Every-frame writes stay, such as the light bridge's sentinel write followed by the full slot rewrite in `renderer_light_slots.rs`. Follow-ups, each filed and none owed here:
  - Fix redundant every-frame writes at their sources, starting with the bridge-then-slot lights rewrite.
  - Merge per-instance mesh writes by target. Build this only if the first slice's measured copy count and copy cost justify it.
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
- [ ] Writes of A, then B, then A again to one range leave A.
- [ ] The skinned-mesh light parameters are written once per frame. That holds when only the world meshes draw, when only the viewmodel draws, and when both draw.
- [ ] A write recorded before a streaming drain lands in the drain's submit, ahead of its copies. The frame submit carries only writes recorded after the drain.
#### Lifetime
- [ ] When the surface acquire fails, that frame's writes are submitted once, alone. The next drawn frame finds no pending writes from it. A diff-gated write made in the skipped frame is visible to the next drawn frame.
- [ ] Capture warmup, sample and PNG captures each submit their own writes with their own work. No write leaks into the next capture or applies twice.
- [ ] After warmup, a long run of frames with two frames in flight creates no new staging buffer. The pooled buffer count stays bounded, and the CPU scratch capacity stops growing.
- [ ] A frame whose batch outgrows every pooled buffer gets a fresh buffer, and the pool stays under its cap.
### Manual
- [ ] Measured finding, on the compatibility-floor Mac under `research.md` §Measurement conditions, on both maps: `[CpuTiming]` medians for `work`, `render_submit`, `render_record` and `render_prep`, before and after. Take both before the sibling briefs in the landing order land.
- [ ] A `sample` profile after the change shows `maintain` freeing only glyphon's and egui's staging buffers. Report what share of `render_submit` remains.
- [ ] Side by side on both maps, no visual difference in: HUD text, skinned meshes and their shadows, smoke, dynamic and animated lights, fog, viewmodel.

## Path
Non-binding.
- **Seams.**
  - `StagedUploads::{from_scratch, write_buffer, write_texture, record}`, `RecordedUploads::finish` and `StagingPool::{acquire, recycle}` are the mechanism.
  - Retain the `copies` vector across frames.
- **One chokepoint.** One renderer-owned upload handle can sit where `queue` is destructured today, in the `let Self { queue, full, .. } = self` pattern.
  - It carries the counter.
  - A debug assertion there can catch a direct write that follows a batched one.
  - One submit helper can serve every row in `research.md` §Submit inventory.
- **Shape.** The chosen shape is a deferred CPU batch, recorded into its own encoder at submit time.
- **Rivals.** Full reasons are in `research.md` §Rivals considered.
  - `wgpu::util::StagingBelt` needs an encoder at write time. But many writes come through `Renderer` methods before any encoder exists. It also moves writes from the queue timeline to the encoder timeline, which `ui.md` §5 relies on not happening. And it has no texture writes.
  - Coalescing at each writer is too weak as the main fix, but it is the right home for dedup and merging.
  - Batching only at record time leaves the binary-side writes direct.
  - An upstream wgpu issue asking to pool Metal staging is worth filing, but it does not replace this.
- **First slice.**
  1. Count queue writes per frame. This settles the estimate in `research.md`.
  2. Route `MeshPass::plan_and_upload` and `UiPass::encode` through the batch on the frame submit only.
  3. Measure `render_submit` on the hallway map.

  This falsifies the riskiest assumption: that copy commands cost little next to the staging they replace (this also sets the per-target merge follow-up's gate), and that the pool recycles on Metal with two frames in flight.
- **Large files.** `renderer_light_slots.rs`, `renderer_render_frame.rs`, `mesh_pass.rs` and `smoke.rs` are past 800 lines. They get call-site swaps only, with new logic in the batch's own module, so no split comes first.
- **Source-text tests.** Some tests anchor on call text: `render/tests/pipeline_budget_tests.rs`, the `renderer_render_frame.rs` tests, and `crates/postretro/src/app/render_extents.rs`. Update them with the swap.

## Open questions
- Drain-internal direct writes, such as `compose_indirection` and sparse `row_pairs` zeroing. Route each through the batch only if it shares a target with a batched write. Otherwise leave it direct, and list it in the drift guard. — **delegated**
- The measured per-frame write count, and the pool's steady-state buffer count on Metal. — **delegated**: record both in the plan of record.
