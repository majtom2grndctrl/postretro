# Planar knob sweep — Mac, 2026-10-22

## Method and evidence

Radeon Pro 5300M, Metal, dev build with dev-tools+capture, indirect validation on, sync-proof SH preload, 960×540, fixed pixel reflection style. Same compiled lake source/hash as the prior Metal baseline. Explicit `planar_reflection` scene controls override the environment seed. No source edits to rendering or shaders; the new control/validation adapter is capture-only.

Fifteen configurations, two poses and two reverse-order runs: **60 primary captures**. Each capture uses 60 warmup frames, 120 measured frames and a deterministic 1/60 animation step. Across camera: (-16.8656, 1.7, -48.768), yaw 90°, pitch −10°. Down camera: (-28.8656, 1.7, -48.768), yaw 90°, pitch −55°, moved 12 m over the lake. The initial downward camera stayed on shore and mostly showed ground; those 30 captures are retained separately as `shore-control-results.json`, excluded from the primary table.

Each scene/report/log and run-1 PNG is in this directory. `results.json` preserves all 60 primary per-run completion samples and counters. Manifests identify binary/map hashes and the base commit; source changes were uncommitted during capture. All primary logs confirm effective indirect validation on. The first runner used the build default; the corrected downward runs force it explicitly. The shipped runner now forces validation on, rejects nonempty output directories, and uses the corrected down pose.

All primary reports have zero visible nonresident lightmap misses, failed reads and failed installs. Logs select a plane in every planar configuration and contain no SolidCellFallback or GPU validation error. Complete renderer CPU windows are in the per-run reports. These static captures prepare mirror visibility once, so they do not measure live per-frame mirror walk or submit cost.

## Synchronized completion results

Each cell lists the **two run medians in ms**, in execution-repeat order. These are CPU+GPU wait-after-submit completion times, not GPU pass times or normal live frame pacing. Baseline planar is half scale, unlimited D/R, four-metre bands. One-variable rows retain that baseline otherwise.

| Configuration | Across | Down over water |
|---|---:|---:|
| ssr-high | 17.046 / 17.180 | 17.911 / 17.549 |
| ssr-medium | 13.394 / 15.139 | 16.099 / 17.036 |
| planar-half | 17.306 / 17.161 | 19.033 / 20.021 |
| planar-quarter | 17.232 / 17.398 | 19.178 / 19.474 |
| planar-full | 18.198 / 18.287 | 21.313 / 21.135 |
| d-30 | 16.479 / 16.358 | 18.848 / 18.804 |
| d-15 | 15.914 / 15.836 | 17.859 / 18.355 |
| d-8 | 15.826 / 16.000 | 18.320 / 17.819 |
| r-30 | 17.443 / 17.471 | 19.912 / 19.666 |
| r-15 | 17.245 / 17.246 | 20.029 / 20.197 |
| r-8 | 16.490 / 16.469 | 19.661 / 19.923 |
| quarter-d15-r15 | 15.749 / 15.183 | 18.098 / 18.064 |
| half-d30-r15 | 16.160 / 16.199 | 18.831 / 18.467 |
| fade-narrow | 16.266 / 16.207 | 18.607 / 18.615 |
| fade-wide | 16.270 / 10.628 | 18.702 / 18.956 |

`quarter-d15-r15`: quarter scale, D=15, R=15. `half-d30-r15`: half scale, D=30, R=15. Fade rows use half/D=30/R=15, changing both bands to 0.5 m (narrow) or 8 m (wide); default is 4 m. Distances and bands are absolute metres.

Across fade-wide run 2 (10.628 ms) is a large outlier relative to its counterpart and adjacent runs. It is retained without attributing it to fade width. SSR Medium also varies markedly. With two repeats and unlocked GPU clocks, small timing differences do not establish an effect. No builds, bakes or other capture workloads ran concurrently.

## Selected Metal traces

Five additional sequential Instruments recordings use the across pose with the same 60/120 workload. Raw bundles/XML: `target/planar-water-knob-traces/`; extracted summaries, capture reports, logs, scenes and scripts: `metal/`. Copy raw bundles before cleaning target.

| Configuration | Reflection labelled GPU mean, ms | Mirror forward mean, ms |
|---|---:|---:|
| Half, unlimited | 1.049 | 0.676 |
| Quarter, unlimited | 0.643 | 0.311 |
| Half, D=15 | 0.500 | 0.176 |
| Half, R=8 | 0.514 | 0.196 |
| SSR High | 2.259 | — |

Planar values sum cull, mask, prepass, forward and resolve over 120 frames with all five labelled events. They are work sums, not elapsed spans; compute/graphics overlap is possible. SSR has 113 isolated reflection events; seven coalesced intervals cannot be attributed cleanly and are excluded, never counted as zero. GPU timings and completion timings have different meanings and must not be combined.

Extraction resolves XML references, accepts every GPU channel (compute queue names vary between runs), and groups using main depth-prepass boundaries. It excludes setup, warmup and the final PNG render. On planar traces, the SSR pass has zero labelled events. Missing/coalesced event groups are excluded from reflection statistics. Nested/overlapping GPU intervals are unioned only for the separately reported GFX activity statistic.

## Work bounds and visual inspection

Across-view mirror visibility from logs:

| Setting | Seeds | Drawable cells | Walk steps | Path |
|---|---:|---:|---:|---|
| Unlimited | 41 | 124 | 20000 | Step-limit fallback |
| D=30 | 41 | 28 | 5091 | Seeded |
| D=15 | 41 | 9 | 339 | Seeded |
| D=8 | 41 | 8 | 98 | Seeded |
| R=8 | 2 | 79 | 17188 | Seeded |

Mirror-only cells are zero at these poses. These fixtures do not prove additional residency demand under a real mirror-only view; automated seam tests remain that evidence. Mask coverage percentage and draw-call counts remain uninstrumented.

Inspected across half, quarter, D=30, R=8, the quarter/D15/R15 combination and wide fade, plus downward half. Quarter retains the large recognizable reflections with coarser edges. D=30 and the aggressive D15 combination remove the distant wall/block reflections and expose the probe’s smeared fallback; R=8 similarly removes much of the distant-water planar reflection. The downward view is correctly over water but mainly shows bed texture, water and a nearby post, so it is weak evidence for distant-reflection quality. Owner visual acceptance, moving-camera transitions and mover/skinned scenes remain outstanding.

## Decision implications

- **D is the strongest useful work-bounding lever here:** D=15 reduces across completion by about 8% versus unlimited planar, and sharply reduces cells and walk steps. D=8 adds little beyond D=15 in this scene.
- Quarter scale cuts measured reflection GPU work by about 39%, but shows little completion improvement. Full scale is consistently slower. Pixel cost alone does not explain completion on this Mac.
- R=8 halves measured reflection GPU work, but only modestly helps across completion and does not improve the down-view repeats. It is not a universal win.
- Quarter/D15/R15 is the fastest across combination (15.749/15.183 ms versus SSR High 17.046/17.180), but has a conspicuous visual cost and remains slower than SSR Medium. Down over water, that combination (18.098/18.064) still trails SSR High (17.911/17.549).
- Fade width is primarily a visual lever at this precision; the noisy wide-band outlier does not justify a performance claim.

Do not select a shipping preset yet. Windows should reproduce this matrix, measure live mirror_walk/render_submit and per-pass GPU spans, then test intermediate D values that preserve more distant geometry (for example 45–60 m), plus moving and entity-heavy scenes. The data now establishes that knobs reduce GPU work; the unresolved decision is whether enough savings survive whole-frame/driver cost at an acceptable image quality.

## Replay on Windows

Build the compiled lake and dev-tools+capture binary as described in `../../planar-water-spike-windows-handoff.md`. From the workspace root:

```powershell
python context/plans/in-progress/planar-water-spike-evidence/knob-sweep/run_sweep.py --binary target/debug/postretro.exe --output target/windows-planar-knob-sweep
```

Use a fresh output path; the runner refuses mixed results. The shipped Windows runner defaults to 120 warmup/240 sample frames to allow timestamp readback windows to complete. This Mac dataset used 60/120; pass `--warmup-frames 60 --sample-frames 120` to reproduce its frame counts, and label the difference in comparisons. `--pose across` or `--pose down` limits the matrix, and `--repeats` sets repetition count. Outputs are generated with native absolute paths on the execution host. Committed Mac scene paths are provenance, not Windows-ready output paths; use the runner to regenerate them. Capture controls also work with the capture feature alone.

The new JSON member is `"planar_reflection": {"on":true,"scale":0.5,"d":null,"r":null,"band_d":4,"band_r":4}`. Empty object uses these defaults; omission preserves the original environment-seeded behavior. Scale must be 0.25/0.5/1, D/R null or 1–100 m, bands 0.1–16 m. Reflection tier Off and diagnostic light-term isolation still suppress mirror preparation.
