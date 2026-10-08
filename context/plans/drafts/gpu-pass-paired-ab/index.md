# gpu-pass-paired-ab

Brief · compact · reads: `context/lib/rendering_pipeline.md` §8, §10 (Target hardware), §12 (GPU Pass Timing, Machine-state confounders) · `context/lib/development_guide.md` §4.1, §6.2, §6.4 · `context/lib/build_pipeline.md` §Distribution packaging (stage 1, SDK bundle) · `context/lib/testing_guide.md` §Resource bounds · read at b9d0a4645 · evidence: `research.md`

## Problem
The developer running the `sh-compose-row-cost-spike` hit a measurement failure on the Mac perf target (Radeon Pro 5300M, Metal, no `TIMESTAMP_QUERY`). A Metal System Trace reducer copied from shadow-fill-cost took its frame count from render-pass encoder counts, undercounted, and produced a false "launch regime" finding. Reduced per encoder instead, between-launch reads agree. Six launches of one binary span 0.03–0.06 ms, and a between-launch batch matched the paired deltas. The heuristic has spread: the spike's copy, `measurements/shadow-fill-cost/gpu_time.py`, and `measurements/release-indirect-validation/runtime/parse-trace.py` on main. The spike's paired A/B runs a second pipeline from alternate WGSL, alternates per frame inside one launch and labels its encoders `[B]`. It adds a tighter null (A/A within ±0.02 ms) and immunity to machine-state swings. That matters for small deltas, such as the 0.07–0.66 ms the spike's lever projected after the contributing-row filter. This is an anticipated need. Its one firm consumer, `sh-compose-array-free`'s rule that a change to a compose kernel file needs a paired Mac re-measure, was retired with that brief on 2026-10-06. Both pieces live only in a throwaway patch and per-study script copies. When this is done, a measurement build can pair any opted-in renderer pass against a WGSL file on disk, and every opted-in pass flips arm on the same frame. One reducer kept in `tools/` turns a trace export into per-arm, per-encoder pass time and paired deltas.

## Decisions
- **Generic mechanism, opt-in per pipeline, renderer-owned.** One renderer helper builds A and an optional B for any compute or render pipeline. Passes opt in through an engine-closed list of pass keys, and all module and pipeline creation stays in the renderer (§4.1). At landing, three pipelines opt in:
  - the streamed indirect compose pass;
  - the streamed Pass B compose pass;
  - the forward world pipeline, alone in the `Textured Pass` encoder. It is the shader-only render consumer: §12 has SH sampling, which runs inside it, measured as forward-pass deltas.

  Rival: compose-only, which the next forward-shader A/B would rebuild.
- **Gated by a dependency-free `paired-ab` cargo feature; env vars select within it.** §6.4 puts diagnostic surfaces behind cargo features, and dist stage 1 builds with no non-default features, "so a payload carries no dev-tools, observability, or capture surface" (`build_pipeline.md` §Distribution packaging). The feature gates disk-loaded B and A-override sources, the frame arm, the `[B]` labels and the dump mode. It is not implied by `dev-tools`, so neither SDK bundle engine carries it. Without the feature, a set paired-mode env var logs one warning that names the var and the missing feature, then renders as if unset.
- **B is a complete module source read from a file.** Paired mode names a directory holding one file per pass key. It may also override A for arm-vs-arm pairs, as the spike's `floor/floor+no-base` split did. Each file is read once per process and handed to the device verbatim, with no helper concat or other assembly step. A dump mode writes each opted-in pass's assembled A source, so B starts from an exact copy. No hot reload.
- **Mid-session rebuilds keep both arms.** Capacity growth rebuilds the streamed compose passes (`grow_sparse` via `StreamingIndirectCompose::new`, and the Pass B replacement in `direct_compose.rs`), and so does a level reinstall. Each such rebuild builds B from the source held in memory, never from disk.
- **One frame arm, owned by the renderer.** The arm flips once per recorded frame, and every opted-in pass reads it, so coupled passes (indirect compose and Pass B) always share an arm. A pass that skips a frame does not shift another. In paired mode, a pass with no B file runs A on both arms and still labels by arm. Rival: the spike's per-pass call counters, which desync when one pass skips a frame.
- **Labelling contract: on a B frame, each opted-in pass's encoder label is its A label plus ` [B]`.** On A frames, and whenever paired mode is off, labels stay as they are today, and no B pipeline is built and no file read.
- **B is exact by construction, and failure is loud.** B shares A's pipeline layout and every descriptor field except the shader module. Any failure is an error naming the pass key and file, and the pass never falls back to A, because a silent A/A run reads as a null result. Failures are a missing or unreadable file, an unknown key, a parse or validation error, a layout mismatch, or a missing entry point. They surface on the existing paths:

  | Where the pipeline is built | How the failure surfaces |
  |---|---|
  | Renderer init (the forward pipeline) | It stops the launch (§6.2: panics in initialization are acceptable). |
  | Level install or capacity growth | It returns through those paths' existing `ShResidencyDrainError` results. |
- **Timestamp timing never blends arms.** While paired mode is on, `POSTRETRO_GPU_TIMING` reports each paired pass as unavailable, with the reason, and leaves other passes unchanged. §12 already keeps unavailable distinct from zero. Rival: per-arm timestamp pairs, a later extension (`research.md` §Timestamp interaction).
- **The reducer lives in `tools/` as Python (stdlib only), ships in SDK bundles, and preflight runs its unittest.**
  - Its metric is time per encoder per arm. An encoder is a (label, command buffer, encoder) triple. Its time is the union of its intervals, and a coalesced row counts once.
  - A per-frame figure needs a once-per-frame encoder named by the caller. The reducer never infers one.
  - A paired summary of a run with no `[B]` encoders is an error, which catches a build without the feature.
  - It replaces the per-study copies for new work.
- **Durable capture at promotion:**
  - §12 gains the Paired A/B subsection, covering the feature and env surface, labelling, the frame arm, the per-encoder metric, the denominator lesson and the reducer.
  - The subsection qualifies §12's "A/B an older commit" advice: shader-only changes use the paired method, and host-side changes keep the older-commit A/B.
  - §8 gains one sentence: B substitution is a diagnostic, not a variant mechanism.
  - §6.4's feature list and dist stage 1's excluded-surface list gain `paired-ab`.
  - `tools/README.md` gains the reducer entry.
- **Non-goals:**
  - Per-backend or shipped shader variants: B exists only in feature builds with the env var set, and §8 stays variant-free.
  - Paired timestamp timing on adapters that support it: §12's path is unchanged apart from the no-blend rule.
  - Automated perf gating in CI.
  - Host-side changes in B (upload layout, dispatch shape, bind groups): B is shader text only. The spike's `coalesced-b` needed both arms to carry its layout.
  - The launch harness (foreground checks, `caffeinate`, trace export and cleanup): see `research.md` §Launch harness.
  - Re-reducing existing `measurements/` archives.

## Acceptance
### Automated
**Gate:**
- [ ] With the feature and paired-mode env vars set, paired mode is active.
- [ ] Without the feature, the same env vars produce one warning naming each set var and the missing feature. No file is read, no B pipeline is built, and labels equal today's.
- [ ] `paired-ab` is in no crate's default feature set, and dist stage 1's release builds pass no feature.

**Frame arm and labels (data logic, no GPU):**
- [ ] Over a sequence of recorded frames, the arm alternates strictly, and every opted-in pass reads the same arm in the same frame.
- [ ] One pass skipping a frame leaves every other pass's arm on later frames unchanged.
- [ ] On a B frame, the encoder label is the A label plus ` [B]`. On an A frame, it is the A label.
- [ ] A pass with no B file in paired mode resolves to the A pipeline on both arms, and its label still follows the frame arm.
- [ ] Feature on but paired mode unset: no B is configured, no file is read, and every label equals today's.

**Sources and failure:**
- [ ] A file whose pass key is not engine-known fails with an error naming the file. A missing or unreadable directory fails with an error naming the path.
- [ ] A B file, or an A override, reaches module creation byte-equal to the file.
- [ ] The dump writes, for every opted-in pass, text byte-equal to the source that pass hands to module creation.
- [ ] At pipeline creation, one log line per opted-in pass names its key, its A source (shipped, or the override's SHA-256), and its B source's SHA-256 or that it has none.
- [ ] GPU, gated by `POSTRETRO_REQUIRE_GPU` (fails under it, skips without it): a B that declares a binding absent from A's layout, a B missing the entry point, and a B that fails to parse each fail with an error naming the pass key. None of them runs A in its place.
- [ ] GPU, same gate: a B equal to the dumped A builds and dispatches for a compose adopter and for the forward pipeline.
- [ ] GPU, same gate: a capacity-growth rebuild of a paired compose pass builds both arms from the held source. Deleting the file after first creation changes nothing.
- [ ] With paired mode and GPU timing both on, each paired pass reports unavailable with the reason, and each unpaired pass reports as before.

**Reducer (synthetic trace-export fixtures, preflight-run unittest):**
- [ ] One encoder's nested rows count as the union of their intervals, not the sum. A coalesced row counts once.
- [ ] `[B]` encoders reduce separately from A encoders under the same base label.
- [ ] Driver rows and other processes' rows are excluded.
- [ ] A fixture with render encoders missing from part of the trace gives a per-frame figure only from the named once-per-frame encoder. With none named, the reducer reports per-encoder time only. A named encoder absent from the trace is an error.
- [ ] A paired summary over identical A and B inputs reports Δ = 0. Over three runs, it reports the median and min..max.
- [ ] A paired summary of a trace with no `[B]` encoders is an error.
- [ ] Preflight runs the reducer's unittest, and a failing reducer test fails preflight.

### Manual
- [ ] Mac A/A null, release build with `paired-ab`: B equals the dumped A for both streamed compose passes at the hallway arena (`--start-pose=21.13,2.44,30.48,0,0`), 3 paired launches. The median Δ per pass is within ±0.02 ms, reduced per encoder.
- [ ] Mac A/A null on the forward pipeline (`Textured Pass`) at the same pose, 3 paired launches, median Δ within ±0.02 ms.
- [ ] Mac known-different B at the arena, 3 paired launches. A is main's compose shaders, and B is the spike's exported `array-free+unroll36` source. The median Δ is within 10% of the spike's −2.23 ms (indirect) and −2.89 ms (Pass B).
- [ ] The reducer runs from an SDK bundle's tree, with no repository checkout, on an exported trace.

## Path
Non-binding.
- **Reference implementation:** `ComposePipelines` in the spike's `probes.patch` (commit 3/7) builds A and B against one layout, picks the `[B]` label at the call site, and reads its env var once per process (`arms_b`). Drop its per-object `calls` counter.
- **Frame arm seam:** the full renderer's `debug_frame` increments once per recorded frame in `record_scene_passes`, just after `FrameTiming::begin_frame`.
- **Compose adopters:**
  - `StreamingIndirectCompose::new`, called from `gpu/setup.rs` and from `grow_sparse` in `gpu/growth.rs`, with its runtime `begin_compute_pass` label.
  - `StreamingAnimatedPass::new`, called from `direct_compose.rs` at install and on id-45 growth, and the `&'static str` label it passes to `dispatch_dynamic_pass`.

  Labels stay static, as an A and `[B]` pair per key.
- **Forward adopter:** `build_renderer_pipelines` builds the `Textured Pipeline` once, from `SHADER_SOURCE` or its `strip_point_shadow_cube` form. The `Textured Pass` in `record_scene_passes` sets only that pipeline. The dump captures whichever form this adapter assembled.
- **Failure capture:** no error scopes exist today. wgpu 30's `Device::push_error_scope` returns an `ErrorScopeGuard`, and `naga` is only a dev-dependency.
- **Timing seam:** `build_frame_timing` and the `TIMING_PAIR_*` labels.
- **Feature plumbing:** a renderer feature forwarded from `postretro`, beside `capture` and `observability`. `xtask` `build_release` passes no features today.
- **Reducer start point:** the spike's `gpu_time.py` (`union_ns`, row parsing) and `summarize_paired.py`, without `ONCE_PER_FRAME` and the run-record coupling.
- **Shape and rival:** each opted-in pass owns its helper and receives the arm at dispatch. The rival is a central registry owning every paired pipeline, which pulls construction away from each pass's module.
- **First slice:** the feature, the helper and the frame arm on the indirect compose pass alone, then one Mac A/A launch at the arena. This tests the riskiest assumption: that a file-sourced B with the frame-arm label reproduces the spike's null.

## Open questions
- Env var names, file naming and pass-key spelling — **delegated**: executor decides and reports in the plan of record
- Whether the legacy (non-streamed) compose pipelines opt in at landing — **delegated**: executor decides and reports in the plan of record
