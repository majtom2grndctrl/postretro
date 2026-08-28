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

## Task 2 — full input-cost family spectrum, all scored vs the same oracle

Extends the Task 1 harness (`crates/level-compiler/src/sh_forward_predict.rs`) with
the rest of the family, all scored against the **unchanged** Task 1 oracle (S2).
Every family reduces to one shape — a per-probe scalar field over the brick, then
the same L2/L1 spatial-variation proxies and the same sweep/confusion scoring;
families differ only in the per-probe scalar. The JSON gains a `families` map keyed
`P1`/`P2`/`distance`/`surface_distance`/`P3`; the top-level P1 surface (`sweep`,
`bricks`, `score_vs_oracle_correlation`, `predictor_eval_seconds`,
`best_operating_point`, `oracle_histogram`) is preserved unchanged.

Oracle (unchanged from Task 1): 802 non-empty bricks, L0 190 / L1 40 / L2 572,
map-p95 0.558, floor 0.0112.

### Per-family measured findings (occlusion-test, warm build)

Best near-zero-FP operating point per family (the harness picks the min-FP row when
no zero-FP row exists — none exists for any contribution-aware family):

| Family | eval wall-time | score↔oracle r | best t | FP-rate (bricks) | recovered-savings | agreement |
|--------|---------------|----------------|--------|------------------|-------------------|-----------|
| **P1** contribution geom | **0.0058 s** | −0.198 | 0.01 | **0.1646** (132) | 0.562 (344/612) | 0.503 |
| **P2** P1 + occlusion | **0.2230 s** | −0.213 | 0.01 | **0.1658** (133) | 0.655 (401/612) | 0.572 |
| **P3** direct-field ceiling | **0.0133 s** | −0.214 | 0.05 | **0.1658** (133) | 0.665 (407/612) | 0.572 |
| distance (control) | 0.0022 s | −0.169 | 0.01 | 0.0461 (37) | 0.131 (80/612) | 0.198 |
| surface_distance (control) | 0.1597 s | +0.026 | 0.075 | 0.0362 (29) | 0.239 (146/612) | 0.279 |

Full FP-vs-recovered sweeps in `occlusion-test.forward-predict.json`
(`families[*].sweep[]`). FP-floor / recovered-ceiling over the sweep:
P1 0.1646..0.1920 (rec 0.562..0.788); P2 0.1658..0.1820 (rec 0.655..0.802);
P3 0.1658..0.1820 (rec 0.655..0.802).

### Reads

- **The ~16.5% unsafe-FP floor is NOT occlusion-driven, and NOT a cheap-signal
  artifact.** Adding a per-light BVH shadow test (P2) moves the FP floor from 132
  to 133 bricks — it does not shrink it; occlusion only *raises recovered savings*
  (0.562 → 0.655) at the same floor. The P3 direct-field **ceiling** — the richest
  available signal, the actual baked direct-at-probe field — floors at the *same*
  133 bricks (0.1658). So on this fixture no forward signal, however rich, reaches
  a near-zero-FP operating point; the FP floor is structural to the fixture, not a
  poverty of the cheap predictors.
- **Cost side (per-family, measured).** P1 0.0058 s and the distance control
  0.0022 s are effectively free; P2's per-light shadow rays cost 0.223 s (≈40× P1,
  still a small fraction of the base-indirect bake — SH Bake stage 0.25 s warm,
  and far below a cold 256-ray hemisphere bake); surface_distance 0.160 s is the
  triangle-scan control. Cost is not the bar that fails — accuracy is.
- **Distance / surface-distance refutation.** The distance control reaches a low
  FP (0.046) only by refusing to coarsen — it recovers just 13% of the oracle's
  coarsenable bricks (80/612); surface_distance has essentially no signal
  (r = +0.026) and recovers 24% at FP 0.036. Contribution-aware P1/P2 recover
  56–66% of savings but pay the 16.5% FP; the distance controls trade nearly all
  the savings away to look safe. This reproduces the archived classifier's
  invalidation: distance cannot separate coarsenable from dense-needed bricks.
  (Correlations on this delta-indirect fixture are all weak — contribution-aware
  −0.20/−0.21 vs distance −0.17 — far below the coarsenability spike's −0.765 for
  angle-off-cone-axis; the operational separation is in the recovered-savings /
  FP tradeoff, not the raw Pearson r. P1's delivered-magnitude-*gradient* score
  remains a much blunter contribution signal than raw angle-off-cone.)

### P3 open-question measurement (id 35 before or after id 34)

**Measured, not assumed: id 35 (base direct) bakes AFTER id 34 (base indirect).**
The compiler stage order (`pipeline.rs` `STAGE_ORDER`) is ShBake (id 34) →
DeltaShBake (id 27) → DirectShBake (id 35) → AnimatedDirectShBake; the base-direct
field does not exist when the base-indirect bake it would predict runs.
Marginal cost of the direct field: DirectShBake stage **0.20 s** warm (vs ShBake
0.25 s). Recorded in the report as `p3_direct_baked_before_base_indirect: false`
with `p3_ordering_note`. **Consequence:** P3 is an accuracy *ceiling* only — never
a viable cheap pre-bake predictor — and even as a ceiling it does not clear the FP
bar on this fixture (see above).

### `pub(crate)` visibility added (reported, not hidden)

Three tested primitives were exposed (minimal; no logic change), preferring reuse
of the existing tested code over re-deriving ray/triangle math (per the task
guidance and Task 1's finding that most were private):

- `sh_bake::segment_clear` `fn` → `pub(crate) fn` — P2's per-light corner shadow
  test against the bake BVH (constructs the already-`pub(crate)` `RaytracingCtx`).
- `sh_analyze::decode_base_direct_tile` `fn` → `pub(crate) fn` — P3 reads the baked
  id-35 direct tile per probe (reuses the tested decode instead of re-implementing
  the atlas addressing).
- `sdf_bake::point_triangle_distance_sq` `fn` → `pub(crate) fn` — the
  surface-distance control's nearest-triangle kernel (the AABB cheap-reject loop is
  re-inlined in the harness; only the delicate barycentric kernel is hoisted).

Routing: the bake BVH (`&bvh`, `&bvh_primitives`) and `&geo_result` are now threaded
from `pipeline.rs::run_sh_forward_predict` into `ForwardPredictInputs` (following
Task 1's base-section routing at the `run_sh_analysis` seam), as Task 1 anticipated
for P2 occlusion and the surface-distance control.

### Further drift from the plan's named symbols

- **P1 never predicts L1; distance/surface do occasionally.** L1-evaluability is
  identical across families (all 8 brick corners present). The delivered-magnitude
  field is smooth enough that L2 passes before L1 is ever needed, so P1/P2/P3's
  confusion middle row is ~empty (mirrors Task 1's L1-rarity note and the oracle's
  own 40 L1 bricks); the distance fields fail L2 but pass L1 slightly more often.
- **Predictor score anchor renamed** `mean_delivered` → `mean_scalar` in
  `BrickPrediction` (the field is now a generic per-probe scalar mean — delivered
  magnitude for P1/P2, distance for the controls, baked direct for P3), and the
  continuous score is still the L2 proxy.

## Verification (Task 2)

- `cargo check -p postretro-level-compiler` — clean.
- `cargo test -p postretro-level-compiler --bin prl-build sh_forward_predict` —
  **14 passed** (Task 1's 9 + 5 new: the occlusion primitive zeros a
  BVH-blocked light and keeps an unobstructed one; nearest-light-distance is
  `None` without finite-origin lights; nearest-surface-distance is `None` without
  geometry; the distance-family brick signal is well-formed; the P3 scalar is
  `None` without a base-direct section; and the empty-grid S3 case yields a
  well-formed empty sweep for EVERY family).
- S1 re-confirmed: `occlusion-test.map` baked with and without
  `--sh-forward-predict` → SHA256 identical, `1bf9ac8a213cbe5540b106a0779ab07af0a7dbbd88f4661fffd6ff3fd66d5f60`
  (127,702,229 bytes) on both, matching Task 1's baseline. (~127 MB `.prl` outputs
  deleted after.)
- One real harness run: all five families populate `families` with non-empty
  confusion matrices and 802 brick records each; the top-level P1 surface is
  unchanged from Task 1.

## Task 3 — measurement sweep across both fixtures

Full family run on both fixtures. The decision/hand-off is `findings.md`; this
section is the raw-numbers log. Primary `occlusion-test.map` re-run for the
record (dev-optimized `target/debug/prl-build`, matching Tasks 1–2); cross-check
`stress-warren-mini.map` run with `target/release/prl-build` — see the build-profile
note below.

### Build-profile / environment note (reported, not hidden)

`stress-warren-mini` **cannot complete its bake under the dev-optimized profile in
this 15 GB-cgroup environment**: the `Lightmap Bake` stage (which precedes `SH Bake`)
accumulates a per-texel, layer-major atlas that OOM-killed the process at ~14 GB /
~6 % lightmap progress (ETA ~21 min for lightmaps alone), on both the dev and release
profiles, twice. The lightmap atlas is orthogonal to everything the harness reads —
the oracle and every predictor consume the **SH probe volumes** (id 34 base-indirect,
id 35 base-direct, id 27 delta-indirect) and `BrickRecord` composed magnitude, none of
which touch the surface lightmap atlas — so the run used `--lightmap-density 0.16`
(vs the 0.04 m/texel default; ~16× fewer texels) to shrink the atlas below the memory
limit. This changes no SH-volume byte, no oracle/predictor input, and no `SH Bake`
stage time; it only makes the unrelated lightmap stage fit. `occlusion-test` bakes
fine at the default density and was left at default. Consequence for the cost axis:
`occlusion-test` timings are dev-optimized; `stress-warren-mini` timings are release.
The **eval-vs-bake ratio is measured within one profile per fixture** (both the
predictor eval and the `SH Bake` denominator come from the same run), so the cost
verdict is apples-to-apples per fixture; only cross-fixture *absolute* times differ by
profile.

### Base-indirect bake wall-time (cost denominator, `SH Bake` stage banner)

| Fixture | profile | `SH Bake` (base-indirect, id 34) | total build |
|---|---|---|---|
| occlusion-test | dev-optimized | **0.24–0.26 s** (warm) | 3.88 s |
| stress-warren-mini | release | **715.94 s (~11.9 min)** | 983.16 s |

`stress-warren-mini`'s `SH Bake` is the honest heavy-fixture cost — a ~12-minute
256-ray-per-probe hemisphere bake over the 125×18×223 grid. This is the number the
bake-time win would have to beat.

### Fixture honesty

| Fixture | `has_delta_indirect` | baked spots (`light_spot`) | animated | static lights | precondition |
|---|---|---|---|---|---|
| occlusion-test | **true** | **2** (static) + 4 animated spots | 4 | 5 | **holds** |
| stress-warren-mini | **true** | **12** (static) | 6 | 49 | **holds** |

`baked_spot_count` was added to the report this task (a trivial, output-preserving
harness field — count of `LightType::Spot` among the static baked light set — so the
aimed-spot half of the honesty gate is self-contained in the JSON; S1 re-verified
byte-identical on occlusion-test after the change, SHA
`1bf9ac8a213cbe5540b106a0779ab07af0a7dbbd88f4661fffd6ff3fd66d5f60`, and the 14 harness
tests still pass). Note the exact counts differ from the plan's Rough-sketch estimate
("16 baked aimed spots + 3 animated") — source has 16 `light_spot` total, of which the
baked static set holds 12 and 6 lights are animated — but the **precondition**
(`has_delta_indirect: true` AND non-zero baked aimed-spots) holds solidly on both.

### Oracle — base-indirect coarsenability on each fixture (Open Question resolved)

| Fixture | non-empty | L0 (dense) | L1 | L2 | coarsenable (L1+L2) | fully L2 |
|---|---|---|---|---|---|---|
| occlusion-test | 802 | 190 (23.7 %) | 40 | 572 | **612 (76.3 %)** | 71.3 % |
| stress-warren-mini | 7889 | 186 (2.4 %) | 200 | 7503 | **7703 (97.6 %)** | 95.1 % |

**Finding 3's "base indirect coarsens near-losslessly almost everywhere" HOLDS on both
fixtures, and more strongly on the heavier realistic map** (97.6 % coarsenable). The
predictor's real job is therefore protecting the small dense-needed minority — 24 % of
bricks on occlusion-test, only **2.4 %** on stress-warren-mini — and that is exactly
where it fails.

### Per-family × per-fixture — best near-zero-FP operating point

FP-rate = predictor-coarser-than-oracle bricks ÷ non-empty bricks (the unsafe,
under-baked direction). "recovered" = fraction of the oracle's L1+L2 bricks the
predictor also frees at a safe level. Harness picks the min-FP sweep row.

**occlusion-test** (base-indirect bake 0.24 s):

| Family | eval s | eval/bake | r | best t | FP-rate (bricks) | recovered | agree |
|---|---|---|---|---|---|---|---|
| P1 contribution geom | 0.0057 | 2.4 % | −0.198 | 0.010 | **0.1646** (132) | 0.562 | 0.503 |
| P2 P1+occlusion | 0.2133 | 88 % | −0.213 | 0.010 | **0.1658** (133) | 0.655 | 0.572 |
| P3 direct-field *ceiling* | 0.0137 | 5.7 % | −0.214 | 0.050 | **0.1658** (133) | 0.665 | 0.572 |
| distance (control) | 0.0022 | — | −0.169 | 0.010 | 0.0461 (37) | 0.131 | 0.198 |
| surface_distance (control) | 0.1624 | — | +0.026 | 0.075 | 0.0362 (29) | 0.239 | 0.279 |

**stress-warren-mini** (base-indirect bake 715.94 s):

| Family | eval s | eval/bake | r | best t | FP-rate (bricks) | recovered | agree |
|---|---|---|---|---|---|---|---|
| P1 contribution geom | 0.2180 | **0.030 %** | −0.068 | 0.010 | 0.0013 (10) | 0.173 | 0.048 |
| P2 P1+occlusion | 6.0996 | **0.85 %** | −0.200 | 0.030 | 0.0134 (106) | 0.276 | 0.126 |
| P3 direct-field *ceiling* | 0.1065 | 0.015 % | −0.205 | 0.030 | 0.0132 (104) | 0.277 | 0.129 |
| distance (control) | 0.0462 | — | +0.037 | 0.010 | 0.0001 (1) | 0.092 | 0.033 |
| surface_distance (control) | 1.8874 | — | −0.134 | 0.075 | 0.0022 (17) | 0.281 | 0.227 |

### The stress-warren-mini "low FP" is a denominator artifact, not a win

On stress-warren-mini FP-as-fraction-of-total is small at the min-FP row **because
97.6 % of bricks are coarsenable** — a predictor that mostly says "coarsen" is mostly
right. But the min-FP row buys that safety by **refusing to coarsen**: P1's confusion
at t=0.010 calls 6546/7889 (83 %) of bricks dense (L0) to recover only 17.3 % of the
coarsenable majority — it behaves like the distance control, not a discriminator
(r=−0.068). To recover a *meaningful* fraction you climb the threshold, and FP climbs
with it — there is no knee:

P1 stress FP-vs-recovered along the sweep: recover 0.173 → FP 10 (t=0.01); recover
0.578 → FP 147 (t=0.05); recover 0.872 → FP 251 (t=0.10); recover 0.974 → FP 382
(t=0.50). There are only **186 L0 dense-needed bricks**: at recover 0.87 the 251 FP
already exceeds the entire dense-needed population's worth of mis-leveling, and at
recover 0.97 the 382 FP ≈ 99 % of the 386 non-L2 bricks. **Meaningful savings ⇒
mis-coarsening essentially the entire dense-needed minority.**

### Confusion matrices at the safest threshold (rows = predictor L0/L1/L2, cols = oracle L0/L1/L2; L2 coarsest)

- occlusion-test P1 @t=0.010: `[[59,39,228],[0,0,0],[131,1,344]]` — **131 of 190
  dense-needed (L0) bricks predicted L2 (fully coarsened) = 69 % of dense regions
  under-baked, even at the safest threshold.** FP floor 132.
- occlusion-test P2 @t=0.010: `[[58,39,171],[0,0,0],[132,1,401]]` — occlusion moves the
  L0→L2 miss from 131 to 132; it does not shrink it.
- stress-warren-mini P1 @t=0.010: `[[182,147,6217],[0,47,1137],[4,6,149]]` — safe (FP
  10) only by calling 6546 bricks dense; 6217 truly-L2 bricks left un-coarsened
  (missed savings), 17.3 % recovered.
- stress-warren-mini P2 @t=0.030: `[[134,143,5377],[0,3,1266],[52,54,860]]`.

### Contribution-aware vs distance correlation (primary fixture, the stronger test)

On the delta-indirect primary fixture (`occlusion-test`) the contribution-aware
signals barely beat the distance control and all sit far below the coarsenability
spike's r=−0.765 for angle-off-cone-axis: **P1 −0.198, P2 −0.213** vs **distance
−0.169, surface_distance +0.026**. On stress-warren-mini P1 collapses to **−0.068**
(no usable signal) while the occlusion-augmented P2/P3 reach −0.20. The operational
separation the recommendation rests on is the recovered-savings/FP tradeoff, not the
raw Pearson r — and on that axis no family separates coarsenable from dense-needed.

### P3 ordering (re-confirmed both fixtures)

`p3_direct_baked_before_base_indirect: false` on both — id 35 (DirectShBake) bakes
AFTER id 34 (ShBake) in `pipeline.rs` `STAGE_ORDER`. P3 is an accuracy **ceiling**
only. And as a ceiling it does not clear the FP bar on either fixture (it floors at the
same 133 bricks as P2 on occlusion-test, and shows the same no-knee tradeoff on
stress-warren-mini) — so the failure is **not** a poverty of the cheap signals.

### Verification (Task 3)

- `cargo check -p postretro-level-compiler` — clean.
- `cargo test -p postretro-level-compiler --bin prl-build sh_forward_predict` — **14
  passed** (unchanged from Task 2; the `baked_spot_count` addition touched only the
  report struct + assembly + log line).
- S1 re-confirmed on occlusion-test after the harness change: `.prl` byte-identical
  with and without `--sh-forward-predict`, SHA
  `1bf9ac8a213cbe5540b106a0779ab07af0a7dbbd88f4661fffd6ff3fd66d5f60` (127,702,229
  bytes), matching the Task 1/2 baseline. All large `.prl` outputs deleted after each
  run (≤1 on disk at a time).
- Both JSON reports written to scratchpad; all five families populate full 3×3
  confusion matrices summing to the non-empty brick count (802 / 7889) on both
  fixtures — no unclassified cell, S3 holds.
