# Base-density forward-predictor efficacy — decision record

**Question.** Can a *cheap forward predictor* — evaluable **before** the base-indirect SH
bake, from light geometry + probe positions alone — decide which affinity bricks may skip
dense probe bakes accurately enough (near-zero unsafe false-positives) and cheaply enough to
justify the foundational base-density (bake-time-win) one-way door (base-grid `density_level`
format + renderer variable-density sampler)?

**Answer: NO — do not open that door on a forward predictor.** This is the measurement-first
research spike the archived base-density plan
(`context/research/archived-plans/lighting-scale--adaptive-base-probe-density/`) said its
go/no-go depended on. It closes the **bake-time** branch of that line. The **storage/bandwidth**
half already shipped (`context/plans/done/lighting-scale--sh-adaptive-coarsening-v2/`); only the
bake-time win depended on the forward predictor, and that is what this closes.

## What was measured

A measurement-only harness (`--sh-forward-predict`, byte-preserving, since reverted — recoverable
from branch `claude/ready-folder-spike-bsk6pa` git history) scored, per non-empty affinity brick,
a family of forward predictors against an **oracle**: the coarsest base-indirect level the
production `sh_coarsen` relative gate admits (numerator base-indirect reconstruction error,
denominator *composed* magnitude — the exact classification the shipped gate applies). Two
fixtures, both `has_delta_indirect: true`: `occlusion-test.map` (primary) and
`stress-warren-mini.map` (heavier cross-check).

Predictor families: **P1** delivered-magnitude-gradient; **P2** P1 + BVH occlusion; **P3** baked
direct-at-probe field (id-35) — a *ceiling only*, since id-35 bakes **after** id-34; **distance**
and **surface-distance** negative controls; and, as a targeted follow-up, **cone_angle** /
**cone_atten** scoring the prior spike's strongest signal (angle-off-cone-axis, r=-0.765) directly.

### Result — no family clears the bar; the winner flips per fixture

| | occlusion-test (802 bricks, 76% coarsenable) | stress-warren-mini (7889 bricks, 97.6% coarsenable) |
|---|---|---|
| FP floor (min unsafe-FP) | cone_angle **2.2%** / cone_atten 8% vs P1/P2/P3 flat **~16.5%** | all low (coarsenable-majority denominator) |
| recover @ FP ≤ 2% | **0** for every family | P3 0.77 / P2 0.76 / P1 0.58 vs cone_angle 0.46 / cone_atten 0.00 |
| recover @ FP ≤ 0.5% | **0** for every family | P1 0.26 / distance 0.23; cone families 0.00 |
| best family | cone_angle | P1 / P3 |

- **Cost was never the bar.** Predictor eval is ~0.003–0.2 s vs the ~716 s base-indirect bake
  (P1 ≈ 0.03%). Every candidate passes the cost bar; every candidate fails the accuracy bar.
- The tuning to the prior spike's strongest signal **helped on the primary fixture** (cone_angle
  cut the unsafe-FP floor from ~16.5% to ~2.2%, a real tradeoff knee where P1 is flat) — so P1
  was not the best contribution-aware predictor. But it **did not generalize**: on the heavier
  fixture the ranking flipped and the cone signal diluted under 49 static + 16 overlapping spots.
- **No family — original or tuned — recovers meaningful savings at strict near-zero FP (≤0.5%)
  on either fixture.**

## Why this is structural, not a weak-signal artifact (adversarial review)

The error that forces a brick to stay dense is **manufactured in composition** (base-indirect +
direct + delta), which is **downstream** of the base-indirect bake the forward predictor must
decide about. The prior spike measured composition amplifying base error ~6–8× (base-L2 mean
0.005–0.015 vs composed-L2 0.03–0.09); a dark receiver with small composed magnitude breaches the
relative gate on small base error. So the forward decision point is **structurally blind** to the
quantity that creates the need for density. This caps every forward approach regardless of feature
or classifier: any oracle reformulation that is *more* forward-predictable (gate against base
magnitude, not composed) moves in the *unsafe* direction. The predictors are not merely weak — they
are asked to forecast a downstream product.

## Alternatives, closed

- **No untried forward input survives.** Of stages baked before id-34 (`STAGE_ORDER`): the surface
  direct lightmap is genuinely forward but strictly *dominated by P3* (already a failed ceiling);
  the SDF/occluder atlas bakes *after* id-34 (not forward); geometry/BVH were already used.
- **Multi-feature offline classifier** (a joint rule over the per-brick feature vectors the harness
  emitted — no engine change, a Python afternoon over the JSON) is the one genuinely untried, cheap
  option. It is **below the bar to run**: occlusion-test's strongest single feature already recovers
  zero savings at ≤0.5% FP, so the dense minority is not separable in these features even before a
  classifier looks; the winner-flip says cross-fixture generalization is absent; the structural cap
  applies regardless. Its only value would be a *clean kill*, not a promote — recorded here as a
  decision, not a gap.
- **Low-ray partial pre-pass** (estimate base-indirect cheaply, predict from the noisy estimate) is
  a **separate new spike with a new hypothesis**, not a reopening of this one, and fights a
  smooth-signal-under-Monte-Carlo-noise wall (the smoothness that makes base-indirect coarsenable is
  what makes it low-SNR at few rays; noise biases toward over-predicting dense — safe only by
  refusing to coarsen). Nothing here obligates it.

## Verdict

The base-density bake-time win is **not reachable by a cheap forward predictor**; the door stays
shut. The base-indirect field is highly coarsenable post-hoc (that is real and productionized for
the storage half), but *forward* identification of the dense-needed minority is capped by the
composition-downstream structure above. Revisiting the bake-time win means a different problem
formulation (e.g. the partial pre-pass), authored as a new spike on new evidence — not a resumption
of this line.

## Lineage

- Prior sibling (post-hoc coarsenability, contribution-aware > distance): `context/research/coarsening-gating-spike/`.
- The one-way door this gates (held/archived): `context/research/archived-plans/lighting-scale--adaptive-base-probe-density/`.
- Shipped storage/bandwidth half: `context/plans/done/lighting-scale--sh-adaptive-coarsening-v2/`.
- Method / posture for build-to-learn specs: `context/lib/experimental_spikes.md`.
- Harness (reverted; measurement-only, byte-preserving): branch `claude/ready-folder-spike-bsk6pa`
  history — `crates/level-compiler/src/sh_forward_predict.rs` and the `--sh-forward-predict` flag.
