# lighting-scale--sh-delta-cell-major-two-pass-bake

Brief · resumable · Epic: compile-time peak RAM · reads: `context/lib/build_pipeline.md` §Compiler pipeline, `context/lib/rendering_pipeline.md` §4 · read at e36e86b

> **Build order (lighting-scale peak-RAM track):** Phase 2 — **not currently needed.** Its gating condition has resolved: `sh-delta-cone-reach-cull` (Phase 1) **shipped to `main`** and cleared the 16 GiB peak-RAM gate on its own (post-cull warren ~6.5 GB at 1 m spacing, ~66 MiB PRL). Retained as a contingency only if a future, larger map's projection exceeds the gate; do not build otherwise.

## Problem
Anticipated need, following `lighting-scale--sh-delta-cone-reach-cull` (Phase 1). Phase 1
makes `stress-warren-hallway-inspection` compile because its lights are cones whose
provably-zero reach can be culled. It does nothing for the general peak-RAM shape: the SH-delta
bake holds the **full dense payload of all three delta sections resident through a
whole-section coarsening pass**, so peak host RAM scales with the cumulative Σ of
`(affinity-cell, light)` entries — unbounded by any per-cell limit. The cases Phase 1 cannot
reach are omnidirectional point lights in open arenas (no cone to cull) and future dense maps.
Cause: the current bake-then-coarsen ordering materializes every dense tile before the
coarsening level decision runs, even though that decision needs only per-cell scalars. When
done: peak RAM during the delta bake+coarsen is bounded by **max lights per cell**, not Σ, so a
densely-lit map compiles regardless of total fan-out, and the emitted `.prl` is byte-for-byte
identical to today's.

## Decisions
- **Cell-major two-pass ordering.** Pass 1 sweeps cell by cell, and for each entry emits the
  scalars the level decision consumes — the three per-level residual scores (L0/L1/L2), the 8
  L1 corner tiles, the L2 mean, and composed magnitude — then discards that cell's dense
  interior. The level decision (classifier, runtime-safe envelope fix-point, level-grid
  smoothing) runs on those cached scalars plus the level grid. Pass 2 re-bakes or cache-reloads
  the dense raw tiles only for the cells that land at L0/L1 (whose emitted payload is verbatim
  dense corners/interior).
- **The level decision needs no dense payload — verified.** `BrickClass` (`sh_coarsen.rs`) is
  entirely precomputed scalars; the classifier, gate, protection/ceiling, and seam-smoothing
  fix-point read only those scalars and the level grid. The envelope fix-point's per-cell
  residual is a deterministic cell-local function of that cell's dense payload and the trial
  level (three levels), so all three scores precompute in one sweep; `smooth_to_fixpoint` /
  `smooth_pair` (`sh_runtime_envelope.rs`) couple the *level grid*, not per-probe values; byte
  accounting (`payload_bytes`) is count-only from offsets + levels + validity masks. This
  diverges from `lighting-scale--compile-peak-ram`'s `research.md`, which states the dense
  buffer must stay resident "because the envelope re-scores dense levels each iteration, runs a
  full compaction internally, and its smoothing couples neighbors" — accurate as a description
  of the current ordering, but the re-scoring is cell-local and precomputable, the internal
  compaction runs once *after* the loop, and the coupling is over levels. The residency is an
  ordering artifact, not a math requirement.
- **Byte-identical output.** Same level decision, same emitted tiles, reordered production.
  Coarsening math, values, section format, and the emitted-cap regime are untouched. Fits the
  epic's acceptance-only posture.
- **Non-goal — the cone cull.** Phase 1 owns it. This brief assumes Phase 1 landed and reduces
  Σ where cones allow; the two-pass bounds the residual peak where they do not.
- **Non-goal — changing the level-decision math, probe placement, or wire format.**
- **Depends on Phase 1 landed** (`lighting-scale--sh-delta-cone-reach-cull` in `main`).

## Acceptance

### Automated
- [ ] A point-light-heavy open-arena fixture (no cones for Phase 1 to cull) that the default
  gate refuses today compiles to completion.

**Peak RAM bound** (per `context/lib/testing_guide.md` §Resource bounds)
- [ ] Measured peak host RAM through the delta bake+coarsen scales with max-lights-per-cell, not
  the cumulative Σ across the three bakes; recorded against a fixture whose Σ far exceeds its
  per-cell max. Proof covers pass 1 (the transient composed-magnitude sweep), the fix-point, and
  pass-2 re-materialization — not only the fix-point.

**Byte identity**
- [ ] A fixture emitting id-27/id-41/id-45 produces a byte-identical `.prl` vs the current
  bake-then-coarsen ordering, cold and warm; SHA-256 in `research.md`.
- [ ] The decided `cell_levels` grid is identical between the current ordering and the two-pass
  on that fixture (the reorder changes when tiles are materialized, never which level a cell
  gets).

## Path
- **Seams.** `classify_direct_levels` / `classify_levels` and `BrickClass` (`sh_coarsen.rs`);
  `apply_runtime_safe_envelope` / `score_dense_levels` / `score_dense_cell` / `smooth_to_fixpoint`
  (`sh_runtime_envelope.rs`, `sh_runtime_envelope_scoring.rs`); `compact_direct_valid_probes` /
  `compact_dense_valid_probe_payload` / `synthesize_l2_mean_tile` (`delta_sections.rs`);
  `dense_reference_magnitudes` / `classifier_darkness_floor` (`sh_analyze.rs`); the coarsening
  pass wiring (`pipeline.rs`).
- **Shape.** Precompute per-entry `EnvelopeStats` (all three levels) + corners + mean + magnitude
  in one cell-major sweep; run the fix-point on scalars; re-materialize L0/L1 in pass 2. Rival —
  out-of-core / mmap spill of the dense buffer — is weaker: `compile-peak-ram` rejected it as
  random-access thrash, and while smoothing is level-grid (not dense) so the thrash claim is
  softer than stated, spill still pages the whole payload where the two-pass never holds it.
- **First slice — the make-or-break.** The composed reference magnitude the id-41 gate/floor
  keys on is `base_indirect + base_direct + Σ(id-27, id-41, id-45)` (`dense_reference_magnitudes`,
  `sh_analyze.rs`). So pass 1's magnitude sweep needs each cell's contribution from all three
  delta sections plus both bases. The O(max-lights-per-cell) peak holds only if that sweep can be
  evaluated cell-major without the full dense of all three sections co-resident (today's
  cumulative peak, which the gate charges 3×). Build a point-light-arena fixture, prototype the
  cell-major composed-magnitude sweep, and measure pass-1 peak: confirm it tracks max-lights-per-cell
  and beats the status-quo cumulative peak. If it cannot, the two-pass does not deliver the bound
  and the approach needs reshaping before the full restructure — this slice falsifies the central
  assumption first.
- **File splits (behavior-preserving, own commit, before extending).** `delta_sections.rs`
  (~2143 lines) along its view/test seams; `sh_analyze.rs` (~2804). Add any streamed-tile writer
  on the section type in the level-format module, not in `pack.rs`.

## Open questions
- Can pass 1's composed-magnitude sweep run cell-major across all three delta sections + both
  bases with peak bounded by max-lights-per-cell, and does it beat the status-quo cumulative
  peak? — owner: reviews after the first-slice measurement — **blocks build** (the whole RAM win
  rests on it; the first slice measures it before the full restructure is committed).
