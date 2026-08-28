# Base-Density Forward-Predictor Spike — Research Notes

> Measure-and-report per `experimental_spikes.md`. No accuracy/cost threshold is
> contracted in code or here; the numbers feed the promote/no-promote decision,
> they do not gate it. Task 1 records the thin-slice results; the recommendation
> is Task 3's.

## Task 1 — P1 contribution-geometry predictor, primary fixture

**Harness.** New module `crates/level-compiler/src/sh_forward_predict.rs` behind
`--sh-forward-predict` (JSON at `--sh-forward-predict-out`, default
`<output>.forward-predict.json`). Output-preserving: the emitted `.prl` is
byte-identical with and without the flag (SHA256
`1bf9ac8a213cbe5540b106a0779ab07af0a7dbbd88f4661fffd6ff3fd66d5f60` both runs,
127,702,229 bytes — S1 verified).

**Fixture honesty (precondition met).** `content/dev/maps/occlusion-test.map`:
grid 71×15×54, 1008 bricks / 802 non-empty; `has_base_direct: true`,
`has_delta_indirect: true`; 5 static baked lights + 4 animated baked lights
(id 27 populated). The delta-indirect + aimed-spot precondition holds.

**Oracle (S2 — reuses `sh_analyze` `BrickRecord` primitives +
`sh_coarsen::CoarsenParams`).** Per-brick coarsest base-indirect level admitted
by the production relative gate — numerator `base_l1`/`base_l2`, denominator
`composed_magnitude`, `map_p95` 0.558, darkness floor 0.0112 (`darkness_frac`
0.02), gate `rel_p95_max` 0.10 / `rel_max_max` 0.25. It mirrors
`sh_coarsen::classify_levels` Phase A (map-p95) + Phase B (per-brick gate)
exactly; seam-smoothing and protection are deliberately excluded (the oracle is
the raw per-brick classification the predictor is scored against).

Oracle level histogram over the 802 non-empty bricks:

| Level | Bricks | Share |
|-------|--------|-------|
| L0 (dense) | 190 | 23.7% |
| L1 (corners) | 40 | 5.0% |
| L2 (brick-mean) | 572 | 71.3% |

**The base-indirect field on this fixture is highly coarsenable — 612/802 (76%)
of non-empty bricks are L1+L2, 71% fully L2.** This is consistent with finding
3's "near-lossless almost everywhere" on the primary fixture; the predictor's
real job is protecting the 190 contribution-concentrated dense-needed (L0)
bricks. (Records the open question; a two-fixture confirmation is Task 3.)

**P1 predictor.** Per brick, delivered-light magnitude
(`sh_bake::incident_radiance_at_point`, summed over the static baked lights,
max-per-channel) at each valid probe, plus its spatial variation across the
brick: an L2 proxy (relative max deviation from the brick mean) and an L1 proxy
(relative residual from a trilinear corner fit, evaluable only when all 8 corners
are valid — as with the oracle's SH L1). The continuous score is the L2 proxy;
`sh_analyze::choose_level`-style thresholding maps the two proxies to a level.

### Measured findings

- **Cost — strongly favorable.** Predictor evaluation: **0.0058 s** for all 802
  bricks. Negligible against the base-indirect hemisphere bake (256 rays ×
  soft-visibility per probe over ~57k probes). The cost bar is not the concern.
- **Accuracy — the FP bar is not cleared on this slice.** The false-positive rate
  (predictor calls a brick strictly **coarser** than the oracle — the unsafe,
  under-baked direction) **never approaches zero across the sweep**. Its minimum
  is **0.1646 (132/802 bricks) at t ≤ 0.03**, rising monotonically to 0.192 at
  t = 0.50. There is no near-zero-FP operating point on this fixture; the "best"
  row the harness reports (t = 0.010) is simply the min-FP row.
- **Signal strength — weak.** Continuous-score↔oracle-level Pearson correlation
  **r = −0.198** (negative as expected: higher L2-deviation score → less
  coarsenable → lower oracle coarseness). Far weaker than the coarsenability
  spike's contribution-aware r ≈ −0.765 for angle-off-cone-axis. P1's
  delivered-magnitude-gradient score is a much blunter contribution signal than
  angle-off-cone; the gap is a candidate cause to probe in the family.
- **Tradeoff curve (FP-rate vs recovered-savings).** Recovered-safe savings (of
  612 oracle-coarsenable bricks, the fraction P1 also frees at a safe level)
  climbs 0.562 → 0.788 as t goes 0.01 → 0.50, but FP climbs alongside it — the
  curve buys savings by paying false-positives, not by separating the two. The
  132-brick FP floor sits under even the most conservative threshold.

Full sweep in `occlusion-test.forward-predict.json` (`sweep[]`: per-threshold
`confusion[pred][oracle]`, `false_positive_rate`, `recovered_savings_fraction`,
`agreement_rate`).

### Read (thin-slice, not the deliverable)

The thin slice falsifies "a cheap forward predictor reproduces the oracle" for
**P1 specifically** on the primary fixture: P1 is essentially free but its
unsafe-FP rate floors at ~16% and its signal correlates only weakly with the
oracle. This is exactly the falsification the thin slice exists to surface before
the family fans out — it does **not** settle the spike (P2 + occlusion and the
P3 direct-field ceiling are unmeasured, and the FP floor may be occlusion-driven,
which P1 is blind to). No promote/no-promote call is made here.

## Drift from the plan's named symbols (build-to-learn; reported, not hidden)

- **`sh_bake` primitive visibility.** The plan lists `incident_radiance_at_point`
  / `spot_cone_attenuation` / `falloff` / `light_reaches_point` as
  `pub(crate)`/available. In source only `incident_radiance_at_point` is
  `pub(crate)`; the other three are private. P1 needs none of them directly —
  `incident_radiance_at_point` already composes falloff + cone into the returned
  radiance — so no visibility change was made.
- **Oracle function name.** The plan references
  `sh_coarsen::classify_section_levels` (~lines 117–161). No such symbol exists;
  those lines are the Phase A/B gate inside `classify_levels`, and the delta
  provider is `classify_direct_levels`. The oracle mirrors `classify_levels`'
  Phase A + Phase B.
- **Probe positions.** Rather than threading `sh_bake::probe_grid_layout` +
  the bake BVH into the harness, probe positions are reconstructed from the base
  section's `grid_origin` + `cell_size` × index (byte-identical to
  `probe_position`, since `cell_size == [probe_spacing; 3]`). The BVH is not
  routed: P1 does no ray tracing and needs no occlusion input. Routing
  `probe_grid_layout` / BVH is deferred to Task 2 (P2 occlusion), which needs
  them.
- **Predictor light set.** The predictor forecasts the **base-indirect** (static)
  field, so it aggregates the static baked light set (the lights the base bake
  iterates), not the animated set — 5 static lights here.
- **Predictor is 3-level, not binary.** The continuous score thresholds to a
  genuine L0/L1/L2 via the L1/L2 proxies + a `choose_level` mirror, so the
  confusion matrix is a full 3×3. L1 predictions are sparse (the 15-probe-tall
  grid leaves brick corners on invalid y-planes, so the L1 corner basis is
  usually unevaluable) — mirroring the oracle's own L1 rarity (40 bricks).

## Verification (Task 1)

- `cargo check -p postretro-level-compiler` — clean.
- `cargo test -p postretro-level-compiler --bin prl-build sh_forward_predict` —
  **9 passed** (predictor/oracle unit gates + S3 degenerate cases: no lights,
  all-invalid brick, empty grid → well-formed empty matrix, no panic).
- One real bake on the primary fixture: well-formed JSON (20 top-level keys,
  11-row sweep, 802 brick records), non-empty confusion matrix, byte-identical
  `.prl` (hashes above).
