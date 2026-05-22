# Compiler Bake Determinism

## Goal

`prl-build` must produce byte-identical bake output for an unchanged `.map`, across separate process invocations. Today the output drifts between recompiles: one pulsing-light surface in the complex test map stays lit when it should fade to zero, and the defect's location moves each rebuild. Root-cause the nondeterminism and fix it, then guard it.

## Background

This is a separate investigation from the SH probe-marker "rainbow flicker," which was a dev-tools readback sync bug and is fixed. The flicker work confirmed the engine's runtime lighting is stable and the delta-SH grid metadata is well-formed (dimensions match probe counts). So the remaining symptom is a **compiler/bake** problem, not a runtime one. See `research.md` for the full diagnosis trail and ruled-out paths.

## Scope

### In scope
- A reproduction harness that compiles one `.map` in **separate processes** and diffs the resulting `.prl` to confirm nondeterminism and isolate which section(s) drift.
- Finding and fixing the nondeterminism source in whichever bake stage drifts.
- Confirming the "surface stays lit" symptom resolves once the drift is fixed (or, if it is a distinct authoring issue, documenting that separately).
- A determinism guard that catches **cross-process** drift, not just in-process repetition.

### Out of scope
- Reworking the SH / delta-SH / lightmap bake algorithms beyond what determinism requires.
- Performance work on the bake.
- The dev-tools SH overlay (already a working diagnostic lens).
- Authoring-side brush cleanup of the test map (may be used to *bisect*, but fixing the map is not the deliverable — the compiler must be deterministic regardless of brush overlap).

## Acceptance criteria
- [ ] Compiling the same unchanged `.map` twice in **separate OS processes** yields byte-identical `.prl` output. Any intentionally-excluded bytes (e.g. embedded timestamps, if any) are documented.
- [ ] The root cause is identified and written down before any fix lands (which stage, which unstable-ordering construct, why it varies per process).
- [ ] In the affected room, the pulsing light's surface reaches zero contribution at its pulse minimum, and this holds across at least 5 consecutive recompiles (defect no longer migrates).
- [ ] A determinism check exists that fails when bake output varies across processes (or under a perturbed hash seed), and passes after the fix.
- [ ] The build cache's stated determinism invariant (`build_pipeline.md` §Build Cache) is upheld — identical inputs still produce identical cached output.

## Tasks

### Task 1: Reproduce and isolate the drifting section
Build a harness (script or test) that runs `prl-build` on `content/dev/maps/campaign-test.map` in two separate processes and diffs the `.prl` byte-for-byte. Report which PRL section IDs differ. This converts an intermittent visual symptom into a deterministic, located failure. The likely suspects feed surface/probe lighting: `ShVolume` (20), `DeltaShVolumes` (27), `Lightmap` (22), `AnimatedLightChunks` (24), `AnimatedLightWeightMaps` (25), or an upstream section that all bakes depend on (`Geometry` (17), `Bvh` (19), `Portals` (15), BSP). Run with `--no-cache` so the cache can't mask the bake.

### Task 2: Find the nondeterminism source in the drifting stage
Once Task 1 names the drifting section, hunt the unstable ordering in that stage. The documented risks (`build_pipeline.md` §Build Cache "Determinism invariant") are `HashMap`/`HashSet` iteration feeding output ordering and non-order-preserving parallel reductions. `std` `HashMap`/`HashSet` use a per-process random seed, so iteration order changes between processes — this matches "moves on rebuild" exactly, and in-process tests never see it. Audit the stage for any collection iterated into output order, any `par_iter` that merges in completion order, and any ordering that depends on a hashed key. Confirm the candidate by perturbing it (e.g. force a hash seed, or sort the suspect collection) and re-running Task 1's diff.

### Task 3: Resolve whether the fade-to-zero symptom is the same root cause
The base SH bake excludes animated lights, and the delta grid is multiplied by the brightness curve at runtime, so a pulse light *should* fade fully. A surface that stays lit is therefore either (a) a probe/texel that received a wrong value from the nondeterministic bake (same root cause as Task 1/2), or (b) a distinct issue — e.g. the animated-light contribution to that surface flows through the animated lightmap weight map, or the authored brightness curve never reaches zero. Determine which, using the dev-tools SH overlay (Irradiance markers, Freeze time, base-only toggle) and inspection of the drifting section. If distinct, file it as a separate finding rather than forcing it into this fix.

### Task 4: Fix the unstable ordering
Apply the minimal change that makes the drifting stage deterministic cross-process — typically replacing hash-ordered iteration with a stable order (sort by a total key, or use an ordered map), or making a parallel reduction order-preserving. Match the existing mitigations' style (exterior leaves are already sorted before the SH bake; BVH primitives sort by a packed stable key).

### Task 5: Add a cross-process determinism guard
Add a test or CI check that would have caught this: it must exercise determinism in a way that survives the per-process hash seed. Options: spawn `prl-build` twice and diff, or randomize the hasher seed within one process across two bakes and assert byte-identity. The existing in-process repeat tests (`sh_bake.rs`, `lightmap_bake.rs`) do **not** cover this and should be noted as insufficient for seed-sensitive drift.

## Sequencing

**Phase 1 (sequential):** Task 1 — isolating the drifting section gates all downstream work.
**Phase 2 (concurrent):** Task 2 (hunt the source in the named stage) and Task 3 (classify the fade-to-zero symptom) — both consume Task 1's output, independent of each other.
**Phase 3 (sequential):** Task 4 — fix, consumes Task 2's identified culprit.
**Phase 4 (sequential):** Task 5 — guard, consumes the fix to prove it holds.

## Rough sketch

Bake entry points confirmed in source:
- Base SH: `bake_sh_volume()` in `crates/level-compiler/src/sh_bake.rs`. Filters to `static_lights`; probes baked via order-preserving `into_par_iter().collect()` over a `0..total` range. Animated lights yield only `AnimationDescriptor` metadata here, no coefficients.
- Delta SH: `bake_delta_sh_volumes()` / `bake_one_light_grid()` in `crates/level-compiler/src/delta_sh_bake.rs`. One grid per animated light, baked at peak brightness (direct + indirect); runtime scales by the brightness curve.
- Lightmap and animated-light weight maps: `lightmap_bake.rs` (and the animated-light chunk/weight-map emission) — the probable home of *surface* (not probe) animated lighting, so a strong candidate for the fade-to-zero surface.

Determinism context:
- Cache key requires bake determinism (`build_pipeline.md` §Build Cache). The cache hides drift; Task 1 must use `--no-cache`.
- Existing mitigations to mirror: exterior-leaf `HashSet` is sorted before the SH bake; BVH primitives sort by a packed `(material_bucket, cell, offset)` key.
- Existing in-process determinism tests live in `sh_bake.rs` and `lightmap_bake.rs`; they share one process so they cannot observe `RandomState`-seeded iteration drift.

The investigation drives the fix — do not pre-commit to a culprit stage before Task 1 names it.

## Open questions
- Which section actually drifts? (Task 1 answers; everything downstream depends on it.)
- Is the "stays lit" surface lit via the delta-SH probe path or the animated lightmap weight-map path? Determines whether Task 3 collapses into the main fix or splits off.
- Does any drift originate upstream in BSP/portal/geometry (affecting every bake) rather than in a single bake stage? If so the fix is higher-leverage but broader.
- Is brush overlap a true trigger or a red herring? Useful as a bisection lever even if the real cause is hash-seed ordering that overlap merely amplifies.
