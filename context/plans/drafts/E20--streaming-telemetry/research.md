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
  - `ServerHello` is serialized once at spawn; its `map` is empty from the start.
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

## Direction validation

The verdict was Reshape. It proposed a file-only rival: drop the ring and the `events` verb, and have `ServerHello` name the file. That rival depends on an assumption: an observe-live client is always on the same machine as the file (the listener binds `127.0.0.1`), so it can read the file directly. The validator gated the live half on a consumer that cannot read the host filesystem. The owner kept the ring and the verb. The event wording came from an active agent consumer, and that consumer is the reason. With the ring kept, the validator's conditional verdict was Direction sound, given three fixes:
- name the file sink as a surface shipped in every build;
- reuse `CpuWindowReport` for the CPU window payload;
- state that the live half is engine-developer-only.
All three landed.

## Prior commitments touched

- `done/agentic-observability` cut the log ring from its first build and deferred streaming telemetry. This brief is that deferral.
- `done/E20--live-socket-channel` made streaming and concurrent clients out of scope, and made the main thread build every reply. This brief amends the latter for one engine-blind verb.
- `drafts/E20--memory-visibility` defers continuous telemetry here, and set the stdlib-Python reducer shape in `tools/`.
- `development_guide.md` §6.4: instrumentation records in every build behind `POSTRETRO_*`; surfaces sit behind dependency-free features.

## Ordering pins

| Id | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| P1 | GPU timing requested on an adapter without timestamp queries | `run_start` (GPU pending) → boot records → adapter created → `producer_state` | GPU reads unsupported, with its reason, before any GPU window could close |
| P2 | Late attach after eviction; a newer load arrives mid-pagination; small `max` | pinned events below `oldest` and at or after `since`, then the retained run from max(`since`, `oldest`), in one merged sequence order; a pin at or above `oldest` is served once, in its place | client reads `run_start`, producer states and the latest load first; the superseded pin falls under the caps and `dropped` advances |
| P3 | Loading-screen level change | partial(old) → unload(old) → geometry_install(new gen) → load_phases(new gen) | unload never carries a null level; it captures the level before `unload_level` clears it |
| P4 | Load beside a running level | geometry_install(new gen, envelope = running) … running_release → load_phases logged after reveal | line C holds `commit_checks` before `running_release`; its commit-frame group has none |
| P5 | Concurrent emitters on main, upload, loader and logging threads | sequence taken, ring insert and file write under one lock | strictly increasing sequence; `t_ms` may run backward across it; a cursor never skips |
| P6 | Exit with open windows and live background threads | partials → `run_end` → nothing | `run_end` last in file and ring, for clean, error, panic and `process::exit` paths |
| P6b | A panic: a caught worker panic in the engine, or a TUI bake panic in `prl-build` | `run_end` written by an unwind guard around the main body, never by a panic hook; never waits on a sink lock the panicking thread may hold | a caught worker panic writes no `run_end` and the run continues; a panic that unwinds out of main writes `run_end` naming it; a TUI bake panic, which runs no hook, still writes it |
| P7 | Overlapping, late-begun or zero-length bake stage | begin/end order = summary order; equal `t_ms` ordered by sequence | durations from the summary; transitions carry order only |
| P8 | `max` 0 | returns at once | nothing returned; cursor unchanged |
| P9 | Streaming events across levels or a session replacement | totals restart at a session's first window | differencing is valid only within one session |
| P10 | Sink write fails while its lock is held | warning formatted and emitted outside the sink lock | no deadlock; sink disabled; ring and stderr continue |
| P11 | Two processes append at once | one write call per whole line | lines interleave whole, never split |
| P12 | Main thread blocked; `dump` then `events` | `dump` holds the transport up to 5 s | `events` answers at once once the `dump` has timed out |
| P13 | A load superseded after its set build | set built (gen k) → cancel → no app take | no geometry_install or load_phases for k; gap explained as cancelled |
| P14 | `prl-build` exits during argument parsing (`--help`, a rejected argument) | sink opened after argument parsing | nothing written; never a lone `run_start` |

## Source pins

| Area | Source fact | Consequence |
|---|---|---|
| Bake summary and exits | Success finalizes through the pipeline's reporter; `finalize_failure` is called by each frontend (plain `main`, the TUI worker) on its concrete reporter. `args.input` and `args.output` are in scope at both reporter construction sites. `--help` exits inside argument parsing, before the logger | The failed summary comes from the frontend's `Err`, not a reporter wrapper. Open the sink after argument parsing |
| Geometry install total | `GeometryInstallMarks::log` recomputes its total from elapsed time at the line | Freeze the total at the line before handing the marks back |
| Unload | Level identity is cleared at the start of unload; streaming sessions end later in it | Capture the unloading level before it is cleared, and pass it to the partials and the unload event |

## Detail review, round 1

- **Premise lens:** checked 32 Decision premises.
  - Cache skips: the Reporter's skip means a stage produced no output; cache hits finish. Per-stage cache hits are counted only in a test seam.
  - Timing windows across a level change: level change calls `level_changed` → `StageWindow::clear`, not `discard_partial`. GPU `FrameTiming` is reset only by capture.
  - A beside load's line C is logged after it becomes the running level.
  - GPU timestamp support is known only after renderer init.
  - The hello is serialized once.
  - postretro-tool sets the project root as working directory.
  - Capture, headless and early-error exits use `process::exit`.
  - Under the TUI, Reporter transitions overlap.
  - Streaming sessions also end mid-level.
- **Rows lens:** supplied the ordering pins above and the rows built on them.
- **Subtract lens:** removed the long-poll, moved implementation detail to Path, cut wire facts restated in Decisions, and named the file-size and hello-map non-goals.
- The owner approved the resulting Decisions.

## Rival shapes

- **Socket-only stream, no file.** Rejected: the agent that launched the run is the common consumer, and bake events have no socket.
- **File only, with the hello naming its path.** Weighed at direction review. Rejected because an active agent consumer reads the stream live, and the MCP frontend exposes live verbs as tools; a path in a hello gives it no tool.
