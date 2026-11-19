# E20--streaming-telemetry — research

Read at 32ac4217. Findings that inform the brief without deciding it.

## Today's emit points

| Family | Typed source | Emission | Gate | Text readers |
|---|---|---|---|---|
| Load phases | `StartupTimings` (`startup/mod.rs`): `entries` plus a private `contained` list. There are three instances: `boot_timings`, `mod_timings` and `level_timings`. Off-thread inputs come from `PrepTimings` and `LevelBundle::line_c_marks`. | `[Startup] …` info lines. Line C is logged once per load, after the reveal frame's present. Unload is logged by `unload_level`. | none (info level) | `parse_line_c` / `logged_line_c` in the lifecycle_load and lifecycle_beside tests |
| Geometry install | `GeometryInstallMarks` (renderer, private) | `[Renderer] Geometry install timing:` from the upload thread on each set build | none | none found |
| SH streaming window | `ShStreamingLogWindow` diffs a baseline `ShStreamingLiveDiagnostics`, which includes `ShMissBuckets` | `[SH streaming] last N s: …`. The window closes after at least `DIAGNOSTICS_LOG_INTERVAL_SECONDS` (5 s), and only when a counter changed or a compose gauge is active. Runs on the main thread. | always on | `sh_streaming_diagnostics.rs` tests and `sh_residency/tests.rs` |
| Lightmap streaming window | `LightmapStreamingLogWindow` over `LightmapStreamingLiveDiagnostics`. It has two miss counters; cause buckets are decided but not built. | `[Lightmap streaming] last N s: …` | always on | its own tests |
| CPU timing | `StageWindow` → `WindowSnapshot`, every 120 counted frames. Each row has average, max, frames, kind and aggregate. Labels are `&'static str` from `StageSet` enums. | `[CpuTiming] frames=120 …` via `FrameTimer::log_last_window` | `POSTRETRO_CPU_TIMING` | `cpu_timing/tests.rs`; measurement scrapers |
| GPU timing | `FrameTiming` → `FrameTimingSnapshot` / `PassTiming`, every 120 readbacks, mean only. Readback is async, and dropped frames are skipped rather than stalled. | `[gpu-timing] …` via `FrameTiming::accumulate` | `POSTRETRO_GPU_TIMING`, plus adapter timestamp features (Metal: none) | measurement scrapers |
| Bake stages | `StageId` (`pipeline/stage_registry.rs`) driven through the `Reporter` trait. Per-stage durations exist only as `BuildSummary.timings`, with some non-stage rows mixed in. | `PlainReporter` writes to stderr, `TuiReporter` draws a screen, and `BuildSummary::print` writes to stdout | none | `compiler_cli_contract.rs`, `coloured_bounce_support`, `reach_verify_env.rs` |

Measurement scrapers: `measurements/shadow-fill-cost/timing.py`, `measurements/release-indirect-validation/runtime/profile-cost.py` and `measurements/sh-compose-row-cost-spike/1660/*.py`. Nothing in xtask, `tools/` or CI parses any of these lines.

## Logging and transport

- The engine binary installs `env_logger::init()` only, and nothing tees records. With `RUST_LOG` unset, env_logger's default filter is error, so info records reach no logger. That is why the tee raises the max level only when a consumer is present.
- `prl-build` installs `CollectingLogger` (default filter warn). It feeds the plain reporter's stderr, the TUI log pane and the end-of-build warning tally.
- No non-test code uses `tracing`. `test-log-capture` is a dev-dependency only.
- observe-live:
  - It is strict request/response with one client, and each connection is served to completion (`second_client_waits_behind_open_first_client`).
  - The queue is a single-slot `sync_channel`. The main thread drains it at the Input stage, `OBSERVE_LIVE_REQUESTS_PER_FRAME` = 1, with a 5 s reply timeout.
  - `ServerHello` is serialized once at spawn, so its `map` goes stale after a level change.
  - `OBSERVE_LIVE_PROTOCOL` = 1.
- `LevelLoads::generation` increments when each load starts, including loads later cancelled. `ActiveLevelLoad` holds `generation` and `map_path`. `LoadOrder::Beside` runs a load while another level is still running.

## Direction review (consumer seat)

An independent agent reviewed the design as its consumer. It agreed on cursor polling over push, and on serving from the transport thread. It added:
- level identity on every event;
- a tee max-level fix, because info never reached a logger;
- streaming totals and a final partial window;
- separate caps plus pinned one-shot events for late attach;
- a full producer list with states;
- a per-line flush and `run_end`;
- bake-to-load join fields.

Two corrections came out of the round. The app already has a load generation, so the brief reuses it rather than adding a counter. And `wait_ms` bounds per-request latency, not connection lockout. Every point landed in Decisions.

## Prior commitments touched

- `done/agentic-observability` cut the log ring from its first build and deferred streaming telemetry. This brief is that deferral.
- `done/E20--live-socket-channel` made streaming and concurrent clients out of scope, and made the main thread build every reply. This brief amends the latter for one engine-blind verb.
- `drafts/E20--memory-visibility` defers continuous telemetry here, and set the stdlib-Python reducer shape in `tools/`.
- `development_guide.md` §6.4: instrumentation records in every build behind `POSTRETRO_*`; surfaces sit behind dependency-free features.
