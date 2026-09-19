# lighting-scale--lightmap-dark-area-measurement

Brief · compact · reads: `context/lib/rendering_pipeline.md` §4 · `context/lib/build_pipeline.md` §Build Cache · `context/lib/experimental_spikes.md` · read at a097035

## Problem
Developer-raised (owner), build-to-learn. Static lightmap charts bake at uniform texel
density — one global default, varied only by authored `lightmap_scale_region`. Every
surviving interior face gets a full-resolution chart regardless of content, so large
near-black regions consume atlas VRAM/disk while carrying almost no recoverable spatial
detail. Coarsening those regions is a candidate feature, but its win is unmeasured: there
is no lightmap magnitude/coverage stats path, and the only committed `.prl` is a synthetic
BC6H fixture. This spike answers one question before any coarsening is built — how much
dark, flat lightmap surface area do representative maps actually carry? When done,
prl-build has an output-preserving `--lightmap-analyze` pass reporting per-map covered-texel
magnitude and flatness distributions and a dark-and-flat surface-area share, and a findings
note records those numbers across theatrical and stress maps with a go/no-go recommendation.

## Decisions
- **Measurement only; coarsening is a gated follow-up.** Build the analysis pass and stop.
  Automatic darkness coarsening (and its mandatory force-fine artist hatch) is a separate
  brief that these findings go/no-go. Warranted non-goal: coarsening is the obvious next
  step, so its absence here is deliberate, not overlooked.
- **This measures an absolute-darkness signal, not the rejected one.**
  `plans/done/lighting-scale--lightmap-bake-scaling` rejected a measured *error/contrast*
  classifier for lightmap density, because its coarsening target (decorative/distant geometry
  off the playing field) is un-measurable design intent. A near-black region is a different
  basis — near-losslessness, measurable — so this spike does not reopen that decision; its
  findings inform whether a later coarsener should.
- **Output-preserving analysis pass, modeled on `--sh-analyze`.** Emits summary + JSON,
  changes no emitted PRL bytes. Layer: compiler measurement pass, not runtime; a debug/analysis
  flag, never a user setting (`experimental_spikes.md` §Tuning levers).
- **Measure the pre-BC6H `CompositedAtlas`, post-dilation, over covered texels only.** Raw f32
  irradiance is exact and the `coverage` mask excludes gutter/uncovered texels. Do not decode
  BC6H output — lossy, needs the crate-internal decoder, and carries no coverage.
- **"Dark" reuses the SH gate's definition** — a map-wide magnitude statistic with the SH
  `darkness_frac`/floor — so the measurement's dark threshold matches what a future coarsener
  would key on. Flatness is a local-gradient measure over covered texels. Both are reported
  measurement parameters, not contracts.
- **Corpus spans theatrical, kinematic, scale, and a lit edge:** `campaign-test`,
  `closet-reveal`, `kinematic-platform`, a `stress-warren` variant, and `gate-heavily-lit`.
  Kinematic mover brushes skip static lightmapping, so mover maps measure the theatrical static
  world around the movers — which is the dark-staging content the feature targets.
- **Non-goals:** the coarsening itself, its force-fine protect hatch, gradient-driven
  refinement, a per-surface lightmap opt-out flag, occlusion-culling never-seen faces;
  bright-area error/contrast coarsening (frequency inversion); denoise (the bake is analytic
  direct × deterministic bounded visibility — no stochastic noise to remove).

## Acceptance

### Automated
- [ ] `--lightmap-analyze` runs on each corpus map and emits a summary + parseable JSON: the
  covered-texel magnitude distribution, the flatness distribution, and the dark-and-flat
  surface-area share.
- [ ] Output-preserving: emitted PRL bytes are byte-identical with and without the flag
  (mirror `sh_analysis_is_byte_preserving`).
- [ ] Deterministic: two runs on one map emit byte-identical stats.
- [ ] Magnitude and coverage read from the pre-BC6H `CompositedAtlas`: on a fixture with a
  known gutter, covered-only stats differ from unmasked, proving gutter texels are excluded.
- [ ] Discrimination, both sides: `gate-heavily-lit` reports a near-zero dark-and-flat share
  (refuses false positive); `closet-reveal` reports a materially higher share (permits).

### Manual
- [ ] Findings note records, per map, the dark-and-flat share (theatrical/kinematic vs stress
  vs fully-lit), the magnitude and flatness distributions, an estimate of atlas texels
  recoverable by coarsening dark-flat regions at candidate density levels, and a
  recommendation: pursue a darkness-coarsen + protect-hatch brief, defer, or drop.

## Path
- Template by symbol: `sh_analyze.rs` (`run_analysis`, `MagnitudeStats`, JSON emission). The
  lightmap analog reads `CompositedAtlas` (`lightmap_bake.rs`) after `dilate()`, keyed on
  `coverage`. Darkness floor: `classifier_darkness_floor` / `CoarsenParams` (`sh_coarsen.rs`,
  `sh_analyze.rs`). Flag dispatch + stage wiring mirror `--sh-analyze` (`main.rs::help_text`,
  `pipeline.rs`).
- Shape: a bake-time pass over the already-composited atlas — no new bake, post-hoc, so it is
  determinism-safe and reuses the exact seam a future coarsener keys on. Strongest rival:
  parse the emitted `.prl` and BC6H-decode — rejected (lossy, crate-internal decoder, no coverage).
- First slice (falsifies the riskiest assumption): magnitude-only over `closet-reveal` — if a
  theatrical map shows near-zero dark-flat area, the coarsening premise is dead and the
  flatness half is not worth building.
- Per-texel over covered texels needs nothing extra; per-chart rollup needs the pack-stage
  chart rects (`pack.rs`/`chart_raster.rs`). Deliver per-texel first; per-chart only if cheap.

## Open questions
- Flatness metric (gradient window and threshold) — **delegated**: executor picks a defensible
  measure and reports it; it is a measurement parameter, not a contract.
