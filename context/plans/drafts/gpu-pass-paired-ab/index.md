# gpu-pass-paired-ab

Brief · compact · reads: `context/lib/rendering_pipeline.md` §8, §10 (Target hardware), §12 (GPU Pass Timing, Machine-state confounders) · `context/lib/development_guide.md` §4.1, §6.2, §6.4 · `context/lib/build_pipeline.md` §Distribution packaging (SDK bundle) · `context/lib/testing_guide.md` §Resource bounds · read at b9d0a4645 · evidence: `research.md`

## Problem
The developer running the `sh-compose-row-cost-spike` found that the Mac perf target (Radeon Pro 5300M, Metal, no `TIMESTAMP_QUERY`) gives reliable GPU pass deltas only from a paired A/B inside one launch: a second pipeline per pass built from alternate WGSL, alternated frame by frame, its encoders labelled `[B]` in Metal System Trace, and reduced per encoder. The A/A null read within ±0.02 ms. A reducer copied from shadow-fill-cost, which counted frames from render-pass encoders, produced a false "launch regime" finding that per-encoder reduction later retracted. That machinery exists only in a throwaway patch and a per-study copy of the scripts. Engine developers measuring a shader change on the Mac need it on main. When this is done, any opted-in renderer pass can be paired against a WGSL file on disk in any build, every opted-in pass flips arm on the same frame, and a durable reducer turns a Metal System Trace export into per-arm, per-encoder pass time and paired deltas.

## Decisions
- **Generic mechanism, opt-in per pipeline, renderer-owned.** One renderer helper builds A and an optional B for any compute or render pipeline. Passes join through an engine-closed list of pass keys, like §12's timing labels. All module and pipeline creation stays in the renderer (§4.1). At landing, both streamed compose passes and one render pass opt in. Rival: compose-only, which the next render-pass A/B (shadow-fill-cost's kind) would rebuild (`research.md` §Generality).
- **Selected by `POSTRETRO_*` env vars in every build; no cargo feature.** §6.4 puts runtime instrumentation in every build. Behavior-altering A/B toggles already ship ungated (`POSTRETRO_SDF_FORCE_VISIBILITY_ONE`, `POSTRETRO_SPEC_SHADOWMASK_FORCE_ONE` in `renderer_full_init`). Measured binaries are plain release builds, so a `dev-tools` gate would change the binary under test.
- **B is a complete module source read from a file at pipeline creation.** Paired mode names a directory holding one file per pass key, and that directory may also override A for arm-vs-arm pairs. The spike's floor split needed one (`floor/floor+no-base`). The file is handed to the device verbatim, with no helper concat or other assembly step. A dump mode writes each opted-in pass's assembled A source, so B starts from an exact copy. Each file is read once, at pipeline creation; a level reinstall reads it again. No hot reload.
- **One frame arm, owned by the renderer.** The arm flips once per recorded frame, and every opted-in pass reads it, so coupled passes (indirect compose and Pass B) always share an arm. A pass that skips a frame does not shift any other pass. In paired mode, a pass with no B file runs A on both arms and still labels by arm, so a downstream pass can be read across its upstream's arms. Rival: the spike's per-pass call counters, which desync when one pass skips a frame.
- **Labelling contract: on a B frame, each opted-in pass's encoder label is its A label plus ` [B]`.** On A frames, and whenever paired mode is off, labels stay as they are today and no B pipeline or file read exists.
- **B is exact by construction, and failure is loud.** B shares A's pipeline layout and every descriptor field except the shader module. Any of these stops the launch with a `[Renderer]` error naming the pass key and file, before that pass first dispatches: missing or unreadable directory or file, unknown pass key, parse or validation error, layout mismatch, missing entry point. It never falls back to A. A silent A/A reads as a null result (§6.2 allows initialization panics).
- **Timestamp timing never blends arms.** While paired mode is on, `POSTRETRO_GPU_TIMING` reports each paired pass as unavailable, with the reason, and leaves other passes unchanged. §12 keeps unavailable distinct from zero. Rival: per-arm timestamp pairs, a later extension (`research.md` §Timestamp interaction).
- **The reducer's metric is time per encoder per arm.** An encoder is a (label, command buffer, encoder) triple. Its time is the union of its intervals, and a coalesced row counts once. A per-frame figure requires a once-per-frame encoder named by the caller, and the reducer never infers one. A paired summary reports per-run Δ = B − A per pass, and per pass the median Δ and min..max across runs.
- **No ordering dependency on `sh-compose-array-free`.** Whichever lands second uses the other's head. That brief's paired re-measure can run on this tool once it lands.
- **Durable capture at promotion.** §12 gains a Paired A/B subsection covering surface, labelling, frame arm, metric, reducer location and the denominator lesson. §8 gains one sentence saying that B substitution is a diagnostic, not a variant mechanism. `tools/README.md` gains the reducer entry.
- **Non-goals:**
  - Per-backend or shipped shader variants: B exists only while its env var is set (§8 stays variant-free).
  - Paired timestamp timing on adapters that support it: §12's path is unchanged apart from the no-blend rule.
  - Automated perf gating in CI.
  - Host-side changes in B (upload layout, dispatch shape, bind groups): B is shader text only. `coalesced-b` needed both arms to carry its layout.
  - The launch harness (foreground checks, `caffeinate`, trace export and cleanup): `research.md` §Launch harness.
  - Re-reducing existing `measurements/` archives.

## Acceptance
### Automated
**Frame arm and labels (data logic, no GPU):**
- [ ] Over a sequence of recorded frames, the arm alternates strictly, and every opted-in pass reads the same arm in the same frame.
- [ ] One pass skipping a frame leaves every other pass's arm on later frames unchanged.
- [ ] On a B frame, the encoder label is the A label plus ` [B]`. On an A frame, it is the A label.
- [ ] A pass with no B file in paired mode resolves to the A pipeline on both arms, and its label still follows the frame arm.
- [ ] Paired mode off: no B is configured, no file is read, and every label equals today's.
**Selection and failure:**
- [ ] A file whose pass key is not engine-known fails with an error naming the file. A missing or unreadable directory fails with an error naming the path.
- [ ] A B file, or an A override, reaches module creation byte-equal to the file: no helper concat or assembly is applied.
- [ ] The dump writes, for every opted-in pass, text byte-equal to the source that pass hands to module creation.
- [ ] At pipeline creation, one log line per opted-in pass names its key, its A source (shipped, or the override's SHA-256) and its B source SHA-256, or says it has no B.
- [ ] GPU, gated by `POSTRETRO_REQUIRE_GPU` (fails under it, skips without it): a B that declares a binding absent from A's layout, a B missing the entry point, and a B that fails to parse each fail with an error naming the pass key. None of them runs A in its place.
- [ ] GPU, same gate: a B equal to the dumped A builds and dispatches on both the compute adopter and the render adopter.
- [ ] Paired mode and GPU timing together: each paired pass reports unavailable with the reason, and each unpaired pass reports as before.
**Reducer (synthetic trace-export fixtures):**
- [ ] One encoder's nested rows count as the union of their intervals, not the sum. A coalesced row counts once.
- [ ] `[B]` encoders reduce separately from A encoders under the same base label.
- [ ] Driver rows and other processes' rows are excluded.
- [ ] A fixture with render encoders missing from part of the trace gives a per-frame figure only from the named once-per-frame encoder. With none named, the reducer reports per-encoder time only. A named encoder absent from the trace is an error.
- [ ] A paired summary over identical A and B inputs reports Δ = 0. Over three runs, it reports the median and min..max.

### Manual
- [ ] Mac A/A null, plain release build with no features: B equals the dumped A for both streamed compose passes at the hallway arena (`--start-pose=21.13,2.44,30.48,0,0`), 3 paired launches. The median Δ per pass is within ±0.02 ms, reduced per encoder.
- [ ] Mac A/A null on the render adopter at a pose where it runs every frame, 3 paired launches, median Δ within ±0.02 ms.
- [ ] Mac known-different B at the arena, 3 paired launches. With the pre-change shaders on main, B is the spike's exported `array-free+unroll36` source. If `sh-compose-array-free` has landed, B is the exported baseline source and the signs flip. The median Δ is within 10% of the spike's −2.23 ms (indirect) and −2.89 ms (Pass B).
- [ ] The reducer runs from an SDK bundle's tree, with no repository checkout, on an exported trace.

## Path
Non-binding.
- **Reference implementation:** `ComposePipelines` in the spike's `probes.patch` (commit 3/7) shows A and B against one layout, the caller-chosen `[B]` label, and `arms_b`'s once-per-process env read. Drop its per-object `calls` counter.
- **Frame arm seam:** the full renderer's `debug_frame` increments once per recorded frame in `record_scene_passes`, just after `FrameTiming::begin_frame`. Derive the arm there, or keep a sibling counter.
- **Compose adopters:** `StreamingIndirectCompose::new` and its runtime `begin_compute_pass` label; `StreamingAnimatedPass::new` and the `&'static str` label it passes to `dispatch_dynamic_pass`. Labels stay static, as A and `[B]` pairs per key.
- **Failure capture:** no error scopes exist today. wgpu 30's `Device::push_error_scope` returns an `ErrorScopeGuard`. `naga` is only a dev-dependency.
- **Timing seam:** `build_frame_timing` and the `TIMING_PAIR_*` labels. Paired passes skip their timestamp writes and report the unavailable reason.
- **Reducer start point:** the spike's `gpu_time.py` (`union_ns`, row parsing) and `summarize_paired.py`, minus `ONCE_PER_FRAME` and the run-record coupling.
- **Shape and rival:** a helper owned by each opted-in pass, with the arm passed in at dispatch. Rival: a central registry that owns every paired pipeline, which would pull pass construction away from its module.
- **First slice:** the helper with the frame arm on the indirect compose pass alone, then one Mac A/A launch at the arena. Riskiest assumption: a file-sourced B and the frame-arm label reproduce the spike's null.

## Open questions
- Where the reducer lives: `tools/` Python (stdlib, ships in SDK bundles, `tools/tests` unittest that nothing runs today) vs a `postretro-tool` subcommand (Rust, `cargo test`-gated, needs an XML dependency). Recommendation: `tools/` Python, with its unittest added to preflight (`research.md` §Location options) — owner: dhiester — **blocks build**
- Env var names, file naming and pass-key spelling — **delegated**: executor decides and reports in the plan of record
- Which render pass is the render adopter (spot shadow depth is the shadow-fill-cost precedent; depth pre-pass is the simplest once-per-frame case) — **delegated**: executor decides and reports in the plan of record
- Whether the legacy (non-streamed) compose pipelines opt in at landing — **delegated**: executor decides and reports in the plan of record
