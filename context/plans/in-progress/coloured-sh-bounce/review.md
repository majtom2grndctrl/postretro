# Review Panel Summary

**Target:** coloured-sh-bounce, `3275d1480..580caa899`.
**Panel:** one correctness tracer at high effort and one hygiene/drift reviewer at medium effort per slice, GPT-6.1 Sol. Initial reviewers were isolated; later slices reused the same role and review context, as authorized, without implementation history.
**Scope:** five stat-grounded slices plus cross-slice seam tracing. Source and checked-in proof review only; no Cargo, builds, tests, visual checks or performance runs were executed by the panel. `work/` was excluded. Owner amendments in the brief govern; old blocked/paused notes are historical.
**Verdict:** request changes for the bounded repair/proof issues below. No must-fix production lighting defect was found in the five slices or the named adaptive-quality seam.

## Code Review Findings

### 🟡 Should fix

1. **Repair cached slots that cannot support statistics.** (Correctness tracer; 1 reviewer.) `crates/level-compiler/src/texture_mips/world.rs:231–237`, acceptance at `220–223`; cross-slot check in `surface_materials/stats.rs:103–109`.

   With valid matching 4×2 diffuse and emissive source PNGs, transpose only the cached emissive slot to 2×4 while preserving its bundle hash. This shape retains valid mip counts and payload lengths, so every declared slot parses and the cached sidecar is accepted. Source-header validation still sees matching PNGs. The new stats call then rejects the cached cross-slot dimensions and propagates the error out of compilation instead of rebaking from the valid sources. Structural acceptance predates this change; the fatal transition through statistics is new. Treat cached inputs incompatible with stats as a miss and repair them before deriving means. This is narrower than parser-invalid/truncated/hash-stale sidecars, which already rebake correctly.

2. **Pin the old compiler used by the empty-case comparison tests.** (Hygiene/proof reviewer; 1 reviewer.) `crates/level-compiler/tests/coloured_bounce_empty.rs:34–45`.

   `grey_compiler()` accepts an existing file from the environment override or saved location without verifying its identity. Pointing `POSTRETRO_GREY_PRL_BUILD` at the current compiler makes all three old-versus-new comparisons self-comparisons. Verify the selected executable against the existing saved-compiler SHA256 pin, as the uniform-id41 oracle already does. This is a proof robustness gap, not evidence that the reported readiness run used the wrong executable.

## Comment Drift Findings

### 🟡 Comments that need updating

1. `crates/level-compiler/src/bvh_build.rs:40–41`: “`material_bucket_id` is currently the face's texture index; the struct shape is stable if bucket criteria expand.” The new lookup at `sh_bake.rs:715` requires that mapping. State it as the bounce-material contract; future bucket expansion must preserve or explicitly carry texture identity. Current producer and consumer agree.

2. `context/lib/build_pipeline.md:231`: “These pre-atlas stages consume position-only geometry”. SH now reads face texture identity and material means. Preserve the density-independence conclusion, but explain that the stages do not consume atlas UVs/block IDs and SH additionally keys texture means.

3. `crates/renderer/src/render/uploads/renderer_tests/indirect_scale.rs:83`: `"[UploadProof] indirect scale: 3 adapter cases ran"`. The test uses one actual adapter through one renderer and exercises three scale states and receiver buffers. Describe that coverage accurately. Historical plan summaries repeating the adapter claim also need correction.

### 🟢 Suggested improvements

- Replace movable plan pointers in permanent file headers with the durable build-pipeline contract: `surface_materials/stats.rs:2`, `texture_mips/world.rs:2`, `sh_bake/colour_tests.rs:2`, `sh_bake/colour_key_tests.rs:2`, `coloured_bounce_test_fixtures.rs:2`, `sh_coarsen/colour_reference_tests.rs:2`, and line 2 of the three `tests/coloured_bounce_{cache,empty,oracle}.rs` files. Links are valid now; this is future routing hygiene.
- Adjacent pre-existing `texture_mips.rs:64–65`: “Re-clamped after the encode in case / the polynomial leaves an out-of-range value at the seam.” The encoder uses piecewise linear/power functions. Replace “polynomial” with “encode” or describe rounding.

## Coverage and Triage

| Slice | Stat-grounded scope | Named flow / surface | Result |
|---|---|---|---|
| A | Stats, shared material move, texture-mips extraction/metrics; about 1k lines | Diffuse/emissive mip 0 → fixed record → snap/saturation → lookup/key metadata; per-name glow and grey fallback | Both approve production; header/adjacent wording nits |
| B | World texture loop and material-stage tests; 943 lines | Source resolution → sidecar validation/rebake → stats/accounting/lifetime | Cached semantic mismatch repair issue; otherwise approve |
| C | Shared SH estimator, colour/key/parity/empty tests and fixtures; about 1.3k lines | BVH hit material → shared sampler → base/delta integrators → uncompressed coefficients | Both approve production; BVH contract documentation issue |
| D | Pipeline/cache wiring and integration/oracle proofs; about 1.5k lines | Admission/animation validation → materials → SH keys/bakes → fluid append → reflection/pack/report | Production approved; old-binary proof pin issue |
| E | Adaptive storage, renderer and durable docs; remaining scope | Frame scale snapshot → uploads → five surface consumers, fog exclusion and reflection mirror | Production approved; durable-doc/proof-label drift |

D also traced the full material-to-cache seam across A/B/C/D: static-face texture indices select exactly the stats both estimators and cache fingerprints consume; both cache epochs advance; mover/fluid-only mean edits stay outside the fingerprint; fluid names append without shifting indices, and the original key/accounting object reaches reflection and packing. Admission and animation validation precede texture access. No-cache/release and no-light/empty states preserve their intended output/status boundaries.

## Adaptive Quality Seam

**No finding.** The change preserves the existing reference-space guarantee; it does not establish a rendered-pixel error guarantee.

The coarsening denominator is explicitly raw base indirect + direct + all delta sections (`sh_coarsen.rs:322–331`). Cancellation-free reconstruction residual is scored against that reference and darkness floor (`sh_runtime_envelope_scoring.rs:5–12`). The runtime envelope scores the actual compacted f16 representation, rejects remaining failures and checks participating seams (`sh_runtime_envelope.rs:174–187`). Protection/refinement cannot repaint dense direct samples; the new colour-reference regression checks direct identity alongside the existing quality/protection/seam rules.

The loader validates id41 and its storage ceiling (`level-loader/src/prl_loader.rs:2411–2442`); the compose carrier preserves levels and payload addressing (`render-cpu/src/sh_compose.rs:359–388`). Runtime subtraction uses clamped selection weights and reconstructs L1/L2 before subtraction (`direct_sh_compose.wgsl:293–310`, `495–507`). Actual mesh/mover consumers scale only indirect (`skinned_mesh.wgsl:631–632`, `kinematic_brush.wgsl:513–517`); billboard direct-atlas fallback is separate (`billboard.wgsl:314–322`). Thus `.33` does not rescale id41's residual. It can increase that residual's fraction of final visible illumination, so the numeric reference bound must not be described as the same percentage bound on rendered pixels. Neither the approved amendment nor the inspected tests makes that stronger promise.

The durable contract permits changed adaptive storage while preserving existing rules (`context/lib/build_pipeline.md:408–410`). Visual quality at the production scale remains an owner A/B result, not a source-proof claim. This conclusion follows producer, controller and consumer code; it is not inferred solely from raw-versus-scaled arithmetic.

## What's Done Well

- Grey absence keeps the literal old scalar multiply and operation order; resolved materials derive albedo and cache inputs from the same snapped values. Stats use fixed-order accumulation and own no texel buffers.
- Shared base/delta transport and meaningful PNG-backed colour, saturation, mixed fallback and peak-signal parity assertions cover the estimator change. The original adaptive oracle remains recorded; the separate uniform-id41 oracle pins compiler revision/hash, fixture hash, meaningful L0 shape and exact direct bytes.
- Runtime scale uses the existing 32-byte uniforms, with one frame snapshot reaching all five surface consumers exactly once. Fog remains unscaled; reflection capture mirrors the default. No new production sampled bindings, GPU buffers or allocations were introduced.

## Limits

Review is not final acceptance. Owner textured-map A/B, serial cold/warm timings, storage/performance comparison, and final preflight remain outside this panel. The readiness results were supplied by the integrating parent; reviewers inspected assertions but did not rerun them. No PRL/PRM layout change or new author-facing bounce control was identified.

Usage checked at each task boundary. At completion, 17% of the five-hour window and 31% of the weekly window remained. Review work stopped after publishing this report; follow-up fixes belong to the integrating parent.
