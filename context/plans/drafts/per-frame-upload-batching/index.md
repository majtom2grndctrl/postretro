# per-frame-upload-batching

Brief · compact · reads: `context/lib/rendering_pipeline.md` §1, §4 (cluster SH residency, upload batches), §12 · `context/lib/development_guide.md` §1.4, §2, §4.1 · `context/lib/testing_guide.md` §Resource bounds · read at 832e20c8a

> Landing order: this brief lands first, then `release-indirect-validation`, then `visible-span-draws`, then `bvh-leaf-clustering` (after `bake-parallelism-large-maps`).

## Problem
Owner-raised, from a profile on the compatibility-floor Mac (MacBook Pro 2019, Radeon Pro 5300M, Metal). Measured: staging buffer churn costs about 4.0 ms (stress) and 3.4 ms (campaign), de-inflated, of a 10–12 ms CPU frame. The cause: every renderer `queue.write_buffer` or `write_texture` call makes wgpu 29 allocate its own staging buffer, and the next `queue.submit` frees it. On Metal each free is a synchronous kernel call, and a steady-state frame makes many of these calls. Numbers, conditions, the call-count estimate and the wgpu call path are in `research.md`. When done, steady-state per-frame uploads reach the GPU through one recycled, renderer-owned staging batch. Most of the staging create and free cost leaves `render_record`, `render_prep` and `render_submit`. Every pass reads the same bytes it reads today.

## Decisions
- **Reuse `StagedUploads` and `StagingPool`; add no second upload mechanism.** The per-frame batch gets its own pool, with its own small steady-state cap: a named constant, sized from the first slice's measured batch size for a few small buffers across two frames in flight.
- **The renderer owns per-frame uploads.** Per-frame writes go through the renderer's batch whether they are gated or not. That covers writes made while recording and those the binary triggers through `Renderer` methods before rendering, including dev-tools setters called mid-frame. This stays renderer-internal (`development_guide.md` §4.1), and the binary-facing `Renderer` API does not change. `ui.md` §5 says UI writes resolve on the queue timeline; the build updates that sentence to name the batch, which keeps the same semantics.
- **Direct writes left.** Load, install and settings writes stay direct, as do the copies and writes inside streaming drains. glyphon and egui keep their own internal writes on their own resources. Routing those needs a fork, and they touch no resource the batch writes. Boot splash writes stay direct: splash frames run before the full renderer and its batch exist. A direct write through the renderer's upload chokepoint never follows a batched write to the same resource before the batch submits; if it did, it would land first and the older bytes would win. The drift guard holds every direct write outside the chokepoint to one of these classes.
- **The batch goes first in the frame, drain and capture submits.** A write lands before the commands of the first submit after it, as today. Writes recorded before a drain land in that drain's submit; writes after it land in the frame submit. Splash, readback and dev-tools submits never carry one; a debug build fails loudly if a write is pending at any of them.
- **Program order holds.** Copies run in call order, and a later write to an overlapping range wins. The batch neither deduplicates nor merges by target; that belongs at each writer.
- **A batch lives for one frame entry.** The windowed, capture and acquire-failure paths each submit it; acquire failure submits it alone, because writers' CPU mirrors already record the bytes as uploaded. It is dropped only on a fatal error.
- **The duplicate mesh light-params write is deleted at its source:** written once per frame.
- **This brief diverges from change-driven writes.** `development_guide.md` §1.4 asks for writes driven by change rather than by frame. This brief makes each write cheaper; it does not remove writes. Every-frame writes stay, such as the light bridge's sentinel write followed by the full slot rewrite in `renderer_light_slots.rs`. Follow-ups, each filed and none owed here:
  - Fix redundant every-frame writes at their sources, starting with the bridge-then-slot lights rewrite.
  - Merge per-instance mesh writes by target. Build this only if the first slice's measured copy count and copy cost justify it.
- **Recycling uses wgpu's own map callbacks.** Pool and CPU storage reach a steady state with no allocation per frame (`development_guide.md` §1.4).
- **Non-goals:**
  - Upgrading or forking wgpu: no released version pools Metal staging buffers.
  - Changing streaming installs: they already batch.

## Acceptance
### Automated
- [ ] On a real map rendered headless, a steady-state frame makes zero direct queue writes to renderer-owned resources, and each submit carries at most one staging batch. A counter proves it, and the test skips itself when no adapter is present. The test makes the binary's pre-render Renderer calls in frame order: light bridge, fog, per-frame uniforms, viewmodel. It then records the window-only branch (UI, viewmodel pass, resolve) into an offscreen target. Its inputs reach every writer in research.md §Per-frame write inventory: mesh instances, particles, fog and animated lights are all present. It asserts each writer staged at least one write. It uses a committed fixture, or is #[ignore]-gated.
- [ ] A drift guard derived from source lists every direct queue write left in the renderer. It tags each with its class from Direct writes left: load, install, settings, drain-internal, boot splash, glyphon or egui. It fails on an untagged site, and on any site from research.md §Per-frame write inventory. It tells a batch write from a queue write by receiver, and it skips #[cfg(test)] code. This is a grep gate.
- [ ] In a debug build, a direct write through the renderer's upload chokepoint to a resource that already holds a batched write awaiting submit fails loudly. Direct writes outside the chokepoint are proven by the drift guard's classes, not by this assertion.
- [ ] A frame with zero writes, a drain with no work, a dev-tools submit, or an acquire-failure skip with nothing pending acquires no staging buffer and submits no extra command buffer. A steady-state frame that streams nothing acquires one staging buffer (pin P2).
- [ ] A per-frame write the batch rejects for alignment or bounds fails as loudly as a direct write does today. It never drops silently, and a rejected bridge write never reports the snapshot committed (pin P12).
- [ ] The headless UI and resolve goldens keep their expected pixels unchanged, with their writes now going through the batch: multi-batch, ring composition, multi-layer text, resolve composite.
- [ ] A grep gate finds no StagingBelt, and no MAP_WRITE staging buffer created outside the shared pool.
- [ ] The build runs every adapter-gated row on a machine with an adapter and reports how many ran without skipping. A self-skip prints its reason and does not count as a pass.
#### Ordering
- [ ] Every ordering and lifetime row runs with its writes staged in the batch, and asserts through the counter that they were.
- [ ] Two writes to the same range in one frame: the pass reads the later bytes.
- [ ] Two writes that partly overlap: each byte takes the later write's value.
- [ ] The light-count patch survives the full per-frame uniform write that precedes it in the same frame.
- [ ] The shadow-slot patch survives the light bridge's full lights-buffer write in the same frame.
- [ ] Writes of A, then B, then A again to one range leave A.
- [ ] The skinned-mesh light parameters are written once per frame. That holds when only the world meshes draw, when only the viewmodel draws, and when both draw. When neither draws, they are not written (pin P10).
- [ ] A write recorded before a streaming drain lands in the drain's submit, ahead of its copies. The frame submit carries only writes recorded after the drain. With both drains working in one frame, each write lands in the first submit after it: before the lightmap drain, between the drains, or in the frame submit. Each lands once (pin P4).
- [ ] A drain that submits more than once carries the staged writes in its first submit, ahead of any growth copy. Its later submits carry none of them (pin P3).
- [ ] A drift guard derived from source finds no renderer queue.submit outside the one submit helper. At a frame, drain or capture submit, the helper puts the batch's command buffer first (pin P7). This is a grep gate.
- [ ] Splash, PNG readback and dev-tools submits carry no batch; a debug build fails loudly if a write is pending at one. That holds for a boot-splash frame before the full renderer exists (pin P11).
- [ ] Every dev-tools setter called between the per-frame uniform write and the frame submit stages its write in the batch. The counter counts each one, and each lands before the frame's passes (pin P8; Decision: the renderer owns per-frame uploads).
#### Lifetime
- [ ] When the surface acquire fails, that frame's writes are submitted once, alone. The next drawn frame finds no pending writes from it. A diff-gated write made in the skipped frame is visible to the next drawn frame. Two failures in a row each submit only their own frame's writes. A test-only acquire outcome drives the skip path headless (pin P9).
- [ ] A lightmap drain that fails and rolls back submits nothing. The writes staged before it land in the next submit of the same frame, once (pin P1).
- [ ] At every frame entry, and at hot-reload commit, level install and level unload, the batch holds no pending write. A debug build fails loudly if one is pending (pin P6).
- [ ] Back-to-back submits with no GPU progress take distinct staging buffers. A buffer rejoins the pool only after the submit that read it completes, and the batch path calls no device.poll (pin P5).
- [ ] Capture warmup, sample and PNG captures each submit their own writes with their own work. No write leaks into the next capture or applies twice.
- [ ] After warmup, a long run of frames with two frames in flight creates no new staging buffer. The test keeps two in flight by waiting on the submission from two frames back. Live staging buffers, pooled plus in flight, stay at or under four per batch-carrying submit per frame. The byte scratch and the copy list stop growing, and acquire builds no per-call list.
- [ ] After warmup, the per-frame batch's acquire, record and recycle make no heap allocation in renderer code. A counting allocator around a long run of frames proves it. The count leaves out wgpu's boxed map callback, and the test names that exclusion.
- [ ] A frame whose batch outgrows every pooled buffer gets a fresh buffer. Afterward the per-frame pool's free list stays under its named steady-state cap.
### Manual
- [ ] Measured finding, on the compatibility-floor Mac under `research.md` §Measurement conditions, on both maps: `[CpuTiming]` medians for `work`, `render_submit`, `render_record` and `render_prep`, before and after. Take both before the sibling briefs in the landing order land.
- [ ] Measured finding, on the compatibility-floor Mac on both maps: writes staged per frame, copies per batch, and live staging buffers after warmup over one 120-frame window. Record them in the plan of record. They set the merge follow-up's gate.
- [ ] A `sample` profile after the change shows `maintain` freeing only glyphon's and egui's staging buffers. Report what share of `render_submit` remains.
- [ ] Side by side on both maps, no visual difference in: HUD text, skinned meshes and their shadows, smoke, dynamic and animated lights, fog, viewmodel.
- [ ] Both follow-ups are filed: redundant every-frame writes, starting with the bridge-then-slot lights rewrite; and per-target mesh write merging, gated on the measured copy count.

## Path
Non-binding.
- **Seams.**
  - `StagedUploads::{from_scratch, write_buffer, write_texture, record}`, `RecordedUploads::finish` and `StagingPool::{acquire, recycle}` are the mechanism.
  - They may move out of `sh_streaming` to a renderer-wide home, at a seam chosen under `development_guide.md` §2, with SH-neutral errors.
  - Retain the `copies` vector across frames.
  - Add no `device.poll`: wgpu fires map callbacks inside `queue.submit`.
- **One chokepoint.** One renderer-owned upload handle can sit where `queue` is destructured today, in the `let Self { queue, full, .. } = self` pattern.
  - It carries the counter.
  - A debug assertion there can catch a direct write that follows a batched one.
  - One submit helper can serve every row in `research.md` §Submit inventory.
- **Shape.** The chosen shape is a deferred CPU batch, recorded into its own encoder at submit time.
- **Rival.** `wgpu::util::StagingBelt`: it needs an encoder at write time, has no texture writes, and moves writes to the encoder timeline. Other rivals and reasons: `research.md` §Rivals considered.
- **Light params.** The skinned pass and the viewmodel pass in `renderer_render_frame.rs` each write the same bytes. Write them once, before whichever of the two passes runs first.
- **First slice.**
  1. Count queue writes per frame.
  2. Route `MeshPass::plan_and_upload` and `UiPass::encode` through the batch on the frame submit only.
  3. Measure `render_submit` on the hallway map.

  This falsifies the riskiest assumption: that copy commands cost little next to the staging they replace, and that the pool recycles on Metal with two frames in flight.
- **Large files.** `renderer_light_slots.rs`, `renderer_render_frame.rs`, `mesh_pass.rs` and `smoke.rs` are past 800 lines. They get call-site swaps only, with new logic in the batch's own module, so no split comes first.
- **Source-text tests.** Some tests anchor on call text: `render/tests/pipeline_budget_tests.rs`, the `renderer_render_frame.rs` tests, and `crates/postretro/src/app/render_extents.rs`. Update them with the swap.

## Open questions
- Drain-internal direct writes, such as `compose_indirection` and sparse `row_pairs` zeroing. Route each through the batch only if it shares a target with a batched write. Otherwise leave it direct, and list it in the drift guard. — **delegated**
