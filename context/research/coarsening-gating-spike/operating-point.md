# Coarsening operating point (metric-only, data-selected)

The gating spike was run metric-only (no rendered visual A/B — owner's call).
This records the precommitted limits the coarsening spec builds on, and *why*
each was chosen from the banked data (`README.md` in this directory). The owner
is explicitly not the SME on the metric definition; these are data-selected, not
fiat, and deliberately conservative because there is no visual verification step.

## The gate

A non-empty, unprotected brick takes the **coarsest** level (L2 > L1 > L0)
whose composed-receiver reconstruction error satisfies **both**:

- **relative p95 ≤ 10%**, and
- **relative max ≤ 25%** (guardrail),

where *relative* = the error statistic divided by the brick's composed-irradiance
magnitude at the same statistic (`composed_magnitude`, added to `--sh-analyze`),
with a darkness floor of 2% of the map's p95 brick magnitude to stop near-black
bricks dividing to infinity. Protected bricks and any brick failing both levels
stay L0.

- **Levels**: L0 = 64 dense; L1 = 8 corners + trilinear; L2 = 1 representative.
- **Classifier = composed-receiver error, never surface distance.** (The stopped
  prior plan keyed on surface distance; that approach is abandoned.)
- **Cosine-weighting dropped** — measured ≈ unweighted at every threshold.

## Why these choices

- **Relative, not absolute.** The raw gate error is an absolute max-per-channel
  irradiance difference; 0.25 against a bright receiver is a few-percent
  deviation, against a dim one it is gross. Dividing by local magnitude yields a
  Weber (perceived-contrast) proxy, so the threshold is a percentage of local
  brightness — the only way a metric-only threshold is defensible without eyes.
- **p95 as the primary statistic, max as a guardrail.** Pure max over-penalizes:
  coarsening error is a *smooth, low-frequency* trilinear reconstruction
  difference, not speckle, so a lone peak texel is the top of a smooth gradient,
  not an isolated artifact. p95 bounds "essentially the whole brick"; the max
  guardrail still catches a pathological single-texel blowup. Measured, the
  guardrail barely binds (arena 12.4% coarsenable with it vs 12.6% without), so
  it is a near-free safety catch.
- **10% is conservative on purpose.** A 10% deviation in the *indirect* term —
  itself only a fraction of final pixel brightness (direct light, albedo
  texture, and normal detail dominate perceived detail) — in a smooth field, is
  well inside typical indirect-lighting tolerance. Metric-only, with no visual
  check, argues for the conservative end; the threshold can be relaxed later if a
  visual A/B is ever added.
- **Seam handling.** Cross-level shared-face residual measured at mean ~0.6% of
  local brightness, worst case ~7.8%. Seams are small, so the interior gate is
  the main lever, but the worst-case boundaries motivate a **level-smoothing
  rule**: a brick may not sit more than one level coarser than a face-adjacent
  neighbor (demote toward the finer neighbor). The current `--sh-analyze`
  reports one aggregate seam, not seam at the chosen assignment — enforcing a
  seam bound as a spec AC needs the analyzer to compute seam at the final level
  assignment (spec/analyzer follow-up).

## Measured savings at this operating point (cut on top of compaction)

| dataset | brick size | coarsenable | delta-payload cut |
|---|---|---|---|
| arena 2 m | 8 m | 12.4% | **11.7%** |
| mini 2 m | 8 m | 30.2% | 19.8% |
| showcase 1.5 m* | 6 m | ~72% | **~65%** |

\* approximate — showcase JSON predates the magnitude field; converted via a
proxy magnitude. Confirming this exactly needs a magnitude-enabled bake at
≤1.5 m, which exceeds the in-container ~2 m ceiling (a 1.5 m arena bake needs
~90 min and did not survive a container restart). The gate nonetheless passes at
the pessimistic 2 m arena floor (11.7% cut, seams small, protection honored),
so spec drafting is unblocked. The shipping-density headline is the one open
measurement for a longer-lived host.

## Non-negotiables carried into the spec

- **Uniform-grid fallback per map** — never silently coarsen to hit a byte cap.
- **Protection volumes forced L0** — mapper-authored AABBs with a dilation
  margin (`--sh-protect-aabb` is the measurement stand-in).
- **Storage-buffer ceiling** — the indirect compose is at 8/8 read-only storage
  buffers (test-pinned `compose_layout_keeps_eight`); coarsening has no free
  slot and must pack into existing buffers or restructure.
- Coarsening is **lossy** and reworks the SH samplers + composed atlases — the
  blast radius compaction deliberately avoided. This is why it is spike-gated.
