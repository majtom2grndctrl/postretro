# per-frame-upload-batching — plan of record

mode: compact
status: blocked
read at: 83489c52b

## Corrections
- Renderer and binary source are unchanged since brief revision 832e20c8a. Grounded Decision reads remain current.
- The existing approved `postretro_sim::alloc_probe` test-support allocator can be reused without introducing unsafe code. GPU adapter tests are explicitly required by this brief despite the general no-GPU unit-test convention.

## Delegated answers
- Drain-internal target overlap — compose indirection, direct sparse row pairs and indirect affinity offsets are lifecycle reset/drain targets, not per-frame write targets. `enable_streamed_atlas` shares dynamic-direct params with a per-frame writer, but successful setup sets `has_direct`, making later growth calls no-ops; retain and verify that guard.

## AC-to-proof

| AC | Requirement | Proof | Status |
|---|---|---|---|
| 1 | On a real map rendered headless, a steady-state frame makes zero direct queue writes to renderer-owned resources, and each submit carries at most one staging batch. A counter proves it, and the test skips itself when no adapter is present. The test makes the binary's pre-render Renderer calls in frame order: light bridge, fog, per-frame uniforms, viewmodel. It then records the window-only branch (UI, viewmodel pass, resolve) into an offscreen target. Its inputs reach every writer in research.md §Per-frame write inventory: mesh instances, particles, fog and animated lights are all present. It asserts each writer staged at least one write. It uses a committed fixture, or is #[ignore]-gated. | real-map offscreen frame inventory test (ignored fixture) | achievable as stated |
| 2 | A drift guard derived from source lists every direct queue write left in the renderer. It tags each with its class from Direct writes left: load, install, drain-internal, boot splash, glyphon or egui. It fails on an untagged site, and on any site from research.md §Per-frame write inventory. It tells a batch write from a queue write by receiver, and it skips #[cfg(test)] code. This is a grep gate. | source-derived direct-write classification gate | achievable as stated |
| 3 | In a debug build, a direct write through the renderer's upload chokepoint to a resource that already holds a batched write awaiting submit fails loudly. Direct writes outside the chokepoint are proven by the drift guard's classes, not by this assertion. | direct-after-staged assertion test | achievable as stated |
| 4 | A frame with zero writes, a drain with no work, a dev-tools submit, or an acquire-failure skip with nothing pending acquires no staging buffer and submits no extra command buffer. A steady-state frame that streams nothing acquires one staging buffer (pin P2). | empty/no-work and non-streaming submission counter tests | achievable as stated |
| 5 | A per-frame write the batch rejects for alignment or bounds fails as loudly as a direct write does today. It never drops silently, and a rejected bridge write never reports the snapshot committed (pin P12). | rejected-write and bridge commit tests | achievable as stated |
| 6 | The headless UI and resolve goldens keep their expected pixels unchanged, with their writes now going through the batch: multi-batch, ring composition, multi-layer text, resolve composite. | existing headless UI and resolve goldens | achievable as stated |
| 7 | A grep gate finds no StagingBelt, and no MAP_WRITE staging buffer created outside the shared pool. | source-derived mechanism/pool gate | achievable as stated |
| 8 | The build runs every adapter-gated row on a machine with an adapter and reports how many ran without skipping. A self-skip prints its reason and does not count as a pass. | adapter run ledger: executed and skipped counts | achievable as stated |
| 9 | Every ordering and lifetime row runs with its writes staged in the batch, and asserts through the counter that they were. | staging counters in each ordering/lifetime test | achievable as stated |
| 10 | Two writes to the same range in one frame: the pass reads the later bytes. | same-range GPU readback | achievable as stated |
| 11 | Two writes that partly overlap: each byte takes the later write's value. | partial-overlap GPU readback | achievable as stated |
| 12 | The light-count patch survives the full per-frame uniform write that precedes it in the same frame. | uniform-count patch GPU readback | achievable as stated |
| 13 | The shadow-slot patch survives the light bridge's full lights-buffer write in the same frame. | bridge-slot patch GPU readback | achievable as stated |
| 14 | Writes of A, then B, then A again to one range leave A. | A/B/A GPU readback | achievable as stated |
| 15 | The skinned-mesh light parameters are written once per frame. That holds when only the world meshes draw, when only the viewmodel draws, and when both draw. When neither draws, they are not written (pin P10). | world/viewmodel/neither light-params counter cases | achievable as stated |
| 16 | A write recorded before a streaming drain lands in the drain's submit, ahead of its copies. The frame submit carries only writes recorded after the drain. With both drains working in one frame, each write lands in the first submit after it: before the lightmap drain, between the drains, or in the frame submit. Each lands once (pin P4). | lightmap + SH + frame submit integration readback | achievable as stated |
| 17 | A drain that submits more than once carries the staged writes in its first submit, ahead of any growth copy. Its later submits carry none of them (pin P3). | SH multi-submit growth integration readback | achievable as stated |
| 18 | A drift guard derived from source finds no renderer queue.submit outside the one submit helper. At a frame, drain or capture submit, the helper puts the batch's command buffer first (pin P7). This is a grep gate. | source-derived submit gate and first-buffer counter | achievable as stated |
| 19 | Splash, PNG readback and dev-tools submits carry no batch; a debug build fails loudly if a write is pending at one. That holds for a boot-splash frame before the full renderer exists (pin P11). | forbidden-submit and boot-splash assertion tests | achievable as stated |
| 20 | Every dev-tools setter called between the per-frame uniform write and the frame submit stages its write in the batch. The counter counts each one, and each lands before the frame's passes (pin P8; Decision: the renderer owns per-frame uploads). | dev-tools setter staging/readback inventory | achievable as stated |
| 21 | An options-menu setter changed mid-session, such as fog step size, stages its write in the batch. The counter counts it, and it lands before the next frame's passes (pin P13; Decision: the renderer owns per-frame uploads). | options setter staging/readback | achievable as stated |
| 22 | When the surface acquire fails, that frame's writes are submitted once, alone. The next drawn frame finds no pending writes from it. A diff-gated write made in the skipped frame is visible to the next drawn frame. Two failures in a row each submit only their own frame's writes. A test-only acquire outcome drives the skip path headless (pin P9). | acquire outcome injection and diff-gated readback | achievable as stated |
| 23 | A lightmap drain that fails and rolls back submits nothing. The writes staged before it land in the next submit of the same frame, once (pin P1). | failed lightmap drain rollback integration | achievable as stated |
| 24 | At every frame entry, and at hot-reload commit, level install and level unload, the batch holds no pending write. A debug build fails loudly if one is pending (pin P6). | frame/lifecycle empty assertions | needs owner direction: entry boundary/API conflict |
| 25 | Back-to-back submits with no GPU progress take distinct staging buffers. A buffer rejoins the pool only after the submit that read it completes, and the batch path calls no device.poll (pin P5). | back-to-back staging identity and callback tests | achievable as stated |
| 26 | Capture warmup, sample and PNG captures each submit their own writes with their own work. No write leaks into the next capture or applies twice. | capture warmup/sample/PNG integration tests | achievable as stated |
| 27 | After warmup, a long run of frames with two frames in flight creates no new staging buffer. The test keeps two in flight by waiting on the submission from two frames back. Live staging buffers, pooled plus in flight, stay at or under four per batch-carrying submit per frame. The byte scratch and the copy list stop growing, and acquire builds no per-call list. | long-run two-in-flight buffer/scratch/copy capacity test | achievable as stated |
| 28 | After warmup, the per-frame batch's acquire, record and recycle make no heap allocation in renderer code. A counting allocator around a long run of frames proves it. The count leaves out wgpu's boxed map callback, and the test names that exclusion. | existing approved counting allocator, scoped renderer storage windows; exclude wgpu callback allocation | achievable as stated |
| 29 | A frame whose batch outgrows every pooled buffer gets a fresh buffer. Afterward the per-frame pool's free list stays under its named steady-state cap. | oversized batch and bounded free-pool test | achievable as stated |
| 30 | Measured finding, on the compatibility-floor Mac under `research.md` §Measurement conditions, on both maps: `[CpuTiming]` medians for `work`, `render_submit`, `render_record` and `render_prep`, before and after. Take both before the sibling briefs in the landing order land. | owner compatibility-floor Mac: before/after timing on both maps | manual, blocks landing |
| 31 | Measured finding, on the compatibility-floor Mac on both maps: writes staged per frame, copies per batch, and live staging buffers after warmup over one 120-frame window. Record them in the plan of record. They set the merge follow-up's gate. | owner compatibility-floor Mac: 120-frame upload/copy/live-buffer counters | manual, blocks landing |
| 32 | A `sample` profile after the change shows `maintain` freeing only glyphon's and egui's staging buffers. Report what share of `render_submit` remains. | owner compatibility-floor Mac: post-change sample attribution | manual, blocks landing |
| 33 | Side by side on both maps, no visual difference in: HUD text, skinned meshes and their shadows, smoke, dynamic and animated lights, fog, viewmodel. | owner side-by-side visual check on both maps | manual, blocks landing |
| 34 | Both follow-ups are filed: redundant every-frame writes, starting with the bridge-then-slot lights rewrite; and per-target mesh write merging, gated on the measured copy count. | file both follow-ups in context/plans/drafts | achievable as stated |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Thin slice: reusable deferred batch, mesh/UI routing, first-slice batch size/copy/timing measurement; choose named pool cap | integrating executor | — | pending |
| 2 | Route every writer and every submit; preserve drain, acquire-failure, capture and lifecycle order; remove duplicate mesh light params | integrating executor | 1 | pending |
| 3 | Source drift gates, ordering/lifetime/resource tests, real-map offscreen inventory and golden verification; file follow-ups | integrating executor; bounded source-audit worker | 2 | pending |
| 4 | Review-readiness gate, review-panel/fix loop, final preflight; record every AC result and external runbook | integrating executor | 3 | pending |

## Hot-path constraints
- Reuse shared StagedUploads/StagingPool; retain byte scratch and copy storage. No target deduplication or merging in the per-frame batch.
- Acquire makes no per-call list; recycle uses map callbacks and never polls the device. Bound free storage with a named per-frame cap selected from measured first-slice batch size.
- Zero pending writes yields no staging acquisition or extra command buffer. Submit consumes a pending batch exactly once, before other work.
- Existing every-frame writers remain as contracted; redundant writer follow-ups own future suppression/merging.

## Landing
- Manual AC 30–33 require compatibility-floor Mac timing, counters, sampling and visual proof. The brief gives no permission to land with gaps, so status becomes test-ready until those results arrive.
- Owner says “land the plane” after results and review to authorize the landing checkpoint.

## Blocking contract check — P6 / AC 24

The Decision says “the binary-facing `Renderer` API does not change.” AC 24 says “At every frame entry ... the batch holds no pending write” with a debug assertion. There is no existing Renderer method at the binary's frame-start boundary before all valid writers.

Source evidence (unchanged from brief read revision):
- `crates/postretro/src/main.rs:2047`: RedrawRequested begins; `poll_os_preferences` follows at 2062.
- Gameplay options are applied at main.rs:3423, before `commit_render_extents` at 3424. Frontend has the same order at 5626–5627.
- `crates/renderer/src/render/renderer_state.rs:39`: Surface Depth option setter calls `rewrite_material_surface_depth`; `material_plan.rs:193` directly writes material uniforms. Under the Decision these valid option writes must become staged before the existing extent commit.
- Gameplay bridge, fog, per-frame uniforms and viewmodel uploads precede `render_frame_indirect` at main.rs:4420. Asserting the whole batch empty at that method's entry would reject them.
- `crates/postretro/src/capture/setup.rs:71`: capture setup uploads a light bridge snapshot before capture entry; capture entrances similarly cannot assert all pending writes absent.
- `Renderer::present` is an existing drawn-frame tail, but skipped frames bypass it. Tail checks can establish empty-at-return, but do not provide an assertion at the binary's next frame entry.

Owner question pending: permit an additive, wgpu-free `Renderer::begin_frame()` hook before any frame setter/upload, keeping all existing method signatures; or explicitly change P6 to frame-tail and lifecycle emptiness instead.

Recommended exact Decision clarification if the hook is authorized:
> Existing binary-facing Renderer method signatures remain unchanged. A wgpu-free `Renderer::begin_frame()` entry hook may be added and called before a windowed or capture frame's first setter/upload to enforce P6.

Alternative exact AC 24 restatement if the owner keeps the API unchanged:
> At every successful or skipped frame exit, and at hot-reload commit, level install and level unload, the batch holds no pending write. A debug build fails loudly if one is pending. Pre-render writes for the current frame are allowed at renderer render/capture method entry.

No renderer implementation or test change has been made. Resume at Verify source / AC-to-proof after the owner chooses the contract; apply only authorized wording and rerun this check before returning status to active.
