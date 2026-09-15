# lighting-scale--sh-delta-cone-reach-cull

Brief · compact · Epic: compile-time peak RAM · reads: `context/lib/build_pipeline.md` §PRL section IDs, `context/lib/rendering_pipeline.md` §4 · read at e36e86b

## Problem
Developer-raised, from the `lighting-scale--compile-peak-ram` gate. `prl-build` refuses
`content/dev/maps/stress-warren-hallway-inspection.map` — the map that exists to expose
baker limits — because the id-41 Direct-SH-delta bake projects a peak host-RAM working
set past the 16 GiB budget: 659,904 `(affinity-cell, light)` CSR entries × 18,432 B ≈
12 GB, times the copy-chain factor. Cause: the affinity reach predicate (`light_aabb` /
`cells_for_light`, `affinity_grid.rs`) models a spot's reach as its full falloff **cube**
and never consults the cone, so each of the map's 338 straight-down 48° downlights claims
~10× the affinity cells it can illuminate. A cell outside a spot's outer cone bakes an
all-zero direct tile (`spot_cone_attenuation` = `smoothstep(cos_outer, cos_inner, …)`,
exactly 0 past the outer cone; `incident_radiance_at_point`, `sh_bake.rs`) — provably
zero, and `drop_direct_zero_entries` already discards it, but only *after* the 12 GB dense
payload is materialized. When done: the reach predicate excludes those provably-zero cells
at decomposition, the map compiles within the default gate, and the emitted `.prl` is
byte-for-byte identical to today's.

## Decisions
- **Cone-aware exact reach in the shared decompose.** Replace the spot cube test with
  sphere ∩ cone-frustum, and skip cells with no id-34-valid probes, in `cells_for_light` /
  `light_aabb` (`affinity_grid.rs`) — the function both the plan-phase CSR and the delta
  bakes route through. This removes only cells that bake exactly zero, at decomposition,
  before any dense tile exists. The predicate must be **conservative**: never exclude a
  cell any of whose probes fall in-cone (a superset of the nonzero set is fine — the drop
  policy removes the fringe remainder; excluding a nonzero cell is a defect).
- **Transport-aware — direct decompositions only.** The cone clamp applies to the direct
  reach (id-41, id-45, and the id-35 base reach index, `direct_sh_bake.rs`). Indirect id-27
  (`delta_sh_bake.rs`, same `decompose_affinity`) keeps the cube reach: bounced light leaves
  the cone, so cone-clamping indirect would cull real contribution. Thread a direct/indirect
  transport flag through the shared predicate.
- **Byte-identical emitted output.** The cull removes exactly the zero set
  `drop_direct_zero_entries` (`delta_drop_policy.rs`) removes today; preserve its
  keep-one-canonical-entry-per-selection-index rule (a fully-culled light retains one entry
  for runtime addressing). Fits the epic's acceptance-only posture — no format, section,
  value, or order change. Load-time / compiler-internal only.
- **Shared decompose is authoritative.** The id-41 bake recomputes its own CSR rather than
  consuming the plan's (`direct_sh_bake.rs`); the cull lives in the shared predicate, and an
  assertion pins plan CSR == bake CSR so the two cannot diverge.
- **Non-goal — per-cell lights cap.** Rejected: `selection_weight` is keyed per global
  selection index and id-40 promotion membership is global, so dropping a still-promoted
  light from one cell's CSR adds the runtime term with no base subtraction → over-brightens
  during crossfade, breaking the no-double-count invariant (`context/lib/index.md` §2).
- **Non-goal — the cold-base-bake ray early-out.** `lighting-scale--cold-bake-reaching-light-spike`
  owns cone/back-face culling of *shadow rays* in the cold base bakes (compile wall-clock);
  its `out-of-scope-findings.md` §4 is a different mechanism (rays, not CSR cells) on a
  different stage. This brief's seam likely *enables* that follow-up; it does not do it, and
  the two must not be merged.
- **Non-goal — emitted-cap regime, probe placement, format.** The 256 MiB bake cap / 128 MiB
  loader floor (`sh-adaptive-coarsening-v2`), the uniform-lattice placement, and the wire
  format are untouched.

## Acceptance

### Automated
- [ ] `stress-warren-hallway-inspection.map` compiles to completion within the default
  `--sh-delta-working-set-max-size` (16 GiB); today it is refused.
- [ ] On a spot-light fixture, the cone-culled CSR equals the CSR left after
  `drop_direct_zero_entries` runs on the unculled bake — the cull removes exactly the zero
  set, nothing more.
- [ ] A fixture that emits id-41 (and one that emits id-45) produces a byte-identical `.prl`
  before and after this change, cold and warm cache; SHA-256 recorded in `research.md`.
- [ ] The id-27 indirect CSR is unchanged by this change (transport-awareness: bounce is not
  cone-clamped).
- [ ] Edge — a spot whose outer cone lies entirely outside a candidate cell's AABB: that cell
  is absent from the direct CSR. A light every one of whose cells is culled: one canonical
  entry is retained. A cell with zero id-34-valid probes: skipped.

## Path
- **Seams.** `cells_for_light` / `light_aabb` / `cell_range` (`affinity_grid.rs`);
  `decompose_affinity` vs `decompose_affinity_for_lights` and `build_csr` (the direct vs
  indirect entry points); the id-41 bake's own reach recompute (`build_reach_index`,
  `direct_sh_bake.rs`). The zero predicate already exists per-ray as `spot_cone_attenuation`
  / `incident_radiance_at_point` (`sh_bake.rs`) and, for the lightmap, in
  `light_texel_contribution_and_visibility` — reuse that cone math; do not reinvent the
  cutoff.
- **Shape.** Conservative cone-frustum-vs-cell-AABB ∩ falloff sphere ∩ valid-probe filter in
  the shared predicate. Strongest rival — a per-cell importance cap — is rejected in
  Decisions (double-count).
- **First slice (falsifies the riskiest assumption).** Before hardening, set
  `--sh-delta-working-set-max-size 0` on the warren to print the plan-phase projection, add
  the cone predicate, and re-measure: confirm the post-cull projection lands under 16 GiB
  with margin. The ~10× reduction is estimated, not measured (no compiler binary was run this
  session). If it does not clear the default gate, that is the signal Phase 2
  (`lighting-scale--sh-delta-cell-major-two-pass-bake`) is required, not optional — report
  the measured post-cull peak rather than silently bumping the default.

## Open questions
- Exact conservative cone-frustum-vs-AABB test form (half-angle vs cell corners, padding) —
  **delegated**: the executor picks the test and reports it in the plan of record; the
  contract is "superset of the nonzero set, never excludes an in-cone cell."
- Does the post-cull warren clear the default 16 GiB gate on its own? — **delegated**: the
  executor measures in the first slice and reports; a shortfall is an owner call between
  Phase 2 and a documented budget note, not a silent default change.
