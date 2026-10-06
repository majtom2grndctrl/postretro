# gpu-pass-paired-ab — research

Read at `b9d0a4645` (origin/main). Spike evidence was read on the `sh-compose-row-cost-spike` branch checkout (`postretro-shspike`, head `dc4666a1d`); it is not on main at the read sha.

## Evidence for the method

| Claim | Source |
|---|---|
| Paired A/B in one launch: a second pipeline per pass, alternating per frame, `[B]` label suffix | spike `findings.md` §Method ("Paired A/B within one launch"); `measurements/sh-compose-row-cost-spike/probes.patch` commit 3/7 (`ComposePipelines`, `arms_b`) |
| A/A null reads −0.01 to +0.02 ms by median, runs to ±0.02 | spike `findings.md` §Method |
| Both compose passes flip together, so Pass B is measured with indirect on the same arm | spike `findings.md` §Method |
| "Launch regimes" were a reducer artifact: the frame denominator came from the largest render-pass encoder count, which undercounts by up to 26% when render encoders are missing from part of a trace | spike `findings.md` §Retraction; `plan.md` Corrections |
| Nested encoder rows were summed and coalesced rows multiplied; fix is union per encoder | spike `gpu_time.py` docstring and `union_ns` |
| Per encoder there is no regime: six launches of one binary read 6.69–6.75 ms | spike `findings.md` §Retraction |
| `measurements/shadow-fill-cost/gpu_time.py` carries the same denominator heuristic | spike `findings.md` §Retraction; shadow-fill-cost `README.md` §Metrics (on main) |
| With timestamps, paired mode would average A and B together | spike `findings.md` §1660 handoff ("Arm per launch") |
| A host-side layout lever (`coalesced-b`) needed A and B to both carry it | spike `README.md` §Arms |
| Known delta for the manual reproduction row: `array-free+unroll36` at the arena, −2.23 ms indirect, −2.89 ms Pass B (median of 3 paired launches) | spike `findings.md` attribution table |
| Exact assembled source of both arms, with SHA-256s | spike `measurements/sh-compose-row-cost-spike/lever-wgsl/README.md` |

## Spike machinery and what changes

`ComposePipelines::new` builds A from `POSTRETRO_SPIKE_ARMS` rewrites and B from `POSTRETRO_SPIKE_ARMS_B`, both against one pipeline layout. `next` alternates on a per-object call counter (`calls % 2`). Callers pick the `[B]` label from the returned flag (`StreamingIndirectCompose` runtime, `StreamingAnimatedPass` runtime via `dispatch_dynamic_pass`).

Gaps for a lasting tool:
- **Per-pass counters desync.** Each pass alternates on its own counter. The pair stays in step only while both dispatch every frame. A frame that skips one pass shifts it a half-cycle against the other, and "Pass B measured with indirect on the same arm" silently breaks. A renderer-owned frame arm removes the failure mode. The full renderer's `debug_frame` already increments once per recorded frame in `record_scene_passes`, just after `FrameTiming::begin_frame`.
- **Arms are rewrites of A.** The rewrite layer (`build_source`, `apply_arm`, `replace_n`) is spike scaffolding. A file-sourced B needs no anchors and works for any pass.
- **Compose-only.** Only the two streamed compose passes adopt it; legacy compose pipelines get A arms but no B twin (`lever-wgsl/README.md`).
- **Failure is a panic from wgpu's default uncaptured-error handler.** No `push_error_scope` or `on_uncaptured_error` exists in the workspace today. wgpu 30 offers `Device::push_error_scope` returning an `ErrorScopeGuard`. `naga` is a renderer dev-dependency only, so runtime pre-validation goes through wgpu, not a new dependency.

## Generality

Pipeline creation is spread across renderer modules (`create_compute_pipeline` / `create_render_pipeline` in compose, cull, shadow, fog, bloom, mesh, UI and others) with no shared helper. A generic mechanism is therefore a small helper each opting-in site adopts, not a hook in one place.

Prior consumers already span both kinds: shadow-fill-cost measured render passes (spot/cube shadow depth and depth-cache passes, labels in `renderer_dynamic_shadow_passes.rs`), and the spike measured compute. Shadow depth passes run many encoders per frame, which is the case the per-frame denominator rule exists for.

Rival: compose-only, matching the spike exactly. Cheaper today; the next render-pass A/B rebuilds it.

## Selection surface

§6.4: runtime instrumentation compiles into every build behind `POSTRETRO_*`; surfaces that add dependencies sit behind features.

Behavior-altering diagnostic env vars already ship ungated:

| Var | Read in | Effect |
|---|---|---|
| `POSTRETRO_SDF_FORCE_VISIBILITY_ONE` | `renderer_full_init` | forces SDF visibility 1.0 for an A/B |
| `POSTRETRO_SPEC_SHADOWMASK_FORCE_ONE` | `renderer_full_init` | forces static shadowmask 1.0 for an A/B |
| `POSTRETRO_BLOOM` | `bloom_enabled_from_environment` | disables bloom |

Measured binaries are plain release builds without `dev-tools` (shadow-fill-cost `README.md` §Builds; spike `README.md` §Build adds only `capture`, for atlas dumps). A `dev-tools` gate would put egui into every measured binary or force a second build lineage.

Shipped-build exposure: with both A and B overridden from one file, the env var is a full shader substitution in a player build. It is undocumented in `docs/`, alternation makes a B-only override visibly unusable as a mod hook, and every override is logged. Noted for the owner; not argued as a blocker.

## Frame alternation semantics

- One arm per rendered frame, flipped once per frame, read by every opted-in pass.
- A pass that skips a frame does not move other passes' arms.
- A pass with no B file runs A on both arms but still labels by frame arm. This lets a downstream pass be read across its upstream's arms without a B of its own. An empty B directory is therefore a label-only A/A null; a B equal to the dumped A source is the pipeline-object A/A null.
- B output lands in the same resources as A's. Persistent outputs written on a B frame feed later A frames. Exactness of B is the measurer's proof (capture byte-compare), not the tool's. An inexact B can change downstream work on later frames; the tool reports per-encoder time and makes no output claim.

## Timestamp interaction

`FrameTiming` holds one query pair per engine-closed label (`build_frame_timing`, `TIMING_PAIR_*` in `pipeline_layout.rs`) and averages over sampled readbacks. Readbacks are not every frame (§12), so a paired pass's average blends arms in an uncontrolled ratio. Options:

| Option | Cost | Note |
|---|---|---|
| Report paired passes unavailable, reason paired | small | honours the stated non-goal; §12 already distinguishes unavailable from zero |
| Split per arm (extra pair per paired pass) | moderate: dynamic label set, pair index by arm | would make the 1660 handoff paired; a later brief can add it |

## Reducer

Spike `gpu_time.py` lessons to keep:
- One encoder = (label, command buffer, encoder). Its time is the union of its intervals.
- A "Coalesced N Encoders" row is one interval.
- Only labelled rows (`Label:Label`) of the postretro process count; driver rows are excluded.
- Per-encoder time per label is the metric; it uses no frame denominator.

Not kept: the once-per-frame render-pass list and its max-count fallback. A per-frame figure needs an explicitly named once-per-frame encoder; the spike's compose encoder served because every frame dispatches it once.

`summarize_paired.py` shape to keep: per run A, B, Δ = B − A per pass; per arm the median Δ and min..max across runs. Its run-record coupling (`run.py` fields, `[SH spike counts]`) is study-specific.

### Location options

| Option | Ships in SDK bundle | Gated by tests | Cost |
|---|---|---|---|
| `tools/` Python, stdlib only | yes (`build_pipeline.md` §SDK bundle copies `tools/`) | `tools/tests` unittest; no runner wires it into preflight today | lowest; precedent `gen_stress_map.py` + `tools/tests/test_gen_stress_map.py` |
| `postretro-tool` subcommand (Rust) | yes | `cargo test` | XML parsing dependency; the tool "compiles nothing" and is otherwise content tooling |
| `xtask` subcommand | no (`build_pipeline.md` §Why the tool is not xtask) | `cargo test` | not runnable from a bundle |
| `measurements/<study>/` | no | none | status quo; copied per study, which is how the heuristic bug spread |

## Launch harness

`run.py` / `batch.sh` carry protocol that every Mac run needs: the foreground helper (`measurements/release-indirect-validation/runtime/foreground.swift`, on main), `caffeinate`, idle gaps, screen-saver and lock checks, `ioreg` snapshots, trace export and deletion. They also carry study-specific validity (`rows_stable` from spike row counters). Kept out of scope here; §12 already documents the trace and confounder protocol. A later brief can lift the generic half.

## Manual tolerance

The known-different row allows 10% of the spike's paired delta. Source is byte-identical to the spike's, so codegen matches. Head drift elsewhere can move the base; the spike's per-arm spread was mostly within ±0.04 ms of a 2–3 ms delta.

## Unverified premises

- Spike findings, patch and tools exist only on the spike branch checkout. If they move before promotion, citations change path, not content.
- That Metal System Trace shows the `[B]` suffix unaltered was observed by the spike (its reducer matched `… [B]` labels), not re-run this session.
