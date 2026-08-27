# Cold SH Bake — Falloff-Range Early-Out

## Goal

Add the missing per-receiver falloff-range early-out to the cold whole-volume SH
indirect bake so it skips the soft-visibility shadow ray for any point/spot light
whose distance to the bounce hit point exceeds its falloff range. This closes the
one gap the cold lightmap bake already covers: the cold SH bake casts a full
32-sample shadow ray for every static light and only zeroes out-of-range lights
via falloff *after* the ray. The change is byte-identical to today's output and,
on the parent spike's synthetic fixture, cut the cold SH stage ~20.9× (245.9 s →
11.8 s). This implements the recommendation of
`context/plans/drafts/lighting-scale--cold-bake-reaching-light-spike/findings.md`.

## Scope

### In scope

- A per-light early-out inside the shared `sample_radiance_rgb`
  (`crates/level-compiler/src/sh_bake.rs`): skip a Point/Spot light when its
  distance to `hit.point` exceeds `falloff_range.max(1e-4)`, before the
  `soft_visibility` shadow ray. Directional lights are never skipped. Placement is
  the **shared sampler, unconditional** (not a cold-only gated branch) — see
  Resolved decisions.
- The early-out is **unconditional** (no env gate) — the production hardening of
  the env-gated `POSTRETRO_SPIKE_REACH_CULL` prototype.
- A determinism test proving the early-out is contribution-neutral (a bake with an
  out-of-range light produces bit-identical SH coefficients to a bake without it),
  plus a unit test pinning the cull predicate (skip iff `dist > range`; Directional
  never skipped).
- Removal of the env-gated spike instrumentation in full (`spike_reach.rs`, its
  `pipeline.rs` / `sh_bake.rs` / `lightmap_bake.rs` hooks, and the spike-only
  `affinity_grid` additions) — both the `CULL` prototype and the `STATS` harness
  (see Resolved decisions).

### Out of scope

- Building an affinity-cell / portal reaching-light cull for the cold bakes. The
  parent spike measured it as looser and more complex than the exact per-point range
  test at these light counts, with a cell-boundary byte-identity hazard. Deferred —
  see Direction. (The production affinity primitive `decompose_affinity_for_lights`
  and each bake's own `ReachIndex` stay; only the spike-only `WorldReachIndex`
  wrapper is removed, in Task 3.)
- Any change to the cold **lightmap** bake — it already gates its shadow rays by
  range (`light_texel_contribution_and_visibility` returns before `soft_visibility`
  when `contribution.length_squared() <= 1e-12`).
- Any change to the direct / delta / animated-direct SH bakes' culling, seeding, or
  bounded-set construction, and any change to the warm grouped SH path's
  per-group light bounding.
- A **tighter** predicate than the pure range test — e.g. additionally screening
  in-range-but-backfacing or out-of-cone lights the way the lightmap's
  `contribution.length_squared()` test does. That would be byte-identical too, but
  the task requires matching `falloff() == 0` exactly and forbids inventing a
  tighter bound; the range test is the specified predicate.
- Any format, section, PRL wire, or runtime change. This is a compile-time bake
  optimization only.

## Direction

**Problem.** The cold whole-volume SH indirect bake casts the 32-sample
soft-visibility shadow ray unconditionally for every static light at every bounce
hit point, applying falloff only afterward — so an out-of-range light pays a full
shadow ray whose contribution is then multiplied to zero. Observation (parent
spike, `stress-warren-lit`, 157 static lights): a typical bounce receiver is
reached by only ~4–6 % of the light set (median 7, p95 10 by exact range), so
~95 % of the shadow rays are provably wasted. The cause is a missing early-out,
not the shadow-ray cost itself.

**Prior commitments.**
- The cold **lightmap** bake already performs this early-out structurally:
  `light_texel_contribution_and_visibility` (`lightmap_bake.rs`) returns
  `(ZERO, ZERO, None)` before `soft_visibility` when the light's contribution is
  zero. This spec mirrors that *structure* (skip the shadow ray for a
  non-contributing light) into the SH bake, using the pure range predicate rather
  than the lightmap's fuller contribution test (see Out of scope).
- **Determinism invariant** (`build_pipeline.md` §Build Cache): the cold
  whole-volume SH bake is byte-identical for identical inputs, and "New code in
  `sh_bake.rs` must preserve it." This change preserves it: a range-culled light's
  `falloff()` is provably zero (every model returns 0 for `dist > range`), so its
  skipped `radiance += light_contribution_lambert(..) * v` term was already
  exactly `ZERO`; and each kept light's soft-visibility seed is a pure function of
  `(probe_index, ray_index, global_index)` (`soft_visibility_seed`), so skipping
  one light shifts no other light's seed. The parent spike verified SHA256-identical
  `.prl` output on the fixture.
- **Directional lights are never range-culled** — they have no falloff sphere, and
  indirect reachability holds for every baked light regardless of shadow type
  (`rendering_pipeline.md` §4). `incident_radiance_at_point` gives Directional a
  range-free intensity.
- The **warm grouped SH path** already bounds each group's light set
  (`build_pipeline.md` §Cache grain — "sh_group" entries, falloff dilated by a
  finite reach cutoff). This spec does not modify that bounding. Divergence: none —
  the early-out is additive and byte-neutral in the warm and delta callers that
  share `sample_radiance_rgb` (they pass a pre-bounded or single-light slice; the
  early-out only ever skips a light already contributing zero).

**Alternatives rejected.** The affinity-cell / portal reaching-light index the
direct/delta/animated-direct SH bakes already consume (`decompose_affinity_for_lights`
→ each bake's own `ReachIndex`; `WorldReachIndex` is the spike-only wrapper this spec
deletes, not a production consumer). The parent spike measured it
on the fixture as *looser* than the exact per-point range test (mean 9.8 vs 6.8
lights/receiver, p95 33 vs 10) because the coarse affinity cell (probe_spacing ×4 ≈
40 m here) over-keeps lights near cell boundaries; it is also more complex (needs
the portal graph, cell decomposition, reachability flood) and carries a
byte-identity hazard: the cell's centroid portal test can disagree with an
arbitrary hit point inside a straddling cell. The exact per-point range test is
tighter, exact, portal-free, and byte-identical. Its only genuine advantage —
amortizing the reach test across a region and skipping the O(N) per-light
*iteration* — is marginal at N = 157 (the shadow ray dominates). Deferred for the
cold bakes; revisit only if static-light counts grow high enough that per-light
iteration becomes material, or if in-range-but-occluded rays are shown to dominate
on real content.

## Acceptance criteria

The byte-identity ACs use the **excluded-trailing-light surrogate**, because the
change is unconditional (no toggle survives to bake "with vs. without the early-out"
in one process — the parent spike's `POSTRETRO_SPIKE_REACH_CULL` toggle is removed in
Task 3). The surrogate bakes a fixture whose out-of-range light(s) sit **last** in the
static-light set, then compares against the same bake with those trailing lights
removed. Since an out-of-range light contributes exactly zero, "present but skipped"
and "absent" produce identical SH output — but only when the excluded light is last,
so its removal does not renumber a kept light's `soft_visibility_seed` (the cold/delta
seed is the light's slice index; the warm seed is its global-set index — both stable
only for a trailing removal). Compare at the **SH volume section** granularity, not
the whole `.prl`: removing a light also shrinks the light-table section, so full-`.prl`
SHA256 would differ for an unrelated reason. The parent spike's full-`.prl` SHA256
evidence came from the toggle (identical light set, ray skipped vs. cast).

- [ ] **Byte-identical cold SH section.** A cold (`--no-cache`) bake of a fixture with
  out-of-range static point/spot lights placed last emits an SH volume section
  byte-identical to the same bake with those trailing lights excluded. (The parent
  spike separately verified full-`.prl` SHA256 identity via the cull toggle on
  `stress-warren-lit`, 16.6 MB output.)
- [ ] **Contribution-neutral early-out (per-ray).** A unit test calls
  `sample_radiance_rgb` with a light set whose last light is a point light beyond its
  falloff range, and asserts the returned SH coefficients / radiance are bit-identical
  to the same call with that trailing light removed.
- [ ] **Predicate matches `falloff() == 0`.** A unit test confirms a Point/Spot
  light is skipped iff `dist > falloff_range.max(1e-4)` (kept at `dist == range`;
  skipped just beyond), across Linear / InverseDistance / InverseSquared models, and
  that a Directional light is never skipped.
- [ ] **Warm and delta SH sections unchanged (discharged by reasoning, not a
  standalone test).** The warm (`bake_probe_rgb_with_moments`) and animated-delta
  (`bake_probe_indirect_rgb`) callers contact the early-out *only* through
  `sample_radiance_rgb`, which the Contribution-neutral-per-ray AC proves byte-neutral
  (it skips only falloff-zero lights in their bounded / single-light slices). No
  separate warm/delta fixture is required — this is satisfied by Task 1's per-ray
  neutrality, matching the Invariants table row. (Constructing a delta-specific
  byte-identity fixture is disproportionate and would test the same shared function.)
- [ ] **Fewer shadow rays on the cold path (evidence, not an in-tree gate).** The cold
  SH stage casts strictly fewer soft-visibility shadow rays on a fixture with
  out-of-range lights. Satisfied by the parent spike's recorded ~20.9× (245.9 s →
  11.8 s) on `stress-warren-lit` — a synthetic-fixture figure, not a production
  projection. No in-tree instrument survives Task 3 (the `POSTRETRO_SPIKE_REACH_STATS`
  ray counter is removed), so this AC is evidence-by-parent-recording; capture any
  wanted independent ray-count confirmation before Task 3 removes STATS.
- [ ] **No env gate, no spike harness.** The early-out runs on every cold bake with
  no environment variable required to enable it, and after Task 3 no
  `POSTRETRO_SPIKE_REACH_*` path or spike-only code (`spike_reach.rs`, the
  `affinity_grid` spike additions) remains in the tree.

## Tasks

### Task 1: Unconditional falloff-range early-out in the SH sampler

In `sample_radiance_rgb` (`crates/level-compiler/src/sh_bake.rs`), before the
`soft_visibility` shadow-ray call for each light, skip any Point/Spot light whose
distance from `hit.point` to `light.origin` exceeds `light.falloff_range.max(1e-4)`;
never skip a Directional light. `hit.point` and `light` are already in scope inside
the loop, so no new plumbing is needed. The skip must occur before `global_index`,
`seed`, and `soft_visibility` are computed, and must not alter the seed of any kept
light (the seed is a pure function of `(probe_index, ray_index, global_index)`, so
skipping a light shifts nothing). Implement the predicate as a small self-contained
range check local to `sh_bake.rs` (mirroring the distance/range math in
`sh_bake::falloff` and `incident_radiance_at_point` — `range = falloff_range.max(1e-4)`,
skip iff `dist > range`). This task also removes the sampler's entire spike block —
the `spike_cull` cull branch and the `spike_active` / `spike_in_range` / `record_sh`
STATS lines currently inside `sample_radiance_rgb` — so the function no longer
references `spike_reach`, and Task 3 can delete the module cleanly (the predicate is
self-contained and does not depend on `spike_reach.rs`). Add: (a) a
determinism/neutrality test that calls `sample_radiance_rgb` with a light set whose
**last** light is an out-of-range point light and asserts bit-identical output to the
same call with that trailing light removed (mirror the existing `sample_radiance_rgb`
call-site test `sh_bounce_scales_by_soft_visibility_fraction` in `sh_bake.rs`) — the
excluded light must be last so its removal does not renumber a kept light's seed; (b)
a predicate test covering the three falloff models at `dist == range` (kept) and
`dist > range` (skipped) plus a Directional light (never skipped). The early-out is
unconditional — no env var gates it.

### Task 2: Cold SH-section byte-identity check

Add a test that compiles a small map fixture containing at least one in-range and at
least one out-of-range static point/spot light — the out-of-range light(s) placed
**last** in the set — through the cold (`--no-cache`-equivalent, in-process) whole-volume
SH bake, and asserts the emitted SH volume section bytes are identical to the same bake
with the trailing out-of-range light(s) excluded (the excluded-trailing-light surrogate;
see Acceptance criteria). This delivers the Byte-identical-cold-SH-section AC and is the
whole-section integration analogue of Task 1's per-ray neutrality test. (Warm and delta
are discharged by Task 1's per-ray neutrality — they contact the change only through
`sample_radiance_rgb` — so they need no separate fixture here; see the
Warm-and-delta-SH-sections-unchanged AC.)

Compare SH-section bytes (`to_bytes()`), not the whole `.prl` (excluding a light also
shrinks the light-table section, so full-`.prl` SHA256 would differ for an unrelated
reason). Model the harness on the byte-equality determinism test
`lightmap_bake_produces_byte_identical_output_on_repeated_runs` in `lightmap_bake.rs`
(asserts `section.to_bytes()` equality) — **not** the adjacent
`two_bakes_decode_within_frozen_tolerance`, which is a decode-within-tolerance test that
does not require byte-equality and is the wrong model for a byte-identity AC.

### Task 3: Remove the spike measurement harness in full

Now that the production early-out is unconditional, delete the entire spike
instrumentation — both the `POSTRETRO_SPIKE_REACH_CULL` prototype (superseded by
Task 1) and the `POSTRETRO_SPIKE_REACH_STATS` distribution harness. Remove:

- `crates/level-compiler/src/spike_reach.rs` and its `pub mod spike_reach;`
  declaration in `main.rs`.
- The install/record/log hooks in `pipeline.rs` (`install_sh` / `install_lm` /
  `log_sh_summary` / `log_lm_summary` and the `WorldReachIndex` construction at the
  two cold-bake call sites) and the `record_lm` call in `lightmap_bake.rs`. (The
  `sh_bake.rs` sampler's spike block — cull branch plus `record_sh` — is already
  removed by Task 1, so nothing spike-related remains in `sample_radiance_rgb`.)
- The spike-only additions in `affinity_grid.rs`: `AffinityGridGeometry`,
  `affinity_grid_geometry`, `WorldReachIndex`, `cell_of`, and the
  `world_reach_index_counts_and_maps_receiver_to_the_lights_own_cell` unit test.
  Verify nothing else consumes them first (the direct/delta/animated bakes use
  `decompose_affinity_for_lights` / `build_csr` / their own `ReachIndex`, which stay).

After this task the compiler carries no `POSTRETRO_SPIKE_REACH_*` env path and no
spike-only code; the cold SH early-out from Task 1 is the only surviving change.
This matches the project's spike convention (the `perf-animated-sh-light-culling`
spike reverted its instrumentation to a clean `git diff`; the level-compiler crate
carries no standing env-gated debug instrumentation) and the lean-binary northstar.
The re-measurement capability the STATS harness provided is preserved by the parent
spike's `findings.md` (which documents the method and flags) plus the harness's git
history on this branch — a `git restore` of the spike commit, not a rebuild, if a
real map later warrants re-measuring the reaching-light distribution.

## Sequencing

**Phase 1 (sequential):** Task 1 — the core early-out plus its per-ray neutrality
and predicate tests. Falsifies the byte-identity assumption at the smallest grain
before anything else builds on it.
**Phase 2 (concurrent):** Task 2, Task 3 — Task 2 adds the section-level fixture
byte-identity check; Task 3 removes the spike measurement harness in full.
Independent: Task 2 touches test code and reads the post-Task-1 sampler; Task 3
deletes `spike_reach.rs`, its hooks, and the spike-only `affinity_grid` additions,
which Task 1's self-contained early-out already superseded. Both depend on Task 1's
early-out existing (Task 1 must not leave the sampler depending on `spike_reach.rs`,
so Task 3 can delete it cleanly).

## Rough sketch

The change is the smallest diff over the env-gated prototype in
`sample_radiance_rgb`: replace the `if spike_cull && !reaches { continue; }`
(gated) with an unconditional `if dist_to(light, hit.point) > range { continue; }`
for Point/Spot, computed from a local predicate. Shape (illustrative, not
prescriptive):

```rust
// Proposed design — in sample_radiance_rgb's per-light loop, before the seed/ray.
// Point/Spot beyond falloff range contribute exactly zero (falloff() == 0), so
// their soft-visibility shadow ray is provably wasted. Directional has no range.
if !reaches_falloff_range(light, hit.point) {
    continue;
}
```

`reaches_falloff_range` mirrors `sh_bake::falloff` / `incident_radiance_at_point`
exactly: Directional → always true; Point/Spot → `(light.origin - hit.point).length()
<= light.falloff_range.max(1e-4)`. Neutrality holds because `falloff` returns 0 for
`dist > range` in every model (Linear clamps to 0; InverseDistance/InverseSquared
short-circuit), and at `dist == range` the light is kept and — for Linear —
contributes exactly zero, so the kept case is byte-identical too.

Existing test anchors: `sh_bounce_scales_by_soft_visibility_fraction` in `sh_bake.rs`
(its `soft` vs. `clear` `sample_radiance_rgb` calls) shows the per-ray test harness;
`lightmap_bake_produces_byte_identical_output_on_repeated_runs` in `lightmap_bake.rs`
(asserts `section.to_bytes()` equality) shows the byte-identity determinism pattern for
the section-level check — not the adjacent `two_bakes_decode_within_frozen_tolerance`,
which only checks decode-within-tolerance.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Cold whole-volume SH section byte-identical to pre-change output | Task 1 (early-out skips only `falloff()==0` lights) | Threatened if the predicate skips any contributing light, or if a skip perturbs a kept light's seed | Byte-identical-cold-SH-section AC; Contribution-neutral-per-ray AC; Task 2 |
| Cull predicate == `falloff()==0` region: skip iff `dist > falloff_range.max(1e-4)`; Directional never skipped; kept at `dist == range` | Task 1 (local range predicate mirroring `falloff`) | Threatened by a looser bound (culls a contributing light) or a tighter bound (screens backface/cone — out of scope) | Predicate-matches AC |
| Kept lights' soft-visibility seeds unchanged by any skip | Task 1 (`soft_visibility_seed` is a pure fn of probe/ray/global index; skip shifts no other index) | Threatened if the skip reindexes or reorders the light loop | Byte-identical-cold-SH-section AC; Contribution-neutral-per-ray AC |
| Warm grouped + animated delta SH sections unchanged | Task 1 (shared `sample_radiance_rgb` early-out is byte-neutral in bounded/single-light callers) | Threatened if placement changes warm's bounded-set semantics or delta's slice | Warm-and-delta-SH-sections-unchanged AC (discharged by Task 1 per-ray neutrality) |
| No spike harness remains (`POSTRETRO_SPIKE_REACH_*`, `spike_reach.rs`, spike `affinity_grid` additions); early-out always runs | Task 1 (self-contained early-out) + Task 3 (deletes the harness) | Threatened if a residual env check or spike-only symbol survives, or if Task 1 left the sampler depending on `spike_reach.rs` | No-env-gate-no-harness AC |

## Resolved decisions

Both questions the draft originally left open were resolved against source, not
left to the reviewer.

- **Early-out placement → shared `sample_radiance_rgb`, unconditional.** The warm
  grouped SH path is not a separate sampler: `sh_group.rs` bakes each group through
  `sh_bake::bake_probe` → `bake_probe_rgb_with_moments` → `sample_radiance_rgb`
  (`sh_group.rs:376`, `:1733`, `:1743`), so it already flows through the exact
  function the early-out lives in. It is byte-neutral there: the warm path bounds a
  group to `falloff_range` *dilated* by a reach cutoff, so any light the early-out
  would skip is beyond exact `falloff_range`, has `falloff() == 0`, and contributes
  zero today — the early-out only ever removes a provably-zero shadow ray, so warm
  output is unchanged and the warm path merely gets faster too. Both existing
  `sh_group.rs` guards stay valid because each side receives the early-out
  identically: `sh_cold_grouped_equals_monolithic_on_fixtures` (grouped-cold
  `to_bytes()` == monolithic, byte-identity) and `warm_sh_within_tolerance_on_fixtures`
  (warm within a bounded p99.9 error of cold — a deliberate approximation, not an
  equivalence check). The strict cold-only
  alternative would need a gate flag threaded through the shared `bake_probe`, adds
  complexity, and leaves warm casting the same wasted rays for no benefit —
  strictly worse. The "scope is the cold path only" language refers to not altering
  any other path's *output* or its light-bounding construction (both preserved),
  not to gating where the early-out physically runs.

- **Spike harness → removed in full (Task 3), STATS included.** Precedent, not
  taste: the comparable `perf-animated-sh-light-culling` spike reverted its
  instrumentation to a clean `git diff` and relied on its findings note, and the
  `level-compiler` crate carries no standing `POSTRETRO_*` env instrumentation
  (that pattern lives only in the runtime renderer/netcode, where a live debug
  toggle has ongoing value). Retaining STATS would be a precedent-less permanent
  addition to the compiler, against the lean-binary northstar — and it would keep
  the `WorldReachIndex` / affinity-cell code tied to the very mechanism this spec
  *rejects*. The re-measurement capability is not lost: the parent spike's
  `findings.md` documents the method and env flags, and the full harness is
  recoverable from this branch's git history (`git restore` of the spike commit)
  if a real map later warrants re-measuring the reaching-light distribution — this
  spec's win is byte-identical and content-independent for correctness, so
  real-map validation refines only the *magnitude*, never a gate on shipping.
