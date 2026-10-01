# per-frame-upload-batching — plan of record

mode: compact
status: blocked
read at: 83489c52b

## Corrections
- At source verification before implementation, renderer and binary source were unchanged since brief revision 832e20c8a; grounded Decision reads were current.
- The existing approved `postretro_sim::alloc_probe` test-support allocator can be reused without introducing unsafe code. GPU adapter tests are explicitly required by this brief despite the general no-GPU unit-test convention.

## Delegated answers
- Drain-internal target overlap — compose indirection, direct sparse row pairs and indirect affinity offsets are lifecycle reset/drain targets, not per-frame write targets. `enable_streamed_atlas` shares dynamic-direct params with a per-frame writer, but successful setup sets `has_direct`, making later growth calls no-ops; retain and verify that guard.

## AC-to-proof

| AC | Requirement | Proof | Status | Result |
|---|---|---|---|---|
| 1 | On a real map rendered headless, a steady-state frame makes zero direct queue writes to renderer-owned resources, and each submit carries at most one staging batch. A counter proves it, and the test skips itself when no adapter is present. The test makes the binary's pre-render Renderer calls in frame order: light bridge, fog, per-frame uniforms, viewmodel. It then records the window-only branch (UI, viewmodel pass, resolve) into an offscreen target. Its inputs reach every writer in research.md §Per-frame write inventory: mesh instances, particles, fog and animated lights are all present. It asserts each writer staged at least one write. It uses a committed fixture, or is #[ignore]-gated. | real-map offscreen frame inventory test (ignored fixture) | achievable as stated | pass |
| 2 | A drift guard derived from source lists every direct queue write left in the renderer. It tags each with its class from Direct writes left: load, install, drain-internal, boot splash, glyphon or egui. It fails on an untagged site, and on any site from research.md §Per-frame write inventory. It tells a batch write from a queue write by receiver, and it skips #[cfg(test)] code. This is a grep gate. | source-derived direct-write classification gate | achievable as stated | pass |
| 3 | In a debug build, a direct write through the renderer's upload chokepoint to a resource that already holds a batched write awaiting submit fails loudly. Direct writes outside the chokepoint are proven by the drift guard's classes, not by this assertion. | direct-after-staged assertion test | achievable as stated | pass |
| 4 | A frame with zero writes, a drain with no work, a dev-tools submit, or an acquire-failure skip with nothing pending acquires no staging buffer and submits no extra command buffer. A steady-state frame that streams nothing acquires one staging buffer (pin P2). | empty/no-work and non-streaming submission counter tests | achievable as stated | pass |
| 5 | A per-frame write the batch rejects for alignment or bounds fails as loudly as a direct write does today. It never drops silently, and a rejected bridge write never reports the snapshot committed (pin P12). | rejected-write and bridge commit tests | achievable as stated | pass |
| 6 | The headless UI and resolve goldens keep their expected pixels unchanged, with their writes now going through the batch: multi-batch, ring composition, multi-layer text, resolve composite. | existing headless UI and resolve goldens | achievable as stated | pass |
| 7 | A grep gate finds no StagingBelt, and no MAP_WRITE staging buffer created outside the shared pool. | source-derived mechanism/pool gate | achievable as stated | pass |
| 8 | The build runs every adapter-gated row on a machine with an adapter and reports how many ran without skipping. A self-skip prints its reason and does not count as a pass. | adapter run ledger: executed and skipped counts | achievable as stated | pass |
| 9 | Every ordering and lifetime row runs with its writes staged in the batch, and asserts through the counter that they were. | staging counters in each ordering/lifetime test | achievable as stated | pass |
| 10 | Two writes to the same range in one frame: the pass reads the later bytes. | same-range GPU readback | achievable as stated | pass |
| 11 | Two writes that partly overlap: each byte takes the later write's value. | partial-overlap GPU readback | achievable as stated | pass |
| 12 | The light-count patch survives the full per-frame uniform write that precedes it in the same frame. | uniform-count patch GPU readback | achievable as stated | pass |
| 13 | The shadow-slot patch survives the light bridge's full lights-buffer write in the same frame. | bridge-slot patch GPU readback | achievable as stated | pass |
| 14 | Writes of A, then B, then A again to one range leave A. | A/B/A GPU readback | achievable as stated | pass |
| 15 | The skinned-mesh light parameters are written once per frame. That holds when only the world meshes draw, when only the viewmodel draws, and when both draw. When neither draws, they are not written (pin P10). | world/viewmodel/neither light-params counter cases | achievable as stated | pass |
| 16 | A write recorded before a streaming drain lands in the drain's submit, ahead of its copies. The frame submit carries only writes recorded after the drain. With both drains working in one frame, each write lands in the first submit after it: before the lightmap drain, between the drains, or in the frame submit. Each lands once (pin P4). | lightmap + SH + frame submit integration readback | achievable as stated | pass |
| 17 | A drain that submits more than once carries the staged writes in its first submit, ahead of any growth copy. Its later submits carry none of them (pin P3). | SH multi-submit growth integration readback | achievable as stated | pass |
| 18 | A drift guard derived from source finds no renderer queue.submit outside the one submit helper. At a frame, drain or capture submit, the helper puts the batch's command buffer first (pin P7). This is a grep gate. | source-derived submit gate and first-buffer counter | achievable as stated | pass |
| 19 | Splash, PNG readback and dev-tools submits carry no batch; a debug build fails loudly if a write is pending at one. That holds for a boot-splash frame before the full renderer exists (pin P11). | forbidden-submit and boot-splash assertion tests | achievable as stated | pass |
| 20 | Every dev-tools setter called between the per-frame uniform write and the frame submit stages its write in the batch. The counter counts each one, and each lands before the frame's passes (pin P8; Decision: the renderer owns per-frame uploads). | dev-tools setter staging/readback inventory | achievable as stated | pass |
| 21 | An options-menu setter changed mid-session, such as fog step size, stages its write in the batch. The counter counts it, and it lands before the next frame's passes (pin P13; Decision: the renderer owns per-frame uploads). | options setter staging/readback | achievable as stated | pass |
| 22 | When the surface acquire fails, that frame's writes are submitted once, alone. The next drawn frame finds no pending writes from it. A diff-gated write made in the skipped frame is visible to the next drawn frame. Two failures in a row each submit only their own frame's writes. A test-only acquire outcome drives the skip path headless (pin P9). | acquire outcome injection and diff-gated readback | achievable as stated | pass |
| 23 | A lightmap drain that fails and rolls back submits nothing. The writes staged before it land in the next submit of the same frame, once (pin P1). | failed lightmap drain rollback integration | achievable as stated | pass |
| 24 | No uploads from a completed frame remain pending when the next frame’s writes begin. Every successful or skipped windowed frame and every successful capture returns with no pending write, and hot-reload commit, level install and level unload begin with none. A debug build fails loudly if a write remains at those boundaries. Current-frame pre-render writes are allowed at render/capture method entry; fatal exits retain the existing exception (pin P6). | frame/lifecycle empty assertions | achievable with owner-approved exit/lifecycle checks | pass |
| 25 | Back-to-back submits with no GPU progress take distinct staging buffers. A buffer rejoins the pool only after the submit that read it completes, and the batch path calls no device.poll (pin P5). | back-to-back staging identity and callback tests | achievable as stated | pass |
| 26 | Capture warmup, sample and PNG captures each submit their own writes with their own work. No write leaks into the next capture or applies twice. | capture warmup/sample/PNG integration tests | achievable as stated | pass |
| 27 | After warmup, a long run of frames with two frames in flight creates no new staging buffer. The test keeps two in flight by waiting on the submission from two frames back. Live staging buffers, pooled plus in flight, stay at or under four per batch-carrying submit per frame. The byte scratch and the copy list stop growing, and acquire builds no per-call list. | long-run two-in-flight buffer/scratch/copy capacity test | achievable as stated | pass |
| 28 | After warmup, the per-frame batch's acquire, record and recycle make no heap allocation in renderer code. A counting allocator around a long run of frames proves it. The count leaves out wgpu's boxed map callback, and the test names that exclusion. | existing approved counting allocator, scoped renderer storage windows; exclude wgpu callback allocation | achievable as stated | pass |
| 29 | A frame whose batch outgrows every pooled buffer gets a fresh buffer. Afterward the per-frame pool's free list stays under its named steady-state cap. | oversized batch and bounded free-pool test | achievable as stated | pass |
| 30 | Measured finding, on the compatibility-floor Mac under `research.md` §Measurement conditions, on both maps: `[CpuTiming]` medians for `work`, `render_submit`, `render_record` and `render_prep`, before and after. Take both before the sibling briefs in the landing order land. | owner compatibility-floor Mac: before/after timing on both maps | manual, blocks landing | outstanding matched-state repeat; qualified final observations recorded |
| 31 | Measured finding, on the compatibility-floor Mac on both maps: writes staged per frame, copies per batch, and live staging buffers after warmup over one 120-frame window. Record them in the plan of record. They set the merge follow-up's gate. | owner compatibility-floor Mac: 120-frame upload/copy/live-buffer counters | manual, blocks landing | pass — final warm 120-frame counters on both maps |
| 32 | A `sample` profile after the change shows `maintain` freeing only glyphon's and egui's staging buffers. Report what share of `render_submit` remains. | owner compatibility-floor Mac: post-change sample attribution | needs restatement; exact proposal below | fail as written; profile complete, owner clarification pending |
| 33 | Side by side on both maps, no visual difference in: HUD text, skinned meshes and their shadows, smoke, dynamic and animated lights, fog, viewmodel. | owner side-by-side visual check on both maps | manual, blocks landing | partial — positive campaign walkthrough; hallway and full comparison coverage outstanding |
| 34 | Both follow-ups are filed: redundant every-frame writes, starting with the bridge-then-slot lights rewrite; and per-target mesh write merging, gated on the measured copy count. | file both follow-ups in context/plans/drafts | achievable as stated | pass — both drafts filed and final total-copy gate recorded |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Thin slice: reusable deferred batch, mesh/UI routing, first-slice batch size/copy/timing measurement; choose named pool cap | integrating executor | — | implemented; size/recycling and final counters verified, matched-state timing repeat outstanding (AC30) |
| 2 | Route every writer and every submit; preserve drain, acquire-failure, capture and lifecycle order; remove duplicate mesh light params | integrating executor | 1 | implemented; focused GPU ordering/lifecycle tests pass |
| 3 | Source drift gates, ordering/lifetime/resource tests, real-map offscreen inventory and golden verification; file follow-ups | integrating executor; bounded source-audit worker | 2 | complete; repaired drift/lifetime/pool/inventory proofs and existing goldens passed |
| 4 | Review-readiness gate, review-panel/fix loop, final preflight; record every AC result and external runbook | integrating executor | 3 | complete automated gates; AC30/33 external proof and AC32 owner clarification outstanding |

## Hot-path constraints
- Reuse shared StagedUploads/StagingPool; retain byte scratch and copy storage. No target deduplication or merging in the per-frame batch.
- Acquire makes no per-call list; recycle uses map callbacks and never polls the device. Bound free storage with a named per-frame cap selected from measured first-slice batch size.
- Zero pending writes yields no staging acquisition or extra command buffer. Submit consumes a pending batch exactly once, before other work.
- Existing every-frame writers remain as contracted; redundant writer follow-ups own future suppression/merging.

## Landing
- Manual AC 30–33 require compatibility-floor Mac timing, counters, sampling and visual proof. The brief gives no permission to land with gaps, so status becomes test-ready until those results arrive.
- Owner says “land the plane” after results and review to authorize the landing checkpoint.

## Resolved contract check — P6 / AC 24

Owner approved the renderer-internal completion approach on 2026-10-01 (“Let’s proceed and continue”). P6 now forbids uploads from a completed frame carrying into the next one. Assert empty at successful/skipped windowed exits, successful capture exits, and lifecycle boundaries. Valid current-frame prewrites at render/capture entry are allowed. Every nonfatal frame exit consumes the batch; tests cover each exit path. No public begin-frame hook and no binary-facing Renderer API change.

Source rechecked: option setters precede extent commit, light/fog/uniform/viewmodel writes precede render entry, and capture setup precedes capture entry. These writes belong to the upcoming frame. Fatal event-loop exits remain the brief's existing exception. This source check preceded implementation; renderer changes below implement the clarified boundaries.

## Implementation evidence — 2026-10-01

Shared `render/uploads` owns `StagedUploads`, `StagingPool`, and `UploadQueue`. The queue records pending frame copies into one command buffer and prepends it at the first subsequent submit. Raw submits have one source site. Installation scopes keep resource creation direct. Window success, acquire failure, captures and lifecycle entries enforce the approved empty-boundary contract. Mesh light params are hoisted once before either mesh pass.

### Pool sizing and resource bounds

The ignored campaign inventory ran four production scene frames (two warmup, two counted): 265 staged writes/copies, 3,053,232 bytes, four batches, peak batch 771,180 bytes, three created/live staging buffers. This peak rounds to the 1 MiB class. The named per-frame free cap is four buffers / 4 MiB; small uniform batches use a 64 KiB minimum. Oversized transients can allocate but return storage remains capped. This real-map test supplements the requested windowed 120-frame measurement, which remains AC 31.

The warmed allocation proof ran 64 warmup + 240 measured frames, 64 writes/frame, with two frames in flight. Created/live staging stayed at two, byte/copy capacities stayed (2048, 64), and renderer writer/acquire/record/recycle/callback allocation counts were zero. Individual wgpu API internals, including its boxed map callback, are excluded; a positive control verifies the counter and nested exclusion. The separate 160-frame lifetime test stabilized at three created/live buffers.

### Focused verification ledger

Executed on this compatibility-floor Mac, AMD Radeon Pro 5300M / Metal; each filter matched the reported tests. Authoritative GPU runs require access beyond the filesystem sandbox. One sandboxed inventory attempt explicitly self-skipped because Metal exposed no adapter; it is excluded from proof. Its rerun with adapter access passed.

| Filter | Passed tests | GPU tests executed | Adapter skips in authoritative run |
|---|---:|---:|---:|
| final `uploads::` with dev-tools | 42, two fixtures ignored | 24 | 0 |
| `upload_order` with dev-tools | 3 | 3 | 0 |
| `lighting::lightmap::stream::tests` | 14 | 14 | 0 |
| `render::ui::` | 12 | 7 | 0 |
| `resolve_composite_gpu_test` | 6 | 6 | 0 |
| final `render_extent_gpu_test` | 9 | 5 | 0 |
| `animated_atlas_parity_test` | 2 | 2 | 0 |
| final ignored `real_map_steady_state` (all-resident modes) | 1 | 1 (two counted frames) | 0 |
| final ignored `real_map_streamed_scene_stages_every_indirect_promotion_and_animated_writer` | 1 | 1 (two counted frames) | 0 |

The ignored streamed campaign companion passed on Metal after correcting test preload to compose/publish owners between drains. It decodes all actual manifest chunks, uses production residency drains and scene recording, and asserts all five streamed writer sites, including both shared-grid callers, in two counted frames. The supplemental six-call/five-site streamed writer fixture also passed. Initial preload-only fixture failure is excluded from the passing ledger.

`cargo check -p postretro-renderer --features dev-tools` and release engine build passed. Review and final focused reruns completed. Final preflight passed. Final logs are `/private/tmp/postretro-upload-final-{uploads,legacy,streamed,render-extent,frontend-reload}.log`. The binary frontend source-order gate matched one test and passed (1,142 filtered). Across the ledger, 63 distinct GPU test functions executed, with zero adapter skips in authoritative runs; repeated tests count once.

### Manual measurement state

Same current baseline source: 4f1015cd5 (before renderer changes); copied release binary retained outside the shared target. No sibling brief has landed. Both maps use the research spawn poses, 1280×720 logical window, vsync on, persisted auto render resolution, no Tracy or GPU timestamp instrumentation. Preliminary baseline medians over the last ten 120-frame windows (ms):

| Map | work | render_submit | render_record | render_prep |
|---|---:|---:|---:|---:|
| campaign-test | 10.504 | 4.7295 | 2.9575 | 2.070 |
| stress-warren-hallway-inspection | 13.325 | 6.9015 | 2.9735 | 1.424 |

These are baseline-only observations, not a before/after finding. The Mac subsequently locked; windowed redraw is suspended (IOConsoleUsers reports screen_locked=true). After timing, 120-frame counters, sample attribution and side-by-side visual checks remain outstanding. Retake a contemporaneous baseline if idle VRAM state differs materially. GPU timings are unavailable on this adapter; release builds contain no Tracy. The normal release profile strips symbols, so the attribution run must retain release symbols (same optimized settings) and run separately from timing.

### Preliminary windowed runs — superseded pending review repair

Screen unlock restored rendering. Paired untraced CPU-only release runs completed on both maps, ten final 120-frame windows per observation:

| Map | Build | work | render_submit | render_record | render_prep |
|---|---|---:|---:|---:|---:|
| campaign | before | 14.100 | 6.6205 | 3.837 | 2.5955 |
| campaign | batched, before review fixes | 9.4125 | 4.8085 | 1.3815 | 2.146 |
| hallway | before | 15.7125 | 8.043 | 3.479 | 1.7415 |
| hallway | batched, before review fixes | 11.7435 | 7.7325 | 1.0715 | 0.7195 |

These do not close AC 30: the review found capped-pool size-class starvation and the final release must be rebuilt/re-measured. Thermally limited machine state is a material qualifier: hallway GPU temperature 89°C in both measured states, CPU speed limit 58% before / 60% after, available VRAM about 36 MiB / 15 MiB. No profiler or compilation in selected windows. Earlier baseline-only runs had a different idle VRAM state and are not used for the paired finding.

Separate upload diagnostics exposed the production reproduction: campaign stabilized around 54 writes/copies per frame, ~355 KiB/frame, created/live=5; hallway stabilized at 71 writes/copies, 567,496 bytes/frame, live=6 **but created rose by 120 every 120-frame window** (5965→6085). These are evidence for the review fix, not steady-state resource acceptance. Re-run AC 31 after repair.

At the preliminary checkpoint, the owner had not yet compared both maps. The subsequent campaign walkthrough result is recorded below; AC33 remains open for the remaining comparison coverage.

### Review and repair checkpoint

The panel approved the code after resolving ten findings: pool size-class starvation, valid frontend option uploads crossing a reload boundary, source-gate shadow bindings, lifetime-proof sequencing, two missing strict real-map writer checks, dev-tools setter ordering, adapter-only extent skipping, and three documentation corrections. Panel workers ran no Cargo; approval does not substitute for the final test or manual gates. The panel covered all four diff slices and the streaming/frame-ownership seams. Some nested dispatches used preserved Sol medium workers after fresh xhigh workers were unavailable. Fresh root-dispatched Sol xhigh resource and frame-ownership reviews subsequently completed.

The final resource review found a second size transition: a retained 4 MiB transient could starve recurring 1 MiB or 2 MiB batches. Acquire now drops an oversized retained class only when it prevents two requested classes fitting the existing free-byte cap. Four GPU transition cases cover both sizes and callback orders, 32 warmup plus 128 measured frames, stable creation and zero renderer allocations. A separate test preserves larger-buffer reuse when both classes fit. The xhigh reread found no remaining issue. Final frame-ownership review found no issue; frontend proof remains source ordering plus actual renderer option/submission boundaries, not a full ActiveEventLoop execution.

The newly strict legacy inventory exposed missing real promotion input. Its mesh now sits inside a selected authored light's actual influence, with a nearby camera; it requires that same selection to appear in production-generated promotion records. No renderer promotion state is injected. Its private fixture module path was corrected before the final format check, which passed. The touched renderer and binary check with dev-tools passed. The stronger legacy and streamed inventories, 42 upload tests, nine extent tests and binary frontend-order gate all passed before final preflight.

### External visual runbook (AC 33)

Compare the baseline and rebuilt final batched release at the same poses on both maps, 1280×720 logical window, persisted auto resolution and vsync on. Baseline source is 4f1015cd5; the preserved binary is `/private/tmp/postretro-upload-baseline-bin/postretro`. The final binary will be `/private/tmp/postretro-upload-final-bin/postretro`. Launch from the workspace root, one build at a time:

```sh
/private/tmp/postretro-upload-baseline-bin/postretro content/dev/maps/campaign-test.prl --start-pose=-65.8368,1.8288,-45.9232,-90,0
/private/tmp/postretro-upload-final-bin/postretro content/dev/maps/campaign-test.prl --start-pose=-65.8368,1.8288,-45.9232,-90,0
/private/tmp/postretro-upload-baseline-bin/postretro content/dev/maps/stress-warren-hallway-inspection.prl --start-pose=42.2656,2.4384,63.3984,0,0
/private/tmp/postretro-upload-final-bin/postretro content/dev/maps/stress-warren-hallway-inspection.prl --start-pose=42.2656,2.4384,63.3984,0,0
```

Check HUD text, skinned meshes and their shadows, smoke, dynamic and animated lights, fog and the viewmodel. Report each map separately. GPU ordering/readback tests and unchanged UI/resolve goldens supplement this check; they cannot infer its result. Owner's current result: positive campaign walkthrough (details below); hallway comparison remains outstanding.

### Final optimized observations and resource counters

Final release build passed in 5m48s using `CARGO_PROFILE_RELEASE_STRIP=none cargo build --release -p postretro`; optimization/LTO are unchanged, symbols retained for separate sampling. Baseline and final copied binaries ran consecutively per map. CPU-only runs used no upload diagnostic or profiler. Each observation used the last ten of thirty 120-frame windows, with twenty windows of warmup; no compilation ran during measurement. Poses/settings are the external runbook above.

| Map | Build | work ms | render_submit ms | render_record ms | render_prep ms |
|---|---|---:|---:|---:|---:|
| campaign | baseline 4f1015cd5 | 16.0735 | 7.6220 | 4.2905 | 2.9380 |
| campaign | final batched | 14.3380 | 7.4800 | 1.8750 | 3.1905 |
| hallway | baseline 4f1015cd5 | 17.7720 | 9.1400 | 3.8930 | 1.9645 |
| hallway | final batched | 12.5760 | 7.5610 | 1.2645 | 0.8845 |

These are qualified observations, not a strictly matched-state causal comparison. All runs were thermally limited. At selected-window start/end, campaign CPU speed limits were 52→40% before / 36→32% after, GPU temperatures 93→89°C / 88→86°C, free VRAM 72→73 MiB / 187→147 MiB. Hallway CPU limits were 54→50% / 48→46%, GPU temperatures 84→83°C / 85→84°C, free VRAM 174→168 MiB / 161→138 MiB. Idle states also varied: campaign in-use VRAM ~1.83 GiB / 1.65 GiB and free ~509 MiB / 406 MiB; hallway ~1.69 GiB / 1.34 GiB and free ~894 MiB / 2.26 GiB. AC30 therefore retains a matched-state repeat as outstanding. Logs, timestamps and start/end state captures are `/private/tmp/postretro-upload-final-{before,after}-{campaign,hallway}*`, with extracted metrics in `/private/tmp/postretro-upload-final-metrics.json`.

Separate diagnostic runs warmed for twenty windows and retained ten consecutive 120-frame windows. Every retained window carried exactly 120 batches and zero direct renderer writes.

| Map | Staged writes/frame (median; range) | Copies/batch | Bytes/frame (median) | Live staging max | Total created (constant across ten windows) |
|---|---:|---:|---:|---:|---:|
| campaign | 53.63; 53.37–53.81 | same as writes | 354,640.465 | 5 | 5 |
| hallway | 71.00; 71.00–71.00 | 71.00 | 567,496 | 5 | 16 |

The final hallway created count remains 16 across all ten windows; the repaired pool eliminates the earlier +120/window churn. The free cap is four buffers / 4 MiB, distinct from live buffers including in-flight work; the controlled two-in-flight proofs retain their separate live/allocation bounds. AC31 passes. Mesh-specific copy cost remains a gate for the draft follow-up, not inferred from total copy counts.

### Final sampling and AC32 clarification request

Separate 10-second, 1 ms `sample` captures ran after diagnostic warmup on both maps. Rust symbols were demangled using the cached rustc-demangle 0.1.27 source in a temporary standalone helper; no repository/dependency change. Inclusive sample counts over all offsets of the UploadQueue submit stack:

| Map | UploadQueue submit | Device maintain | TempResource drop | maintain/submit | temp-drop/submit |
|---|---:|---:|---:|---:|---:|
| campaign | 624 | 382 | 347 | 61.22% | 55.61% |
| hallway | 700 | 430 | 418 | 61.43% | 59.71% |

These are sampled shares, not absolute timings or calibrated attribution to individual freed buffers. The sampled creation stacks contain StagingBuffer::new beneath wgpu indirect-validation injection (54/70 campaign/hallway samples) and glyphon preparation (93/86); no other staging-creation owner appears. Pool acquire was sampled once on campaign and zero times on hallway; it is not evidence of buffer creation, and counter creation stayed flat. No egui path exists in the default-feature measurement build. `TempResource` destruction is shared, so sampling alone cannot split drop cost by owner. Pinned wgpu-core 29.0.1 `src/indirect_validation/draw.rs:264,278,363` creates these validation staging buffers and retains them as TempResource. The research explicitly excludes this validation cost from batching.

AC32's literal “only glyphon's and egui's staging buffers” therefore fails as written. Owner clarification requested, not yet applied to index.md: **“A sample profile attributes remaining temporary staging churn to glyphon, egui and wgpu indirect validation, with no renderer per-frame queue-write staging creation. Report maintain's share of render_submit.”** No renderer implementation change is needed for that clarification. Raw/demangled captures are `/private/tmp/postretro-upload-final-counters-{campaign,hallway}.{sample,demangled}.txt`; count summary is `/private/tmp/postretro-upload-final-profile-metrics.json`.

Final preflight ran after all measurement engines exited, sequentially: `cargo fmt --check` passed; `cargo clippy --target-dir target/preflight-clippy -- -D warnings` passed in 33.99s; `cargo test` exited 0 with 9,043 passed, zero failed, 39 ignored across 60 harness summaries including doc tests (build 10m58s). No code edits were needed. Default-feature renderer suite: 788 passed / four ignored; binary suite: 1,107 passed / five ignored. Ordinary ignores are not inferred passes; the two required real-map upload fixtures were explicitly executed in the dev-tools focused ledger. Full-suite output alone is not used to infer adapter execution; the focused authoritative ledger supplies it. Logs: `/private/tmp/postretro-upload-final-preflight-{fmt,clippy,test}.log`. After compile completion, only idle Cargo incremental caches were removed (15 GiB active-target cache and 491 MiB clippy cache); free disk recovered to 22 GiB. Full targets, artifacts and source were retained.

### Remaining matched-state timing runbook (AC30)

Use the preserved baseline/final commands in the external runbook, prefixed with `POSTRETRO_CPU_TIMING=1 RUST_LOG=info`, and redirect each build/map to a separate log. Keep the window foreground at the pinned pose, vsync on, auto resolution, 1280×720 logical window. Run no compiler, upload diagnostic, Tracy or profiler during the timing windows. Warm twenty 120-frame windows and retain the next ten; take medians of the average before `/` for work, render_submit, render_record and render_prep.

With the engine closed before each observation, record idle GPU utilization/in-use/free VRAM using `ioreg -r -c IOAccelerator -a`, and thermal/CPU limits using `pmset -g therm`. Stabilize the background GPU load and allow thermal recovery; do not treat a materially different idle memory/thermal state as a matched pair. Record the same state at the beginning and end of selected windows. The current host's variable idle memory and severe throttling prevented a strict match in the final consecutive pair, so the recorded medians alone do not close this row. No engine/GPU timestamps are required or available on this adapter.

### Results checkpoint and blocking state

All 34 acceptance rows have results: 31 pass, AC30 and AC33 retain outstanding external proof, and AC32 fails its literal owner list with an exact proposed clarification awaiting owner direction. `status: blocked` records the owner-owned AC32 wording issue; after approval, apply only the approved wording and set `test-ready` for remaining manual proof. The implementation/review/focused/preflight gates are complete. No source work remains unless new evidence identifies a concrete defect. The feature remains on `codex/per-frame-upload-batching`; the brief stays in `in-progress/`. Baseline worktree and both copied binaries are retained for the external checks. Do not land or remove those artifacts until blocking results arrive and the owner says “land the plane.”

### Owner visual evidence — campaign walkthrough, 2026-10-01

Owner report: “Walkthrough of campaign-test looked solid. No flickering or stripes. Fog and smoke effects all look as I remember. Character models were the same, and animated SH probes hit models as before.”

Record positive campaign evidence for fog, smoke, character appearance and animated SH lighting, with no observed flickering or stripes. This report compares against remembered appearance; the exact build and a fresh baseline/final side-by-side comparison were not specified. HUD text, character shadows and the viewmodel were not separately reported, so no unobserved feature is inferred as passed. Hallway evidence remains outstanding. AC33 retains a partial result; automated proof and the AC30/AC32 dispositions are unchanged.
