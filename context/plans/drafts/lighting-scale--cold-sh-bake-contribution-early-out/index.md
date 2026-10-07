# lighting-scale--cold-sh-bake-contribution-early-out

Brief · compact · reads: `context/lib/build_pipeline.md` §Compiler pipeline · read at c855e3e

## Problem
Developer-raised. Owner goal: the baker takes less time in ways players cannot notice, with no
added load or frame time. A profile of the cold SH bake on `stress-warren-mini` (the stage is
721 s of a 1052 s ship bake; `research.md`) puts **46 % of the stage in `SoftProbes::new`**:
`soft_visibility` re-snaps a light's four probe indices against its whole sample lattice
(~132 sin/cos evaluations) for every bounce hit and every in-range light, although the snap
depends only on the light and the sample count. The shadow rays themselves are 40 %. Every
other per-call `soft_visibility` caller (direct and delta SH, billboard scatter, animated
weight maps) pays the same re-snap; the lightmap already hoists it per chart.

Second, smaller: the bounce's pre-ray guard inherited from
`lighting-scale--cold-sh-bake-falloff-early-out` (`light_reaches_point`) tests only falloff
range. A spot aimed away, or any light behind the surface, contributes exactly zero yet still
pays its shadow ray (four probe traces for a soft light, one for a hard light — not the 32 the
escalated penumbra costs; agreeing probes never escalate), multiplied by that zero afterward.
`light_contribution_lambert` already folds range, cone and `max(n·l, 0)`; the cold lightmap
gates on the full term (`light_texel_contribution_and_visibility`).

When done: the re-snap runs once per thread and count for point and spot lights, the bounce
skips any light whose Lambert term at the hit is exactly zero, and every emitted `.prl` is
byte-for-byte identical to today's — nothing reaches the runtime.

## Decisions
- **Reuse the point/spot probe snap; leave directional per call.** For `Point`/`Spot`,
  `probe_sample_direction` is `fibonacci_sphere_sample(i, count, 0)` — light-independent — so
  `SoftProbes::new` keeps a one-entry thread-local memo keyed by `full_samples`. Pure function
  of its key, so every caller gets the identical set it computes today, with no plumbing
  through the cold, warm-group, delta or scatter paths. Directional snaps depend on the
  light's axis basis and angular diameter; suns are few, so they keep the per-call snap.
  Rival rejected: a per-light `SoftProbes` table threaded through each caller (the lightmap's
  shape) — more plumbing at five call sites for the same bytes.
- **Guard on the full zero-contribution set, using the SH bake's own math.** Extend the
  pre-ray guard in the shared sampler `sample_radiance_rgb` (`sh_bake.rs`) to skip a light
  when `light_contribution_lambert(light, hit.point, hit.normal)` is exactly `Vec3::ZERO`.
  This subsumes the range-only `light_reaches_point` and adds cone (`spot_cone_attenuation`
  is exactly 0 past the outer cone) and back-face (`max(n·l, 0)` is exactly 0 when the
  surface faces away). Compute against the SH bake's own `incident_radiance_at_point` — not
  the lightmap's `contribution_covers_shadowmask`, whose epsilon is a different predicate.
- **Keep the range test ahead of the zero test.** For finite light parameters an out-of-range
  term is already exactly zero, but a non-finite origin a `.map` can author (`parse_origin`
  accepts `nan`) yields a NaN term, not zero. The range-only guard skipped it; keeping
  `light_reaches_point` first preserves that, so the loop is bit-identical for every input.
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
- **Compute the Lambert term once.** The guard's term is the accumulation's term; the
  sampler reuses it after visibility instead of re-evaluating.
- **Cone-culling here does not reopen `sh-delta-cone-reach-cull`.** That plan kept cone
  culling out of indirect bakes because bounced light leaves the cone at the *probe*. This
  guard tests the light→hit-point leg, which is direct; the bounce leg is untouched.
- **Non-goal — fewer soft-visibility samples.** Escalation already holds agreeing receivers to
  four probe rays; changing the probe or escalated count is a quality tradeoff, not
  byte-identical. Not here.
- **Non-goal — the affinity-cell / portal reaching-light index for the cold bakes.** The
  parent spike measured it as looser and more complex than the exact per-point test at these
  light counts, with a cell-boundary byte-identity hazard (`findings.md`). Deferred there;
  this brief keeps the per-receiver test.
- **Non-goal — guards in the other bakes.** The lightmap already gates on the full term; the
  direct family culls by reach. They gain only the shared probe-snap reuse.
- **Layer placement — compile-time, bake-internal.** No format, section, wire, or runtime
  change; no cache-key or stage-version change — warm and cold output is byte-identical and
  `main`'s cache reads back identically (`research.md`).

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
- [ ] `SoftProbes::new` returns exactly a fresh `probe_indices` snap for point, spot and
  directional lights across interleaved counts (a count change misses the memo; a different
  point or spot light at the same count hits it).
- [ ] `campaign-test.map` and a spot-heavy fixture (`stress-warren-mini`) emit a byte-identical `.prl`
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

**Guard boundaries the review added**
- [ ] A light with a NaN origin is skipped on range, exactly as before (it sits in the
  bit-identity fixture beside the culled lights, which now lead the slice so a renumbered seed
  would change the bits).
- [ ] A spot term just inside the outer cone, below the lightmap's `1e-12` coverage epsilon,
  keeps its ray and reaches the bounce sample.

**Regression guards**
- [ ] The existing range-boundary behavior is preserved — a light at exactly its falloff
  range keeps or drops as before (the range early-out is subsumed, not regressed).
- [ ] A directional light is never skipped for a surface that faces it (front-facing sun keeps
  its ray across the world).

### Manual
- [ ] Per-stage wall-clock, baseline vs. change, cold, on `campaign-test` and
  `stress-warren-mini`, plus a profile of the SH stage before and after, recorded in
  `research.md`. Recorded result, not a pass/fail threshold — synthetic fixtures bound the
  mechanism, not the shipping magnitude.

## Path
- **Seam.** The pre-ray guard in `sample_radiance_rgb` (`sh_bake.rs`), before seed
  derivation and the `soft_visibility` trace — formerly the range-only `light_reaches_point`
  alone, now `bounce_contribution`. Reuse `light_contribution_lambert` / `incident_radiance_at_point`
  (same module) as the zero predicate; do not reinvent the cone or `n·l` math. Precedent for
  the full-term gate: `light_texel_contribution_and_visibility` (`lightmap_bake.rs`).
- **Seam (probe reuse).** `SoftProbes::new` (`lightmap_bake.rs`), the single constructor
  every `soft_visibility` call goes through; the memo lives beside it.
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
- Does a cache key capture the culled ray set or the probe snap, requiring a stage-version
  bump? — **resolved: no.** Warm output is byte-identical to `main`'s, including when this
  change reads `main`'s cache (`research.md`).
