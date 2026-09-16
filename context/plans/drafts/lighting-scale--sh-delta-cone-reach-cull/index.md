# lighting-scale--sh-delta-cone-reach-cull

Brief · compact · Epic: compile-time peak RAM · reads: `context/lib/build_pipeline.md` §PRL section IDs, `context/lib/rendering_pipeline.md` §4 · read at e36e86b

## Problem
Developer-raised, from the `lighting-scale--compile-peak-ram` gate. `prl-build` refuses
`content/dev/maps/stress-warren-hallway-inspection.map` — the map that exists to expose
baker limits — because the id-41 Direct-SH-delta bake projects a peak host-RAM working
set (~12 GB dense, times the copy-chain factor) past the 16 GiB budget. Cause: the affinity
reach predicate (`light_aabb` / `cells_for_light`, `affinity_grid.rs`) models a spot's reach
as its full falloff **cube** and never consults the cone, so each promotable downlight claims
far more affinity cells than it can illuminate (≈10× on this map, estimated; counts in
`research.md`). A cell outside a spot's outer cone bakes an
all-zero direct tile (`spot_cone_attenuation` = `smoothstep(cos_outer, cos_inner, …)`,
exactly 0 past the outer cone; `incident_radiance_at_point`, `sh_bake.rs`) — provably
zero, and `drop_direct_zero_entries` already discards it, but only *after* the 12 GB dense
payload is materialized. The fixture is being regenerated to also carry animated lights (via
`tools/gen_stress_map.py`), so it exercises the id-45 animated-direct and id-27 indirect delta
bakes too — a torture test for all three delta sections, not id-41 alone. When done: the reach
predicate excludes those provably-zero cells at decomposition, the map compiles within the
default gate, and the emitted `.prl` is byte-for-byte identical to today's for the same input.

## Decisions
- **Cone-aware conservative reach in the shared decompose.** Make the spot reach cone-aware
  (today it is the full falloff cube) in `cells_for_light` / `light_aabb` (`affinity_grid.rs`)
  — the function both the plan-phase CSR and the delta bakes route through — so it removes only
  cells that bake exactly zero, at decomposition, before any dense tile exists. The binding
  invariant is **conservative**: never exclude a cell any of whose probes fall in-cone (a
  superset of the nonzero set is fine — the drop policy removes the fringe remainder; excluding
  a nonzero cell is a defect). The exact test geometry is Path/delegated (Open questions).
- **Transport-aware — direct decompositions only.** The cone clamp applies to the direct
  reach (id-41, id-45, and the id-35 base reach index, `direct_sh_bake.rs`). Indirect id-27
  (`delta_sh_bake.rs`, same `decompose_affinity`) keeps the cube reach: bounced light leaves
  the cone, so cone-clamping indirect would cull real contribution. Thread a direct/indirect
  transport flag through the shared predicate. id-45's cull today carries an explicit
  `animated_direct_sh_bake.rs` comment that spotlight cones *intentionally* do not clip it —
  the executor establishes why before reversing it, and confirms the frozen animated
  rest-direction cone matches the bake.
- **Fixture carries animated lights.** Regenerate `stress-warren-hallway-inspection` with a
  nonzero animated-light share and extend `tools/gen_stress_map.py` so its preset emits them,
  so the torture map exercises id-27 and id-45, not id-41 alone. Lands before the cull (it
  sets the byte-identity baseline). Consequence: id-27 activates with cube reach and is *not*
  cone-cullable, so the first-slice measurement includes it; if id-27 alone pushes the map
  over budget, that bound is Phase 2's, not this brief's.
- **Byte-identical emitted output.** The cull removes exactly the zero set
  `drop_direct_zero_entries` (`delta_drop_policy.rs`) removes today. Fits the epic's
  acceptance-only posture — no format, section, value, or order change. Load-time /
  compiler-internal only.
- **Canonical-entry retention moves into the cull — preserving an existing invariant, not
  inventing one.** Every id-40-selected light must emit exactly one canonical id-41 entry even
  when its entire baked contribution is zero. This is *required*, not incidental: a promoted
  light's baked far-LOD direct-SH can legitimately be zero (its cone reaches no valid probe)
  while its runtime near-tier shadow-pool term is not, so the zero entry is the crossfade's
  "baked contribution is zero here, use the runtime term" slot — dropping the light from id-40
  would remove real runtime entity shadowing. Today the invariant emerges from cube reach
  (always ≥1 cell) + the drop policy retaining one canonical entry per selection index. The
  cone cull breaks the first half, so it must retain — for a selected light it would fully
  cull — **the same canonical entry `drop_direct_zero_entries` keeps today** (not merely some
  cell, or the bytes diverge), keeping the emitted CSR byte-identical and the id-40/id-41
  all-or-nothing contract intact. Not by changing id-40 selection.
- **Shared decompose is authoritative.** The id-41 bake recomputes its own CSR rather than
  consuming the plan's (`direct_sh_bake.rs`); the cull lives in the shared predicate, and an
  assertion pins plan CSR == bake CSR so the two cannot diverge (a divergence would let the
  gate admit a small plan while the bake materializes a larger dense set).
- **Non-goal — per-cell lights cap.** Rejected: `selection_weight` is keyed per global
  selection index and id-40 promotion membership is global, so dropping a still-promoted
  light from one cell's CSR adds the runtime term with no base subtraction → over-brightens
  during crossfade, breaking the no-double-count invariant (`context/lib/index.md` §2).
- **Non-goal — the cold-base-bake ray early-out.** `lighting-scale--cold-bake-reaching-light-spike`
  owns cone/back-face culling of *shadow rays* in the cold base bakes (compile wall-clock);
  its `out-of-scope-findings.md` §4 is a different mechanism (rays, not CSR cells) on a
  different stage. This brief's seam likely *enables* that follow-up; it does not do it, and
  the two must not be merged.
- **Non-goal — the emitted-cap regime.** The 256 MiB bake cap / 128 MiB loader floor
  (`sh-adaptive-coarsening-v2`) is a *loadability* limit, not this brief's host-RAM
  working-set budget — untouched, so the two are not conflated. Probe placement and wire
  format are already foreclosed by the byte-identical decision.

## Acceptance

### Automated
- [ ] `stress-warren-hallway-inspection.map` compiles to completion within the default
  `--sh-delta-working-set-max-size` (16 GiB); today it is refused.
- [ ] The first-slice measured post-cull working-set projection for the warren is recorded in
  `research.md`; if it does not clear the default 16 GiB, the shortfall is reported (a Phase-2
  / budget-note owner call) rather than the default being raised.
- [ ] `DEFAULT_MAX_WORKING_SET_BYTES` stays exactly 16 GiB (constant/grep guard), so the
  compile-success row proves the cone cull admitted the warren — not a raised budget.
- [ ] On a spot-light fixture, the cone-culled CSR equals the CSR left after
  `drop_direct_zero_entries` runs on the unculled bake — the cull removes exactly the zero
  set, nothing more.
- [ ] The plan-phase direct CSR that feeds the working-set gate equals the id-41 bake's CSR
  (offsets and flat light list); a fixture where they would otherwise differ is caught, so the
  gate cannot admit on a small plan while the bake materializes a larger dense set.
- [ ] A fixture that emits id-41 (and one that emits id-45) produces a byte-identical `.prl`
  before and after this change, cold and warm cache; SHA-256 recorded in `research.md`, beside
  the pinned BC6H run-to-run determinism assumption (research.md R8).
- [ ] The id-27 indirect CSR is unchanged by this change, including for an animated spot light
  that also appears in id-45 (bounce is never cone-clamped; transport is per-decompose-call)
  (research.md R5).
- [ ] `DIRECT_SH_STAGE_VERSION` (and the delta stage version) advance with the reach-cull
  change: a warm cache written by the pre-change binary misses cleanly, and two warm builds
  under the new predicate emit byte-identical `.prl` (research.md R7).

**Cone boundary (conservative — never exclude a cell any of whose probes are in-cone)**
- [ ] Edge — a spot whose outer cone lies entirely outside a candidate cell's AABB: that cell
  is absent from the direct CSR. A light every one of whose cells is culled: one canonical
  entry is retained (research.md R4).
- [ ] Edge — a spot whose cone-frustum only grazes a cell's AABB with no id-34-valid probe
  strictly in-cone: the cell is absent, or kept-then-removed by `drop_direct_zero_entries` —
  never emitted with nonzero payload, and never excluded when a valid probe is in-cone
  (research.md R2).
- [ ] A cell with ≥1 id-34-valid probe in-cone and ≥1 out-of-cone is retained in the direct
  CSR, emitted payload byte-identical to today (research.md R3).

**Regression guards**
- [ ] A `light_sun` (Directional) is never cone-culled: it retains every affinity cell
  overlapping the world AABB, and its id-35/id-41 contribution is byte-identical (research.md R6).

## Path
- **Seams.** `cells_for_light` / `light_aabb` / `cell_range` (`affinity_grid.rs`);
  `decompose_affinity` vs `decompose_affinity_for_lights` and `build_csr` (the direct vs
  indirect entry points); the id-41 bake's own reach recompute (`build_reach_index`,
  `direct_sh_bake.rs`). The zero predicate already exists per-ray as `spot_cone_attenuation`
  / `incident_radiance_at_point` (`sh_bake.rs`) and, for the lightmap, in
  `light_texel_contribution_and_visibility` — reuse that cone math; do not reinvent the
  cutoff.
- **Shape.** Conservative cone-frustum-vs-cell-AABB ∩ falloff sphere in the shared predicate.
  Strongest rival — a per-cell importance cap — is rejected in Decisions (double-count).
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
- Does the post-cull warren clear the default 16 GiB gate on its own, with id-27/id-45 now
  active? — **delegated**: the executor measures in the first slice and reports; a shortfall is
  an owner call between Phase 2 and a documented budget note, not a silent default change.
