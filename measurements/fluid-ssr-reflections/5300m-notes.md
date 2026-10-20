# 5300M quick set: notes

Binary `af8bfef61`, release `capture,dev-tools`, Metal System Trace, 1536×960 (the laptop's half-render Auto extent, which binds; the external monitor is played at about quarter render). One round, 120 warmup + 360 samples, pixel style, all light terms, `sync-proof` SH. Generated tables: `results-quick.md`, `results-pilot.md`. Kit: `metal/README.md`.

## Whole-frame GPU at 1536×960 (ms; reflection pass in brackets)

| config | near (straight down) | B vault oblique | A pillar oblique |
|---|---|---|---|
| off | 6.03 (pilot) | 6.51* | 6.66* |
| medium 0.5 / 48 / 180 | 8.69 [0.61] | 7.61* [0.50] | — |
| high 96 / 270 (unsaturated) | 11.48 [3.42] | 8.72 [2.32] | 10.34 [2.25] |
| very_high 192 / 180 | 11.29 [3.04] | — | — |

\* Flagged by the kit's complete-versus-incomplete cross-check (incomplete frames 6–15% slower, about 20% of frames incomplete). Directional only. The quick set's near off run failed to export; the pilot's valid near off run stands in.

Saturated divisor walk at near (fraction 1.0, steps = ceil(1.07 d), thickness 1.5):

| divisor | reflection pass | whole frame |
|---|---|---|
| 135 | 2.29 | 10.38 |
| 270 | 4.49 | 12.55 |
| 540 | 12.03 | 21.48 |

Medium fraction at near (64 steps, preset divisor 180): 0.375 gives 8.70 [0.45], 0.75 gives 9.84 [1.49].

## Reading

- Against a 60 fps budget of 16.7 ms, every measured point holds except d = 540, which runs at about 46 fps.
- Interpolating, d = 360 lands at roughly 14–15 ms. This was not measured.
- The reflecting fluid variant costs about 2 ms over off at near (fluid_surface 0.46 → 2.4 ms), separate from the reflection pass. Low pays this too.
- The large water volume (B) costs less than straight-down near at every tier. Cost tracks how much of the screen the water fills, not the volume's area.
- Caveats:
  - One round only.
  - Clock: the busy median varied from 814 to 1134 MHz between runs. At this extent, capture does not keep the GPU busy enough to hold its top clock (`rendering_pipeline.md` §12, "Pin the GPU clock"). Absolute figures read slightly high, which is conservative against the budget.
  - Not measured: Samsung extent, 4K, smooth style, and ultra on the Mac.
