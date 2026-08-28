# Task 1 addendum — composed-error failure disambiguation

Diagnostic follow-up to the first 1.25 m Stress-Warren run. **Measurement only, no source change.** Resolves *why* the composed reconstruction error failed before any spec or mechanism decision.

## The finding

At base `--sh-probe-spacing 1.25` on `stress-warren-showcase.map`:

- Participating-I5: pass, zero violations.
- Payload 155.1 → 17.1 MiB (0.110×); id 41 71.1 → 14.2 MiB (0.200×).
- Composed reconstruction error **fails**: rel p95 `0.485` (limit 0.10), rel max `0.952` (limit 0.25).

The win is real and large. The blocker is quality. But `0.485` is **not yet interpretable**, because the classifier does not gate the way this number was measured.

## What the classifier actually gates (grounded — do not re-derive)

Source: `crates/level-compiler/src/sh_coarsen.rs`, `classify_levels`.

- **Floored relative gate** (lines 117, 154–159): a bright brick coarsens only if `l2_p95 / max(mag_p95, floor) ≤ 0.10` **and** `l2_max / max(mag_max, floor) ≤ 0.25` (L1 analogous), where `floor = max(darkness_frac · map_p95, 1e-6)`, `darkness_frac = 0.02`, and `map_p95` is the p95 over valid bricks' `mag_p95`. Error is normalized by magnitude **or the darkness floor, whichever is larger**.
- **Darkness bypass** (lines 133–150): if `mag_p95 < floor`, the brick takes the **coarsest evaluable level with no error check at all**. The comment justifies this as "reconstruction error bounded near zero and thus imperceptible." **The bypass keys on `mag_p95` only — never `mag_max`.**
- Per-section: each of id 27/41/45 is classified against the **shared composed magnitude** (`classify_section_levels`); by linearity a section's own error is the composed error it induces. Three sections each ≤ 0.10 cap composed error near ~0.30 — so **0.485 requires error that was never checked**, which points at the bypass.

The reported `0.485 / 0.952` is almost certainly **un-floored** relative error (error ÷ actual magnitude). On a dark brick that is a huge ratio with a tiny absolute delta.

## Three scenarios — the data must pick one

| # | Cause | Signature in the data | Consequence |
|---|---|---|---|
| **(a) Metric artifact** | Failures on genuinely dark bricks; tiny absolute error. Classifier's floored gate is correct; the validation's un-floored metric is misleading. | Failing bricks are bypassed (`mag_p95 < floor`), low `mag_max`, tiny **absolute** error; failures vanish under the floored metric. | Fix the **validation metric** (floor-normalize). Mechanism proceeds as v2 assumes. |
| **(b) Bypass defect** | Bypass keys on p95 only, so a mostly-dark brick with a bright corner (`mag_p95 < floor`, high `mag_max`) is coarsened to a brick-mean and the spot vanishes. | Failing bricks are bypassed, **high `mag_max`**, real absolute delta at the max texel. | Classifier fix (bypass must also check `mag_max`). Breaks v2's no-classifier-change scope. |
| **(c) Composition defect** | Bright, **gated** bricks fail — the per-section linearity gate doesn't hold when sections compound. | Failing bricks are **not** bypassed (`mag_p95 ≥ floor`) yet composed error > limit. | Deeper rethink of composed-error control before any activation. |

(b) and (c) can co-occur. Report the split.

## Measurements

Run on `stress-warren-showcase.map`. Reuse the already-baked 1.25 m PRLs for the re-analysis; add fresh bakes only for the density trend.

1. **Absolute vs. relative, per failing brick.** For every brick over either relative limit, record absolute composed error (p95 and max), `mag_p95`, `mag_max`, and the dominant section (id 27/41/45). Distinguishes "real delta" from "tiny-over-tiny."
2. **Floored relative error.** Recompute the composed relative error with the classifier's own normalization — `err / max(mag, floor)`, same `floor = max(0.02 · map_p95, 1e-6)` — and report pass/fail under it. If the failures vanish, that is scenario (a).
3. **Bypassed vs. gated split.** Tag each failing brick: bypassed (`mag_p95 < floor`) or bright-gated (`mag_p95 ≥ floor`). Report counts and the worst brick in each class. Bypassed+high-`mag_max` → (b); gated → (c).
4. **Density trend.** Re-bake off/on and re-measure composed error at **1.0 m** (ship default) and **0.75 m**, alongside the existing 1.25 m. If error falls sharply with density, brick size is a lever; if it holds near 0.485, it is the bypass (which keys on relative darkness, not size). Record payload/id-41 ratios at each density too.

## Output

Append to the Task 1 `research.md`: the per-scenario evidence, the failing-brick table (absolute + relative + floored + bypassed/gated + section), and the density-trend table. State which scenario the data selects — (a), (b), (c), or a named mix. Do not change any source or the spec; the scenario chosen drives the next spec decision:

- **(a)** → validation-metric fix; mechanism proceeds.
- **(b)** → small classifier fix (bypass checks `mag_max`); owner decides widen-v2 vs. split a spec.
- **(c)** → composed-error-control rethink before activation.
