# Coarsening gating-spike measurement evidence

Measurement-first evidence for the adaptive SH probe-density (coarsening) spec.
Not a design doc — this is the banked data the spec's operating point is derived
from. All bakes used the `--sh-analyze` pass in `prl-build` extended with the
per-brick `composed_magnitude` field (commit that adds it to
`crates/level-compiler/src/sh_analyze.rs`), which lets the coarsening gate be
stated as a *relative* (Weber) deviation — error as a fraction of local
composed-irradiance magnitude — instead of a raw absolute irradiance value with
no perceptual anchor.

## Artifacts

| File | Map | Density | Notes |
|---|---|---|---|
| `mini-2m.sh-analysis.json` | `content/dev/maps/stress-warren-mini.map` | 2 m probe, 0.08 lm | No arena. 70,560 probes, 1,344 non-empty bricks. |
| `arena-2m.sh-analysis.json` | `warren-arena.map` (below) | 2 m probe, 0.25 lm | 1 NFL arena. 115,200 probes, 1,920 non-empty bricks. Full clean bake. |
| `warren-arena.map` | `gen_stress_map.py --preset warren --seed 1` | — | 6×5×3 grid, 1 arena, 85 static + 45 dynamic lights, movers/enemies/pickups. |
| `relerr_opmap.py` | — | — | Relative-error operating-point map generator. Run: `python3 relerr_opmap.py <json>`. |

`sw-1p5m.json` (repo root) is the prior-session showcase bake at 1.5 m — richer
arena data at finer density, but predates the magnitude field, so its errors are
absolute only.

## Key measured result (relative-p95 gate = error as % of local brightness)

Bandwidth cut is *on top of* compaction (the lossless prerequisite already
merged). Coarsenable% is of non-empty bricks.

| dataset | brick size | 10% → cut | 15% → cut | 25% → cut |
|---|---|---|---|---|
| arena 2 m | 8 m | 13% / 12% | 40% / 35% | 71% / 65% |
| mini 2 m | 8 m | 30% / 20% | 58% / 45% | 80% / 71% |
| showcase 1.5 m* | 6 m | 72% / 65% | 85% / 78% | 94% / 89% |

\* showcase converted absolute→relative via measured magnitude (~3.44 p95) as a
proxy, because its JSON predates the magnitude field — approximate.

## What the data establishes

- **Density is the dominant lever.** At the same conservative relative-p95 ≤ 10%
  gate, the delta-payload cut jumps 12% → 65% going from 8 m to 6 m bricks:
  finer bricks resolve empty/low-variance space that 8 m bricks smear across
  geometry. The in-container ceiling is ~2 m probe spacing (8 m bricks) — a
  1.5 m arena bake needs ~90 min and does not reliably survive a container
  restart cycle. The exact shipping-density (≤1.5 m) headline on a
  magnitude-enabled bake is the one measurement that needs a longer-lived host.
- **Seams are small.** Cross-level shared-face residual mean ~0.6% of local
  brightness; worst case ~7.8%. The interior threshold, not the seam, is the
  main quality lever — but the analyzer reports one aggregate seam, not seam at
  the chosen operating point, so a per-assignment seam bound is a follow-up.
- **Cosine-weighting ≈ unweighted** at every threshold — drop it.
- **Coarsenability tracks low variance, not raw openness** — the composed-error
  classifier is doing its job; it is not a geometric openness heuristic.

## Bake command (reproduce / extend on a longer-lived host)

```bash
prl-build <map> -o out.prl --no-tui -j 4 \
  --sh-probe-spacing <1.0|1.5|2> --lightmap-density 0.25 --no-cache \
  --sh-delta-max-size 2GiB \
  --sh-analyze --sh-analyze-out out.sh-analysis.json
```

Note: baking `stress-warren-*` maps requires the `scripts-build` sidecar
(`cargo build -p postretro-script-compiler --bin scripts-build`) or the
data-script stage hangs.
