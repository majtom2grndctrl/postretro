# Research notes — compiler bake determinism

Investigation trail behind `index.md`. Not the spec; raw findings and ruled-out paths.

## How this surfaced

Found while debugging the SH probe-marker "rainbow flicker" (now fixed, separate bug). During that work the user noted a second, long-standing symptom in the same room of `content/dev/maps/campaign-test.map`: four slow-pulse, pre-baked (Quake-style) animated lights, and one square surface that "doesn't fade all the way to zero" when the pulse should bring it dark. The defining clue: **recompiling the map enough times moves the location of the problem.** User suspects too many overlapping brushes confusing the compiler.

## Ruled out (from the flicker session)

- **Runtime GPU sync.** The flicker was a dev-tools readback artifact: `copy_texture_to_buffer` read the composed SH texture before the compose compute pass's storage writes retired on Metal. Fixed with a `poll(wait_indefinitely())` before the readback copy in `crates/postretro/src/render/mod.rs`. Real-time forward lighting was stable throughout — so "stays lit" is baked data, not a runtime race.
- **Malformed delta-grid metadata.** A load-time validation in `build_delta_buffers` (`crates/postretro/src/render/sh_compose.rs`) confirmed every delta grid's `grid_dimensions` product equals its written probe count, on both the complex map and `occlusion-test`. Not a dimensions mismatch.

## Code grounding (confirmed against source)

- **Base SH excludes animated lights.** `bake_sh_volume()` (`crates/level-compiler/src/sh_bake.rs:91`) filters `inputs.static_lights` and bakes probes from those only (`sh_bake.rs:109-114,144`). Animated lights produce `AnimationDescriptor` metadata (`animation_descriptor_for()`, `sh_bake.rs:728`) — no SH coefficients in the base section. So a pulse light cannot leave residual light in the base bake.
- **Delta grid baked at peak.** `bake_one_light_grid()` (`crates/level-compiler/src/delta_sh_bake.rs:113`) bakes direct + indirect at full brightness; runtime multiplies by the brightness curve. At a curve minimum of 0 the contribution is 0. So the SH path *should* fade to zero.
- **Probe loops are order-preserving.** Both bakes use `into_par_iter().map().collect()` over `0..total` ranges, which reconstructs in index order. Per-probe accumulation is sequential. Given deterministic inputs, the bake math is deterministic.
- **Section IDs:** `ShVolume` 20, `Lightmap` 22, `AnimatedLightChunks` 24, `AnimatedLightWeightMaps` 25, `DeltaShVolumes` 27 (`build_pipeline.md` §PRL section IDs).

## Why cross-process, not in-process

IEEE float ops are deterministic for a fixed operation order, and the collects preserve order — so float results are identical across runs. The exploration's "raytrace float tie-break" HIGH-risk rating does **not** explain cross-process drift and is treated as a low-probability lead.

The realistic mechanism: `std` `HashMap`/`HashSet` use `RandomState`, seeded **per process**. Any iteration of a hash collection that feeds output ordering produces a different order in each process — invisible to in-process repeat tests (one seed per process), and a perfect fit for "location moves on rebuild." `build_pipeline.md` §Build Cache already names this as the determinism risk to avoid.

## Nondeterminism candidates (from a thorough compiler read — verify before trusting)

Mitigated / safe:
- Exterior-leaf `HashSet` is sorted before the SH bake (`main.rs` ~373-374).
- `probe_is_valid` uses `HashSet::contains` (membership, not iteration) — order-independent.
- BVH primitives sorted by a packed stable key (`bvh_build.rs` ~92).
- Light namespace assembly preserves source order.

Worth auditing (the actual hunt — Task 2):
- Face extraction / coplanar dedup (`partition/face_extract.rs`) — emission order depends on brush order; confirm brush parse order is stable (parse reportedly uses a `BTreeMap`).
- Any `HashMap`/`HashSet` iterated into output in BSP construction, portal generation, or geometry packing (the uncached stages that feed every bake).
- The animated lightmap weight-map bake (sections 24/25) — strongest candidate for a *surface* that stays lit, distinct from the probe-based delta SH.

These are leads, not conclusions. Task 1 (cross-process `.prl` diff with `--no-cache`) names the drifting section first; the audit then focuses on that one stage.

## Diagnosis tools available

- Dev-tools SH overlay: probe markers in Irradiance mode, "Freeze animation time," "SH compose: base only." Inspect baked base vs base+delta per probe.
- `--no-cache` flag on `prl-build` to bypass the build cache during repro.
- Existing in-process determinism tests in `sh_bake.rs` and `lightmap_bake.rs` — useful precedent, insufficient for seed-sensitive drift.
