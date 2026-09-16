# lighting-scale--cold-sh-bake-contribution-early-out

Brief · compact · reads: `context/lib/build_pipeline.md` §Compiler pipeline · read at 6c4946a

## Problem
Developer-raised, from the cold-bake reaching-light spike's out-of-scope findings (§4).
The cold whole-volume SH indirect bake casts the full 32-sample `soft_visibility` shadow
ray for every in-range light at each bounce hit point, but the pre-ray guard it inherited
from `lighting-scale--cold-sh-bake-falloff-early-out` (`light_reaches_point`) tests only
falloff **range**. A spot light in range but aimed away, or any light behind the surface,
contributes exactly zero to the bake — its cone attenuation or its `n·l` term is zero — yet
still pays the wasted shadow ray, which is then multiplied by that zero. The bake's own
contribution term already folds in all three factors: `light_contribution_lambert` =
`incident_radiance_at_point` (falloff × `spot_cone_attenuation` for spots) × `max(n·l, 0)`.
The guard tests one factor of the three. The cold lightmap bake already gates on the full
term (`light_texel_contribution_and_visibility`). When done: the SH bake skips the shadow
ray for any light whose Lambert contribution at the hit point is exactly zero — range, cone,
or back-face — and the emitted `.prl` is byte-for-byte identical to today's.

## Decisions
- **Guard on the full zero-contribution set, using the SH bake's own math.** Extend the
  pre-ray guard in the shared sampler `sample_radiance_rgb` (`sh_bake.rs`) to skip a light
  when `light_contribution_lambert(light, hit.point, hit.normal)` is exactly `Vec3::ZERO`.
  This subsumes the range-only `light_reaches_point` and adds cone (`spot_cone_attenuation`
  is exactly 0 past the outer cone) and back-face (`max(n·l, 0)` is exactly 0 when the
  surface faces away). Compute against the SH bake's own `incident_radiance_at_point` — not
  the lightmap's `contribution_covers_shadowmask`, whose epsilon is a different predicate.
- **Exact zero, not an epsilon.** Skip only when the contribution is exactly zero. An
  epsilon (near-zero cone fringe) would drop a tiny nonzero term the f32 accumulation keeps,
  breaking byte-identity. Byte-identity is then true by construction: a light contributing
  exactly zero adds `radiance += 0 · v = 0` today regardless of visibility, so dropping its
  shadow ray cannot change any coefficient. Near-zero fringe culling is a separate,
  non-byte-identical decision — not this brief.
- **Back-face cull applies to every light type, directional included.** A surface facing away
  from the sun has an exactly-zero Lambert term, so skipping its shadow ray is byte-identical.
  This is distinct from the range/reach cull, which never drops a directional (it has no
  falloff sphere and reaches every cell). The guard must not over-inherit "directionals are
  never skipped" from `cold-sh-bake-falloff-early-out` and leave the back-face ray uncut —
  while never skipping a directional for a surface that faces it.
- **Non-goal — adaptive soft-visibility sample count.** The SH bounce always casts
  `DEFAULT_AREA_SAMPLE_COUNT` samples per kept ray. Cutting that count (spike out-of-scope
  §1) is a visual-quality tradeoff, a separate spike, and not byte-identical. Not here.
- **Non-goal — the affinity-cell / portal reaching-light index for the cold bakes.** The
  parent spike measured it as looser and more complex than the exact per-point test at these
  light counts, with a cell-boundary byte-identity hazard (`findings.md`). Deferred there;
  this brief keeps the per-receiver test.
- **Non-goal — the cold lightmap bake and the direct/delta/animated bakes.** The lightmap
  already gates on the full contribution term; the direct family already culls by reach.
  Untouched.
- **Layer placement — compile-time, bake-internal.** No format, section, wire, or runtime
  change; no cache-key or stage-version change (output is byte-identical and the guard is
  internal to the bake loop — the executor confirms the cold SH cache key does not capture
  the culled ray set).

## Acceptance

### Automated
- [ ] The cull predicate is unit-tested directly: skip iff `light_contribution_lambert` at the
  hit point is exactly zero — exercised for range-out, cone-out, and back-face lights (skipped)
  and for an in-range/in-cone/front-face light (kept). This pins that the cull fires, so the
  byte-identity rows below cannot pass on a build that culls nothing.
- [ ] On a fixture with an out-of-cone spot light, the extended-guard bake produces
  bit-identical SH coefficients to the range-only-guard bake — same fixture, unchanged light
  set, so the skipped cone ray is proven contribution-neutral without renumbering any kept
  light's visibility seed.
- [ ] On a fixture with a back-facing light (point, spot, and a directional/sun), the
  extended-guard bake is bit-identical to the range-only-guard bake — same framing.
- [ ] `campaign-test.map` and a spot-heavy fixture (the warren) emit a byte-identical `.prl`
  before and after this change, cold and warm; SHA-256 recorded in `research.md` (per bake
  mode — the intentionally-approximate warm base SH is compared warm-to-warm).

**Cone predicate (both sides)**
- [ ] A hit point exactly on a spot's outer cone (`cos θ == cos_outer`, `spot_cone_attenuation
  == 0`): the light is skipped.
- [ ] A hit point just inside the outer cone: the light is kept and its baked contribution is
  byte-identical to today.

**Back-face predicate (both sides)**
- [ ] A hit point whose surface normal is exactly perpendicular to the light direction
  (`n·l == 0`): the light is skipped, directional included.
- [ ] A slightly front-facing hit point (`n·l > 0`): the light is kept, contribution
  byte-identical.

**Regression guards**
- [ ] The existing range-boundary behavior is preserved — a light at exactly its falloff
  range keeps or drops as before (the range early-out is subsumed, not regressed).
- [ ] A directional light is never skipped for a surface that faces it (front-facing sun keeps
  its ray across the world).

### Manual
- [ ] Cold SH stage wall-clock, baseline vs. extended guard, on the spot-heavy fixture,
  recorded in `research.md`. Recorded result, not a pass/fail threshold — the spike rates the
  win "marginal on top of range but cheap," and the synthetic fixture bounds the mechanism,
  not the shipping magnitude.

## Path
- **Seam.** The pre-ray guard in `sample_radiance_rgb` (`sh_bake.rs`) — today `if
  !light_reaches_point(light, hit.point) { continue; }` before seed derivation and the
  `soft_visibility` trace. Reuse `light_contribution_lambert` / `incident_radiance_at_point`
  (same module) as the zero predicate; do not reinvent the cone or `n·l` math. Precedent for
  the full-term gate: `light_texel_contribution_and_visibility` (`lightmap_bake.rs`).
- **Shape.** Replace the range-only guard with an exact-zero test on the Lambert term. The
  strongest rival — matching the lightmap's `contribution_covers_shadowmask` epsilon — is
  rejected in Decisions (not byte-identical).
- **Seed determinism.** The guard precedes global-index and seed derivation, exactly as the
  range guard does; kept lights retain their slice/global index and deterministic visibility
  seed, so a kept light's ray is unchanged. Skipped lights never contributed, so their seed is
  irrelevant.
- **First slice.** On the spot-heavy fixture, add the cone/back-face test and assert the
  emitted `.prl` SHA-256 is unchanged from the range-only build — byte-identity is the
  make-or-break, and it holds by construction or the predicate is wrong.

## Open questions
- Does the cold SH cache key capture the culled ray set, requiring a stage-version bump? —
  **delegated**: the executor confirms output byte-identity across cold and warm settles it;
  if a bump is needed, it is a note in the plan of record, not a decision to make here.
