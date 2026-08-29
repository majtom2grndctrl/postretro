# Base-density forward-predictor efficacy — decision record

**Question.** Can a cheap predictor, evaluable *before* the base-indirect SH bake from light
geometry and probe positions alone, decide which affinity bricks may skip dense probe bakes —
accurately enough (near-zero unsafe false-positives) and cheaply enough to justify the
base-density bake-time-win one-way door (base-grid `density_level` format + renderer
variable-density sampler)?

**Answer: no.** Do not open that door on a forward predictor.

This is the measurement-first spike the archived base-density plan
(`context/research/archived-plans/lighting-scale--adaptive-base-probe-density/`) said its go/no-go
depended on. It closes the **bake-time** branch of that line. The storage/bandwidth branch already
shipped (`context/plans/done/lighting-scale--sh-adaptive-coarsening-v2/`); only the bake-time win
rode on the predictor.

## What was measured

A measurement-only harness scored each non-empty affinity brick against an **oracle**: the coarsest
base-indirect level the production `sh_coarsen` relative gate admits. Numerator is base-indirect
reconstruction error; denominator is *composed* magnitude. That is the exact classification the
shipped gate applies. The harness was byte-preserving and is now reverted (recoverable from branch
`claude/ready-folder-spike-bsk6pa`). Two fixtures, both `has_delta_indirect: true`:
`occlusion-test.map` (primary) and `stress-warren-mini.map` (heavier cross-check).

Predictor families:

- **P1** — delivered-magnitude gradient (light falloff + cone, no ray tracing).
- **P2** — P1 plus a per-light BVH occlusion test.
- **P3** — baked direct-at-probe field (id-35). A *ceiling only*: id-35 bakes after id-34, so it
  is never a forward input.
- **distance** / **surface-distance** — negative controls (the invalidated archived classifier).
- **cone_angle** / **cone_atten** — the prior spike's strongest signal (angle-off-cone-axis,
  r=-0.765) scored directly, added as a targeted follow-up.

### Result — no family clears the bar; the winner flips per fixture

| | occlusion-test (802 bricks, 76% coarsenable) | stress-warren-mini (7889 bricks, 97.6% coarsenable) |
|---|---|---|
| FP floor (min unsafe-FP) | cone_angle **2.2%** / cone_atten 8% vs P1/P2/P3 flat **~16.5%** | all low (coarsenable-majority denominator) |
| recover @ FP ≤ 2% | **0** for every family | P3 0.77 / P2 0.76 / P1 0.58 vs cone_angle 0.46 / cone_atten 0.00 |
| recover @ FP ≤ 0.5% | **0** for every family | P1 0.26 / distance 0.23; cone families 0.00 |
| best family | cone_angle | P1 / P3 |

- **Cost was never the bar.** Predictor eval runs ~0.003–0.2 s against a ~716 s base-indirect bake
  (P1 ≈ 0.03%). Every family passes on cost and fails on accuracy.
- **The tuning helped, unevenly.** Scoring the prior spike's strongest signal cut the FP floor from
  ~16.5% to ~2.2% on the primary fixture — a real knee where P1 is flat. So P1 was not the best
  contribution-aware predictor. But it did not generalize: on the heavier fixture the ranking
  flipped and the cone signal diluted under 49 static + 16 overlapping spots.
- **No family recovers meaningful savings at strict near-zero FP (≤0.5%) on either fixture.**

## Root cause (structural)

An adversarial review of this call confirmed it and named the reason the empirics only circle. The
error that forces a brick to stay dense is manufactured in *composition* — base-indirect + direct +
delta — which is downstream of the base-indirect bake the predictor must decide about. The prior
spike measured composition amplifying base error ~6–8× (base-L2 mean 0.005–0.015 vs composed-L2
0.03–0.09). A dark receiver with small composed magnitude breaches the relative gate on small base
error. So the forward decision point is structurally blind to the quantity that creates the need for
density.

This caps every forward approach, whatever the feature or classifier. Any oracle reformulation that
is *more* forward-predictable — gate against base magnitude instead of composed — moves in the
*unsafe* direction. The predictors are not merely weak; they are asked to forecast a downstream
product.

## Alternatives, closed

- **No untried forward input survives.** Of the stages baked before id-34: the surface direct
  lightmap is genuinely forward but strictly dominated by P3, an already-failed ceiling; the
  SDF/occluder atlas bakes after id-34, so it is not forward; geometry/BVH were already used.
- **Multi-feature offline classifier** — a joint rule over the per-brick feature vectors the harness
  emitted, no engine change, an afternoon of Python. This is the one genuinely untried, cheap
  option, and it is below the bar to run. The primary fixture's strongest single feature already
  recovers zero savings at ≤0.5% FP, so the dense minority is not separable in these features at all.
  The winner-flip says a rule trained on one fixture will not generalize. The structural cap applies
  regardless. Its only value would be a clean kill, not a promote — recorded as a decision, not a gap.
- **Low-ray partial pre-pass** — estimate base-indirect cheaply, predict from the noisy estimate.
  This is a *new spike with a new hypothesis*, not a resumption of this one. It fights a
  smooth-signal-under-noise wall: the smoothness that makes base-indirect coarsenable is what makes
  it low-SNR at few rays, and noise biases toward over-predicting dense — safe only by refusing to
  coarsen. Nothing here obligates it.

## Verdict

The base-density bake-time win is not reachable by a cheap forward predictor. The door stays shut.
The base-indirect field is highly coarsenable post-hoc — real, and productionized for the storage
branch — but forward identification of the dense-needed minority is capped by the
composition-downstream structure above. Any revisit is a different problem formulation (e.g. the
partial pre-pass), authored as a new spike on new evidence, not a resumption of this line.

## Lineage

- Prior sibling (post-hoc coarsenability; contribution-aware beats distance): `context/research/coarsening-gating-spike/`.
- The door this gates (held/archived): `context/research/archived-plans/lighting-scale--adaptive-base-probe-density/`.
- Shipped storage/bandwidth branch: `context/plans/done/lighting-scale--sh-adaptive-coarsening-v2/`.
- Build-to-learn method and posture: `context/lib/experimental_spikes.md`.
- Harness (reverted; measurement-only, byte-preserving): branch `claude/ready-folder-spike-bsk6pa`
  history — `crates/level-compiler/src/sh_forward_predict.rs`, behind the `--sh-forward-predict` flag.
