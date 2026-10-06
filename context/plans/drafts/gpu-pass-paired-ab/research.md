# gpu-pass-paired-ab — research

Read at `b9d0a4645` (origin/main). Spike evidence was read on the `sh-compose-row-cost-spike` branch checkout (`postretro-shspike`, head `dc4666a1d`). It is not on main at the read sha.

## Evidence for the need

| Claim | Source |
|---|---|
| A reducer heuristic produced a false "launch regime" finding. It took the frame denominator from the largest render-pass encoder count, which undercounts by up to 26% when render encoders are missing from part of a trace. | spike `findings.md` §Retraction; spike `plan.md` Corrections |
| Nested encoder rows were summed and coalesced rows multiplied. The fix is the union of intervals per encoder. | spike `gpu_time.py` docstring and `union_ns` |
| Per encoder, between-launch reads agree: six `diag1` launches of one binary read 6.69–6.75 / 7.99–8.02 ms, and between-launch `arena3` matches the paired deltas (`accum-scalar` −2.98 / −4.48 vs paired −2.86 / −4.38) | spike `findings.md` §Retraction |
| A/A paired null: −0.01 to +0.02 ms by median, runs to ±0.02 | spike `findings.md` §Method |
| Deltas still ahead are small: the lever on top of the contributing-row filter projects −0.07 to −0.66 ms per pass | spike `findings.md` post-filter table ("Lever on top") |
| Firm consumer: a change to a compose kernel file needs a paired Mac re-measure | `drafts/sh-compose-array-free/index.md` Decisions (owner, 2026-10-06) |
| Both compose passes flip together, so Pass B is measured with indirect on the same arm | spike `findings.md` §Method |
| With timestamps, paired mode would average A and B together | spike `findings.md` §1660 handoff ("Arm per launch") |
| A host-side layout lever (`coalesced-b`) needed A and B to both carry it | spike `README.md` §Arms |
| Known delta for the manual reproduction row: `array-free+unroll36` at the arena, −2.23 ms indirect, −2.89 ms Pass B (median of 3 paired launches) | spike `findings.md` attribution table |
| Exact assembled source of both arms, with SHA-256s | spike `measurements/sh-compose-row-cost-spike/lever-wgsl/README.md` |

### Where the denominator heuristic lives

| Copy | Denominator |
|---|---|
| spike `gpu_time.py` (copied from shadow-fill-cost, then fixed) | the compose encoder count; render-pass max kept only as a fallback |
| `measurements/shadow-fill-cost/gpu_time.py` (main) | the largest count among once-per-frame passes (`README.md` §Metrics) |
| `measurements/release-indirect-validation/runtime/parse-trace.py` (main) | `min(counts['Depth Pre-Pass'], counts['Textured Pass'])` |

Pairing does not fix a reducer. What a durable reducer fixes is the copying. What pairing adds is a tighter null and immunity to machine-state swings (§12 Machine-state confounders) for deltas near the between-launch spread.

## Spike machinery and what changes

`ComposePipelines::new` builds A from `POSTRETRO_SPIKE_ARMS` rewrites and B from `POSTRETRO_SPIKE_ARMS_B`, both against one pipeline layout. `next` alternates on a per-object call counter (`calls % 2`). Callers pick the `[B]` label from the returned flag (`StreamingIndirectCompose` runtime, `StreamingAnimatedPass` runtime via `dispatch_dynamic_pass`).

Gaps for a lasting tool:
- **Per-pass counters desync.** Each pass alternates on its own counter. The pair stays in step only while both dispatch every frame. A frame that skips one pass shifts it a half-cycle against the other, and "Pass B measured with indirect on the same arm" silently breaks. A renderer-owned frame arm removes the failure mode. The full renderer's `debug_frame` already increments once per recorded frame in `record_scene_passes`, just after `FrameTiming::begin_frame`.
- **Arms are rewrites of A.** The rewrite layer (`build_source`, `apply_arm`, `replace_n`) is spike scaffolding. A file-sourced B needs no anchors and works for any pass.
- **Compose-only.** Only the two streamed compose passes adopt it. Legacy compose pipelines get A arms but no B twin (`lever-wgsl/README.md`).
- **Rebuilds.** The spike reads arms once per process (`OnceLock`), so growth rebuilds got the same arms by accident of construction. The durable tool states it: sources are read once and held.
- **Failure is a panic from wgpu's default uncaptured-error handler.** The workspace has no `push_error_scope` or `on_uncaptured_error` today. wgpu 30 offers `Device::push_error_scope`, which returns an `ErrorScopeGuard`. `naga` is a renderer dev-dependency only, so runtime validation goes through wgpu, not a new dependency.

## Pipeline build sites for the adopters

| Adopter | First build | Rebuilds | Error path |
|---|---|---|---|
| Streamed indirect compose | `StreamingIndirectCompose::new` from `gpu/setup.rs` at level install | `grow_sparse` (`gpu/growth.rs`) when section 27 grows | `Result<_, ShResidencyDrainError>` |
| Streamed Pass B compose | `StreamingAnimatedPass::new` from `direct_compose.rs` at install | the id-45 growth replacement in `direct_compose.rs` | `Result<_, ShResidencyDrainError>` |
| Forward world pipeline | `build_renderer_pipelines`, whose only caller is `build_full_renderer`, once per process after the boot splash (`SHADER_SOURCE`, or its `strip_point_shadow_cube` form without cube arrays) | none | init: §6.2 permits a panic |

Growth builds every replacement before swapping ("a later constructor failure must leave all active buffers … untouched"), so a B failure there leaves the active pair intact. With the source held in memory and validated at first build, a growth failure needs a layout change, and none is expected.

## Generality

Pipeline creation is spread across renderer modules (`create_compute_pipeline` / `create_render_pipeline` in compose, cull, shadow, fog, bloom, mesh, UI and others), with no shared helper. A generic mechanism is therefore a small helper that each opting-in site adopts, not a hook in one place.

**The render adopter is the forward world pipeline.** §12 GPU Pass Timing: "SH sampling is not separately timestamp-bracketed because it runs inside the forward fragment shader; measure it as `forward` timing deltas." On the Mac that delta has to come from a trace. The `Textured Pass` encoder sets only the world pipeline: the kinematic brush, mesh and smoke passes record into their own encoders. So a forward B changes exactly one encoder's shader. shadow-fill-cost is not the precedent, because its change was host-side (CPU shadow reach), which the non-goals exclude.

Rival: compose-only, matching the spike exactly. It is cheaper today, and the next forward-shader A/B would rebuild it.

## Gate

§6.4, verbatim in substance:
- runtime instrumentation compiles into every build, toggled by a `POSTRETRO_*` env var;
- diagnostic surfaces sit behind cargo features. `observability`, `observe-live` and `capture` add no dependencies, `dev-tools` carries egui, and `tracy` is its own feature;
- release and dist builds enable none of them.

`build_pipeline.md` §Distribution packaging, stage 1: release binaries "with no non-default features, so a payload carries no dev-tools, observability, or capture surface". xtask `build_release` passes no `--features`. The SDK bundle's authoring engine is debug with `dev-tools` only, and its `postretro-release` follows stage 1.

Paired A/B substitutes shader source from disk. That makes it a diagnostic surface, not passive instrumentation, so it sits behind a feature like `capture`. It stays dependency-free, and `postretro` forwards it to the renderer.

**Without the feature: warn, then ignore.** Rejecting the launch would make a stray env var fatal in a player build. Silent ignore would let a measurer run A/A on a featureless binary without noticing. One warning, plus the reducer's error on a paired summary with no `[B]` encoders, covers the measurer. The warning check reads env vars only; it builds nothing.

**Not implied by `dev-tools`.** Measurement builds need not carry egui, and the SDK debug engine is unfit for timing anyway. So neither SDK engine carries the feature. The reducer still ships in `tools/` for traces from source builds.

## Frame alternation semantics

- There is one arm per recorded frame. It flips once per frame, and every opted-in pass reads it.
- A pass that skips a frame does not move any other pass's arm.
- A pass with no B file runs A on both arms but still labels by frame arm. This lets a downstream pass be read across its upstream's arms without a B of its own. An empty B directory is therefore a label-only A/A null, and a B equal to the dumped A source is the pipeline-object A/A null.
- B output lands in the same resources as A's, so persistent outputs written on a B frame feed later A frames. Exactness of B is the measurer's proof (capture byte-compare), not the tool's. An inexact B can change downstream work on later frames. The tool reports per-encoder time and makes no claim about output.

## Timestamp interaction

`FrameTiming` holds one query pair per engine-closed label (`build_frame_timing`, `TIMING_PAIR_*` in `pipeline_layout.rs`) and averages over sampled readbacks. Readbacks are not taken every frame (§12), so a paired pass's average would blend the arms in an uncontrolled ratio.

| Option | Cost | Note |
|---|---|---|
| Report paired passes unavailable, with reason paired | small | Honours the stated non-goal. §12 already distinguishes unavailable from zero. |
| Split per arm (an extra pair per paired pass) | moderate: dynamic label set, pair index chosen by arm | Would make the 1660 handoff paired. A later brief can add it. |

## Reducer

Spike `gpu_time.py` lessons kept:
- One encoder is a (label, command buffer, encoder) triple, and its time is the union of its intervals.
- A "Coalesced N Encoders" row is one interval.
- Only labelled rows (`Label:Label`) from the postretro process count. Driver rows are excluded.
- Per-encoder time per label is the metric. It uses no frame denominator.

Dropped: the once-per-frame render-pass list and its fallback. A per-frame figure needs a once-per-frame encoder named explicitly. The spike's compose encoder served because every frame dispatches it once.

`summarize_paired.py` shape kept: per run, A, B and Δ = B − A per pass; per arm, the median Δ and min..max across runs. Its run-record coupling (`run.py` fields, `[SH spike counts]`) is study-specific.

**Location: `tools/`, Python stdlib.** Precedent is `gen_stress_map.py` with `tools/tests/test_gen_stress_map.py`. `tools/` ships in SDK bundles (`build_pipeline.md` §SDK bundle). No runner wires `tools/tests` into preflight today; the preflight skill gains the reducer's unittest.

Rejected locations:
- a `postretro-tool` subcommand: needs an XML dependency in otherwise content tooling;
- `xtask`: can't ship (`build_pipeline.md` §Why the tool is not xtask);
- per-study `measurements/` copies: that copying is how the heuristic spread.

## §12 text ownership

`sh-compose-array-free` promotes a per-encoder method and a short paired method into §12 if it lands first. That brief's Decisions say `gpu-pass-paired-ab` owns the Paired A/B subsection and folds that text in when it lands. Either order leaves one subsection.

The subsection also qualifies §12 Machine-state confounders, "Then A/B an older commit under the same machine state":
- shader-only changes use the paired method;
- host-side changes (buffers, dispatch shape, CPU work) keep the older-commit A/B.

## Launch harness

`run.py` and `batch.sh` carry protocol that every Mac run needs:
- the foreground helper (`measurements/release-indirect-validation/runtime/foreground.swift`, on main);
- `caffeinate` and idle gaps;
- screen-saver and lock checks;
- `ioreg` snapshots;
- trace export and deletion.

They also carry study-specific validity checks (`rows_stable` from the spike's row counters). The harness is out of scope here, since §12 already documents the trace and confounder protocol. A later brief can lift its generic half.

## Manual tolerance

The known-different row allows 10% of the spike's paired delta. The source is byte-identical to the spike's, so codegen matches. Head drift elsewhere can move the base. The spike's per-arm spread was mostly within ±0.04 ms of a 2–3 ms delta.

## Unverified premises

- The spike's findings, patch and tools exist only on the spike branch checkout. If they move before promotion, the citations change path, not content.
- That Metal System Trace shows the `[B]` suffix unaltered was observed by the spike (its reducer matched `… [B]` labels). It was not re-run this session.
- The claim that `Textured Pass` sets only the world pipeline comes from reading `record_scene_passes` this session: one `set_pipeline` inside that encoder, and the mover, mesh and smoke passes open their own.
