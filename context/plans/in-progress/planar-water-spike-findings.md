# Planar water reflection spike findings

Status: implementation, local verification, Mac sweeps and Windows measurements collected. T1–T3 implemented and landing review complete. Windows release live tests show half D15 plus probes reduces mirror-walk cost from 2.749 to 0.061 ms, averaging 5.070 ms/frame versus SSR High 4.828 ms at 1600×900. Distant probe handoff fits the owner's image goal. Moving-camera/entity checks and near-water scale behavior remain before a shipping decision; quarter D15 contributes no visible planar image in the latest static pose. Mac completion behavior remains separate.
Contract: `planar-water-spike-contract.md`.

Product drafting and artifact retention: [current handoff](planar-water-spike-drafting-handoff.md). Recommendation: proceed with bounded planar + probes as an optional large-flat-water mode, retaining SSR/probes-only alternatives; exact presets and shipping acceptance remain open.

## Environment

Windows (2026-10-23): GTX 1660 SUPER / driver 621.01. Dev capture build, fresh lake/e24/probe bakes, 128 focused checks, 144 matched captures and four accepted live CPU probes completed. [Windows findings and evidence](planar-water-spike-evidence/windows-2026-10-23/findings.md) include hashes, settings, GPU readback counts, repeats, images, exclusions and replay scripts. No engine or shader changes during the measurements.

Follow-up: [bounded live release findings](planar-water-spike-evidence/windows-bounded-live-2026-10-23/findings.md), 16 accepted uncapped runs across eight settings, plus eight stills and a separate accepted pilot. Minimal dev-tools launch controls enabled repeatable settings; 110 focused renderer tests and check/lint/format gates passed. No composite shader, visibility algorithm or player-setting changes.

Mac baseline host: AMD Radeon Pro 5300M through Metal. Metal System Trace measurements were collected locally on 2026-10-22 (below). Unmeasured entries are not zero-cost results. This adapter lacks the timestamp-query features required by the engine GPU timing logger; no engine timing windows are available. The Mac native launch reached Window ready but did not reach a world frame in the bounded run; process group was closed. Headless capture uses the shared app planar preparation path for further verification.

## Mac automated evidence

- `cargo test -p postretro-render-cpu planar_reflection`: 7 passed.
- `cargo test -p postretro-visibility mirror`: 5 passed.
- Renderer planar gate: 7 passed, including 5 GPU frame tests executed on host Metal with approved GPU access. Sandbox-only runs had no adapter and skipped GPU cases; those runs are not counted as GPU evidence.
- Existing renderer `fluid_` gate: 100 passed with existing tests and protected shader/reference files unchanged.
- App planar gate: 5 passed, including explicit mirror-only lightmap demand during main visibility fallback.
- Existing lightmap controller checks: 37 passed.
- Landing review: one P2 fixed (mask geometry cache could become stale on fluid reload); new host-Metal GPU regression passed 1/1. No remaining correctness findings from source review, including capture extension.
- Dev-tools engine build and `cargo fmt --check`: passed.
- FXC command matched zero tests on macOS; Windows subsequently passed the explicitly executed ignored test plus six DX12 lake captures (Windows evidence below).
- Final checks: formatting passed; Clippy with warnings denied passed for the default build and app/renderer with dev-tools+capture, and app with capture alone. Focused capture projection tests: 2 passed; final app planar tests with dev-tools+capture: 5 passed. Full workspace tests were not run, following the spike’s focused-only execution rule.
- Default lint caught a feature-gated diagnostic-mask getter used by app/capture; getter is now available wherever the existing mask state/setter are available. Both build configurations pass after the fix.
- Capture-feature planar gate: 5 passed. Headless lake captures completed in both modes and produced images and reports (see evidence below).

The determinant assertion checks positive combined handedness correction and the same projection determinant sign as the main view. A perspective projection itself has a negative determinant; requiring a positive full raster VP determinant would test the wrong property.

## Measurements

Mac baselines and a two-pose knob sweep are collected below and in `planar-water-spike-evidence/knob-sweep/README.md`. Windows covers across/down lake, e24 and probe-room poses, scales, distance/fade controls, 1080p/4K, tier gates, DX12 and live CPU/submission. Full values: [Windows tables](planar-water-spike-evidence/windows-2026-10-23/tables.md).

Record GPU spans, frame deltas, mirror walk time/path, visible cells/leaves, mirror-only cells, mask coverage, draws, and Mac submit delta. Record adapter and pose with each result.

## Lake capture evidence

Saved in `planar-water-spike-evidence/`: on/off PNGs, reports, logs and rerunnable scene JSONs. Run the dev-tools+capture engine with `POSTRETRO_PLANAR_REFLECTION=1` for on and `0` for off, using `--capture <scene.json>`.

These early captures predate the later local lake-fixture edits; compare their times only within this pair. Both captures: 960×540, SSR High baseline, planar half-scale with unlimited D/R, eight warmup frames and sixteen sample frames, camera (-16.8656, 1.7, -48.768), yaw 90°, pitch −10°. Reports name base revision a44e073c; implementation was uncommitted. Sync-proof SH preload and a wait after every submitted sample make these correctness captures, not normal frame pacing measurements.

Planar log: h=−1.219, 41 seeds, 124 drawable and reachable cells, zero mirror-only cells at this pose, StepLimitFallback after 20,000 considered steps. No SolidCellFallback or GPU validation error in either capture. Both report zero visible lightmap misses, failed installs and reads. The bounded walk fallback was expected by the contract.

CPU+GPU completion median: on 16.484 ms, off 14.472 ms (delta +2.012 ms). These short, ordered, synchronized runs are preliminary observations only; they cannot establish GPU pass cost or a performance recommendation. GPU timing explicitly reports unsupported; the 16-frame CPU stage window is incomplete.

## Metal System Trace measurements — 2026-10-22

Evidence: `planar-water-spike-evidence/metal-2026-10-22/` contains reports, per-frame extracted summaries, scene files, logs, extraction script and binary/map hashes. Raw `.trace` bundles and exported XML remain under `target/planar-spike-measurements/` (ignored build tree; preserve before cleaning target). The capture-enabled dev binary was rebuilt before recording; no source changed during this measurement session. An initial launch with a non-capture binary exited before rendering and is excluded.

Method: Xcode `xctrace`, Metal System Trace, Radeon Pro 5300M; same lake pose/resolution as above. Planar half-scale, unlimited D/R; SSR High and Medium. Each run has 120 warmup frames and 240 timed sample frames, sync-proof SH preload, indirect validation on, no concurrent builds or captures. Recordings ran sequentially in planar → High → Medium order. Trace postprocessing completed before the next recording. This is a static headless workload, not live gameplay.

XML references are resolved before aggregation. Render boundaries use successive main depth-prepass events. Extract the last 240 complete measured intervals before the final PNG render, excluding setup/warmup. Instruments’ own “Frame” field numbers individual Metal submissions on this backend, so it is not used as an engine-frame identifier. Timings below are observed GPU interval durations, not the engine’s unavailable timestamp-query spans.

| GPU metric, mean ms over 240 frames | Planar | SSR High | SSR Medium |
|---|---:|---:|---:|
| Reflection labelled-pass sum | 0.876 | 2.075 | 0.590 |
| Main world forward | 4.799 | 4.400 | 4.967* |
| Union of labelled GFX activity | 8.857 | 9.043 | 8.653 |
| Synchronized completion median, traced | 17.132 | 16.713 | 16.476 |

*Medium world-forward value covers 239 ordinary events; one event was coalesced with another encoder. Whole-frame GFX union includes coalesced intervals without double counting. Fluid-pass events also occasionally coalesce. Treat small cross-run differences cautiously: scene-wide timings vary, and GPU performance state was not locked.

Planar components (mean ms): candidate cull 0.045 (239 ordinary events plus one coalesced event), water mask 0.066, mirror prepass 0.101, mirror forward 0.607, resolve 0.056. Mirror forward is about 69% of the reflection work. Reflection sum p95: planar 1.049 ms, High 2.262 ms, Medium 0.628 ms. The planar sum adds compute and graphics intervals and is **not** an elapsed cull-through-resolve span; overlap is possible. GFX union excludes compute and is **not** whole-device utilization or complete frame time.

SSR reflection events: zero in planar; 240 each in High and Medium. No SolidCellFallback or GPU validation error in the three workload logs. All three capture reports show zero nonresident visible lightmap misses. At this pose the mirror walk logs 41 seeds, 124 visible/reachable cells, zero mirror-only cells, and StepLimitFallback at 20,000 steps.

### Completion without Instruments

Six additional runs used the same 120/240 workload in Medium → High → planar → planar → High → Medium order. Reported values are each run’s median synchronized CPU+GPU completion, not GPU pass durations.

| Mode | Run medians, ms |
|---|---|
| Planar | 16.875, 16.948 |
| SSR High | 15.372, 15.017 |
| SSR Medium | 13.467, 14.520 |

The slower planar completion persists without Instruments at this pose. The planar reflection work is about 58% cheaper than SSR High in the trace, but about 49% more expensive than SSR Medium. Neither implies a frame-time win: the aggregate GFX advantage over High is only about 0.19 ms, while completion is slower.

Renderer recording averages across the two traced 120-frame CPU windows: planar 0.470 ms, High 0.408 ms, Medium 0.422 ms. The capture report does not expose live `render_submit`; Metal application intervals here report encoding lifetimes, which must not be relabelled submission time. Mirror preparation/walk is performed once for the fixed capture pose and is not represented as a per-frame live CPU walk measurement. Mask coverage percentage and draw counts were not exported by these tables; they remain unmeasured.

## Visual evidence

Inspected both lake PNGs: planar reflects the lit walls, raised blocks and thin posts across the lake. This one dark, across-water pose proves integrated output, not grazing precision, mover/skinned correctness or owner visual acceptance.

Owner checks pending: planar against SSR High, grazing depth precision, waterline seam, D/R fade visibility, movers and skinned meshes.

## Recommendation

Keep planar dev-only. Windows quarter/half scale has promising large-water GPU cost, but its unlimited live mirror walk adds about 3 ms CPU work per frame. Bound or improve the walk, then measure live again. Metal traces show cheaper reflection work than High while traced and untraced completion remain slower. Tight D/R bounds have a substantial visual cost; full scale gives up much of the gain at high coverage. Keep SSR Medium as a serious alternative. Owner visual acceptance, bounded live profiling, moving/entity-heavy scenes and broader Mac measurements remain required for a shipping decision.

## Remaining evidence

Measure bounded live mirror-walk cost and moving-camera/entity quality; extend the Mac hardware/pose evidence and resolve its native reveal issue if needed. Mask coverage and draw-call counts are not exposed by the current measurement reports. Preserve protected fluid files and keep this spike out of `context/lib/`. Existing untracked `context/plans/drafts/fluid-ssr-reflections/` predates this work. No shipping PR created. See the Windows findings for completed measurements and the handoff for build/replay commands.

## Knob sweep follow-up

See [full knob results](planar-water-spike-evidence/knob-sweep/README.md): 60 primary captures, 30 separate shore-control captures, five new Metal traces, portable Windows runner and run-1 screenshots. New capture controls cover scale, D, R and both fade widths; scene values override the environment seed and invalid controls fail before GPU creation.

At 960×540 on this Mac, across-water half planar takes 17.306/17.161 ms synchronized completion. D=15 lowers that to 15.914/15.836 ms; quarter/D15/R15 reaches 15.749/15.183 ms, versus SSR High 17.046/17.180 and Medium 13.394/15.139. That combination loses clear distant reflections. Over-water downward, it still trails High (18.098/18.064 versus 17.911/17.549).

Selected new reflection GPU means: half unlimited 1.049 ms, quarter unlimited 0.643, D=15 at half 0.500, R=8 at half 0.514; SSR High 2.259 ms from 113 isolated events (seven coalesced events excluded). These are fresh matched 60-warmup/120-sample traces, not directly interchangeable with the earlier 120/240 results. Quarter's GPU saving does not translate into a clear completion benefit. D limits reduce both draw cells and walk steps; no shipping preset is selected.

Verification: two new capture-control tests and 24 existing scene parsing tests passed; formatting and both dev-tools+capture and capture-only Clippy passed. Read-only review found two runner reproducibility issues (output-directory reuse and inherited validation state), both fixed in the shipped runner. Effective validation-on state was checked in all 60 primary logs; all report zero visible nonresident lightmap misses and failed reads/installs. Full workspace tests remain outside this spike’s focused-only execution rule.

## Windows measurements — 2026-10-23

Full [Windows findings](planar-water-spike-evidence/windows-2026-10-23/findings.md), [tables](planar-water-spike-evidence/windows-2026-10-23/tables.md) and [evidence index](planar-water-spike-evidence/windows-2026-10-23/README.md). GTX 1660 SUPER, driver 621.01, dev-tools+capture, indirect validation on. 144 captures / 34,560 measured frames, with reverse-order repeats; 128 focused checks passed, including actual FXC and adapter-backed GPU execution. No engine or shader edits.

Across-water lake at 4K, Vulkan, unlimited D/R:

| Mode | Synchronized completion medians, ms | Reflection GPU mean, ms |
|---|---:|---:|
| SSR High | 18.318 / 18.300 | 5.339 |
| SSR Medium | 14.020 / 14.000 | 1.130 |
| Planar half | 13.846 / 13.700 | 0.734 |
| Planar quarter | 13.248 / 13.212 | 0.295 |
| Planar full | 15.643 / 15.416 | 2.278 |

Half improves synchronized completion about 25% against High here. At 4K downward/high coverage, quarter takes 16.552/16.517 ms, half 18.064/18.260, full 23.796/23.927, High 24.474/24.673 and Medium 17.035/17.231. Full scale is an expensive default; Medium remains competitive. Small e24/probe views show much less benefit, and DX12 at 960 shows essentially no High-to-half completion gain. See tables for all D/R/fade/tier values.

Four earlier valid live launches at 1600×900 with ordinary streaming and vsync expose the missing CPU cost: unlimited planar mirror walk takes 2.976/2.915 ms per frame, hitting the 20,000-step fallback; work CPU rises from High 6.193/5.996 to planar 9.255/9.204 ms. Submit rises about 0.2 ms. All modes remain near 16.53 ms/frame with vsync, so this is not a live FPS win. The bounded release follow-up below measures D15 live; the distant probe handoff is intentional under the owner's clarified goal. D100 restores much of the unlimited image and also restores the full walk/GPU cost.

Mirror-only cells are exercised in e24/probe views (4–18 additional cells), with zero visible lightmap misses or read/install failures. No accepted capture has a malformed GPU readback, renderer validation error or mirror SolidCellFallback; planar records zero SSR. Known failed Vulkan overlay-loader startup messages and invalid initial live-start attempts are retained and distinguished from accepted evidence. Mask coverage/draw counts and moving-camera/entity acceptance remain open. All launched processes are closed.

## Bounded live release follow-up — 2026-10-23

Owner intent: probes deliberately supply distant reflection, while planar should provide useful local detail without SSR's main-screen coverage dependency. Losing distant planar detail is acceptable. Judge the handoff and local image, not parity with unlimited planar.

Eight configurations, two forward/reverse repeats, release opt-level 3 + thin LTO, Vulkan, ordinary streaming, indirect validation release default off and VSync off. Actual surface/scene 1600×900. Five tail 120-frame CPU windows and three tail 120-readback GPU windows per run: 9,600 CPU frames / 5,760 GPU readbacks. These window means are separate from earlier synchronized capture medians and dev/VSync results.

| Mode | Frame means, ms | Mirror CPU mean, ms | Reflection GPU mean, ms |
|---|---:|---:|---:|
| SSR High | 4.874 / 4.781 | absent | 0.947 |
| Half unlimited | 7.855 / 7.804 | 2.749 | 0.210 |
| Half D30 | 5.833 / 5.801 | 0.775 | 0.120 |
| Half D15 | 5.071 / 5.068 | 0.061 | 0.090 |
| Quarter D15 | 5.018 / 5.037 | 0.060 | 0.070 |
| Half R8 | 7.804 / 7.805 | 2.773 | 0.060 |

D15 removes about 98% of the unlimited mirror-walk cost and lowers frame time about 35%; it remains about 5% slower than High here. R8 alone retains almost all CPU walk cost. Quarter adds under 1% frame saving over half D15. D30 + R15 and quarter D30 are included in the [full results](planar-water-spike-evidence/windows-bounded-live-2026-10-23/tables.md).

Eight accepted static illustrations have complete GPU windows and clean counters. Half/quarter D15 repeat byte-identically; quarter D15 is also byte-identical to probes-only at this pose. Half D15 has a substantial near-water contribution absent from quarter; this reproducible scale difference needs visual/correctness investigation, rather than being assumed to be an acceptable resolution tradeoff. Capture omits game VM/manifest fluid tuning and does not establish motion quality.

Recommendation: **half D15, R unlimited, 4 m fade as the first motion-test candidate**, with D30 as the reach comparison. This supports the owner's hybrid strategy and largely resolves traversal cost without a visibility rewrite; it does not select a shipping preset or establish a live FPS win. Full provenance, source review, 110 focused renderer checks, exclusions, stills, replay and limitations: [bounded live findings](planar-water-spike-evidence/windows-bounded-live-2026-10-23/findings.md).

## Walk budget follow-up — 2026-10-23

Summary of the [walk budget findings](planar-water-spike-evidence/windows-walk-budget-2026-10-23/findings.md):

- **The seed.** `POSTRETRO_PLANAR_WALK_STEPS` is a new dev-tools launch seed, ranging 0–20,000. It caps the mirror walk only; 0 skips the walk and goes straight to the mirror-frustum fallback. Default behaviour is unchanged.
- **Method.** Release live runs at the bounded-live across pose, 1600×900, VSync off. Seven configurations, forward and reverse: 14 accepted runs. The engine window was visible and unoccluded but not foreground, with the GeForce Overlay present.

| Mode | Frame means, ms | Mirror walk, ms | Reflection GPU, ms |
|---|---:|---:|---:|
| SSR High | 4.745 / 4.717 | absent | 0.947 |
| SSR very_high | 5.071 / 5.079 | absent | 1.748 |
| Half unlimited | 7.719 / 7.635 | 2.689 | 0.210 |
| Half, budget 2,000 | 5.200 / 5.259 | 0.283 | 0.210 |
| Half, budget 500 | 5.047 / 4.956 | 0.071 | 0.210 |
| Half, budget 0 | 4.969 / 4.962 | 0.000 | 0.210 |
| Half D15 | 4.953 / 4.961 | 0.060 | 0.090 |

**Images.** Budgets 0, 500 and 2,000 draw the same 126 frustum-fallback cells as unlimited. Their stills are byte-identical to unlimited (AE 0, RMSE 0). The reason is structural: a tripped walk's partial result is discarded.

**D15 and probes.** Half D15 differs from probes-only in only 2 pixels. That reproduces in the bounded-live half and quarter D15 stills. The earlier note of a broad half-D15 near-water contribution is not supported by pixel data at this pose.

**Not measured:**
- A pose where the unlimited walk completes seeded. There, a small budget would widen to the frustum superset.
- The downward over-water pose. It was conditional on the budgets being indistinguishable, and they were not.
- Motion.

No shipping preset is selected.
