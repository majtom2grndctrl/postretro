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

- A per-light early-out inside `sample_radiance_rgb` (`crates/level-compiler/src/sh_bake.rs`):
  skip a Point/Spot light when its distance to `hit.point` exceeds
  `falloff_range.max(1e-4)`, before the `soft_visibility` shadow ray. Directional
  lights are never skipped.
- The early-out is **unconditional** (no env gate) — the production hardening of
  the env-gated `POSTRETRO_SPIKE_REACH_CULL` prototype.
- A determinism test proving the early-out is contribution-neutral (a bake with an
  out-of-range light produces bit-identical SH coefficients to a bake without it),
  plus a unit test pinning the cull predicate (skip iff `dist > range`; Directional
  never skipped).
- Disposition of the env-gated spike instrumentation (`spike_reach.rs` plus its
  `pipeline.rs` install/log hooks): remove the now-superseded `CULL` path; retain
  or remove the `STATS` distribution harness per the reviewer's call (see Open
  Questions; default below).

### Out of scope

- The affinity-cell / portal reaching-light index for the cold bakes
  (`WorldReachIndex` / `decompose_affinity_for_lights`). The parent spike measured
  it as looser and more complex than the exact per-point range test at these light
  counts, with a cell-boundary byte-identity hazard. Deferred — see Direction.
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
direct/delta/animated-direct SH bakes already consume
(`decompose_affinity_for_lights` → `WorldReachIndex`). The parent spike measured it
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

- [ ] **Byte-identical cold output.** A cold (`--no-cache`) bake of a fixture
  containing both in-range and out-of-range static point/spot lights produces a
  `.prl` byte-identical (SHA256) to a bake of the same inputs built without the
  early-out. (Parent spike verified this on `stress-warren-lit`, full 16.6 MB
  output.)
- [ ] **Contribution-neutral early-out.** A unit test bakes a probe (or samples one
  ray via `sample_radiance_rgb`) whose light set includes a point light positioned
  beyond its falloff range, and asserts the resulting SH coefficients / radiance are
  bit-identical to the same bake with that far light removed from the set.
- [ ] **Predicate matches `falloff() == 0`.** A unit test confirms a Point/Spot
  light is skipped iff `dist > falloff_range.max(1e-4)` (kept at `dist == range`;
  skipped just beyond), across Linear / InverseDistance / InverseSquared models, and
  that a Directional light is never skipped.
- [ ] **Warm and delta output unchanged.** A warm (cached) SH bake and an animated
  delta SH bake of the same inputs are byte-identical before and after the change —
  the early-out skips only zero-contribution lights in their bounded / single-light
  slices.
- [ ] **Fewer shadow rays on the cold path (context, not a gate).** On a fixture
  with out-of-range lights the cold SH stage casts strictly fewer soft-visibility
  shadow rays and completes measurably faster. The parent spike saw ~20.9× (245.9 s
  → 11.8 s) on `stress-warren-lit`; this is a synthetic-fixture figure, not a
  production projection, and is recorded, not thresholded.
- [ ] **No env gate.** The early-out runs on every cold bake with no environment
  variable required to enable it.

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
skip iff `dist > range`), so this task does not depend on `spike_reach.rs` and the
harness can be removed independently in Task 3. Add: (a) a determinism/neutrality
test that bakes a ray or probe with an out-of-range point light present vs. absent
and asserts bit-identical output (mirror the existing `sample_radiance_rgb` direct
call-site test around `sh_bake.rs:2356`); (b) a predicate test covering the three
falloff models at `dist == range` (kept) and `dist > range` (skipped) plus a
Directional light (never skipped). The early-out is unconditional — no env var
gates it.

### Task 2: Fixture byte-identity check

Add or extend a test that compiles a small map fixture containing at least one
in-range and one out-of-range static point/spot light through the cold
(`--no-cache`-equivalent, in-process) SH bake, and asserts the emitted SH volume
section bytes are identical to a reference produced from the pre-change code path
(e.g. a bake where the far light is excluded from the set, which is provably the
same output). This is the whole-section analogue of Task 1's per-ray neutrality
test; it guards the determinism invariant at the section boundary. Use the existing
cold-bake / determinism test patterns in `sh_bake.rs` and
`lightmap_bake.rs:2936` (the "two `--no-cache` bakes" pattern) as the harness model.

### Task 3: Retire the env-gated cull prototype; decide STATS disposition

Remove the `POSTRETRO_SPIKE_REACH_CULL` path now that the production early-out is
unconditional: delete the `cull_enabled()` flag and its branch in
`sample_radiance_rgb`, and any code reachable only through it. **Default
disposition of the STATS harness:** retain `POSTRETRO_SPIKE_REACH_STATS` and its
`spike_reach.rs` distribution histogram plus the `pipeline.rs` `install_sh` /
`log_sh_summary` hooks (off by default, byte-neutral) so a real map can be
re-measured with one command later — the parent spike's measurement was on a
synthetic fixture, and the STATS path is the re-measurement tool. If the reviewer
elects full removal (see Open Questions), delete `spike_reach.rs`, its `mod`
declaration, the `install_sh` / `install_lm` / `record_sh` / `record_lm` /
`log_sh_summary` / `log_lm_summary` call sites in `pipeline.rs`, and the
`record_lm` hook in `lightmap_bake.rs`. Either way, after this task no code path is
gated on `POSTRETRO_SPIKE_REACH_CULL`, and the cold SH early-out remains
unconditional. Keeping STATS while making the cull unconditional is coherent: STATS
counts the reaching-light *distribution* at each hit point, independent of whether
the cull fires; only the removed CULL flag's baseline-vs-cull A/B comparison is lost
(and it is lost by design, since the cull now always runs).

## Sequencing

**Phase 1 (sequential):** Task 1 — the core early-out plus its per-ray neutrality
and predicate tests. Falsifies the byte-identity assumption at the smallest grain
before anything else builds on it.
**Phase 2 (concurrent):** Task 2, Task 3 — Task 2 adds the section-level fixture
byte-identity check; Task 3 retires the prototype cull path. Independent: Task 2
touches test code and reads the post-Task-1 sampler; Task 3 removes the env-gated
branch Task 1 already superseded. Both depend on Task 1's early-out existing.

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

Existing test anchors: the direct `sample_radiance_rgb` call sites at
`sh_bake.rs:2356` / `:2368` (soft-visibility test) show the per-ray test harness;
`lightmap_bake.rs:2936` shows the "two `--no-cache` bakes are identical"
determinism pattern for the section-level check.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Cold whole-volume SH `.prl` byte-identical to pre-change output | Task 1 (early-out skips only `falloff()==0` lights) | Threatened if the predicate skips any contributing light, or if a skip perturbs a kept light's seed | Byte-identical cold output AC; Contribution-neutral AC; Task 2 |
| Cull predicate == `falloff()==0` region: skip iff `dist > falloff_range.max(1e-4)`; Directional never skipped; kept at `dist == range` | Task 1 (local range predicate mirroring `falloff`) | Threatened by a looser bound (culls a contributing light) or a tighter bound (screens backface/cone — out of scope) | Predicate-matches AC |
| Kept lights' soft-visibility seeds unchanged by any skip | Task 1 (`soft_visibility_seed` is a pure fn of probe/ray/global index; skip shifts no other index) | Threatened if the skip reindexes or reorders the light loop | Byte-identical cold output AC; Contribution-neutral AC |
| Warm grouped + animated delta SH output unchanged | Task 1 (shared `sample_radiance_rgb` early-out is byte-neutral in bounded/single-light callers) | Threatened if placement changes warm's bounded-set semantics or delta's slice | Warm-and-delta-unchanged AC |
| No path gated on `POSTRETRO_SPIKE_REACH_CULL`; early-out always runs | Task 3 (removes the CULL flag/branch) | Threatened if a residual env check survives | No-env-gate AC |

## Open questions

- **STATS harness disposition (reviewer decision).** Default: retain the
  off-by-default `POSTRETRO_SPIKE_REACH_STATS` distribution instrumentation
  (`spike_reach.rs` + `pipeline.rs` install/log hooks) for future real-map
  re-measurement, and remove only the now-superseded `CULL` path (Task 3).
  Alternative: delete the whole harness for a smaller surface. The parent spike's
  measurement was synthetic-only, so the re-measurement tool has residual value;
  the counter-argument is dead-code hygiene. Owner's call — surfaced per the
  findings' note that the spike prototype demonstrates the change and the
  production version makes the early-out unconditional.
- **Early-out placement (recommendation, flag for review).** Recommended: place the
  early-out inside the shared `sample_radiance_rgb`, matching the prototype and the
  lightmap's early-out-before-shadow-ray structure. It executes in the warm and
  animated-delta callers too, but is byte-neutral there (they pass a pre-bounded or
  single-light slice; the early-out only skips lights already contributing zero) and
  adds only a cheap per-light distance check. The strict alternative — gating the
  early-out so it runs only on the cold monolithic caller — needs a threaded flag
  and would still leave the delta path casting the same wasted rays; it is not
  recommended, but the placement is surfaced because "scope is the cold path only"
  admits both readings.
