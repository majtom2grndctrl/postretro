# Foreground runtime proof — release-indirect-validation

Collected on 2026-10-01 (America/Los_Angeles), from `dcde8f292`, after the upload-batching merge. All four manual acceptance rows pass. Binary and map digests are in `builds.json`; structured evidence is in `results.json`, the run records, and cache/profile summaries. Large raw logs and sample reports remain locally in this directory and are ignored by Git.

## Submit savings (M1)

Plain symbol-preserving release, no `dev-tools`, AMD Radeon Pro 5300M / Metal, map spawn, vsync on, `POSTRETRO_CPU_TIMING=1`. Baseline and treatment use the identical binary: override `1` versus unset. Each unsampled run collected eight complete 120-frame windows; discard the first three and take the median of the final five. Foreground PID checks remained true throughout gameplay. No compiler, `sample`, or Instruments recording ran during these timing pairs.

| Map | Validation on | Release default off | Savings | Half reconciled cost | Result |
|---|---:|---:|---:|---:|---|
| Hallway inspection | 5.094 ms | 3.811 ms | 1.283 ms | 0.856 ms | pass |
| Campaign test | 3.685 ms | 3.477 ms | 0.208 ms | 0.158 ms | pass |

Both also exceed the provisional thresholds (0.65 / 0.18 ms). Values are CPU `render_submit` window averages, not whole-frame or GPU savings.

Separate Metal traces of each map/mode establish warm-cache state with **zero uncached spot world passes and zero uncached cube-face world passes** (the overflow paths). Hallway records 374 / 113 camera-frame equivalents for enabled / unset, with no world-cache refreshes. Its promoted spot entity-pass totals are 2,990 / 904, approximately eight per frame. Campaign records 104 / 137 camera-frame equivalents, including **five / six dynamic spot world-cache refreshes**, approximately 0.048 / 0.044 per frame; the two modes therefore have comparable warm-cache behavior with periodic refreshes. Campaign also has promoted cube entity passes (six faces per frame) and promoted/dynamic spot entity passes reusing world depth.

Every observed labelled pass is retained and deduplicated by Metal command-buffer/encoder IDs. Camera-frame normalization uses the smaller depth/textured-pass count and is approximate at trace boundaries. An initial temporal grouping incorrectly excluded the campaign refresh frames; the final parser and summaries correct this and retain all five/six fills.

Ordinary warm frames issue `L × (2 + 0)` indirect world draws: 16,874 hallway and 1,548 campaign. Each campaign world-cache refresh adds another 774 draws. Including the measured refresh rate gives roughly 1,585 / 1,582 campaign indirect world draws per frame (enabled / unset), subject to trace-boundary sampling. Separate in-level enabled profiles estimate validation CPU cost at 1.712 / 0.316 ms per frame. Estimate = inclusive samples in `DrawBatcher::add` and `inject_validation_pass`, divided by main-thread samples, multiplied by mean frame duration from complete timing windows contained in the profile. This is a statistical point estimate; acceptance timings come from the unsampled pairs. Injection's fixed per-pass work is a larger share on campaign (107 of 137 validation samples, versus 294 of 581 hallway samples), so total validation cost need not scale with leaf count alone.

Engine-closed `ioreg -c IOAccelerator` snapshots are recorded in `results.json`. In-use VRAM ranges roughly 0.87–0.93 GB and free VRAM 2.43–2.71 GB; sampled utilization varies 0–58%, including driver counters just after engine shutdown. Other desktop applications were left running. These results apply to this recorded shared-GPU state and settings: render resolution `auto`, shadow/fog quality `low`, Surface Depth `off` (`settings-summary.json`). Options and window geometry were unchanged across each pair. GPU timestamps are unsupported; `POSTRETRO_GPU_TIMING=1` did not enable their timing/counter path.

## Profile and startup proof (M2/M3)

On **both maps**, ten-second in-level `sample` reports have this result:

| Build / override | DrawBatcher::add | inject_validation_pass |
|---|---|---|
| Plain release / `1` | present | present |
| Same release / unset | absent | absent |
| Debug / unset | present | present |

Startup evidence covers plain release default off, `dev-tools` release default off, and debug default on. Explicit overrides identify their source. Every accepted run emits exactly one effective-state line. See `results.json` for exact lines and profile digests.

## Visual proof (M4)

Owner confirmed both maps looked unchanged at spawn and during walks: “Yes, both looked unchanged.” Seven earlier static offscreen on/default-off pairs were byte-identical; see the sibling `screenshots/` gallery. macOS `screencapture` of the live window failed with `could not create image from window`, so there is no additional live-window screenshot claim. Existing PRLs were used; no re-bake was needed.

## Reproduction and retained artifacts

Build commands: `CARGO_PROFILE_RELEASE_STRIP=none cargo build --release -p postretro -p postretro-script-compiler`, `cargo build -p postretro -p postretro-script-compiler`, and `CARGO_PROFILE_RELEASE_STRIP=none cargo build --release -p postretro --features dev-tools`. Plain release was saved before the feature build and its digest verified; it is restored after startup checks.

`foreground.swift` reads/activates only the launched engine PID. Compile it with `xcrun swiftc -module-cache-path /private/tmp/postretro-swift-cache measurements/release-indirect-validation/runtime/foreground.swift -o measurements/release-indirect-validation/runtime/foreground`.

From the workspace root, `RUNTIME_PROFILE=0 python3 measurements/release-indirect-validation/runtime/run-runtime.py example-on release campaign-test.prl on` collects an unsampled run; use a different label and `unset` for treatment. Default profiling starts `sample <pid> 10 1 -file <label>.sample.txt` after two timing windows. `RUNTIME_TRACE=1` records a separate Metal System Trace after profiling completes (or after two windows with profiling disabled), pauses the engine after the recording interval, and waits for trace finalization. `RUNTIME_WINDOWS=1 RUNTIME_PROFILE=0` is sufficient for startup checks. Each run terminates only its own engine process.

Export the trace's `metal-gpu-intervals` table with `xcrun xctrace export --input <trace> --xpath '/trace-toc/run[@number="1"]/data/table[@schema="metal-gpu-intervals"]' --output <xml>`. `parse-trace.py <xml> <summary.json>` produces cache evidence; `profile-cost.py <run-label>` produces a profile summary. Trace bundles and their session-owned `instruments*.ktrace` temporary files are removed after exports are parsed. Raw XML exports remain under `/private/tmp/release-indirect-validation/` and their digests are retained in the summaries.
