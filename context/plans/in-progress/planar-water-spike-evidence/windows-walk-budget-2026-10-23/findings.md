# Mirror walk budget follow-up — Windows, 2026-10-23

At the bounded-live across-lake pose, the unlimited mirror walk spends 2.69 ms tripping its 20,000-step limit. After that, the frustum fallback ignores the walk's result. A zero budget skips the walk and draws the same 126 cells. Its still is byte-identical to unlimited (AE 0, RMSE 0). Its frame mean is 4.965 ms, against 4.957 ms for half D15 and 7.677 ms for half unlimited. The hypothesis holds at this pose. The unlimited image costs about D15's CPU here, plus unlimited's 0.21 ms reflection GPU span.

SSR very_high, measured for the first time here, averages 5.075 ms per frame with a 1.748 ms `fluid_reflection` span. SSR High averages 4.731 ms with a 0.947 ms span.

## Method

The harness, pose, extents and reduction are the same as [bounded live](../windows-bounded-live-2026-10-23/findings.md); `run.py` here extends that runner.

- **Hardware and build:** GTX 1660 SUPER, driver 621.01, Vulkan. Release opt-level 3 with thin LTO, features `dev-tools,capture`.
- **Launch:** native window at 1600×900 (surface and scene, divisor 1). VSync off through `POSTRETRO_UNCAPPED=1`. Indirect validation at the release default (off). Ordinary SH/lightmap streaming, dev mod, pool seed 1.
- **Pose:** start pose (-16.8656, 1.2, -48.768), yaw 90°, pitch −10°, standing on the deck.
- **Matrix:** seven configurations, 20 s each, run forward and then in reverse with the same binary: 14 accepted runs.
- **Reduction:** each run keeps its last five complete 120-frame CPU windows and last three complete 120-readback GPU windows. That totals 8,400 CPU frames and 5,040 GPU readbacks.
- **Busy machine:** nothing else ran during the measurements: no builds, bakes or other experiments.

**Measurement state.** The engine window was visible and unoccluded, but it was not the foreground window. Windows kept focus on the taskbar, because a process launched in the background cannot take focus. The NVIDIA GeForce Overlay, a transparent full-window layer, sat above the engine in 7 of the 10 two-second probe samples in every accepted run.

`foreground/occlusion.ps1` ran beside every run and wrote `runs/*-occlusion.log`. The exclusion rule was any user window above the engine, or the engine minimized; no run met it. The overlay is recorded as a confounder, not an exclusion. The probe itself costs a little CPU once every 2 s. The bounded-live controls were taken with the same unfocused harness launch.

**Walk budget.** The `POSTRETRO_PLANAR_WALK_STEPS` seed takes 0–20,000 and defaults to `MAX_PORTAL_WALK_STEPS`. It caps the mirror walk only. A budget of 0 skips `portal_traverse_seeded` and goes straight to the mirror-frustum fallback (`StepLimitFallback`, considered 0); `mirror_walk` still logs, at zero.

**SSR very_high.** This tier is a persisted player setting with no launch seed. Its runs used `--app-name postretro-walk-budget-vh`. That profile's `settings.toml` is the default player file with one line changed, `reflection_quality = "very_high"`, and `postretro.dev/state.json` is copied unchanged. Each log confirms the profile path and `Reflection quality: VeryHigh`. The very_high profile lives in `%APPDATA%\postretro-walk-budget-vh`, outside the repo. The owner's default profile was not modified.

**Stills.** `capture_stills.py` follows the bounded-live capture pattern. The eye is fixed at y=1.5. SH loads through sync-proof preload. GPU timing is on, with 120 warmup and 120 measured frames. The walk budget reaches capture through the same seed. `compare.py` runs `magick compare -metric AE|RMSE`.

**AE scale.** ImageMagick 7.1.2's AE output is not a pixel count. Its channel values come back scaled. Differing-pixel counts below were therefore computed directly: the per-channel max of a difference image, thresholded at 0.

## Provenance

| Item | Value |
|---|---|
| Base commit | `2afe2613b`, plus the walk-budget source change committed with this evidence. The binary was built from that working tree. |
| Binary SHA-256 | `e37e17b4c63ba694b8b1d56f83ef1eaf3aafa7c0c516899e4ee0cbe3f1189ddf` |
| Map SHA-256 | `7cce7c5f69397b20f2cab3740deecd97b5114d5f8985b737244752315b5f15f4`, the same Windows lake bake as bounded live |
| Adapter | NVIDIA GeForce GTX 1660 SUPER, Vulkan, DiscreteGpu, driver NVIDIA 621.01 |
| Build | `cargo build --release -j 2 -p postretro -p postretro-script-compiler --features postretro/dev-tools,postretro/capture`, with `CARGO_INCREMENTAL=0` and worktree-local TEMP/TMP. Warm incremental, 7m46s. Log: `release-build.log`. |
| GPU clocks (telemetry) | Mostly 1935–1950 MHz; not locked |

## Results

Values are run 1 / run 2. Full columns, including work, submit and visibility, are in [tables](tables.md). Per-run data is in `summary.json` and `runs/results.json`.

| Configuration | Frame, ms | Mirror walk, ms | Main walk, ms | Reflection GPU, ms | planar_forward, ms | Path | Considered | Drawable cells |
|---|---:|---:|---:|---:|---:|---|---:|---:|
| SSR High | 4.745 / 4.717 | — | 2.913 / 2.902 | 0.943 / 0.950 | — | — | — | — |
| SSR very_high | 5.071 / 5.079 | — | 2.949 / 2.953 | 1.747 / 1.750 | — | — | — | — |
| Half unlimited (20,000) | 7.719 / 7.635 | 2.692 / 2.686 | 3.023 / 2.946 | 0.210 / 0.210 | 0.140 / 0.140 | StepLimitFallback | 20,000 | 126 / 126 |
| Half unlimited, 2,000 | 5.200 / 5.259 | 0.284 / 0.282 | 2.948 / 2.946 | 0.210 / 0.210 | 0.140 / 0.140 | StepLimitFallback | 2,000 | 126 / 126 |
| Half unlimited, 500 | 5.047 / 4.956 | 0.071 / 0.072 | 3.019 / 2.920 | 0.210 / 0.210 | 0.140 / 0.140 | StepLimitFallback | 500 | 126 / 126 |
| Half unlimited, 0 | 4.969 / 4.962 | 0.000 / 0.000 | 2.986 / 2.982 | 0.210 / 0.210 | 0.140 / 0.140 | StepLimitFallback | 0 | 126 / 126 |
| Half D15 | 4.953 / 4.961 | 0.060 / 0.061 | 2.957 / 2.952 | 0.090 / 0.090 | 0.030 / 0.030 | Seeded | 354 | 11 / 11 |

Counters across all 14 accepted runs:

- 41 seeds and plane h = −1.219 for every planar run.
- Mirror-only cells: 0.
- Lightmap: zero visible misses, both outside the baked set and not resident, and zero failed reads or installs.
- No SolidCellFallback, renderer validation error, panic or invalid-seed warning.
- Planar runs record no `fluid_reflection`, and SSR runs record no `planar_reflection`.
- Every planar run logged exactly one `[Planar reflection]` diagnostic line. Its considered and accepted counts come from the first frame.

`mirror_walk` times the portal walk only. The fallback's per-cell frustum cull runs inside `visibility`. `visibility` minus the main walk gives the full mirror visibility cost: 0.030 ms at budget 0, 0.086 ms at D15 and 2.72 ms unlimited.

The controls reproduce the bounded-live means within about 2%:

| Configuration | Bounded live, ms | This dataset, ms |
|---|---:|---:|
| SSR High | 4.828 | 4.731 |
| Half unlimited | 7.830 | 7.677 |
| Half D15 | 5.070 | 4.957 |
| Half unlimited mirror walk | 2.749 | 2.689 |

## Stills

There are eight stills (`stills/`): the seven configurations plus probes-only (Low). Each has 120 GPU readbacks with zero malformed, and the reflection pass counts are correct. Lightmap counters are zero. Images are 1600×900. Capture mirror diagnostics match the live runs: budgets 0/500/2000 and unlimited all draw 126 cells on StepLimitFallback, and D15 draws 11 cells Seeded.

| Pair | Differing pixels | IM AE (raw) | IM RMSE |
|---|---:|---|---|
| Budget 2,000 vs unlimited | 0 | 0 | 0 (0) |
| Budget 500 vs unlimited | 0 | 0 | 0 (0) |
| Budget 0 vs unlimited | 0 | 0 | 0 (0) |
| Half D15 vs unlimited | 136,417 | 2.34622e+08 | 1300.92 (0.0198508) |
| Half D15 vs probes-only | 2 | 6039.5 | 4.27529 (6.52368e-05) |
| Unlimited vs probes-only | 136,417 | 2.34628e+08 | 1300.94 (0.0198511) |
| SSR High vs unlimited | 102,883 | 8.49897e+07 | 673.068 (0.0102704) |
| SSR very_high vs unlimited | 96,109 | 6.91673e+07 | 589.27 (0.00899168) |
| SSR very_high vs SSR High | 43,258 | — | — |

The budget-limited stills are byte-identical to unlimited (SHA-256 prefix `195236756b8d0972`). Because they match exactly, no diff image was made. Unlimited, half D15, probes-only and SSR High are also byte-identical to the bounded-live stills of the same names. Full results are in `stills/compare.json`.

## Interpretation

**Why the budget changes nothing visible.** On `StepLimitFallback`, the mirror's visible set is every non-solid cell inside the mirror frustum (water half-space plus D). The walk's partial result is discarded. Any budget that trips therefore yields exactly the set unlimited yields at this pose. Only walk time changes. The identity is structural, not a coincidence of this view.

**What a budget would cost at a non-tripping pose.** Where the unlimited walk completes on the seeded path, a small budget would trip it. That replaces a tight seeded set with the larger frustum superset: conservative, but more cells drawn. That case was not measured here. The over-water downward pose was not run, because the brief made it conditional on budgets 0/500/2000 being indistinguishable. They are distinguishable on CPU: at 2,000 steps, the mirror walk costs 0.28 ms and the frame about 0.26 ms more. Budgets 500 and 0 are within run-to-run spread of each other.

**Budget 0 against the alternatives at this pose:**
- About 0.23 ms per frame slower than SSR High.
- About 0.11 ms faster than SSR very_high.
- Matches half D15's frame time while keeping unlimited planar's image.

**D15 is essentially probes-only.** Half D15 differs from probes-only in only 2 pixels, at (1569–1570, 559) with a maximum delta of 22/255. The bounded-live half D15 and quarter D15 stills show the same 2-pixel difference. The earlier finding described a "broad near-water, deck-textured region" in half D15 that quarter D15 lacked. The pixel data does not support that. At this static pose, D15 at either scale contributes almost nothing beyond the probes.

## Limitations

- One static pose. Moving-camera behaviour and the seeded-path-to-fallback transition are unmeasured.
- The engine window was never in the foreground. The GeForce Overlay was present, GPU clocks were not locked, and the desktop was in use.
- CPU and GPU tail windows are asynchronous. GPU log values are rounded to 0.01 ms.
- The very_high runs used a separate player profile, identical to the default except for one setting.
- Stills use sync-proof capture without game VM/manifest fluid tuning, as in bounded live.
- Mac is not measured.

## Excluded and diagnostic runs

- `runs-occluded-excluded/` and `pilot-occluded-excluded/` are labelled "occluded-window, excluded pending rerun". The engine window may have been behind other windows. They contain 10 completed runs, one partial run and 2 pilot runs; see `EXCLUDED.md`.
- `pilot-foreground-diagnostic/` and `pilot-occlusion-diagnostic/` are the two window-state pilots, with logs in `foreground/`. They are excluded from the tables.

## Verification

Focused tests:

| Command | Tests passed | Log |
|---|---:|---|
| `cargo test -p postretro-visibility mirror` | 6, including the new budget test | `visibility-tests.log` |
| `cargo test -p postretro-renderer --features dev-tools planar` | 11, including the new seed parser test | `renderer-planar-tests.log` |
| `cargo test -p postretro --features dev-tools,capture planar` | 7 | `app-planar-tests.log` |

Lint and formatting:
- `cargo fmt --check` passed.
- Clippy with `-D warnings` passed for postretro, renderer and visibility with dev-tools and capture, all targets (`clippy.log`).
- Clippy also passed for renderer and visibility without dev-tools (`clippy-plain.log`).