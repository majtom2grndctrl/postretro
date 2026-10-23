# Planar water reflection spike findings

Status: implementation and local verification complete; hardware sweeps and owner visual checks outstanding. T1–T3 implemented and landing review complete. Implementation, fixture snapshot and small evidence are included in the Windows handoff commit. Initial Mac evidence: planar reduces reflection-pass GPU work against SSR High, but synchronized completion is slower at the measured pose.
Contract: `planar-water-spike-contract.md`.

## Environment

Execution host: macOS. GTX 1660 Super Windows measurements remain outstanding. Metal System Trace measurements were collected locally on 2026-10-22 (below). Unmeasured entries are not zero-cost results. Host adapter is AMD Radeon Pro 5300M through Metal. It lacks the timestamp-query features required by the engine GPU timing logger; no timing windows are available. Native launch reached Window ready but did not reach a world frame in the bounded run; process group was closed. Headless capture uses the shared app planar preparation path for further verification.

## Automated evidence

- `cargo test -p postretro-render-cpu planar_reflection`: 7 passed.
- `cargo test -p postretro-visibility mirror`: 5 passed.
- Renderer planar gate: 7 passed, including 5 GPU frame tests executed on host Metal with approved GPU access. Sandbox-only runs had no adapter and skipped GPU cases; those runs are not counted as GPU evidence.
- Existing renderer `fluid_` gate: 100 passed with existing tests and protected shader/reference files unchanged.
- App planar gate: 5 passed, including explicit mirror-only lightmap demand during main visibility fallback.
- Existing lightmap controller checks: 37 passed.
- Landing review: one P2 fixed (mask geometry cache could become stale on fluid reload); new host-Metal GPU regression passed 1/1. No remaining correctness findings from source review, including capture extension.
- Dev-tools engine build and `cargo fmt --check`: passed.
- FXC command matched zero tests on macOS; Windows check remains unverified.
- Final checks: formatting passed; Clippy with warnings denied passed for the default build and app/renderer with dev-tools+capture, and app with capture alone. Focused capture projection tests: 2 passed; final app planar tests with dev-tools+capture: 5 passed. Full workspace tests were not run, following the spike’s focused-only execution rule.
- Default lint caught a feature-gated diagnostic-mask getter used by app/capture; getter is now available wherever the existing mask state/setter are available. Both build configurations pass after the fix.
- Capture-feature planar gate: 5 passed. Headless lake captures completed in both modes and produced images and reports (see evidence below).

The determinant assertion checks positive combined handedness correction and the same projection determinant sign as the main view. A perspective projection itself has a negative determinant; requiring a positive full raster VP determinant would test the wrong property.

## Measurements

Mac SSR High/Medium baselines collected at one lake pose below. Still pending: Windows baselines and planar scale ¼, ½, 1; D and R unlimited, 30, 15, 8 metres. Capture across-water and downward poses in `reflection-lake-test`, `e24`, and `reflection-probes-test`.

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

Keep the planar path as a dev-only experiment for the hardware/pose sweep. CPU, binding and pixel-level checks plus integrated lake output support proceeding to measurements. Do not replace SSR in shipping quality tiers yet: Metal traces show cheaper reflection work than High, but both traced and untraced completion runs are slower, and the D/R/scale sweep is still absent. Next investigate per-frame Metal submission/validation and the water-composite cost, then measure bounded D/R and lower scales before choosing a default. Owner visual acceptance and Windows profiling plus broader Mac measurements remain required for a shipping decision.

## Remaining evidence

Investigate native reveal if needed for interactive profiling. Run the scale/D/R/pose sweep and owner visual read. Use Windows for FXC and GTX measurements; extend the recorded Metal System Trace baseline for this Mac. Add mask-coverage and draw-count evidence if current diagnostics do not expose them. Preserve protected fluid files and keep this spike out of `context/lib/`. Existing untracked `context/plans/drafts/fluid-ssr-reflections/` predates this work. No shipping PR created. See `planar-water-spike-windows-handoff.md` for Windows commands and outstanding measurements.
