# research — lightmap-dark-area-measurement

Derivation behind the measurement-first route. Informs the follow-up coarsening decision; does
not decide this spike. Read at a097035.

## Why measurement-first

The mechanism is cheap and already present: per-chart density flows end-to-end
(`resolved_chart_density` → `chart_texel_dimension` → heterogeneous `pack_layers`), the runtime
samples UV-only, and `LightmapSection.irr_texel_density` is informational — so coarsening needs
no format or runtime change. What is unknown is the *win*: no committed real `.prl` exists (only
a synthetic BC6H fixture), and there is no lightmap stats path. The SH analog's footprint payoff
was marginal (~0.02–0.76% of stored tiles) because its dense baseline sat near a storage floor.
Lightmaps have no darkness compaction today, so the win could be larger — but that is a
hypothesis, so measure before building the coarsener.

## Signal taxonomy (for the follow-up decision)

Candidate signals for adaptive lightmap density, sorted by measurability and coarsen-safety.
Maps onto the engine's existing paradigm: auto-defaults for the measurable/safe cases, artist
escape hatches for the rest.

Auto-defaults (measurable, safe):
- **S1 darkness + flatness → coarsen.** Absolute near-lossless. The subject of this spike.
- **S2 gradient → refine, never coarsen.** Refining only costs budget, never quality — so a
  shadow-edge signal is safe as a *refine* driver though dangerous as a coarsen driver.
  Pairing S1+S2 redistributes a fixed budget (coarsen darks/flats, spend on edges).

Artist escape hatches (un-measurable intent):
- **H1 `lightmap_scale_region`** — exists; author sets density.
- **H2 force-fine protect region** — missing; mirror `sh_protect_volume` (hard-pin to finest,
  overrides the auto gate). Mandatory the moment S1 ships.
- **H3 per-surface no/low-lightmap material flag** — missing; `BrushSide` carries no surface
  flags at all. Cheap, zero-risk, independently useful.

Bigger / separate:
- **B1 occlusion-cull never-seen interior faces.** The bake already culls solid-facing
  (`face_extract.rs`) and sealed-exterior (`find_exterior_leaves`) faces, but every visible-leaf
  interior face still gets a full chart. Automating occlusion is larger and riskier (a false
  cull is a black surface).

Off-limits as an auto-coarsen driver:
- **X1 relative-error/contrast gating of bright detail.** Frequency inversion — coarsens shadow
  edges. This is what `plans/done/lighting-scale--lightmap-bake-scaling` legitimately rejected.
  Darkness (S1) is the safe carve-out: an absolute floor, not relative error.

## SH precedent (the shape S1 would reuse)

- Darkness gate: floor = `darkness_frac (0.02) × map-p95 magnitude`, absolute min 1e-6; below it
  a brick coarsens, bypassing the error gate (`sh_coarsen.rs::CoarsenParams`,
  `sh_analyze.rs::classifier_darkness_floor`). Error gate beside it: `rel_p95 ≤ 0.10`,
  `rel_max ≤ 0.25`.
- Boundary protection: a ≤1-level fixpoint smoothing bound grades dark↔lit L2→L1→L0, so a coarse
  region never abuts a fine one across a transition (`smooth_pair`/`demote_one`).
- Escape hatch: `sh_protect_volume` brush entity + `--sh-protect-aabb`, union-composed
  (`combined_protect_aabbs`), hard-pin overlapping regions to finest (`sh_coarsen.rs` Phase C).
- Analysis pass: `--sh-analyze` is output-preserving (emits summary + JSON, changes no bytes) —
  the template for `--lightmap-analyze`.

## Determinism constraint (binds the follow-up coarsener, not this spike)

Density derived from a lighting pre-pass must be deterministic AND identical in warm and cold
builds, or warm/cold resolve different atlas layouts and break the pre-BC6H byte-identity
invariant (`build_pipeline.md` §Build Cache). The safe pattern is the SH one: decide post-hoc
over the unchanged dense bake. `atlas_layout_fingerprint` already folds per-chart dims into the
cache key, so a coarsening that only changes chart dimensions re-keys correctly. A density
derived from all-lights analysis, however, collapses the per-light incremental cache grain.

## Measurement seam

The honest seam is the pre-BC6H `CompositedAtlas` in `lightmap_bake.rs` — raw f32 irradiance,
`direction`, and a `coverage` mask, after `dilate()` and before the lossy encode. Existing tests
already bake fixtures and inspect f32 irradiance (`occluder_produces_dark_texel`, penumbra tests).
Per-texel magnitude/flatness over covered texels needs nothing more; per-chart rollup needs the
pack-stage chart rects.
